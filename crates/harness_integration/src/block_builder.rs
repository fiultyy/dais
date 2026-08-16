//! BlockBuilder — parse Anthropic Messages API / OpenAI Chat Completions
//! request/response bodies into typed [`HarnessBlock`]s.
//!
//! ## Request parsing
//! [`parse_anthropic_request`] extracts:
//! - `SystemPrompt` block from the `system` field (string or content-block array)
//! - `ToolCall` blocks for each tool definition in `tools` (content = tool name,
//!   parent_id points to the SystemPrompt block if present)
//! - `UserPrompt` block for each user-role message
//! - Tool definitions are additionally recorded in the SystemPrompt block's `metadata`
//!
//! [`parse_openai_request`] (T6) handles the Chat Completions wire shape:
//! system/developer-role messages inside `messages` merge into one
//! `SystemPrompt` block; the rest are preserved in wire order (`user` →
//! `UserPrompt`, `assistant` history → `PromptSegment` with `role` metadata).
//! ## Response parsing
//! [`parse_anthropic_response`] handles both:
//! - Non-streaming JSON (single `message` object)
//! - Streaming SSE (reconstructed from accumulated chunks)
//!
//! [`parse_openai_response`] (T6) handles both openai shapes — non-streaming
//! `chat.completion` JSON and streaming `chat.completion.chunk` SSE — and is
//! strict: bodies that are not chat-completion shaped yield no blocks so the
//! caller can fall back to the anthropic parser.
//!
//! Produces a single `Response` block with assistant text as `content` and
//! token `usage` / `stop_reason` (anthropic) or `finish_reason` (openai) /
//! `model` in `metadata`.

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

// ── anthropic request ────────────────────────────────────────────────────

/// Parse an Anthropic Messages API request body into blocks.
///
/// Produces at most one `SystemPrompt` block (carrying tool definitions in
/// metadata), one `ToolCall` block per tool definition (content = tool name,
/// parent_id → SystemPrompt if present), and one `UserPrompt` block per
/// user-role message.
///
/// Silently returns an empty vec on invalid JSON so the capture pipeline
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

    // SystemPrompt block（可选，有 system 文本或 tools 时生成）
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

    // 为每个工具定义生成 ToolCall block（content = 工具名字节）
    if let Some(tools) = tools_meta.as_array() {
        // tools 存在时 SystemPrompt 必已生成（has_tools = true 分支必走）
        let system_prompt_id = blocks
            .iter()
            .find(|b| b.block_type == BlockType::SystemPrompt)
            .map(|b| b.id.clone());
        for tool in tools {
            let name = tool
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            if name.is_empty() {
                continue;
            }
            let description = tool
                .get("description")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let input_schema = tool
                .get("input_schema")
                .cloned()
                .unwrap_or(Value::Null);
            let metadata = serde_json::json!({
                "source": "anthropic_request",
                "kind": "definition",
                "description": description,
                "input_schema": input_schema,
            });
            let mut block = make_block(
                ctx,
                BlockType::ToolCall,
                name.into_bytes(),
                metadata,
            );
            block.parent_id = system_prompt_id.clone();
            blocks.push(block);
        }
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

// ── openai request ───────────────────────────────────────────────────────

/// Parse an OpenAI Chat Completions request body into blocks.
///
/// Mirrors the anthropic-form extraction on the `messages`-array wire shape:
/// - `system`/`developer`-role messages merge into one `SystemPrompt` block
///   (anthropic hoists system to a top-level field, openai keeps it inside
///   `messages`; the at-most-one semantics stay aligned with the
///   anthropic form)
/// - remaining messages are preserved in wire order: `user` → `UserPrompt`,
///   `assistant` history → `PromptSegment` (metadata `role` = `assistant`)
/// - `tool`-role messages are skipped (their content is a bare tool_call_id)
///
/// Silently returns an empty vec when the body is not JSON or has no
/// `messages` array (e.g. the `/v1/responses` wire form) so the capture
/// pipeline degrades exactly like the anthropic parsers.
pub fn parse_openai_request(body: &[u8], ctx: &SessionContext) -> Vec<HarnessBlock> {
    let root: Value = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(e) => {
            tracing::debug!("parse_openai_request: not valid JSON ({e})");
            return Vec::new();
        }
    };
    let Some(messages) = root.get("messages").and_then(|v| v.as_array()) else {
        tracing::debug!("parse_openai_request: no messages array, skipping");
        return Vec::new();
    };

    let mut blocks = Vec::new();
    let model = root
        .get("model")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    // System messages → one merged SystemPrompt (aligned with the anthropic
    // form's at-most-one semantics).
    let system_text = messages
        .iter()
        .filter(|m| is_system_role(m))
        .map(|m| extract_content_text(m.get("content").unwrap_or(&Value::Null)))
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    if !system_text.is_empty() {
        let metadata = serde_json::json!({
            "source": "openai_request",
            "model": model,
        });
        blocks.push(make_block(
            ctx,
            BlockType::SystemPrompt,
            system_text.into_bytes(),
            metadata,
        ));
    }

    // Remaining messages in wire order.
    for msg in messages {
        let role = msg.get("role").and_then(|v| v.as_str()).unwrap_or("");
        let text = extract_content_text(msg.get("content").unwrap_or(&Value::Null));
        if text.is_empty() {
            continue;
        }
        let (block_type, role_meta) = match role {
            "user" => (BlockType::UserPrompt, None),
            "assistant" => (BlockType::PromptSegment, Some("assistant")),
            _ => continue, // system handled above; tool/unknown out of T6 scope
        };
        let mut metadata = serde_json::json!({
            "source": "openai_request",
            "model": model,
        });
        if let Some(r) = role_meta {
            metadata["role"] = Value::String(r.to_string());
        }
        blocks.push(make_block(ctx, block_type, text.into_bytes(), metadata));
    }

    blocks
}

