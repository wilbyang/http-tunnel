//! ConnectHandler - Handles WebSocket $connect route
//!
//! This module contains the logic for handling new WebSocket connections.
//! It generates a unique subdomain, stores connection metadata in DynamoDB, and
//! returns a success response.

use aws_lambda_events::apigw::{ApiGatewayProxyResponse, ApiGatewayWebsocketProxyRequest};
use aws_sdk_dynamodb::Client as DynamoDbClient;
use aws_sdk_dynamodb::types::AttributeValue;
use http_tunnel_common::ConnectionMetadata;
use http_tunnel_common::constants::CONNECTION_TTL_SECS;
use http_tunnel_common::utils::{calculate_ttl, current_timestamp_secs, generate_subdomain};
use http_tunnel_common::validation::validate_tunnel_id;
use lambda_runtime::{Error, LambdaEvent};
use tracing::{error, info};

use crate::{SharedClients, auth, env, error_handling::sanitize_error, save_connection_metadata};

fn is_enabled(var_name: &str, default: bool) -> bool {
    std::env::var(var_name)
        .unwrap_or_else(|_| default.to_string())
        .to_lowercase()
        == "true"
}

fn build_public_urls(tunnel_id: &str) -> (String, Option<String>, String) {
    let custom_domain_enabled = is_enabled("ENABLE_CUSTOM_DOMAIN", false);

    if !custom_domain_enabled {
        let http_api_endpoint = std::env::var("HTTP_API_ENDPOINT").unwrap_or_else(|_| {
            "https://example.execute-api.us-east-1.amazonaws.com/dev".to_string()
        });
        let path_based_url = format!("{}/{}", http_api_endpoint.trim_end_matches('/'), tunnel_id);
        return (path_based_url.clone(), None, path_based_url);
    }

    let domain = std::env::var("DOMAIN_NAME").unwrap_or_else(|_| "tunnel.example.com".to_string());
    let subdomain_enabled = is_enabled("ENABLE_SUBDOMAIN_ROUTING", true);

    let path_based_url = format!("https://{}/{}", domain, tunnel_id);
    let subdomain_url = if subdomain_enabled {
        Some(format!("https://{}.{}", tunnel_id, domain))
    } else {
        None
    };
    let public_url = subdomain_url.as_ref().unwrap_or(&path_based_url).clone();

    (public_url, subdomain_url, path_based_url)
}

fn requested_tunnel_id(
    request: &ApiGatewayWebsocketProxyRequest,
) -> Result<Option<String>, String> {
    let Some(tunnel_id) = request.query_string_parameters.first("tunnel_id") else {
        return Ok(None);
    };

    validate_tunnel_id(tunnel_id).map_err(|e| format!("Invalid tunnel_id: {}", e))?;

    Ok(Some(tunnel_id.to_string()))
}

async fn tunnel_id_exists(client: &DynamoDbClient, tunnel_id: &str) -> Result<bool, Error> {
    let table_name = env::get_connections_table_name().map_err(|e| format!("{}", e))?;
    let index_name = env::get_tunnel_id_index_name();

    let result = client
        .query()
        .table_name(table_name)
        .index_name(index_name)
        .key_condition_expression("tunnelId = :tunnel_id")
        .expression_attribute_values(":tunnel_id", AttributeValue::S(tunnel_id.to_string()))
        .limit(1)
        .send()
        .await
        .map_err(|e| format!("Failed to check tunnel_id availability: {}", e))?;

    Ok(result
        .items
        .as_ref()
        .map(|items| !items.is_empty())
        .unwrap_or(false))
}

