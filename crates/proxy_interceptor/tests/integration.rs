//! 端到端: 启动 proxy → reqwest HTTPS 请求 → 验证透传 + auth 注入 + raw 事件捕获。

use std::sync::Mutex;
use std::time::Duration;

use axum::body::Body;
use axum::http::StatusCode;
use axum::response::Response;
use axum::routing::post;
use axum::Router;
use futures_util::{stream, StreamExt};
use proxy_interceptor::{ProxyManager, RawEvent, ResponseFormat, UpstreamConfig};

/// mock 上游收到的 x-api-key (auth 注入验证)
static SEEN_API_KEY: Mutex<Option<String>> = Mutex::new(None);

async fn mock_upstream(req: Request) -> Response {
    let api_key = req
        .headers()
        .get("x-api-key")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let _body = axum::body::to_bytes(req.into_body(), 1024 * 1024).await.unwrap();
    *SEEN_API_KEY.lock().unwrap() = api_key;
    // SSE 流式响应, 分块带延迟 — 验证 proxy 不缓冲
    let chunks = vec!["data: hello\n\n", "data: [DONE]\n\n"];
    let stream = stream::iter(chunks)
        .then(|c| async move {
            tokio::time::sleep(Duration::from_millis(30)).await;
            Ok::<bytes::Bytes, std::io::Error>(bytes::Bytes::from(c))
        });
    Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "text/event-stream")
        .body(Body::from_stream(stream))
        .unwrap()
}

type Request = axum::http::Request<Body>;

#[tokio::test]
async fn proxy_passthrough_and_capture() {
    // 1. mock 上游 (HTTP)
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let upstream_port = listener.local_addr().unwrap().port();
    let app = Router::new().route("/v1/messages", post(mock_upstream));
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    // 2. 启动 proxy
    std::env::set_var("ZAP_TEST_API_KEY", "sk-test-123");
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

    // 3. 客户端信任本地 CA, 走 HTTPS
    let ca_pem = std::fs::read(&handle.ca_cert_path).unwrap();
    let client = reqwest::Client::builder()
        .add_root_certificate(reqwest::Certificate::from_pem(&ca_pem).unwrap())
        .build()
        .unwrap();
    let resp = client
        .post(format!("https://127.0.0.1:{}/v1/messages", handle.port))
        .header("content-type", "application/json")
        .body(r#"{"model":"claude-3","stream":true}"#)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.headers()["content-type"], "text/event-stream");
    let body = resp.text().await.unwrap();
    assert_eq!(body, "data: hello\n\ndata: [DONE]\n\n");

    // 4. auth 注入验证
    assert_eq!(
        SEEN_API_KEY.lock().unwrap().as_deref(),
        Some("sk-test-123"),
        "proxy 应从 env 读 key 并注入 x-api-key"
    );

    // 5. raw 事件捕获
    let mut got_request = false;
    let mut chunks = 0;
    let mut got_done = false;
    while let Ok(event) = handle.raw_rx.try_recv() {
        match event {
            RawEvent::Request { path, body, .. } => {
                got_request = true;
                assert_eq!(path, "/v1/messages");
                assert!(String::from_utf8_lossy(&body).contains("claude-3"));
            }
            RawEvent::ResponseChunk { .. } => chunks += 1,
            RawEvent::ResponseDone { status, .. } => {
                got_done = true;
                assert_eq!(status, 200);
            }
        }
    }
    assert!(got_request, "缺少 Request 事件");
    assert!(chunks >= 2, "SSE 应按 chunk 捕获, 实际 {chunks}");
    assert!(got_done, "缺少 ResponseDone 事件");

    // 6. Drop 发 shutdown signal (端口应释放)
    let port = handle.port;
    drop(handle);
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(tokio::net::TcpListener::bind(("127.0.0.1", port)).await.is_ok());
}

#[test]
fn upstream_resolve_priority() {
    // 默认: ClaudeCode → anthropic
    let cc = UpstreamConfig::resolve(proxy_interceptor::HarnessType::ClaudeCode, None).unwrap();
    assert_eq!(cc.api_base, "https://api.anthropic.com");
    assert_eq!(cc.auth_header, "x-api-key");
    assert_eq!(cc.response_format, ResponseFormat::AnthropicSSE);

    // Codex → openai bearer
    let cx = UpstreamConfig::resolve(proxy_interceptor::HarnessType::Codex, None).unwrap();
    assert_eq!(cx.api_base, "https://api.openai.com");
    assert_eq!(cx.auth_prefix, "Bearer ");

    // 显式 > 默认
    let ex = UpstreamConfig::resolve(
        proxy_interceptor::HarnessType::ClaudeCode,
        Some("http://localhost:9999"),
    )
    .unwrap();
    assert_eq!(ex.api_base, "http://localhost:9999");
}
