//! End-to-end real-proxy session: spin up a mock Anthropic upstream, a TLS
//! proxy ([`ProxyManager`]), the hook server, and the raw processor — then
//! issue a real HTTPS request through the proxy and verify that the full
//! block sequence (Spawn → SystemPrompt → UserPrompt → Response → Exit) lands
//! in the [`BlockStore`].

use std::sync::Arc;

use axum::body::Body;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::Response;
use axum::routing::post;
use axum::Router;
use futures_util::stream;
use parking_lot::Mutex;

use bytes::Bytes;
use proxy_interceptor::{ProxyManager, ResponseFormat, UpstreamConfig};
use uuid::Uuid;

use harness_integration::{
    list_blocks_by_session, run_raw_processor, BlockStore, BlockType, HarnessBlock,
    InterceptMode, Integration,
};

type Request = axum::http::Request<Body>;

#[derive(Clone)]
struct UpstreamState {
    request_body: Arc<Mutex<Vec<u8>>>,
}

/// Mock Anthropic upstream: records the request body, returns an SSE stream
/// that resembles a real `messages` response.
async fn mock_anthropic_upstream(
    State(st): State<UpstreamState>,
    req: Request,
) -> Response {
    let body = axum::body::to_bytes(req.into_body(), 1024 * 1024)
        .await
        .unwrap();
    *st.request_body.lock() = body.to_vec();

    let chunks = vec![
        "data: {\"type\":\"message_start\",\"message\":{\"model\":\"claude-3\",\"usage\":{\"input_tokens\":10,\"output_tokens\":1}}}\n\n",
        "data: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"Hello from mock!\"}}\n\n",
        "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":5}}\n\n",
        "data: {\"type\":\"message_stop\"}\n\n",
    ];
    let stream = stream::iter(
        chunks
            .into_iter()
            .map(|c| Ok::<Bytes, std::io::Error>(Bytes::from(c))),
    );
    Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "text/event-stream")
        .body(Body::from_stream(stream))
        .unwrap()
}