/// Handler for WebSocket $connect route
pub async fn handle_connect(
    event: LambdaEvent<ApiGatewayWebsocketProxyRequest>,
    clients: &SharedClients,
) -> Result<ApiGatewayProxyResponse, Error> {
    use aws_lambda_events::encodings::Body;
    let payload = event.payload;

    // Authenticate request if auth is enabled (before extracting connection_id)
    if let Err(e) = auth::authenticate_request(&payload) {
        error!("Authentication failed: {}", e);
        let mut response = ApiGatewayProxyResponse::default();
        response.status_code = 401;
        response.body = Some(Body::Text("Unauthorized".to_string()));
        return Ok(response);
    }

    let requested_tunnel_id = requested_tunnel_id(&payload);
    let request_context = payload.request_context;
    let connection_id = request_context
        .connection_id
        .ok_or("Missing connection ID")?;

    info!("New WebSocket connection: {}", connection_id);

    let tunnel_id = match requested_tunnel_id {
        Ok(Some(tunnel_id)) => {
            if tunnel_id_exists(&clients.dynamodb, &tunnel_id).await? {
                let mut response = ApiGatewayProxyResponse::default();
                response.status_code = 409;
                response.body = Some(Body::Text(
                    "Requested tunnel_id is already in use".to_string(),
                ));
                return Ok(response);
            }

            info!("Using requested tunnel_id: {}", tunnel_id);
            tunnel_id
        }
        Ok(None) => generate_subdomain(),
        Err(e) => {
            let mut response = ApiGatewayProxyResponse::default();
            response.status_code = 400;
            response.body = Some(Body::Text(e));
            return Ok(response);
        }
    };

    let (public_url, subdomain_url, path_based_url) = build_public_urls(&tunnel_id);

    // Calculate TTL (2 hours from now)
    let created_at = current_timestamp_secs();
    let ttl = calculate_ttl(CONNECTION_TTL_SECS);

    // Store connection metadata in DynamoDB
    let connection_metadata = ConnectionMetadata {
        connection_id: connection_id.clone(),
        tunnel_id: tunnel_id.clone(),
        public_url: public_url.clone(),
        subdomain_url: subdomain_url.clone(),
        path_based_url: Some(path_based_url.clone()),
        created_at,
        ttl,
        client_info: None,
    };

    save_connection_metadata(&clients.dynamodb, &connection_metadata)
        .await
        .map_err(|e| {
            error!(
                "Failed to save connection metadata for {}: {}",
                connection_id, e
            );
            // Sanitize error - don't expose internal details
            sanitize_error(&e)
        })?;

    info!(
        "✅ Tunnel established for connection: {} -> {} (tunnel_id: {})",
        connection_id, public_url, tunnel_id
    );
    info!("🌐 Public URL: {}", public_url);
    if let Some(ref subdomain) = subdomain_url {
        info!("🌐 Subdomain URL: {}", subdomain);
    }
    info!("🌐 Path-based URL: {}", path_based_url);

    // Return success response
    // Note: Forwarder will send Ready message to get connection info
    let mut response = ApiGatewayProxyResponse::default();
    response.status_code = 200;
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::{build_public_urls, requested_tunnel_id};
    use aws_lambda_events::{apigw::ApiGatewayWebsocketProxyRequest, query_map::QueryMap};
    use http_tunnel_common::utils::generate_subdomain;
    use std::collections::HashMap;

    #[test]
    fn test_subdomain_format() {
        let subdomain = generate_subdomain();
        assert_eq!(subdomain.len(), 12);
        assert!(subdomain.chars().all(|c| c.is_ascii_alphanumeric()));
    }

    #[test]
    fn test_public_url_format() {
        let subdomain = "abc123def456";
        let domain = "tunnel.example.com";
        let public_url = format!("https://{}.{}", subdomain, domain);
        assert_eq!(public_url, "https://abc123def456.tunnel.example.com");
    }

    #[test]
    fn test_public_urls_without_custom_domain_use_http_api_endpoint() {
        unsafe {
            std::env::set_var("ENABLE_CUSTOM_DOMAIN", "false");
            std::env::set_var(
                "HTTP_API_ENDPOINT",
                "https://api-id.execute-api.eu-west-1.amazonaws.com/dev",
            );
        }

        let (public_url, subdomain_url, path_based_url) = build_public_urls("abc123def456");

        assert_eq!(
            public_url,
            "https://api-id.execute-api.eu-west-1.amazonaws.com/dev/abc123def456"
        );
        assert_eq!(
            path_based_url,
            "https://api-id.execute-api.eu-west-1.amazonaws.com/dev/abc123def456"
        );
        assert_eq!(subdomain_url, None);
    }

    #[test]
    fn test_requested_tunnel_id_missing() {
        let request = ApiGatewayWebsocketProxyRequest::default();

        assert_eq!(requested_tunnel_id(&request).unwrap(), None);
    }

    #[test]
    fn test_requested_tunnel_id_valid() {
        let mut query_params = HashMap::new();
        query_params.insert("tunnel_id".to_string(), "abc123def456".to_string());

        let mut request = ApiGatewayWebsocketProxyRequest::default();
        request.query_string_parameters = QueryMap::from(query_params);

        assert_eq!(
            requested_tunnel_id(&request).unwrap(),
            Some("abc123def456".to_string())
        );
    }

    #[test]
    fn test_requested_tunnel_id_invalid() {
        let mut query_params = HashMap::new();
        query_params.insert("tunnel_id".to_string(), "invalid-id".to_string());

        let mut request = ApiGatewayWebsocketProxyRequest::default();
        request.query_string_parameters = QueryMap::from(query_params);

        assert!(requested_tunnel_id(&request).is_err());
    }
}
