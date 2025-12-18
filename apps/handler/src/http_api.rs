//! HTTP API Gateway Abstraction Layer
//!
//! This module provides a unified abstraction over API Gateway v1 (REST API) and v2 (HTTP API)
//! event formats. The Lambda handler can receive either format depending on the infrastructure
//! configuration, so we need to support both seamlessly.
//!
//! ## Design
//!
//! The core abstraction is `HttpApiRequest` which wraps either format and provides
//! a consistent interface for extracting request data. Similarly, `HttpApiResponse`
//! can produce the correct response format based on the request version.
//!
//! ## Detection Strategy
//!
//! v2 format has `requestContext.http` object, v1 format has `httpMethod` at root level.
//! We attempt v2 deserialization first since that's our current infrastructure.

use aws_lambda_events::apigw::{
    ApiGatewayProxyRequest, ApiGatewayProxyResponse, ApiGatewayV2httpRequest,
    ApiGatewayV2httpResponse,
};
use aws_lambda_events::encodings::Body;
use http::header::{HeaderMap, HeaderName, HeaderValue};
use serde_json::Value;

/// API Gateway version detected from the incoming event
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApiGatewayVersion {
    /// REST API (v1) - uses `httpMethod`, `path` at root level
    V1,
    /// HTTP API (v2) - uses `requestContext.http.method`, `rawPath`
    V2,
}

/// Unified HTTP API request that can hold either v1 or v2 format
#[derive(Debug)]
pub enum HttpApiRequest {
    V1(ApiGatewayProxyRequest),
    V2(ApiGatewayV2httpRequest),
}

impl HttpApiRequest {
    /// Detect and deserialize the appropriate request type from raw JSON
    ///
    /// Detection strategy:
    /// 1. Check for `requestContext.http` - if present, it's v2
    /// 2. Check for `httpMethod` at root - if present, it's v1
    /// 3. Otherwise, try v2 first (our current infra), then v1 as fallback
    ///
    /// Note: We also normalize the event to ensure `requestContext.httpMethod` is present
    /// for v1 events, as some API Gateway configurations may omit it.
    pub fn from_value(value: Value) -> Result<Self, String> {
        // Detection based on structure
        let is_v2 = value
            .get("requestContext")
            .and_then(|rc| rc.get("http"))
            .is_some();

        let is_v1 = value.get("httpMethod").is_some();

        if is_v2 {
            serde_json::from_value::<ApiGatewayV2httpRequest>(value)
                .map(HttpApiRequest::V2)
                .map_err(|e| format!("Failed to parse HTTP API v2 event: {}", e))
        } else if is_v1 {
            // Normalize v1 event: ensure requestContext.httpMethod is present
            let normalized = Self::normalize_v1_event(value);
            serde_json::from_value::<ApiGatewayProxyRequest>(normalized)
                .map(HttpApiRequest::V1)
                .map_err(|e| format!("Failed to parse HTTP API v1 event: {}", e))
        } else {
            // Try v2 first (current infrastructure), then v1 as fallback
            serde_json::from_value::<ApiGatewayV2httpRequest>(value.clone())
                .map(HttpApiRequest::V2)
                .or_else(|_| {
                    let normalized = Self::normalize_v1_event(value);
                    serde_json::from_value::<ApiGatewayProxyRequest>(normalized)
                        .map(HttpApiRequest::V1)
                })
                .map_err(|e| {
                    format!(
                        "Failed to parse HTTP API event (tried both v1 and v2): {}",
                        e
                    )
                })
        }
    }

    /// Normalize a v1 API Gateway event to ensure all required fields are present.
    ///
    /// The `aws_lambda_events` crate requires `requestContext.httpMethod` to be present,
    /// but some API Gateway configurations (like HTTP API with payload format v1.0)
    /// may only include `httpMethod` at the root level. This function copies the
    /// root-level `httpMethod` to `requestContext` if it's missing there.
    fn normalize_v1_event(mut value: Value) -> Value {
        // Copy httpMethod from root to requestContext if missing
        if let Some(http_method) = value.get("httpMethod").cloned()
            && let Some(request_context) = value.get_mut("requestContext")
            && let Some(rc_obj) = request_context.as_object_mut()
            && !rc_obj.contains_key("httpMethod")
        {
            rc_obj.insert("httpMethod".to_string(), http_method);
        }
        value
    }

    /// Get the API Gateway version
    pub fn version(&self) -> ApiGatewayVersion {
        match self {
            HttpApiRequest::V1(_) => ApiGatewayVersion::V1,
            HttpApiRequest::V2(_) => ApiGatewayVersion::V2,
        }
    }

