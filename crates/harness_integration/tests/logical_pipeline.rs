//! End-to-end logical pipeline: feed synthetic proxy RawEvents + hook HTTP
//! callbacks through the [`Integration`] orchestrator and verify the complete
//! block sequence lands in the [`BlockStore`] in the right order.
//!
//! This exercises the full data path without a real TLS proxy, so it is fast
//! and deterministic. The companion test in `real_proxy_session.rs` exercises
//! the actual TLS interception.

use std::time::Duration;

use harness_integration::{
    list_blocks_by_session, BlockType, HarnessBlock, InterceptMode, Integration,
};
use uuid::Uuid;

/// Simulate a full BYOP session:
///   Spawn → (proxy) SystemPrompt + UserPrompt → (proxy) Response
///         → (hook) UserPrompt → (hook) ToolCall → (hook) ToolResult
///         → (proxy) Response → Exit
#[tokio::test]
async fn full_session_block_sequence() {
    let session = format!("test-{}", Uuid::new_v4());
    let mut integ = Integration::in_memory(&session, "claude").unwrap();

    // 1. Start hooks
    integ.start_hooks().await.unwrap();
    let hook_url = integ.hook_url().unwrap();

    // ── Record lifecycle start ────────────────────────────────────────────
    integ.record_spawn(InterceptMode::Full);

    // ── Simulate proxy request (system + user) ────────────────────────────
    let req_body = br#"{
        "model": "claude-3-5-sonnet",
        "max_tokens": 1024,
        "system": "You are a coding assistant.",
        "messages": [{"role": "user", "content": "Write hello world in Rust"}],
        "stream": true
    }"#;
    let blocks = harness_integration::parse_anthropic_request(req_body, integ.ctx());
    for b in &blocks {
        integ.with_store(|s| s.insert_block(b).unwrap());
    }

    // ── Simulate proxy response (SSE) ────────────────────────────────────
    let resp_body = b"\
event: message_start\n\
data: {\"type\":\"message_start\",\"message\":{\"model\":\"claude-3\",\"usage\":{\"input_tokens\":10,\"output_tokens\":1}}}\n\
\n\
event: content_block_delta\n\
data: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"fn main() { println!(\\\"hello\\\"); }\"}}\n\
\n\
event: message_delta\n\
data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":12}}\n";
    let blocks = harness_integration::parse_anthropic_response(resp_body, integ.ctx());
    for b in &blocks {
        integ.with_store(|s| s.insert_block(b).unwrap());
    }

    // ── Simulate hooks ───────────────────────────────────────────────────
    let client = reqwest::Client::new();
    // UserPromptSubmit
    client
        .post(format!("{}/hooks/user_prompt_submit", hook_url))
        .json(&serde_json::json!({"prompt": "now refactor it"}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    // PreToolUse
    client
        .post(format!("{}/hooks/pre_tool_use", hook_url))
        .json(&serde_json::json!({"tool_name": "str_replace_editor"}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    // PostToolUse
    client
        .post(format!("{}/hooks/post_tool_use", hook_url))
        .json(&serde_json::json!({"tool_name": "str_replace_editor", "result": "ok"}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    // Stop
    client
        .post(format!("{}/hooks/stop", hook_url))
        .json(&serde_json::json!({"exit_code": 0}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();

    // Allow hook server async inserts to settle.
    tokio::time::sleep(Duration::from_millis(50)).await;

    // ── Record exit ──────────────────────────────────────────────────────
    integ.record_exit(0);

    // ── Verify full block sequence ───────────────────────────────────────
    let blocks: Vec<HarnessBlock> =
        integ.with_store(|s| list_blocks_by_session(s, &session).unwrap());

    let types: Vec<BlockType> = blocks.iter().map(|b| b.block_type).collect();

    // Expected order:
    //   Spawn, SystemPrompt, UserPrompt(proxy), Response,
    //   UserPrompt(hook), ToolCall, ToolResult, Exit(hook), Exit(lifecycle)
    assert!(types.contains(&BlockType::Spawn), "missing Spawn: {types:?}");
    assert!(types.contains(&BlockType::SystemPrompt), "missing SystemPrompt: {types:?}");
    assert!(types.contains(&BlockType::UserPrompt), "missing UserPrompt: {types:?}");
    assert!(types.contains(&BlockType::Response), "missing Response: {types:?}");
    assert!(types.contains(&BlockType::ToolCall), "missing ToolCall: {types:?}");
    assert!(types.contains(&BlockType::ToolResult), "missing ToolResult: {types:?}");
    assert!(types.contains(&BlockType::Exit), "missing Exit: {types:?}");

    // Spawn is first
    assert_eq!(types[0], BlockType::Spawn, "Spawn should be first: {types:?}");

    // Verify content of the proxy-sourced SystemPrompt
    let sys = blocks
        .iter()
        .find(|b| b.block_type == BlockType::SystemPrompt)
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&sys.content),
        "You are a coding assistant."
    );
    assert_eq!(sys.metadata["source"], "anthropic_request");

    // Verify content of the proxy-sourced Response
    let resp = blocks
        .iter()
        .find(|b| b.block_type == BlockType::Response)
        .unwrap();
    assert!(String::from_utf8_lossy(&resp.content).contains("fn main()"));
    assert_eq!(resp.metadata["usage"]["input_tokens"], 10);
    assert_eq!(resp.metadata["usage"]["output_tokens"], 12);

    // Verify hook-sourced ToolCall
    let tc = blocks
        .iter()
        .find(|b| b.block_type == BlockType::ToolCall)
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&tc.content),
        "str_replace_editor"
    );
    assert_eq!(tc.metadata["source"], "hook");

    // Sequences are unique and monotonic
    let mut seqs: Vec<u32> = blocks.iter().map(|b| b.sequence).collect();
    let original = seqs.clone();
    seqs.sort();
    seqs.dedup();
    assert_eq!(seqs.len(), original.len(), "duplicate sequences");

    // At least 8 blocks total
    assert!(blocks.len() >= 8, "expected >=8 blocks, got {}: {types:?}", blocks.len());
}
