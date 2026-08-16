//! Raw-event processor — drains the proxy's [`RawEvent`] channel and converts
//! captured traffic into blocks.
//!
//! Data flow per request/response pair:
//! ```text
//! RawEvent::Request       → RawCache("request") + form-dispatched request parser → BlockStore
//! RawEvent::ResponseChunk → accumulate per request-id
//! RawEvent::ResponseDone  → RawCache("response") + form-dispatched response parser → BlockStore
//! ```
//!
//! ## Wire-form dispatch (T6)
//! Anthropic-form traffic keeps the existing [`parse_anthropic_request`]/
//! [`parse_anthropic_response`] behavior unchanged; openai-form (Chat
//! Completions) traffic routes to [`parse_openai_request`]/
//! [`parse_openai_response`]. The form is detected **per event** from the
//! wire itself (endpoint path + body shape), with the channel's
//! [`ResponseFormat`] (from [`proxy_interceptor::UpstreamConfig`]) as the
//! tiebreak default — a single entry-gateway prefix can carry both forms at
//! once (T4 pins exactly that), and the `/omp`/`/pi` upstream config
//! hot-reloads per request, so a static per-channel format would misroute.
//! The openai parsers are strict; bodies they do not recognise fall back to
//! the anthropic parsers, preserving the pre-T6 tolerance (RawCache is
//! written before parsing either way, and parse failures never panic).

use std::collections::HashMap;
use std::sync::Arc;

use bytes::Bytes;
use harness_blocks::{BlockStore, HarnessBlock, RawCache};
use parking_lot::Mutex;
use proxy_interceptor::{RawEvent, ResponseFormat};
use uuid::Uuid;

use crate::block_builder::{
    parse_anthropic_request, parse_anthropic_response, parse_openai_request, parse_openai_response,
};
use crate::session::SessionContext;

type Store = Arc<Mutex<BlockStore>>;
type Cache = Arc<Mutex<RawCache>>;

/// Run the raw-event processor until the proxy's sender is dropped (channel
/// closed). Designed to be `tokio::spawn`-ed as a background task.
///
/// `default_format` is the channel's [`ResponseFormat`] — the caller passes
/// the [`proxy_interceptor::UpstreamConfig::response_format`] it captured
/// traffic through (mixed-form channels pass [`ResponseFormat::Generic`]).
/// It only breaks ties when the wire itself does not reveal the form (e.g. a
/// system-less openai body on a non-`chat/completions` path).
pub async fn run_raw_processor(
    mut rx: tokio::sync::mpsc::Receiver<RawEvent>,
    store: Store,
    raw_cache: Cache,
    ctx: Arc<SessionContext>,
    default_format: ResponseFormat,
) {
    // Accumulated response chunks keyed by the proxy's request id.
    let mut pending: HashMap<Uuid, Vec<(u64, Bytes)>> = HashMap::new();

    while let Some(event) = rx.recv().await {
        match event {
            RawEvent::Request { id, body, path, .. } => {
                let ts = ctx.now_ms();
                let session = ctx.session_id.clone();

                // 1. Raw cache
                {
                    let cache = raw_cache.lock();
                    if let Err(e) = cache.insert_raw(&session, "request", &body, ts) {
                        tracing::warn!("raw_cache insert request failed: {e}");
                    }
                }

                // 2. Parse → blocks (wire-form dispatch)
                let blocks = parse_request_blocks(&path, &body, default_format, &ctx);
                {
                    let s = store.lock();
                    for b in &blocks {
                        if let Err(e) = s.insert_block(b) {
                            tracing::warn!("block insert (request) failed: {e}");
                        }
                    }
                }

                // Track id so we can correlate the response.
                pending.entry(id).or_default();
            }

            RawEvent::ResponseChunk { id, seq, chunk } => {
                pending.entry(id).or_default().push((seq, chunk));
            }

            RawEvent::ResponseDone { id, .. } => {
                let Some(mut chunks) = pending.remove(&id) else {
                    continue;
                };

                // Sort by proxy sequence to guarantee byte order.
                chunks.sort_by_key(|(seq, _)| *seq);
                let mut body: Vec<u8> = Vec::with_capacity(chunks.len() * 128);
                for (_, c) in chunks {
                    body.extend_from_slice(&c);
                }

                let ts = ctx.now_ms();
                let session = ctx.session_id.clone();

                {
                    let cache = raw_cache.lock();
                    if let Err(e) = cache.insert_raw(&session, "response", &body, ts) {
                        tracing::warn!("raw_cache insert response failed: {e}");
                    }
                }

                let blocks = parse_response_blocks(&body, &ctx);
                {
                    let s = store.lock();
                    for b in &blocks {
                        if let Err(e) = s.insert_block(b) {
                            tracing::warn!("block insert (response) failed: {e}");
                        }
                    }
                }
            }
        }
    }
}

