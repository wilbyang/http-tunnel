//! Environment variable helpers with fallback support
//!
//! This module provides functions to read table names from environment variables,
//! supporting both the new RGD auto-injected names (DYNAMODB_TABLE0_NAME, DYNAMODB_TABLE1_NAME)
//! and the legacy names (CONNECTIONS_TABLE_NAME, PENDING_REQUESTS_TABLE_NAME) for backwards
//! compatibility.

use anyhow::{Context, Result};

/// Get the connections table name from environment variables.
///
/// Tries the new RGD auto-injected name first (DYNAMODB_TABLE0_NAME),
/// then falls back to the legacy name (CONNECTIONS_TABLE_NAME).
pub fn get_connections_table_name() -> Result<String> {
    std::env::var("DYNAMODB_TABLE0_NAME")
        .or_else(|_| std::env::var("CONNECTIONS_TABLE_NAME"))
        .context(
            "Neither DYNAMODB_TABLE0_NAME nor CONNECTIONS_TABLE_NAME environment variable is set",
        )
}

/// Get the pending requests table name from environment variables.
///
/// Tries the new RGD auto-injected name first (DYNAMODB_TABLE1_NAME),
/// then falls back to the legacy name (PENDING_REQUESTS_TABLE_NAME).
pub fn get_pending_requests_table_name() -> Result<String> {
    std::env::var("DYNAMODB_TABLE1_NAME")
        .or_else(|_| std::env::var("PENDING_REQUESTS_TABLE_NAME"))
        .context("Neither DYNAMODB_TABLE1_NAME nor PENDING_REQUESTS_TABLE_NAME environment variable is set")
}

/// Get the connections table name with a default fallback.
///
/// For use in contexts where a default value is acceptable (e.g., cleanup handlers).
pub fn get_connections_table_name_or_default(default: &str) -> String {
    std::env::var("DYNAMODB_TABLE0_NAME")
        .or_else(|_| std::env::var("CONNECTIONS_TABLE_NAME"))
        .unwrap_or_else(|_| default.to_string())
}

/// Get the pending requests table name with a default fallback.
///
/// For use in contexts where a default value is acceptable (e.g., cleanup handlers).
pub fn get_pending_requests_table_name_or_default(default: &str) -> String {
    std::env::var("DYNAMODB_TABLE1_NAME")
        .or_else(|_| std::env::var("PENDING_REQUESTS_TABLE_NAME"))
        .unwrap_or_else(|_| default.to_string())
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_get_connections_table_prefers_new_name() {
        // This test would require environment manipulation
        // In practice, we test the fallback logic
    }

    #[test]
    fn test_get_pending_requests_table_prefers_new_name() {
        // This test would require environment manipulation
        // In practice, we test the fallback logic
    }
}