fn is_system_role(msg: &Value) -> bool {
    matches!(
        msg.get("role").and_then(|v| v.as_str()),
        Some("system") | Some("developer")
    )
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

// ── openai response ──────────────────────────────────────────────────────

/// Parse an OpenAI Chat Completions response body into a `Response` block.
///
/// Handles both wire shapes:
/// - Non-streaming JSON (single `chat.completion` object)
/// - Streaming SSE (`chat.completion.chunk` events reconstructed from the
///   accumulated chunks; usage arrives on the final chunk when the harness
///   opted into it via `stream_options.include_usage`)
///
/// Produces a single block with:
/// - `content` = assistant text (`choices[0].message.content` / accumulated
///   `choices[0].delta.content`)
/// - `metadata` = `{ source, model, finish_reason, usage: { input_tokens,
///   output_tokens } }` — `prompt_tokens`/`completion_tokens` are renamed to
///   the anthropic-aligned `input_tokens`/`output_tokens`.
///
/// Strict: returns an empty vec for bodies that are not chat-completion
/// shaped, so the caller can fall back to the anthropic parser.
pub fn parse_openai_response(body: &[u8], ctx: &SessionContext) -> Vec<HarnessBlock> {
    let text = String::from_utf8_lossy(body);

    let parsed = if text.trim_start().starts_with('{') {
        parse_openai_json_response(&text)
    } else {
        parse_openai_sse_response(&text)
    };

    let Some(p) = parsed else {
        tracing::debug!("parse_openai_response: unrecognised body, skipping");
        return Vec::new();
    };

    let metadata = serde_json::json!({
        "source": "openai_response",
        "model": p.model,
        "finish_reason": p.stop_reason,
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

fn parse_openai_json_response(text: &str) -> Option<ParsedResponse> {
    let root: Value = serde_json::from_str(text).ok()?;
    // Strict chat-completion shape: anthropic JSON (`type: "message"`)
    // never carries `object: "chat.completion"` or a `choices` array.
    if root.get("object").and_then(|v| v.as_str()) != Some("chat.completion")
        && root.get("choices").and_then(|v| v.as_array()).is_none()
    {
        return None;
    }
    let choice = root
        .get("choices")
        .and_then(|v| v.as_array())
        .and_then(|a| a.first())?;
    let message = choice.get("message").unwrap_or(&Value::Null);
    let usage = root.get("usage").unwrap_or(&Value::Null);
    Some(ParsedResponse {
        text: extract_content_text(message.get("content").unwrap_or(&Value::Null)),
        model: root
            .get("model")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        stop_reason: choice
            .get("finish_reason")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        input_tokens: usage
            .get("prompt_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0),
        output_tokens: usage
            .get("completion_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0),
    })
}

/// Parse a reconstructed openai SSE stream (concatenated chunks).
///
/// Walks every `data: {json}` line, keeping only `chat.completion.chunk`
/// objects: `delta.content` accumulates, the last non-null `finish_reason`
/// wins, and `usage` (present on the final chunk when requested) is taken
/// last-wins. Returns `None` when no chunk object was seen.
fn parse_openai_sse_response(text: &str) -> Option<ParsedResponse> {
    let mut content = String::new();
    let mut model = String::new();
    let mut finish_reason = String::new();
    let mut input_tokens = 0u64;
    let mut output_tokens = 0u64;
    let mut saw_chunk = false;

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
            continue; // includes the `data: [DONE]` sentinel
        };
        if evt.get("object").and_then(|v| v.as_str()) != Some("chat.completion.chunk") {
            continue;
        }
        saw_chunk = true;
        if model.is_empty() {
            if let Some(m) = evt.get("model").and_then(|v| v.as_str()) {
                model = m.to_string();
            }
        }
        if let Some(choice) = evt
            .get("choices")
            .and_then(|v| v.as_array())
            .and_then(|a| a.first())
        {
            if let Some(t) = choice
                .get("delta")
                .and_then(|d| d.get("content"))
                .and_then(|v| v.as_str())
            {
                content.push_str(t);
            }
            if let Some(f) = choice.get("finish_reason").and_then(|v| v.as_str()) {
                finish_reason = f.to_string();
            }
        }
        if let Some(u) = evt.get("usage") {
            if let Some(p) = u.get("prompt_tokens").and_then(|v| v.as_u64()) {
                input_tokens = p;
            }
            if let Some(c) = u.get("completion_tokens").and_then(|v| v.as_u64()) {
                output_tokens = c;
            }
        }
    }

    if !saw_chunk {
        return None;
    }

    Some(ParsedResponse {
        text: content,
        model,
        stop_reason: finish_reason,
        input_tokens,
        output_tokens,
    })
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

        // 1 SystemPrompt + 1 ToolCall (calc) + 2 UserPrompt (assistant skipped)
        assert_eq!(blocks.len(), 4);

        assert_eq!(blocks[0].block_type, BlockType::SystemPrompt);
        assert_eq!(
            String::from_utf8_lossy(&blocks[0].content),
            "You are a helpful assistant."
        );
        assert_eq!(blocks[0].metadata["source"], "anthropic_request");
        assert_eq!(blocks[0].metadata["model"], "claude-3-5-sonnet");
        assert!(blocks[0].metadata["tools"].is_array());

        // ToolCall block for "calc" definition
        assert_eq!(blocks[1].block_type, BlockType::ToolCall);
        assert_eq!(String::from_utf8_lossy(&blocks[1].content), "calc");
        assert_eq!(blocks[1].metadata["source"], "anthropic_request");
        assert_eq!(blocks[1].metadata["kind"], "definition");
        assert_eq!(blocks[1].metadata["description"], "calculator");
        assert_eq!(blocks[1].parent_id.as_deref(), Some(blocks[0].id.as_str()));

        assert_eq!(blocks[2].block_type, BlockType::UserPrompt);
        assert_eq!(String::from_utf8_lossy(&blocks[2].content), "Hello");

        assert_eq!(blocks[3].block_type, BlockType::UserPrompt);
        assert_eq!(String::from_utf8_lossy(&blocks[3].content), "What is 2+2?");
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
        let body =
            br#"{"model":"claude","max_tokens":1,"messages":[{"role":"user","content":"hi"}]}"#;
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
    // ── openai ───────────────────────────────────────────────────────────

    #[test]
    fn parse_openai_request_system_user_assistant() {
        let body = br#"{
            "model": "gpt-4o",
            "messages": [
                {"role": "system", "content": "You are a helpful assistant."},
                {"role": "user", "content": "Hello"},
                {"role": "assistant", "content": "Hi there"},
                {"role": "user", "content": [{"type":"text","text":"What is 2+2?"}]}
            ]
        }"#;
        let c = ctx();
        let blocks = parse_openai_request(body, &c);

        // 1 SystemPrompt (merged system role) + UserPrompt + PromptSegment
        // (assistant history) + UserPrompt — wire order preserved.
        assert_eq!(blocks.len(), 4);
        assert_eq!(blocks[0].block_type, BlockType::SystemPrompt);
        assert_eq!(
            String::from_utf8_lossy(&blocks[0].content),
            "You are a helpful assistant."
        );
        assert_eq!(blocks[0].metadata["source"], "openai_request");
        assert_eq!(blocks[0].metadata["model"], "gpt-4o");

        assert_eq!(blocks[1].block_type, BlockType::UserPrompt);
        assert_eq!(String::from_utf8_lossy(&blocks[1].content), "Hello");
        assert_eq!(blocks[1].metadata["source"], "openai_request");

        assert_eq!(blocks[2].block_type, BlockType::PromptSegment);
        assert_eq!(String::from_utf8_lossy(&blocks[2].content), "Hi there");
        assert_eq!(blocks[2].metadata["source"], "openai_request");
        assert_eq!(blocks[2].metadata["role"], "assistant");

        assert_eq!(blocks[3].block_type, BlockType::UserPrompt);
        assert_eq!(String::from_utf8_lossy(&blocks[3].content), "What is 2+2?");
    }

    #[test]
    fn parse_openai_request_merges_system_messages() {
        let body = br#"{
            "model": "gpt-4o",
            "messages": [
                {"role": "system", "content": "Line one"},
                {"role": "developer", "content": "Line two"},
                {"role": "user", "content": "hi"}
            ]
        }"#;
        let c = ctx();
        let blocks = parse_openai_request(body, &c);
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].block_type, BlockType::SystemPrompt);
        assert_eq!(
            String::from_utf8_lossy(&blocks[0].content),
            "Line one\nLine two"
        );
        assert_eq!(blocks[1].block_type, BlockType::UserPrompt);
    }

    #[test]
    fn parse_openai_request_without_system() {
        let body = br#"{"model":"gpt-4o","messages":[{"role":"user","content":"hi"}]}"#;
        let c = ctx();
        let blocks = parse_openai_request(body, &c);
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].block_type, BlockType::UserPrompt);
        assert_eq!(blocks[0].metadata["source"], "openai_request");
    }

    #[test]
    fn parse_openai_request_not_chat_shape() {
        let c = ctx();
        // /v1/responses wire form (input, no messages array) → no blocks.
        assert!(parse_openai_request(br#"{"model":"gpt-5","input":"hi"}"#, &c).is_empty());
        assert!(parse_openai_request(b"not json", &c).is_empty());
    }

    #[test]
    fn parse_openai_response_json_chat_completion() {
        let body = br#"{
            "id": "chatcmpl-1",
            "object": "chat.completion",
            "model": "gpt-4o",
            "choices": [{"index": 0,
                         "message": {"role": "assistant", "content": "ok-openai"},
                         "finish_reason": "stop"}],
            "usage": {"prompt_tokens": 3, "completion_tokens": 4, "total_tokens": 7}
        }"#;
        let c = ctx();
        let blocks = parse_openai_response(body, &c);
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].block_type, BlockType::Response);
        assert_eq!(String::from_utf8_lossy(&blocks[0].content), "ok-openai");
        assert_eq!(blocks[0].metadata["source"], "openai_response");
        assert_eq!(blocks[0].metadata["model"], "gpt-4o");
        assert_eq!(blocks[0].metadata["finish_reason"], "stop");
        assert_eq!(blocks[0].metadata["usage"]["input_tokens"], 3);
        assert_eq!(blocks[0].metadata["usage"]["output_tokens"], 4);
    }

    #[test]
    fn parse_openai_response_sse_chunks() {
        let body = b"\
data: {\"id\":\"c1\",\"object\":\"chat.completion.chunk\",\"model\":\"gpt-4o\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"He\"},\"finish_reason\":null}]}\n\
\n\
data: {\"id\":\"c1\",\"object\":\"chat.completion.chunk\",\"model\":\"gpt-4o\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"llo\"},\"finish_reason\":null}]}\n\
\n\
data: {\"id\":\"c1\",\"object\":\"chat.completion.chunk\",\"model\":\"gpt-4o\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\
\n\
data: {\"id\":\"c1\",\"object\":\"chat.completion.chunk\",\"model\":\"gpt-4o\",\"choices\":[],\"usage\":{\"prompt_tokens\":9,\"completion_tokens\":12}}\n\
\n\
data: [DONE]\n";

        let c = ctx();
        let blocks = parse_openai_response(body, &c);
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].block_type, BlockType::Response);
        assert_eq!(String::from_utf8_lossy(&blocks[0].content), "Hello");
        assert_eq!(blocks[0].metadata["source"], "openai_response");
        assert_eq!(blocks[0].metadata["model"], "gpt-4o");
        assert_eq!(blocks[0].metadata["finish_reason"], "stop");
        assert_eq!(blocks[0].metadata["usage"]["input_tokens"], 9);
        assert_eq!(blocks[0].metadata["usage"]["output_tokens"], 12);
    }

    #[test]
    fn parse_openai_response_strict_not_chat_shape() {
        let c = ctx();
        // Anthropic JSON must not be claimed by the openai parser.
        let anthropic = br#"{"type":"message","content":[{"type":"text","text":"Hi"}],"model":"claude-3","usage":{"input_tokens":1,"output_tokens":2}}"#;
        assert!(parse_openai_response(anthropic, &c).is_empty());
        // Anthropic SSE lines are not chat.completion.chunk objects.
        let anthropic_sse =
            b"data: {\"type\":\"message_start\",\"message\":{\"model\":\"claude-3\"}}\n";
        assert!(parse_openai_response(anthropic_sse, &c).is_empty());
        assert!(parse_openai_response(b"garbage", &c).is_empty());
    }
}