/// Dispatch a captured request body to the right wire-form parser.
///
/// Openai-form detection, in precedence order:
/// 1. Chat-Completions endpoint path (`chat/completions`)
/// 2. a `system`/`developer`-role message inside `messages` (anthropic
///    hoists system to a top-level field, so `role=system` never appears in
///    an anthropic request body)
/// 3. the channel default ([`ResponseFormat::OpenAISSE`])
///
/// If the openai parser yields nothing (body not chat-shaped), fall back to
/// the anthropic parser — same tolerance as pre-T6.
fn parse_request_blocks(
    path: &str,
    body: &[u8],
    default_format: ResponseFormat,
    ctx: &SessionContext,
) -> Vec<HarnessBlock> {
    if request_is_openai_form(path, body, default_format) {
        let blocks = parse_openai_request(body, ctx);
        if !blocks.is_empty() {
            return blocks;
        }
    }
    parse_anthropic_request(body, ctx)
}

fn request_is_openai_form(path: &str, body: &[u8], default_format: ResponseFormat) -> bool {
    if path.contains("chat/completions") {
        return true;
    }
    // Cheap pre-filter: a system-role message can only exist if the bytes
    // mention "system" at all.
    if body.windows(6).any(|w| w == b"system") {
        if let Ok(root) = serde_json::from_slice::<serde_json::Value>(body) {
            if let Some(messages) = root.get("messages").and_then(|v| v.as_array()) {
                let has_system_role = messages.iter().any(|m| {
                    matches!(
                        m.get("role").and_then(|r| r.as_str()),
                        Some("system") | Some("developer")
                    )
                });
                if has_system_role {
                    return true;
                }
            }
        }
    }
    matches!(default_format, ResponseFormat::OpenAISSE)
}

