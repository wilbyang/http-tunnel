# HTTP API v2 Payload Format Migration

## Overview

The http-tunnel Lambda handler currently uses API Gateway REST API (v1) event types, but is deployed with API Gateway HTTP API using payload format version 2.0. This mismatch causes the handler to fail with:

```
ERROR: "Failed to parse HTTP API event: missing field `httpMethod`"
```

## Problem

### Current State

The code uses `ApiGatewayProxyRequest` and `ApiGatewayProxyResponse` from `aws_lambda_events::apigw`, which expect the REST API (v1) payload format:

```rust
// apps/handler/src/handlers/forwarding.rs
use aws_lambda_events::apigw::{ApiGatewayProxyRequest, ApiGatewayProxyResponse};

pub async fn handle_forwarding(
    event: LambdaEvent<ApiGatewayProxyRequest>,  // <-- v1 format
    clients: &SharedClients,
) -> Result<ApiGatewayProxyResponse, Error> {
```

### API Gateway HTTP API v2 Payload Format

The deployed infrastructure uses `payloadFormatVersion: "2.0"`, which sends events in a different format:

**v1 format (REST API):**
```json
{
  "httpMethod": "GET",
  "path": "/api/users",
  "headers": { ... },
  "queryStringParameters": { ... },
  "body": "...",
  "isBase64Encoded": false
}
```

**v2 format (HTTP API):**
```json
{
  "requestContext": {
    "http": {
      "method": "GET",
      "path": "/api/users"
    },
    "requestId": "...",
    "domainName": "ttf.int.tubi.io"
  },
  "headers": { ... },
  "queryStringParameters": { ... },
  "body": "...",
  "isBase64Encoded": false,
  "rawPath": "/api/users",
  "rawQueryString": "..."
}
```

## Solution

### Option A: Update Code to Use v2 Types (Recommended)

Migrate the handler to use `ApiGatewayV2httpRequest` and `ApiGatewayV2httpResponse`.

**Benefits:**
- HTTP API v2 is the modern AWS standard
- Better performance and lower latency
- Lower cost ($1.00/million vs $3.50/million for REST API)
- Already deployed infrastructure uses v2

### Option B: Change Infrastructure to Use v1 Payload Format

Modify the API Gateway integration to use `payloadFormatVersion: "1.0"`.

**Drawbacks:**
- Legacy format, not recommended for new deployments
- Requires infrastructure changes

## Implementation Plan (Option A)

### Phase 1: Update Dependencies

Verify `aws_lambda_events` supports v2 types (it does since v0.6.0):

```rust
// Cargo.toml - no changes needed, types already available
aws_lambda_events = "0.15"  // or your current version
```

### Phase 2: Update Type Imports

**File: `apps/handler/src/handlers/forwarding.rs`**

```rust
// Change from:
use aws_lambda_events::apigw::{ApiGatewayProxyRequest, ApiGatewayProxyResponse};

// To:
use aws_lambda_events::apigw::{ApiGatewayV2httpRequest, ApiGatewayV2httpResponse};
```

**File: `apps/handler/src/lib.rs`**

```rust
// Change from:
use aws_lambda_events::apigw::{ApiGatewayProxyRequest, ApiGatewayProxyResponse};

// To:
use aws_lambda_events::apigw::{ApiGatewayV2httpRequest, ApiGatewayV2httpResponse};
```

### Phase 3: Update Function Signatures

**File: `apps/handler/src/handlers/forwarding.rs`**

```rust
// Change from:
pub async fn handle_forwarding(
    event: LambdaEvent<ApiGatewayProxyRequest>,
    clients: &SharedClients,
) -> Result<ApiGatewayProxyResponse, Error> {

// To:
pub async fn handle_forwarding(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    clients: &SharedClients,
) -> Result<ApiGatewayV2httpResponse, Error> {
```

### Phase 4: Update Field Access Patterns

The v2 request has different field names and structure:

| v1 Field | v2 Field |
|----------|----------|
| `request.http_method` | `request.request_context.http.method` |
| `request.path` | `request.raw_path` or `request.request_context.http.path` |
| `request.headers` | `request.headers` (same) |
| `request.query_string_parameters` | `request.query_string_parameters` (same) |
| `request.body` | `request.body` (same) |
| `request.is_base64_encoded` | `request.is_base64_encoded` (same) |
| `request.request_context.request_id` | `request.request_context.request_id` (same) |

**Update `handle_forwarding` in `forwarding.rs`:**

```rust
pub async fn handle_forwarding(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    clients: &SharedClients,
) -> Result<ApiGatewayV2httpResponse, Error> {
    let request = event.payload;

    // Extract request context info
    let request_id_context = request.request_context.request_id.clone();
    let http_context = &request.request_context.http;

    // Get domain from environment
    let domain = std::env::var("DOMAIN_NAME").unwrap_or_else(|_| "tunnel.example.com".to_string());

    // Extract host header
    let host = request
        .headers
        .get("host")
        .and_then(|h| h.to_str().ok())
        .ok_or_else(|| "Missing Host header".to_string())?;

    // Use raw_path for the original path
    let original_path = request.raw_path.as_deref().unwrap_or("/");

    // ... rest of the handler logic
}
```

