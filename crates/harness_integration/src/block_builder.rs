//! BlockBuilder — parse Anthropic Messages API request/response bodies into
//! typed [`HarnessBlock`]s.
//!
//! ## Request parsing
//! [`parse_anthropic_request`] extracts:
//! - `SystemPrompt` block from the `system` field (string or content-block array)
//! - `UserPrompt` block for each user-role message
//! - Tool definitions are recorded in the SystemPrompt block's `metadata`
//!
//! ## Response parsing
//! [`parse_anthropic_response`] handles both:
//! - Non-streaming JSON (single `message` object)
//! - Streaming SSE (reconstructed from accumulated chunks)
//!
//! Produces a single `Response` block with assistant text as `content` and
//! token `usage` / `stop_reason` / `model` in `metadata`.

use harness_blocks::{BlockType, HarnessBlock};
use serde_json::Value;

use crate::session::SessionContext;

// ── helpers ──────────────────────────────────────────────────────────────

fn make_block(
    ctx: &SessionContext,
    block_type: BlockType,
    content: Vec<u8>,
    metadata: Value,
) -> HarnessBlock {
    let mut b = HarnessBlock::new(
        &ctx.session_id,
        &ctx.harness_type,
        block_type,
        ctx.next_seq(),
        content,
        ctx.now_ms(),
    );
    b.metadata = metadata;
    b
}