/// Dispatch a captured (reassembled) response body to the right wire-form
/// parser.
///
/// Both openai wire shapes carry the `chat.completion` marker (`object:
/// "chat.completion"` JSON / `chat.completion.chunk` SSE); anthropic bodies
/// never do. The openai parser is strict, so a false-positive marker (e.g.
/// anthropic assistant text that quotes "chat.completion") falls back to the
/// anthropic parser with unchanged behavior.
fn parse_response_blocks(body: &[u8], ctx: &SessionContext) -> Vec<HarnessBlock> {
    if body.windows(15).any(|w| w == b"chat.completion") {
        let blocks = parse_openai_response(body, ctx);
        if !blocks.is_empty() {
            return blocks;
        }
    }
    parse_anthropic_response(body, ctx)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> SessionContext {
        SessionContext::new("t6-dispatch", "claude")
    }

    const OPENAI_BODY: &[u8] = br#"{"model":"m","messages":[
        {"role":"system","content":"s"},{"role":"user","content":"u"}]}"#;
    const ANTHROPIC_BODY: &[u8] = br#"{"model":"m","system":"s",
        "messages":[{"role":"user","content":"u"}]}"#;
    /// System-less user-only body: ambiguous, resolved by the default.
    const PLAIN_BODY: &[u8] = br#"{"model":"m","messages":[{"role":"user","content":"u"}]}"#;

    #[test]
    fn request_form_detection_precedence() {
        // 1. endpoint path
        assert!(request_is_openai_form(
            "/v1/chat/completions",
            PLAIN_BODY,
            ResponseFormat::AnthropicSSE
        ));
        // 2. body sniff — system-role message inside messages
        assert!(request_is_openai_form(
            "/v1/messages",
            OPENAI_BODY,
            ResponseFormat::AnthropicSSE
        ));
        // anthropic bodies never trip the sniff (system is top-level)
        assert!(!request_is_openai_form(
            "/v1/messages",
            ANTHROPIC_BODY,
            ResponseFormat::AnthropicSSE
        ));
        // ambiguous body stays anthropic by default
        assert!(!request_is_openai_form(
            "/v1/messages",
            PLAIN_BODY,
            ResponseFormat::AnthropicSSE
        ));
        // 3. channel default breaks ties
        assert!(request_is_openai_form(
            "/v1/responses",
            PLAIN_BODY,
            ResponseFormat::OpenAISSE
        ));
    }

    #[test]
    fn request_dispatch_routes_openai_and_falls_back() {
        let c = ctx();
        let blocks = parse_request_blocks(
            "/v1/chat/completions",
            OPENAI_BODY,
            ResponseFormat::Generic,
            &c,
        );
        assert_eq!(blocks.len(), 2); // SystemPrompt + UserPrompt
        assert_eq!(
            blocks[0].block_type,
            harness_blocks::BlockType::SystemPrompt
        );
        assert_eq!(blocks[0].metadata["source"], "openai_request");

        // anthropic form → anthropic parser, zero change
        let blocks =
            parse_request_blocks("/v1/messages", ANTHROPIC_BODY, ResponseFormat::Generic, &c);
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].metadata["source"], "anthropic_request");

        // openai detection but non-chat body → openai parser yields nothing
        // → anthropic fallback (pre-T6 tolerance)
        let blocks = parse_request_blocks(
            "/v1/chat/completions",
            b"not json",
            ResponseFormat::Generic,
            &c,
        );
        assert!(blocks.is_empty());
    }

    #[test]
    fn response_dispatch_by_marker_with_strict_fallback() {
        let c = ctx();

        let openai_json = br#"{"object":"chat.completion","model":"g",
            "choices":[{"message":{"role":"assistant","content":"hi"},"finish_reason":"stop"}],
            "usage":{"prompt_tokens":1,"completion_tokens":2}}"#;
        let blocks = parse_response_blocks(openai_json, &c);
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].metadata["source"], "openai_response");
        assert_eq!(blocks[0].metadata["finish_reason"], "stop");

        // anthropic SSE → anthropic parser, zero change
        let anthropic_sse = b"data: {\"type\":\"message_start\",\"message\":{\"model\":\"claude-3\",\"usage\":{\"input_tokens\":7,\"output_tokens\":1}}}\n\
             \n\
             data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hi\"}}\n";
        let blocks = parse_response_blocks(anthropic_sse, &c);
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].metadata["source"], "anthropic_response");
        assert_eq!(String::from_utf8_lossy(&blocks[0].content), "Hi");

        // false-positive marker (anthropic text quoting "chat.completion"):
        // openai parser strictly rejects → anthropic fallback unchanged
        let tricky = b"data: {\"type\":\"message_start\",\"message\":{\"model\":\"claude-3\",\"usage\":{\"input_tokens\":7,\"output_tokens\":1}}}\n\
             \n\
             data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"chat.completion mentioned in text\"}}\n";
        let blocks = parse_response_blocks(tricky, &c);
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].metadata["source"], "anthropic_response");
    }
}