### Phase 5: Update `build_http_request` Function

**File: `apps/handler/src/lib.rs`**

```rust
/// Build HttpRequest from API Gateway v2 event
pub fn build_http_request(request: &ApiGatewayV2httpRequest, request_id: String) -> HttpRequest {
    let method = request.request_context.http.method.to_string();

    let uri = format!("{}{}",
        request.raw_path.as_deref().unwrap_or("/"),
        request.raw_query_string.as_ref()
            .filter(|s| !s.is_empty())
            .map(|s| format!("?{}", s))
            .unwrap_or_default()
    );

    let headers = request
        .headers
        .iter()
        .map(|(k, v)| {
            (
                k.as_str().to_string(),
                vec![v.to_str().unwrap_or("").to_string()],
            )
        })
        .collect();

    let body = request
        .body
        .as_ref()
        .map(|b| {
            if request.is_base64_encoded {
                b.to_string()
            } else {
                http_tunnel_common::encode_body(b.as_bytes())
            }
        })
        .unwrap_or_default();

    HttpRequest {
        request_id,
        method,
        uri,
        headers,
        body,
        timestamp: current_timestamp_millis(),
    }
}
```

### Phase 6: Update `build_api_gateway_response` Function

**File: `apps/handler/src/lib.rs`**

```rust
/// Convert HttpResponse to API Gateway v2 response
pub fn build_api_gateway_response(response: HttpResponse) -> ApiGatewayV2httpResponse {
    use http::header::{HeaderName, HeaderValue};

    let headers = response
        .headers
        .iter()
        .filter_map(|(k, v)| {
            v.first().and_then(|val| {
                HeaderName::from_bytes(k.as_bytes())
                    .ok()
                    .and_then(|name| HeaderValue::from_str(val).ok().map(|value| (name, value)))
            })
        })
        .collect();

    ApiGatewayV2httpResponse {
        status_code: response.status_code as i64,
        headers,
        multi_value_headers: Default::default(),
        body: if !response.body.is_empty() {
            Some(aws_lambda_events::encodings::Body::Text(response.body))
        } else {
            None
        },
        is_base64_encoded: true,
        cookies: vec![],
    }
}
```

### Phase 7: Update Error Responses

Update all error responses to use `ApiGatewayV2httpResponse`:

```rust
// Example error response
ApiGatewayV2httpResponse {
    status_code: 413,
    headers: [(
        HeaderName::from_static("content-type"),
        HeaderValue::from_static("text/plain"),
    )]
    .into_iter()
    .collect(),
    multi_value_headers: Default::default(),
    body: Some(Body::Text("Request Entity Too Large".to_string())),
    is_base64_encoded: false,
    cookies: vec![],
}
```

### Phase 8: Update Tests

Update all tests that use `ApiGatewayProxyRequest` or `ApiGatewayProxyResponse`:

```rust
#[test]
fn test_build_http_request_simple_get() {
    let request = ApiGatewayV2httpRequest {
        raw_path: Some("/api/users".to_string()),
        request_context: ApiGatewayV2httpRequestContext {
            http: ApiGatewayV2httpRequestContextHttpDescription {
                method: http::Method::GET,
                path: Some("/api/users".to_string()),
                ..Default::default()
            },
            ..Default::default()
        },
        ..Default::default()
    };

    let http_request = build_http_request(&request, "req_123".to_string());

    assert_eq!(http_request.request_id, "req_123");
    assert_eq!(http_request.method, "GET");
    assert_eq!(http_request.uri, "/api/users");
}
```

## Files to Modify

1. **`apps/handler/src/handlers/forwarding.rs`**
   - Update imports
   - Update function signature
   - Update field access patterns
   - Update error responses

2. **`apps/handler/src/lib.rs`**
   - Update imports
   - Update `build_http_request()` function
   - Update `build_api_gateway_response()` function
   - Update tests

3. **`apps/handler/src/main.rs`**
   - Update the `EventType::HttpApi` case to deserialize as `ApiGatewayV2httpRequest`

## Testing

1. Build and run unit tests:
   ```bash
   cargo test
   ```

2. Build Lambda package:
   ```bash
   cargo lambda build --release --arm64
   ```

3. Deploy and test:
   ```bash
   # Upload new package
   aws s3 cp target/lambda/http-tunnel-handler/bootstrap.zip \
     s3://titc-lambda-deployments/http-tunnel/v0.1.2/bootstrap.zip

   # Update TubiLambdaService to use new version
   # Then test with curl
   curl -v https://ttf.int.tubi.io/health
   ```

## Rollback Plan

If issues arise, either:
1. Revert the code changes and redeploy v0.1.1
2. Or change infrastructure to use `payloadFormatVersion: "1.0"` (not recommended)

## References

- [AWS Lambda Events Rust Crate](https://docs.rs/aws_lambda_events/latest/aws_lambda_events/apigw/index.html)
- [API Gateway HTTP API Payload Format](https://docs.aws.amazon.com/apigateway/latest/developerguide/http-api-develop-integrations-lambda.html)
- [API Gateway REST vs HTTP API](https://docs.aws.amazon.com/apigateway/latest/developerguide/http-api-vs-rest.html)
