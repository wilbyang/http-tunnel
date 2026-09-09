use anyhow::{Context, Result, anyhow};
use aws_sdk_s3::{Client, presigning::PresigningConfig, primitives::ByteStream};
use http_tunnel_common::constants::TRANSFER_URL_TTL_SECS;
use sha2::{Digest, Sha256};
use std::time::Duration;

use crate::env;

pub fn request_key(request_id: &str) -> String {
    format!("transfers/{request_id}/request")
}

pub fn response_key(request_id: &str) -> String {
    format!("transfers/{request_id}/response")
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn presigning_config() -> Result<PresigningConfig> {
    PresigningConfig::expires_in(Duration::from_secs(TRANSFER_URL_TTL_SECS))
        .context("Failed to configure S3 presigned URL")
}

pub async fn put(client: &Client, key: &str, bytes: Vec<u8>) -> Result<()> {
    client
        .put_object()
        .bucket(env::get_transfer_bucket_name()?)
        .key(key)
        .body(ByteStream::from(bytes))
        .send()
        .await
        .context("Failed to upload transfer body to S3")?;
    Ok(())
}

pub async fn get(client: &Client, key: &str, max_bytes: u64) -> Result<Vec<u8>> {
    let output = client
        .get_object()
        .bucket(env::get_transfer_bucket_name()?)
        .key(key)
        .send()
        .await
        .context("Failed to download transfer body from S3")?;

    if output.content_length().unwrap_or_default() as u64 > max_bytes {
        return Err(anyhow!("External body exceeds the configured size limit"));
    }

    let bytes = output
        .body
        .collect()
        .await
        .context("Failed to read transfer body from S3")?
        .into_bytes();
    if bytes.len() as u64 > max_bytes {
        return Err(anyhow!("External body exceeds the configured size limit"));
    }
    Ok(bytes.to_vec())
}

pub async fn presign_get(client: &Client, key: &str) -> Result<String> {
    Ok(client
        .get_object()
        .bucket(env::get_transfer_bucket_name()?)
        .key(key)
        .presigned(presigning_config()?)
        .await
        .context("Failed to presign S3 download")?
        .uri()
        .to_string())
}

pub async fn presign_put(client: &Client, key: &str) -> Result<String> {
    Ok(client
        .put_object()
        .bucket(env::get_transfer_bucket_name()?)
        .key(key)
        .presigned(presigning_config()?)
        .await
        .context("Failed to presign S3 upload")?
        .uri()
        .to_string())
}

pub async fn delete(client: &Client, key: &str) {
    if let Ok(bucket) = env::get_transfer_bucket_name()
        && let Err(error) = client.delete_object().bucket(bucket).key(key).send().await
    {
        tracing::warn!(key, %error, "Failed to delete transfer object");
    }
}