    /// Get the HTTP method
    pub fn method(&self) -> String {
        match self {
            HttpApiRequest::V1(req) => req.http_method.to_string(),
            HttpApiRequest::V2(req) => req.request_context.http.method.to_string(),
        }
    }

    /// Get the request path
    pub fn path(&self) -> &str {
        match self {
            HttpApiRequest::V1(req) => req.path.as_deref().unwrap_or("/"),
            HttpApiRequest::V2(req) => req.raw_path.as_deref().unwrap_or("/"),
        }
    }

    /// Set the path (for path rewriting in forwarding)
    pub fn set_path(&mut self, new_path: String) {
        match self {
            HttpApiRequest::V1(req) => req.path = Some(new_path),
            HttpApiRequest::V2(req) => req.raw_path = Some(new_path),
        }
    }

    /// Get the Host header value
    pub fn host(&self) -> Option<&str> {
        match self {
            HttpApiRequest::V1(req) => req
                .headers
                .get("host")
                .or_else(|| req.headers.get("Host"))
                .and_then(|h| h.to_str().ok()),
            HttpApiRequest::V2(req) => req
                .headers
                .get("host")
                .or_else(|| req.headers.get("Host"))
                .and_then(|h| h.to_str().ok()),
        }
    }

    /// Get the request context request ID
    pub fn request_id(&self) -> Option<&str> {
        match self {
            HttpApiRequest::V1(req) => req.request_context.request_id.as_deref(),
            HttpApiRequest::V2(req) => req.request_context.request_id.as_deref(),
        }
    }

    /// Get the request body
    pub fn body(&self) -> Option<&str> {
        match self {
            HttpApiRequest::V1(req) => req.body.as_deref(),
            HttpApiRequest::V2(req) => req.body.as_deref(),
        }
    }

    /// Check if body is base64 encoded
    pub fn is_base64_encoded(&self) -> bool {
        match self {
            HttpApiRequest::V1(req) => req.is_base64_encoded,
            HttpApiRequest::V2(req) => req.is_base64_encoded,
        }
    }

    /// Get headers as an iterator of (name, value) pairs
    pub fn headers(&self) -> impl Iterator<Item = (&str, &str)> {
        match self {
            HttpApiRequest::V1(req) => HeaderIterator::V1(req.headers.iter()),
            HttpApiRequest::V2(req) => HeaderIterator::V2(req.headers.iter()),
        }
    }

    /// Get a reference to the underlying HeaderMap
    pub fn header_map(&self) -> &HeaderMap {
        match self {
            HttpApiRequest::V1(req) => &req.headers,
            HttpApiRequest::V2(req) => &req.headers,
        }
    }

    /// Get query string (formatted as key=value&key2=value2)
    pub fn query_string(&self) -> String {
        match self {
            HttpApiRequest::V1(req) => {
                let params = &req.query_string_parameters;
                if params.is_empty() {
                    String::new()
                } else {
                    params
                        .iter()
                        .map(|(k, v)| format!("{}={}", k, v))
                        .collect::<Vec<_>>()
                        .join("&")
                }
            }
            HttpApiRequest::V2(req) => {
                // v2 has raw_query_string which preserves the original format
                req.raw_query_string.clone().unwrap_or_default()
            }
        }
    }
}

