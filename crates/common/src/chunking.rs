/// Response chunking utilities for handling large responses over 32KB API Gateway limit
///
/// AWS API Gateway has a 32KB per-message limit for WebSocket messages. To support
/// larger HTTP responses, we split the body into chunks and send them separately.
///
/// Protocol:
/// 1. Send `HttpResponse` with empty body and `total_chunks = Some(n)` (metadata only)
/// 2. Send n `ResponseChunk` messages carrying the body in 30KB segments
/// 3. Handler reassembles all chunks into the final response

use crate::protocol::Message;
use crate::HttpResponse;

/// Maximum safe chunk size leaving room for JSON overhead
/// 32KB API Gateway limit - JSON overhead (~500 bytes for metadata)
const CHUNK_SIZE_BYTES: usize = 30 * 1024; // 30KB per chunk

/// Check if a response needs chunking based on body size
pub fn should_chunk_response(response: &HttpResponse) -> bool {
    // Estimate total JSON size: metadata (~300 bytes) + base64 body
    300 + response.body.len() > 28 * 1024
}

/// Split a large response into a header message plus body chunk messages.
///
/// Returns:
/// - `[HttpResponse(metadata, empty body, total_chunks=n), ResponseChunk(0), ..., ResponseChunk(n-1)]`
/// - Or `[HttpResponse(full response)]` if small enough to fit in one message.
pub fn chunk_response(response: HttpResponse) -> Vec<Message> {
    if !should_chunk_response(&response) {
        return vec![Message::HttpResponse(response)];
    }

    let body = response.body.clone();
    let total_chunks = ((body.len() + CHUNK_SIZE_BYTES - 1) / CHUNK_SIZE_BYTES) as u32;

    // First: send metadata with empty body and total_chunks flag
    let mut messages = vec![Message::HttpResponse(HttpResponse {
        body: String::new(),
        total_chunks: Some(total_chunks),
        ..response.clone()
    })];

    // Then: send body parts as sequential chunks
    for (chunk_index, chunk_bytes) in body.as_bytes().chunks(CHUNK_SIZE_BYTES).enumerate() {
        // Safety: base64 only contains ASCII, so byte slice is valid UTF-8
        let chunk_data = String::from_utf8_lossy(chunk_bytes).into_owned();
        messages.push(Message::ResponseChunk {
            request_id: response.request_id.clone(),
            chunk_index: chunk_index as u32,
            total_chunks,
            chunk_data,
        });
    }

    messages
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn make_response(body_size: usize) -> HttpResponse {
        HttpResponse {
            request_id: "req-123".to_string(),
            status_code: 200,
            headers: HashMap::from([(
                "content-type".to_string(),
                vec!["text/plain".to_string()],
            )]),
            body: "x".repeat(body_size),
            processing_time_ms: 10,
            total_chunks: None,
        }
    }

    #[test]
    fn test_small_response_not_chunked() {
        let r = make_response(1000);
        assert!(!should_chunk_response(&r));
        let msgs = chunk_response(r.clone());
        assert_eq!(msgs.len(), 1);
        assert!(matches!(msgs[0], Message::HttpResponse(_)));
    }

    #[test]
    fn test_large_response_chunked() {
        let r = make_response(100 * 1024); // 100KB
        assert!(should_chunk_response(&r));

        let msgs = chunk_response(r.clone());
        assert!(msgs.len() > 2); // header + multiple chunks

        // First message is HttpResponse with empty body and total_chunks set
        match &msgs[0] {
            Message::HttpResponse(resp) => {
                assert!(resp.body.is_empty());
                assert!(resp.total_chunks.is_some());
                assert_eq!(resp.status_code, 200);
            }
            _ => panic!("First message should be HttpResponse"),
        }

        // Remaining messages are ResponseChunk
        let total_chunks = match &msgs[0] {
            Message::HttpResponse(r) => r.total_chunks.unwrap(),
            _ => panic!(),
        };
        assert_eq!(msgs.len() as u32, 1 + total_chunks);

        // Verify chunks can be reassembled into original body
        let reassembled_body: String = msgs[1..]
            .iter()
            .map(|m| match m {
                Message::ResponseChunk { chunk_data, .. } => chunk_data.as_str(),
                _ => panic!("Expected ResponseChunk"),
            })
            .collect();
        assert_eq!(reassembled_body, r.body);
    }

    #[test]
    fn test_chunk_indices_are_sequential() {
        let r = make_response(100 * 1024);
        let msgs = chunk_response(r);

        for (expected_idx, msg) in msgs[1..].iter().enumerate() {
            match msg {
                Message::ResponseChunk { chunk_index, .. } => {
                    assert_eq!(*chunk_index, expected_idx as u32);
                }
                _ => panic!("Expected ResponseChunk"),
            }
        }
    }
}