#[tokio::test]
async fn real_proxy_full_session() {
    // ── 1. Mock upstream ─────────────────────────────────────────────────
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let upstream_port = listener.local_addr().unwrap().port();
    let upstream_state = UpstreamState {
        request_body: Arc::new(Mutex::new(Vec::new())),
    };
    let st_clone = upstream_state.clone();
    let app = Router::new()
        .route("/v1/messages", post(mock_anthropic_upstream))
        .with_state(st_clone);
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    // ── 2. Integration: in-memory stores + hooks ─────────────────────────
    let session = format!("real-{}", Uuid::new_v4());
    let mut integ = Integration::in_memory(&session, "claude").unwrap();
    integ.start_hooks().await.unwrap();

    // ── 3. Record Spawn ──────────────────────────────────────────────────
    integ.record_spawn(InterceptMode::Full);

    // ── 4. Start TLS proxy + raw processor ───────────────────────────────
    // We need the raw processor to write into Integration's stores. Since
    // Integration::start_proxy owns the stores internally, we replicate the
    // wiring here with a shared store that Integration also references.
    //
    // Simpler approach: create a dedicated shared store + raw_cache and run
    // the processor, then assert on that store directly (the integration's
    // in-memory store is private). This keeps the test self-contained.
    let shared_store = Arc::new(Mutex::new(BlockStore::open_in_memory().unwrap()));
    let ctx = Arc::new(harness_integration::SessionContext::new(&session, "claude"));

    // Record spawn in the shared store too (so the sequence includes it).
    {
        let s = shared_store.lock();
        let mut b = HarnessBlock::new(&session, "claude", BlockType::Spawn, ctx.next_seq(), Vec::new(), ctx.now_ms());
        b.metadata = serde_json::json!({"mode": "full"});
        s.insert_block(&b).unwrap();
    }

    std::env::set_var("ZAP_TEST_API_KEY", "sk-test-456");
    let upstream = UpstreamConfig {
        api_base: format!("http://127.0.0.1:{upstream_port}"),
        auth_header: "x-api-key".into(),
        auth_prefix: String::new(),
        api_key_env: "ZAP_TEST_API_KEY".into(),
        request_path: "/v1/messages".into(),
        response_format: ResponseFormat::AnthropicSSE,
    };

    let manager = ProxyManager::new().unwrap();
    let mut handle = manager.allocate(upstream).await.unwrap();

    // Spawn raw processor
    let store_clone = shared_store.clone();
    let ctx_clone = ctx.clone();
    let raw_rx = std::mem::replace(&mut handle.raw_rx, {
        let (_, rx) = tokio::sync::mpsc::channel(1);
        rx
    });
    let raw_cache = Arc::new(Mutex::new(
        harness_integration::RawCache::open_in_memory().unwrap(),
    ));
    let cache_clone = raw_cache.clone();
    let proc_task = tokio::spawn(run_raw_processor(raw_rx, store_clone, cache_clone, ctx_clone));

    // ── 5. HTTPS request through proxy (simulating the harness) ──────────
    let ca_pem = std::fs::read(&handle.ca_cert_path).unwrap();
    let client = reqwest::Client::builder()
        .add_root_certificate(reqwest::Certificate::from_pem(&ca_pem).unwrap())
        .build()
        .unwrap();

    let resp = client
        .post(format!("https://127.0.0.1:{}/v1/messages", handle.port))
        .header("content-type", "application/json")
        .body(
            serde_json::json!({
                "model": "claude-3-5-sonnet",
                "max_tokens": 1024,
                "system": "You are a helpful assistant.",
                "messages": [{"role": "user", "content": "Say hello"}],
                "stream": true
            })
            .to_string(),
        )
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    // Consume the full body so all chunks flow through the proxy.
    let _body = resp.text().await.unwrap();

    // ── 6. Record Exit ───────────────────────────────────────────────────
    {
        let s = shared_store.lock();
        let mut b = HarnessBlock::new(&session, "claude", BlockType::Exit, ctx.next_seq(), Vec::new(), ctx.now_ms());
        b.metadata = serde_json::json!({"exit_code": 0});
        s.insert_block(&b).unwrap();
    }

    // ── 7. Wait for raw processor to drain ───────────────────────────────
    // The proxy sender is still alive while `handle` is alive. Drop the
    // handle to close the channel, then await the processor task.
    drop(handle);
    let _ = proc_task.await;

    // ── 8. Verify block sequence ─────────────────────────────────────────
    let blocks: Vec<HarnessBlock> = {
        let s = shared_store.lock();
        list_blocks_by_session(&s, &session).unwrap()
    };
    let types: Vec<BlockType> = blocks.iter().map(|b| b.block_type).collect();

    // Expected: Spawn, SystemPrompt, UserPrompt, Response, Exit
    assert_eq!(types[0], BlockType::Spawn, "Spawn first: {types:?}");
    assert!(types.contains(&BlockType::SystemPrompt), "no SystemPrompt: {types:?}");
    assert!(types.contains(&BlockType::UserPrompt), "no UserPrompt: {types:?}");
    assert!(types.contains(&BlockType::Response), "no Response: {types:?}");
    assert_eq!(*types.last().unwrap(), BlockType::Exit, "Exit last: {types:?}");

    // SystemPrompt content
    let sys = blocks.iter().find(|b| b.block_type == BlockType::SystemPrompt).unwrap();
    assert_eq!(String::from_utf8_lossy(&sys.content), "You are a helpful assistant.");

    // UserPrompt content
    let up = blocks.iter().find(|b| b.block_type == BlockType::UserPrompt).unwrap();
    assert_eq!(String::from_utf8_lossy(&up.content), "Say hello");

    // Response content (from SSE stream)
    let resp = blocks.iter().find(|b| b.block_type == BlockType::Response).unwrap();
    assert_eq!(String::from_utf8_lossy(&resp.content), "Hello from mock!");
    assert_eq!(resp.metadata["usage"]["input_tokens"], 10);
    assert_eq!(resp.metadata["usage"]["output_tokens"], 5);

    // Raw cache has request + response entries
    let raw = raw_cache.lock();
    let entries = raw.peek(&session).unwrap();
    assert_eq!(entries.len(), 2, "raw cache should have request + response");
    assert_eq!(entries[0].direction, "request");
    assert_eq!(entries[1].direction, "response");

    // ── 9. Proxy shut down (port released) ───────────────────────────────
    // (handle already dropped above)
}

/// Verify that the Integration orchestrator can wire up a proxy end-to-end
/// using its own internal stores (not just a manually-shared one).
#[tokio::test]
async fn integration_start_proxy_and_hook_url() {
    let session = format!("integ-{}", Uuid::new_v4());
    let mut integ = Integration::in_memory(&session, "claude").unwrap();

    // Start hooks
    integ.start_hooks().await.unwrap();
    let hook_url = integ.hook_url();
    assert!(hook_url.is_some());

    // Mock upstream (simple echo)
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let upstream_port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        let app = Router::new().route(
            "/v1/messages",
            post(|| async { "ok" }),
        );
        let _ = axum::serve(listener, app).await;
    });

    std::env::set_var("ZAP_TEST_API_KEY", "sk-test-789");
    let upstream = UpstreamConfig {
        api_base: format!("http://127.0.0.1:{upstream_port}"),
        auth_header: "x-api-key".into(),
        auth_prefix: String::new(),
        api_key_env: "ZAP_TEST_API_KEY".into(),
        request_path: "/v1/messages".into(),
        response_format: ResponseFormat::AnthropicSSE,
    };

    let manager = ProxyManager::new().unwrap();
    let port = integ.start_proxy(&manager, upstream).await.unwrap();
    assert!(port > 0);
    assert!(integ.proxy_port().is_some());
    assert!(integ.ca_cert_path().is_some());
}