/// Iterator over headers that works with both v1 and v2 formats
enum HeaderIterator<'a> {
    V1(http::header::Iter<'a, HeaderValue>),
    V2(http::header::Iter<'a, HeaderValue>),
}

impl<'a> Iterator for HeaderIterator<'a> {
    type Item = (&'a str, &'a str);

    fn next(&mut self) -> Option<Self::Item> {
        match self {
            HeaderIterator::V1(iter) => iter
                .next()
                .and_then(|(k, v)| v.to_str().ok().map(|v| (k.as_str(), v))),
            HeaderIterator::V2(iter) => iter
                .next()
                .and_then(|(k, v)| v.to_str().ok().map(|v| (k.as_str(), v))),
        }
    }
}

/// Unified HTTP API response builder
///
/// Creates responses that match the request version automatically
#[derive(Debug)]
pub struct HttpApiResponseBuilder {
    version: ApiGatewayVersion,
    status_code: i64,
    headers: HeaderMap,
    body: Option<String>,
    is_base64_encoded: bool,
}

impl HttpApiResponseBuilder {
    /// Create a new response builder for the given API Gateway version
    pub fn new(version: ApiGatewayVersion) -> Self {
        Self {
            version,
            status_code: 200,
            headers: HeaderMap::new(),
            body: None,
            is_base64_encoded: false,
        }
    }

    /// Create a response builder matching the request version
    pub fn for_request(request: &HttpApiRequest) -> Self {
        Self::new(request.version())
    }

    /// Set the status code
    pub fn status_code(mut self, code: i64) -> Self {
        self.status_code = code;
        self
    }

    /// Set a header
    pub fn header(mut self, name: &'static str, value: &str) -> Self {
        let header_name = HeaderName::from_static(name);
        if let Ok(header_value) = HeaderValue::from_str(value) {
            self.headers.insert(header_name, header_value);
        }
        self
    }

    /// Set headers from a HeaderMap
    pub fn headers(mut self, headers: HeaderMap) -> Self {
        self.headers = headers;
        self
    }

    /// Set the body
    pub fn body(mut self, body: String) -> Self {
        self.body = Some(body);
        self
    }

    /// Set the body as optional
    pub fn body_opt(mut self, body: Option<String>) -> Self {
        self.body = body;
        self
    }

    /// Set whether the body is base64 encoded
    pub fn base64_encoded(mut self, encoded: bool) -> Self {
        self.is_base64_encoded = encoded;
        self
    }

    /// Build the response as a serde_json::Value (works for both versions)
    pub fn build_value(self) -> serde_json::Value {
        match self.version {
            ApiGatewayVersion::V1 => {
                let response = self.build_v1();
                serde_json::to_value(response).unwrap_or_default()
            }
            ApiGatewayVersion::V2 => {
                let response = self.build_v2();
                serde_json::to_value(response).unwrap_or_default()
            }
        }
    }

    /// Build as v1 response
    pub fn build_v1(self) -> ApiGatewayProxyResponse {
        let body = self.body.map(Body::Text);

        let mut response = ApiGatewayProxyResponse::default();
        response.status_code = self.status_code;
        response.headers = self.headers;
        response.body = body;
        response.is_base64_encoded = self.is_base64_encoded;
        response
    }

    /// Build as v2 response
    pub fn build_v2(self) -> ApiGatewayV2httpResponse {
        let body = self.body.map(Body::Text);

        let mut response = ApiGatewayV2httpResponse::default();
        response.status_code = self.status_code;
        response.headers = self.headers;
        response.body = body;
        response.is_base64_encoded = self.is_base64_encoded;
        response
    }
}

/// Unified response enum for returning from handlers
#[derive(Debug)]
pub enum HttpApiResponse {
    V1(ApiGatewayProxyResponse),
    V2(ApiGatewayV2httpResponse),
}

impl HttpApiResponse {
    /// Create a response matching the request version
    pub fn from_builder(builder: HttpApiResponseBuilder) -> Self {
        match builder.version {
            ApiGatewayVersion::V1 => HttpApiResponse::V1(builder.build_v1()),
            ApiGatewayVersion::V2 => HttpApiResponse::V2(builder.build_v2()),
        }
    }

    /// Convert to serde_json::Value for Lambda response
    pub fn into_value(self) -> Result<Value, String> {
        match self {
            HttpApiResponse::V1(resp) => serde_json::to_value(resp)
                .map_err(|e| format!("Failed to serialize v1 response: {}", e)),
            HttpApiResponse::V2(resp) => serde_json::to_value(resp)
                .map_err(|e| format!("Failed to serialize v2 response: {}", e)),
        }
    }
}

/// Helper to create error responses
pub fn error_response(
    version: ApiGatewayVersion,
    status_code: i64,
    message: &str,
) -> HttpApiResponse {
    HttpApiResponse::from_builder(
        HttpApiResponseBuilder::new(version)
            .status_code(status_code)
            .header("content-type", "text/plain")
            .body(message.to_string())
            .base64_encoded(false),
    )
}

/// Helper to create error responses with custom headers
pub fn error_response_with_headers(
    version: ApiGatewayVersion,
    status_code: i64,
    message: &str,
    extra_headers: &[(&'static str, &str)],
) -> HttpApiResponse {
    let mut builder = HttpApiResponseBuilder::new(version)
        .status_code(status_code)
        .header("content-type", "text/plain")
        .body(message.to_string())
        .base64_encoded(false);

    for (name, value) in extra_headers {
        builder = builder.header(name, value);
    }

    HttpApiResponse::from_builder(builder)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_detect_v1_format() {
        // V1 format requires requestContext with identity and http_method fields
        let event = json!({
            "httpMethod": "GET",
            "path": "/api/users",
            "headers": {},
            "queryStringParameters": null,
            "body": null,
            "isBase64Encoded": false,
            "requestContext": {
                "requestId": "test-123",
                "httpMethod": "GET",
                "identity": {}
            }
        });

        let request = HttpApiRequest::from_value(event).unwrap();
        assert_eq!(request.version(), ApiGatewayVersion::V1);
        assert_eq!(request.method(), "GET");
        assert_eq!(request.path(), "/api/users");
    }

    #[test]
    fn test_detect_v2_format() {
        let event = json!({
            "version": "2.0",
            "rawPath": "/api/users",
            "rawQueryString": "",
            "headers": {},
            "requestContext": {
                "http": {
                    "method": "GET",
                    "path": "/api/users"
                },
                "requestId": "test-456"
            },
            "isBase64Encoded": false
        });

        let request = HttpApiRequest::from_value(event).unwrap();
        assert_eq!(request.version(), ApiGatewayVersion::V2);
        assert_eq!(request.method(), "GET");
        assert_eq!(request.path(), "/api/users");
    }

    #[test]
    fn test_v1_response_builder() {
        let response = HttpApiResponseBuilder::new(ApiGatewayVersion::V1)
            .status_code(200)
            .header("content-type", "application/json")
            .body(r#"{"status":"ok"}"#.to_string())
            .build_v1();

        assert_eq!(response.status_code, 200);
        assert!(response.body.is_some());
    }

    #[test]
    fn test_v2_response_builder() {
        let response = HttpApiResponseBuilder::new(ApiGatewayVersion::V2)
            .status_code(201)
            .header("content-type", "application/json")
            .body(r#"{"id":123}"#.to_string())
            .build_v2();

        assert_eq!(response.status_code, 201);
        assert!(response.body.is_some());
        assert!(response.cookies.is_empty());
    }

    #[test]
    fn test_error_response() {
        let resp = error_response(ApiGatewayVersion::V2, 500, "Internal Server Error");
        let value = resp.into_value().unwrap();
        assert_eq!(value["statusCode"], 500);
    }

    #[test]
    fn test_v2_with_query_string() {
        let event = json!({
            "version": "2.0",
            "rawPath": "/api/search",
            "rawQueryString": "q=test&limit=10",
            "headers": {},
            "requestContext": {
                "http": {
                    "method": "GET",
                    "path": "/api/search"
                },
                "requestId": "test-789"
            },
            "isBase64Encoded": false
        });

        let request = HttpApiRequest::from_value(event).unwrap();
        assert_eq!(request.query_string(), "q=test&limit=10");
    }

    #[test]
    fn test_host_header_extraction() {
        let event = json!({
            "version": "2.0",
            "rawPath": "/",
            "rawQueryString": "",
            "headers": {
                "host": "example.tunnel.io"
            },
            "requestContext": {
                "http": {
                    "method": "GET",
                    "path": "/"
                },
                "requestId": "test-host"
            },
            "isBase64Encoded": false
        });

        let request = HttpApiRequest::from_value(event).unwrap();
        assert_eq!(request.host(), Some("example.tunnel.io"));
    }

    #[test]
    fn test_v1_format_without_requestcontext_httpmethod() {
        // HTTP API with payload format 1.0 may omit httpMethod in requestContext
        // but include it at the root level. This test ensures we normalize the event.
        let event = json!({
            "version": "1.0",
            "httpMethod": "POST",
            "path": "/api/data",
            "headers": {
                "content-type": "application/json"
            },
            "queryStringParameters": null,
            "body": "{\"key\":\"value\"}",
            "isBase64Encoded": false,
            "requestContext": {
                "accountId": "123456789012",
                "apiId": "api-id",
                "domainName": "id.execute-api.us-east-1.amazonaws.com",
                "domainPrefix": "id",
                "requestId": "test-no-httpmethod",
                "identity": {
                    "sourceIp": "192.0.2.1",
                    "userAgent": "test-agent"
                },
                "stage": "$default"
            }
        });

        let request = HttpApiRequest::from_value(event).unwrap();
        assert_eq!(request.version(), ApiGatewayVersion::V1);
        assert_eq!(request.method(), "POST");
        assert_eq!(request.path(), "/api/data");
    }

    #[test]
    fn test_normalize_v1_event() {
        let event = json!({
            "httpMethod": "GET",
            "requestContext": {
                "requestId": "test-123"
            }
        });

        let normalized = HttpApiRequest::normalize_v1_event(event);

        // Check that httpMethod was copied to requestContext
        assert_eq!(normalized["requestContext"]["httpMethod"], json!("GET"));
    }

    #[test]
    fn test_normalize_v1_event_preserves_existing() {
        // If requestContext already has httpMethod, don't overwrite
        let event = json!({
            "httpMethod": "POST",
            "requestContext": {
                "requestId": "test-123",
                "httpMethod": "GET"  // Different from root
            }
        });

        let normalized = HttpApiRequest::normalize_v1_event(event);

        // Should preserve existing value
        assert_eq!(normalized["requestContext"]["httpMethod"], json!("GET"));
    }
}
