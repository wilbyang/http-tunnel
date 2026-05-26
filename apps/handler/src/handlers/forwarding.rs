//! ForwardingHandler - Handles HTTP API requests
//!
//! This module receives public HTTP requests via API Gateway HTTP API (v1 or v2),
//! looks up the connection by subdomain, forwards the request to the agent via WebSocket,
//! and polls for the response. If no response is received within the timeout,
//! it returns a 504 Gateway Timeout.
//!
//! ## API Gateway Version Support
//!
//! This handler supports both API Gateway v1 (REST API) and v2 (HTTP API) formats
//! through the unified `HttpApiRequest` abstraction. The response format automatically
//! matches the request format.

use http_tunnel_common::constants::MAX_BODY_SIZE_BYTES;
use http_tunnel_common::protocol::Message;
use http_tunnel_common::utils::generate_request_id;
use lambda_runtime::{Error, LambdaEvent};
use tracing::{debug, error, info, warn};

use crate::http_api::{HttpApiRequest, HttpApiResponse, error_response_with_headers};
use crate::{
    SharedClients, build_api_gateway_response, build_http_request, content_rewrite,
    detect_routing_mode, lookup_connection_by_tunnel_id, save_pending_request, send_to_connection,
    wait_for_response,
};

fn strip_http_api_stage_prefix(host: &str, path: &str) -> String {
    let Some(endpoint) = std::env::var("HTTP_API_ENDPOINT").ok() else {
        return path.to_string();
    };

    let endpoint = endpoint
        .strip_prefix("https://")
        .or_else(|| endpoint.strip_prefix("http://"))
        .unwrap_or(&endpoint);

    let Some((endpoint_host, endpoint_path)) = endpoint.split_once('/') else {
        return path.to_string();
    };

    if host != endpoint_host {
        return path.to_string();
    }

    let stage_prefix = format!("/{}", endpoint_path.trim_matches('/'));
    if stage_prefix == "/" {
        return path.to_string();
    }

    match path.strip_prefix(&stage_prefix) {
        Some("") => "/".to_string(),
        Some(stripped) if stripped.starts_with('/') => stripped.to_string(),
        Some(stripped) => format!("/{}", stripped),
        None => path.to_string(),
    }
}

