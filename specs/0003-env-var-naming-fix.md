# Environment Variable Naming Fix

## Problem

The http-tunnel code expects these environment variables:
- `CONNECTIONS_TABLE_NAME`
- `PENDING_REQUESTS_TABLE_NAME`

But the TubiLambdaService RGD auto-injects:
- `DYNAMODB_TABLE0_NAME` (for table0 = connections)
- `DYNAMODB_TABLE1_NAME` (for table1 = pendingRequests)

The RGD's `env` field uses a `[]string` format with `KEY=VALUE` entries, but due to CEL limitations, these are stored as values of `ENV_0`, `ENV_1`, etc. keys - not as actual environment variables with the specified key names.

## Solution

Update the http-tunnel code to use the RGD's auto-injected environment variable names.

### Files to Update

**File: `apps/handler/src/lib.rs`**

Change all occurrences of:
```rust
std::env::var("CONNECTIONS_TABLE_NAME")
```
to:
```rust
std::env::var("DYNAMODB_TABLE0_NAME")
```

Change all occurrences of:
```rust
std::env::var("PENDING_REQUESTS_TABLE_NAME")
```
to:
```rust
std::env::var("DYNAMODB_TABLE1_NAME")
```

### Specific Changes

1. **`lookup_connection_by_tunnel_id` function:**
```rust
// Change from:
let table_name = std::env::var("CONNECTIONS_TABLE_NAME")
    .context("CONNECTIONS_TABLE_NAME environment variable not set")?;

// To:
let table_name = std::env::var("DYNAMODB_TABLE0_NAME")
    .context("DYNAMODB_TABLE0_NAME environment variable not set")?;
```

2. **`save_connection_metadata` function:**
```rust
// Change from:
let table_name = std::env::var("CONNECTIONS_TABLE_NAME")
    .context("CONNECTIONS_TABLE_NAME environment variable not set")?;

// To:
let table_name = std::env::var("DYNAMODB_TABLE0_NAME")
    .context("DYNAMODB_TABLE0_NAME environment variable not set")?;
```

3. **`delete_connection` function:**
```rust
// Change from:
let table_name = std::env::var("CONNECTIONS_TABLE_NAME")
    .context("CONNECTIONS_TABLE_NAME environment variable not set")?;

// To:
let table_name = std::env::var("DYNAMODB_TABLE0_NAME")
    .context("DYNAMODB_TABLE0_NAME environment variable not set")?;
```

4. **`save_pending_request` function:**
```rust
// Change from:
let table_name = std::env::var("PENDING_REQUESTS_TABLE_NAME")
    .context("PENDING_REQUESTS_TABLE_NAME environment variable not set")?;

// To:
let table_name = std::env::var("DYNAMODB_TABLE1_NAME")
    .context("DYNAMODB_TABLE1_NAME environment variable not set")?;
```

5. **`wait_for_response_*` functions:**
```rust
// Change from:
let table_name = std::env::var("PENDING_REQUESTS_TABLE_NAME")
    .context("PENDING_REQUESTS_TABLE_NAME environment variable not set")?;

// To:
let table_name = std::env::var("DYNAMODB_TABLE1_NAME")
    .context("DYNAMODB_TABLE1_NAME environment variable not set")?;
```

6. **`update_pending_request_with_response` function:**
```rust
// Change from:
let table_name = std::env::var("PENDING_REQUESTS_TABLE_NAME")
    .context("PENDING_REQUESTS_TABLE_NAME environment variable not set")?;

// To:
let table_name = std::env::var("DYNAMODB_TABLE1_NAME")
    .context("DYNAMODB_TABLE1_NAME environment variable not set")?;
```

### Also Check

- `apps/handler/src/handlers/` - any files that read these env vars
- `apps/handler/src/handlers/connect.rs` - likely uses CONNECTIONS_TABLE_NAME
- `apps/handler/src/handlers/disconnect.rs` - likely uses CONNECTIONS_TABLE_NAME
- `apps/handler/src/handlers/response.rs` - likely uses PENDING_REQUESTS_TABLE_NAME

### After Update

1. Build and upload new version:
```bash
cargo lambda build --release --arm64
aws s3 cp target/lambda/http-tunnel-handler/bootstrap.zip \
  s3://titc-lambda-deployments/http-tunnel/v0.3.2/bootstrap.zip \
  --profile internal-tools-infra-admin
```

2. Update deploy.yaml to use v0.3.2

3. Remove the env vars from deploy.yaml that are no longer needed:
```yaml
env:
  - "RUST_LOG=info"
  - "ENABLE_SUBDOMAIN_ROUTING=true"
  - "USE_EVENT_DRIVEN=true"
  - "REQUIRE_AUTH=false"
  - "PER_TUNNEL_RATE_LIMIT=1000"
  # Remove these - no longer needed:
  # - "CONNECTIONS_TABLE_NAME=..."
  # - "PENDING_REQUESTS_TABLE_NAME=..."
```

## Alternative: Update deploy.yaml (Not Recommended)

Instead of updating code, you could hardcode env vars using Lambda's native environment variable format. However, the RGD doesn't support this pattern well, so code changes are preferred.
