/// Response chunking utilities for handling large responses over 32KB API Gateway limit
///
/// AWS API Gateway has a 32KB per-message limit for WebSocket messages. To support
/// larger HTTP responses, we split them into chunks and reassemble on the receiving end.

use crate::protocol::Message;
use crate::HttpResponse;

/// Maximum safe chunk size that leaves room for JSON overhead
/// 32KB API Gateway limit - JSON overhead (~500 bytes for metadata)
const CHUNK_SIZE_BYTES: usize = 30 * 1024; // 30KB per chunk

/// Check if a response needs chunking
pub fn should_chunk_response(response: &HttpResponse) -> bool {
    // Estimate JSON size: response metadata + base64 body
    // Base64 is ~33% larger than binary, plus JSON overhead
    let estimated_json_size = estimate_response_json_size(response);
    estimated_json_size > 28 * 1024 // Leave 4KB buffer from 32KB limit
}

/// Estimate the JSON serialized size of an HttpResponse
fn estimate_response_json_size(response: &HttpResponse) -> usize {
    // Rough estimate: response metadata (~200 bytes) + body size
    200 + response.body.len()
}

/// Split a response into chunks
///
/// Returns a vector of Message::ResponseChunk messages that should be sent in order.
/// The receiving end will reassemble them by matching request_id and collecting chunks in order.
pub fn chunk_response(response: HttpResponse) -> Vec<Message> {
    let request_id = response.request_id.clone();
    let body = response.body.clone();

    // If body is small enough, send as regular response
    if body.len() <= CHUNK_SIZE_BYTES {
        return vec![Message::HttpResponse(response)];
    }

    // Split body into chunks
    let mut chunks = Vec::new();
    let total_chunks = (body.len() + CHUNK_SIZE_BYTES - 1) / CHUNK_SIZE_BYTES;

    for (chunk_index, chunk) in body
        .as_bytes()
        .chunks(CHUNK_SIZE_BYTES)
        .enumerate()
    {
        let chunk_data = String::from_utf8_lossy(chunk).to_string();
        chunks.push(Message::ResponseChunk {
            request_id: request_id.clone(),
            chunk_index: chunk_index as u32,
            total_chunks: total_chunks as u32,
            chunk_data,
        });
    }

    chunks
}

/// Reassemble chunks back into a response
///
/// Collects chunks indexed by chunk_index. Returns Some(HttpResponse) when all chunks
/// are received, or None if any are missing or out of order.
pub fn reassemble_chunks(
    response_template: HttpResponse,
    chunks: &[(u32, String)], // (chunk_index, chunk_data)
) -> Option<HttpResponse> {
    if chunks.is_empty() {
        return Some(response_template);
    }

    // Verify we have all chunks
    let total_chunks = chunks.iter().map(|(i, _)| i + 1).max().unwrap_or(0);
    if chunks.len() as u32 != total_chunks {
        return None;
    }

    // Verify sequential indices
    for (i, (idx, _)) in chunks.iter().enumerate() {
        if *idx != i as u32 {
            return None;
        }
    }

    // Reassemble body
    let body = chunks.iter().map(|(_, data)| data.as_str()).collect::<String>();

    let mut reassembled = response_template;
    reassembled.body = body;
    Some(reassembled)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn make_test_response(body_size: usize) -> HttpResponse {
        HttpResponse {
            request_id: "test-123".to_string(),
            status_code: 200,
            headers: HashMap::from([(
                "content-type".to_string(),
                vec!["text/plain".to_string()],
            )]),
            body: "x".repeat(body_size),
            processing_time_ms: 0,
        }
    }

    #[test]
    fn test_small_response_not_chunked() {
        let response = make_test_response(1000);
        assert!(!should_chunk_response(&response));
    }

    #[test]
    fn test_large_response_chunked() {
        let response = make_test_response(100 * 1024); // 100KB
        assert!(should_chunk_response(&response));
    }

    #[test]
    fn test_chunk_small_response() {
        let response = make_test_response(1000);
        let chunks = chunk_response(response.clone());
        assert_eq!(chunks.len(), 1);
        match &chunks[0] {
            Message::HttpResponse(resp) => {
                assert_eq!(resp.body, response.body);
            }
            _ => panic!("Expected HttpResponse"),
        }
    }

    #[test]
    fn test_chunk_large_response() {
        let response = make_test_response(100 * 1024); // 100KB
        let chunks = chunk_response(response.clone());

        // Should be split into multiple chunks
        assert!(chunks.len() > 1);

        // All should be ResponseChunk messages
        for chunk in &chunks {
            match chunk {
                Message::ResponseChunk {
                    request_id,
                    chunk_index,
                    total_chunks,
                    chunk_data,
                } => {
                    assert_eq!(request_id, &response.request_id);
                    assert!(*chunk_index < *total_chunks);
                    assert!(!chunk_data.is_empty());
                }
                _ => panic!("Expected ResponseChunk"),
            }
        }
    }

    #[test]
    fn test_reassemble_chunks() {
        let original = make_test_response(100 * 1024);
        let chunks = chunk_response(original.clone());

        // Extract chunk data
        let mut chunk_data = Vec::new();
        for chunk in &chunks {
            match chunk {
                Message::ResponseChunk {
                    chunk_index,
                    chunk_data: data,
                    ..
                } => {
                    chunk_data.push((*chunk_index, data.clone()));
                }
                _ => panic!("Expected ResponseChunk"),
            }
        }

        // Reassemble
        let template = make_test_response(0);
        let reassembled = reassemble_chunks(template, &chunk_data).unwrap();

        assert_eq!(reassembled.body, original.body);
    }
}