/// Handler for HTTP API requests (supports both v1 and v2 formats)
pub async fn handle_forwarding(
    event: LambdaEvent<HttpApiRequest>,
    clients: &SharedClients,
) -> Result<HttpApiResponse, Error> {
    let mut request = event.payload;
    let api_version = request.version();
    let request_id_context = request.request_id().map(|s| s.to_string());

    // Get domain from environment
    let domain = std::env::var("DOMAIN_NAME").unwrap_or_else(|_| "tunnel.example.com".to_string());

    // Extract host header
    let host = request
        .host()
        .ok_or_else(|| "Missing Host header".to_string())?;

    let original_path = request.path();
    let normalized_path = strip_http_api_stage_prefix(host, original_path);

    debug!(
        "Processing HTTP request (API Gateway {:?}), host: {}, path: {}, normalized_path: {}",
        api_version, host, original_path, normalized_path
    );

    // Detect routing mode (subdomain vs path-based)
    let routing_mode = detect_routing_mode(host, &normalized_path, &domain).map_err(|e| {
        error!(
            "Failed to detect routing mode for host {} path {}: {}",
            host, normalized_path, e
        );
        // Sanitized error - don't leak internal details
        "Invalid request".to_string()
    })?;

    let tunnel_id = routing_mode.tunnel_id();
    let forwarding_path = routing_mode.forwarding_path();

    info!(
        "Routing mode: {:?}, tunnel_id: {}, forwarding_path: {}",
        routing_mode, tunnel_id, forwarding_path
    );

    // Update request path to forwarding path
    request.set_path(forwarding_path.to_string());

    // Enforce request size limits
    if let Some(body) = request.body() {
        let body_size = if request.is_base64_encoded() {
            // Estimate decoded size (base64 is ~33% larger than binary)
            (body.len() * 3) / 4
        } else {
            body.len()
        };

        if body_size > MAX_BODY_SIZE_BYTES {
            warn!(
                "Request body too large: {} bytes (max: {} bytes) for tunnel {}",
                body_size, MAX_BODY_SIZE_BYTES, tunnel_id
            );

            return Ok(error_response_with_headers(
                api_version,
                413,
                &format!(
                    "Request body too large: {} bytes (maximum: {} bytes)",
                    body_size, MAX_BODY_SIZE_BYTES
                ),
                &[("x-tunnel-error", "Request Entity Too Large")],
            ));
        }
    }

    // Look up connection ID by tunnel ID
    let connection_id = lookup_connection_by_tunnel_id(&clients.dynamodb, tunnel_id)
        .await
        .map_err(|e| {
            error!(
                "Failed to lookup connection for tunnel_id {}: {}",
                tunnel_id, e
            );
            // Sanitized error - don't leak internal details
            "Tunnel not found or unavailable".to_string()
        })?;

    debug!("Found connection: {}", connection_id);

    // Generate request ID
    let request_id = generate_request_id();

    // Build HttpRequest payload
    let http_request = build_http_request(&request, request_id.clone());

    // Store pending request in DynamoDB for response correlation
    let api_gateway_req_id = request_id_context.as_deref().unwrap_or("unknown");
    save_pending_request(
        &clients.dynamodb,
        &request_id,
        &connection_id,
        api_gateway_req_id,
    )
    .await
    .map_err(|e| {
        error!("Failed to save pending request {}: {}", request_id, e);
        // Sanitized error - don't leak internal details
        "Service temporarily unavailable".to_string()
    })?;

    // Forward request to agent via WebSocket
    let message = Message::HttpRequest(http_request);
    let message_json = serde_json::to_string(&message).map_err(|e| {
        error!("Failed to serialize message: {}", e);
        // Sanitized error - don't leak internal details
        "Service temporarily unavailable".to_string()
    })?;

    let apigw_management = clients
        .apigw_management
        .as_ref()
        .ok_or("API Gateway Management client not initialized")?;

    send_to_connection(apigw_management, &connection_id, &message_json)
        .await
        .map_err(|e| {
            error!(
                "Failed to send request {} to connection {}: {}",
                request_id, connection_id, e
            );
            // Sanitized error - don't leak internal details
            "Tunnel connection unavailable".to_string()
        })?;

    info!(
        "Forwarded request {} to connection {} for tunnel_id {}",
        request_id, connection_id, tunnel_id
    );

    // Poll for response with timeout
    match wait_for_response(&clients.dynamodb, &request_id).await {
        Ok(mut response) => {
            info!(
                "Received response for request {}: status {}",
                request_id, response.status_code
            );

            // Apply content rewriting based on routing mode
            if routing_mode.should_rewrite_content() {
                // Path-based routing: apply content rewriting
                let content_type = response
                    .headers
                    .get("content-type")
                    .and_then(|v| v.first())
                    .map(|s| s.as_str())
                    .unwrap_or("");

                // Only decode and rewrite if content type needs rewriting (performance optimization)
                let should_rewrite = content_rewrite::should_rewrite_content(content_type);

                let (rewritten_body, was_rewritten) = if should_rewrite {
                    // Decode body for rewriting
                    let body_bytes = http_tunnel_common::decode_body(&response.body)
                        .map_err(|e| format!("Failed to decode response body: {}", e))?;
                    let body_str = String::from_utf8_lossy(&body_bytes);

                    // Rewrite content (default strategy: FullRewrite)
                    content_rewrite::rewrite_response_content(
                        &body_str,
                        content_type,
                        tunnel_id,
                        content_rewrite::RewriteStrategy::FullRewrite,
                    )
                    .unwrap_or_else(|e| {
                        warn!("Content rewrite failed: {}, returning original", e);
                        (body_str.to_string(), false)
                    })
                } else {
                    // Skip decoding for binary content (images, videos, etc.)
                    debug!("Skipping rewrite for binary content type: {}", content_type);
                    (String::new(), false)
                };

                if was_rewritten {
                    debug!(
                        "Content rewritten for request {}: {} bytes",
                        request_id,
                        rewritten_body.len()
                    );

                    // Re-encode the rewritten body
                    response.body = http_tunnel_common::encode_body(rewritten_body.as_bytes());

                    // Update Content-Length header
                    response.headers.insert(
                        "content-length".to_string(),
                        vec![rewritten_body.len().to_string()],
                    );

                    // Remove Transfer-Encoding header if present (we're not chunking)
                    response.headers.remove("transfer-encoding");

                    // Add debug header to indicate rewriting was applied
                    response.headers.insert(
                        "x-tunnel-rewrite-applied".to_string(),
                        vec!["true".to_string()],
                    );
                }
            } else {
                // Subdomain-based routing: skip content rewriting
                debug!(
                    "Subdomain mode: skipping content rewriting for request {}",
                    request_id
                );
                response.headers.insert(
                    "x-tunnel-routing-mode".to_string(),
                    vec!["subdomain".to_string()],
                );
            }

            // Convert HttpResponse to API Gateway response (matching request version)
            Ok(build_api_gateway_response(api_version, response))
        }
        Err(e) => {
            error!("Request {} timeout or error: {}", request_id, e);
            // Return 504 Gateway Timeout
            Ok(error_response_with_headers(
                api_version,
                504,
                "Gateway Timeout: No response from agent",
                &[("x-tunnel-error", "Gateway Timeout")],
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http_api::ApiGatewayVersion;

    #[test]
    fn test_timeout_response_format_v1() {
        let response = error_response_with_headers(
            ApiGatewayVersion::V1,
            504,
            "Gateway Timeout: No response from agent",
            &[("x-tunnel-error", "Gateway Timeout")],
        );

        match response {
            HttpApiResponse::V1(resp) => {
                assert_eq!(resp.status_code, 504);
                assert!(!resp.headers.is_empty());
                assert!(resp.body.is_some());
            }
            _ => panic!("Expected V1 response"),
        }
    }

    #[test]
    fn test_timeout_response_format_v2() {
        let response = error_response_with_headers(
            ApiGatewayVersion::V2,
            504,
            "Gateway Timeout: No response from agent",
            &[("x-tunnel-error", "Gateway Timeout")],
        );

        match response {
            HttpApiResponse::V2(resp) => {
                assert_eq!(resp.status_code, 504);
                assert!(!resp.headers.is_empty());
                assert!(resp.body.is_some());
            }
            _ => panic!("Expected V2 response"),
        }
    }

    #[test]
    fn test_body_too_large_response() {
        let response = error_response_with_headers(
            ApiGatewayVersion::V2,
            413,
            "Request body too large: 20000000 bytes (maximum: 10485760 bytes)",
            &[("x-tunnel-error", "Request Entity Too Large")],
        );

        let value = response.into_value().unwrap();
        assert_eq!(value["statusCode"], 413);
    }
}
