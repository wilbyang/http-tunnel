use serde::{Deserialize, Serialize};

/// A request or response body transported either in the WebSocket control message
/// or through the configured object store.
///
/// `Legacy` keeps protocol v1 messages readable during rolling upgrades. Protocol
/// v2 peers serialize `Reference` using the tagged `kind` representation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum BodyRef {
    Legacy(String),
    Reference(BodyLocation),
}

impl Default for BodyRef {
    fn default() -> Self {
        Self::Legacy(String::new())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BodyLocation {
    Inline {
        base64: String,
    },
    External {
        store: ObjectStore,
        key: String,
        total_bytes: u64,
        sha256: String,
        expires_at: u64,
        /// A short-lived GET URL. It is present only when the Lambda sends a
        /// request body to a forwarder. Responses are fetched by Lambda using IAM.
        #[serde(skip_serializing_if = "Option::is_none")]
        download_url: Option<String>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObjectStore {
    S3,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UploadGrant {
    pub store: ObjectStore,
    pub key: String,
    pub upload_url: String,
    pub expires_at: u64,
}

impl BodyRef {
    pub fn legacy(base64: String) -> Self {
        Self::Legacy(base64)
    }

    pub fn inline(base64: String) -> Self {
        Self::Reference(BodyLocation::Inline { base64 })
    }

    pub fn external(
        key: String,
        total_bytes: u64,
        sha256: String,
        expires_at: u64,
        download_url: Option<String>,
    ) -> Self {
        Self::Reference(BodyLocation::External {
            store: ObjectStore::S3,
            key,
            total_bytes,
            sha256,
            expires_at,
            download_url,
        })
    }

    pub fn is_empty(&self) -> bool {
        match self {
            Self::Legacy(body) => body.is_empty(),
            Self::Reference(BodyLocation::Inline { base64 }) => base64.is_empty(),
            Self::Reference(BodyLocation::External { total_bytes, .. }) => *total_bytes == 0,
        }
    }

    pub fn inline_base64(&self) -> Option<&str> {
        match self {
            Self::Legacy(body) => Some(body),
            Self::Reference(BodyLocation::Inline { base64 }) => Some(base64),
            Self::Reference(BodyLocation::External { .. }) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_string_remains_compatible() {
        let body: BodyRef = serde_json::from_str(r#""dGVzdA==""#).unwrap();
        assert_eq!(body.inline_base64(), Some("dGVzdA=="));
        assert_eq!(serde_json::to_string(&body).unwrap(), r#""dGVzdA==""#);
    }

    #[test]
    fn external_reference_has_tagged_shape() {
        let body = BodyRef::external(
            "transfers/request/response".to_string(),
            42,
            "abc123".to_string(),
            1000,
            None,
        );
        let json = serde_json::to_value(body).unwrap();
        assert_eq!(json["kind"], "external");
        assert_eq!(json["store"], "s3");
        assert_eq!(json["total_bytes"], 42);
        assert!(json.get("download_url").is_none());
    }
}