/// Extract plain text from an Anthropic `system` field.
///
/// The field is either a bare string or an array of content blocks
/// (`[{"type":"text","text":"..."}, ...]`).
fn extract_system_text(system: &Value) -> String {
    match system {
        Value::String(s) => s.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .filter_map(|b| {
                if b.get("type").and_then(|v| v.as_str()) == Some("text") {
                    b.get("text").and_then(|v| v.as_str()).map(String::from)
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// Extract plain text from a message `content` field.
///
/// Content is either a bare string or an array of content blocks. Only
/// `text` blocks are extracted (image/tool blocks are skipped for the
/// text payload).
fn extract_content_text(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .filter_map(|b| {
                if b.get("type").and_then(|v| v.as_str()) == Some("text") {
                    b.get("text").and_then(|v| v.as_str()).map(String::from)
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join(""),
        _ => String::new(),
    }
}

// ── request ──────────────────────────────────────────────────────────────

/// Parse an Anthropic Messages API request body into blocks.
///
/// Produces at most one `SystemPrompt` block (carrying tool definitions in
/// metadata) plus one `UserPrompt` block per user-role message.
/// Silently returns an empty vec on invalid JSON so the capture pipeline
/// never crashes on a malformed payload.
pub fn parse_anthropic_request(body: &[u8], ctx: &SessionContext) -> Vec<HarnessBlock> {
    let root: Value = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(e) => {
            tracing::debug!("parse_anthropic_request: not valid JSON ({e})");
            return Vec::new();
        }
    };

    let mut blocks = Vec::new();
    let model = root
        .get("model")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    // System prompt (+ tool definitions)
    let system_text = root
        .get("system")
        .map(extract_system_text)
        .unwrap_or_default();

    let tools_meta = root.get("tools").map(|t| t.clone()).unwrap_or(Value::Null);

    let has_system = !system_text.is_empty();
    let has_tools = !tools_meta.is_null();

    if has_system || has_tools {
        let metadata = serde_json::json!({
            "source": "anthropic_request",
            "model": model,
            "tools": tools_meta,
        });
        blocks.push(make_block(
            ctx,
            BlockType::SystemPrompt,
            system_text.into_bytes(),
            metadata,
        ));
    }

    // User messages
    if let Some(messages) = root.get("messages").and_then(|v| v.as_array()) {
        for msg in messages {
            let role = msg.get("role").and_then(|v| v.as_str()).unwrap_or("");
            if role != "user" {
                continue;
            }
            let text = extract_content_text(msg.get("content").unwrap_or(&Value::Null));
            if text.is_empty() {
                continue;
            }
            let metadata = serde_json::json!({
                "source": "anthropic_request",
                "model": model,
            });
            blocks.push(make_block(
                ctx,
                BlockType::UserPrompt,
                text.into_bytes(),
                metadata,
            ));
        }
    }

    blocks
}

// ── response ─────────────────────────────────────────────────────────────

/// Parse an Anthropic Messages API response body into a `Response` block.
///
/// Handles both non-streaming JSON and streaming SSE (detected by the
/// presence of `data:` lines). Produces a single block with:
/// - `content` = concatenated assistant text
/// - `metadata` = `{ source, model, stop_reason, usage: { input_tokens, output_tokens } }`
pub fn parse_anthropic_response(body: &[u8], ctx: &SessionContext) -> Vec<HarnessBlock> {
    let text = String::from_utf8_lossy(body);

    let parsed = if text.trim_start().starts_with('{') {
        parse_json_response(&text)
    } else {
        parse_sse_response(&text)
    };

    let Some(p) = parsed else {
        tracing::debug!("parse_anthropic_response: unrecognised body, skipping");
        return Vec::new();
    };

    let metadata = serde_json::json!({
        "source": "anthropic_response",
        "model": p.model,
        "stop_reason": p.stop_reason,
        "usage": {
            "input_tokens": p.input_tokens,
            "output_tokens": p.output_tokens,
        },
    });

    vec![make_block(
        ctx,
        BlockType::Response,
        p.text.into_bytes(),
        metadata,
    )]
}

struct ParsedResponse {
    text: String,
    model: String,
    stop_reason: String,
    input_tokens: u64,
    output_tokens: u64,
}

fn parse_json_response(text: &str) -> Option<ParsedResponse> {
    let root: Value = serde_json::from_str(text).ok()?;
    let content_text = root
        .get("content")
        .and_then(|c| c.as_array())
        .map(|blocks| {
            blocks
                .iter()
                .filter_map(|b| {
                    if b.get("type").and_then(|v| v.as_str()) == Some("text") {
                        b.get("text").and_then(|v| v.as_str()).map(String::from)
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>()
                .join("")
        })
        .unwrap_or_default();

    let usage = root.get("usage").unwrap_or(&Value::Null);
    Some(ParsedResponse {
        text: content_text,
        model: root
            .get("model")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        stop_reason: root
            .get("stop_reason")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        input_tokens: usage
            .get("input_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0),
        output_tokens: usage
            .get("output_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0),
    })
}

/// Parse a reconstructed SSE stream (concatenated chunks) into a response.
///
/// Walks every `data: {json}` line, accumulating `text_delta` content and
/// capturing usage from `message_start` / `message_delta`.
fn parse_sse_response(text: &str) -> Option<ParsedResponse> {
    let mut content = String::new();
    let mut model = String::new();
    let mut stop_reason = String::new();
    let mut input_tokens = 0u64;
    let mut output_tokens = 0u64;
    let mut saw_any = false;

    for line in text.lines() {
        let line = line.trim();
        let json_str = if let Some(rest) = line.strip_prefix("data: ") {
            rest
        } else if let Some(rest) = line.strip_prefix("data:") {
            rest
        } else {
            continue;
        };
        let Ok(evt) = serde_json::from_str::<Value>(json_str) else {
            continue;
        };
        saw_any = true;
        match evt.get("type").and_then(|v| v.as_str()) {
            Some("message_start") => {
                if let Some(msg) = evt.get("message") {
                    model = msg
                        .get("model")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    if let Some(u) = msg.get("usage") {
                        input_tokens = u.get("input_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
                        output_tokens =
                            u.get("output_tokens").and_then(|v| v.as_u64()).unwrap_or(1);
                    }
                }
            }
            Some("content_block_delta") => {
                if let Some(delta) = evt.get("delta") {
                    if delta.get("type").and_then(|v| v.as_str()) == Some("text_delta") {
                        if let Some(t) = delta.get("text").and_then(|v| v.as_str()) {
                            content.push_str(t);
                        }
                    }
                }
            }
            Some("message_delta") => {
                if let Some(d) = evt.get("delta") {
                    stop_reason = d
                        .get("stop_reason")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                }
                if let Some(u) = evt.get("usage") {
                    // output_tokens accumulates across the stream; take the
                    // last reported value.
                    if let Some(o) = u.get("output_tokens").and_then(|v| v.as_u64()) {
                        output_tokens = o;
                    }
                }
            }
            _ => {}
        }
    }

    if !saw_any {
        return None;
    }

    Some(ParsedResponse {
        text: content,
        model,
        stop_reason,
        input_tokens,
        output_tokens,
    })
}

// ── tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> SessionContext {
        SessionContext::new("test-session", "claude")
    }

    #[test]
    fn parse_request_string_system() {
        let body = br#"{
            "model": "claude-3-5-sonnet",
            "max_tokens": 1024,
            "system": "You are a helpful assistant.",
            "messages": [
                {"role": "user", "content": "Hello"},
                {"role": "assistant", "content": "Hi there"},
                {"role": "user", "content": "What is 2+2?"}
            ],
            "tools": [{"name": "calc", "description": "calculator", "input_schema": {}}],
            "stream": true
        }"#;
        let c = ctx();
        let blocks = parse_anthropic_request(body, &c);

        // 1 SystemPrompt + 2 UserPrompt (assistant skipped)
        assert_eq!(blocks.len(), 3);

        assert_eq!(blocks[0].block_type, BlockType::SystemPrompt);
        assert_eq!(
            String::from_utf8_lossy(&blocks[0].content),
            "You are a helpful assistant."
        );
        assert_eq!(blocks[0].metadata["source"], "anthropic_request");
        assert_eq!(blocks[0].metadata["model"], "claude-3-5-sonnet");
        assert!(blocks[0].metadata["tools"].is_array());

        assert_eq!(blocks[1].block_type, BlockType::UserPrompt);
        assert_eq!(String::from_utf8_lossy(&blocks[1].content), "Hello");

        assert_eq!(blocks[2].block_type, BlockType::UserPrompt);
        assert_eq!(String::from_utf8_lossy(&blocks[2].content), "What is 2+2?");
    }

    #[test]
    fn parse_request_array_system_and_content() {
        let body = br#"{
            "model": "claude-3",
            "max_tokens": 256,
            "system": [{"type":"text","text":"Line one"},{"type":"text","text":"Line two"}],
            "messages": [
                {"role": "user", "content": [{"type":"text","text":"Hello "},{"type":"text","text":"world"}]}
            ]
        }"#;
        let c = ctx();
        let blocks = parse_anthropic_request(body, &c);

        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].block_type, BlockType::SystemPrompt);
        assert_eq!(
            String::from_utf8_lossy(&blocks[0].content),
            "Line one\nLine two"
        );
        assert_eq!(blocks[1].block_type, BlockType::UserPrompt);
        assert_eq!(String::from_utf8_lossy(&blocks[1].content), "Hello world");
    }

    #[test]
    fn parse_request_no_system() {
        let body = br#"{"model":"claude","max_tokens":1,"messages":[{"role":"user","content":"hi"}]}"#;
        let c = ctx();
        let blocks = parse_anthropic_request(body, &c);
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].block_type, BlockType::UserPrompt);
    }

    #[test]
    fn parse_request_invalid_json() {
        let c = ctx();
        let blocks = parse_anthropic_request(b"not json", &c);
        assert!(blocks.is_empty());
    }

    #[test]
    fn parse_response_json() {
        let body = br#"{
            "id": "msg_01",
            "type": "message",
            "role": "assistant",
            "content": [{"type": "text", "text": "Hello!"}],
            "model": "claude-3-5-sonnet",
            "stop_reason": "end_turn",
            "usage": {"input_tokens": 25, "output_tokens": 150}
        }"#;
        let c = ctx();
        let blocks = parse_anthropic_response(body, &c);
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].block_type, BlockType::Response);
        assert_eq!(String::from_utf8_lossy(&blocks[0].content), "Hello!");
        assert_eq!(blocks[0].metadata["model"], "claude-3-5-sonnet");
        assert_eq!(blocks[0].metadata["stop_reason"], "end_turn");
        assert_eq!(blocks[0].metadata["usage"]["input_tokens"], 25);
        assert_eq!(blocks[0].metadata["usage"]["output_tokens"], 150);
    }

    #[test]
    fn parse_response_sse() {
        let body = b"\
event: message_start\n\
data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_01\",\"type\":\"message\",\"role\":\"assistant\",\"model\":\"claude-3\",\"content\":[],\"stop_reason\":null,\"usage\":{\"input_tokens\":10,\"output_tokens\":1}}}\n\
\n\
event: content_block_start\n\
data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\
\n\
event: content_block_delta\n\
data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hello\"}}\n\
\n\
event: content_block_delta\n\
data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\" world\"}}\n\
\n\
event: content_block_stop\n\
data: {\"type\":\"content_block_stop\",\"index\":0}\n\
\n\
event: message_delta\n\
data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\",\"stop_sequence\":null},\"usage\":{\"output_tokens\":5}}\n\
\n\
event: message_stop\n\
data: {\"type\":\"message_stop\"}\n";

        let c = ctx();
        let blocks = parse_anthropic_response(body, &c);
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].block_type, BlockType::Response);
        assert_eq!(String::from_utf8_lossy(&blocks[0].content), "Hello world");
        assert_eq!(blocks[0].metadata["model"], "claude-3");
        assert_eq!(blocks[0].metadata["stop_reason"], "end_turn");
        assert_eq!(blocks[0].metadata["usage"]["input_tokens"], 10);
        assert_eq!(blocks[0].metadata["usage"]["output_tokens"], 5);
    }

    #[test]
    fn parse_response_unrecognised() {
        let c = ctx();
        let blocks = parse_anthropic_response(b"garbage", &c);
        assert!(blocks.is_empty());
    }
}
