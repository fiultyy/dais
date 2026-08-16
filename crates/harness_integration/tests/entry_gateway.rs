//! T5 entry gateway 集成测试。
//!
//! 端到端: 假上游(cc/omp 各一) ← EntryGateway(单端口明文) ← 客户端。
//! 断言: 前缀分流正确、auth 透明转发、external-{cc,omp} 归并 session
//! 落 Spawn+请求块(懒发: 零流量前缀零块)、stop 落 Exit + 端口释放。

use std::sync::Mutex;
use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::response::Response;
use axum::routing::post;
use axum::Router;
use futures_util::{stream, StreamExt};
use harness_integration::{BlockStore, BlockType, EntryGateway};

/// mock 上游收到的 (tag, x-api-key) 对(透明管道验证)。
static SEEN: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());

async fn mock_upstream(tag: &'static str, req: Request<Body>) -> Response {
    let key = req
        .headers()
        .get("x-api-key")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let _ = axum::body::to_bytes(req.into_body(), 1024 * 1024)
        .await
        .unwrap();
    SEEN.lock().unwrap().push((tag.to_string(), key));
    let msg = format!("data: hello-from-{tag}\n\ndata: [DONE]\n\n");
    let stream = stream::iter([msg]).then(|c| async move {
        tokio::time::sleep(Duration::from_millis(20)).await;
        Ok::<bytes::Bytes, std::io::Error>(bytes::Bytes::from(c))
    });
    Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "text/event-stream")
        .body(Body::from_stream(stream))
        .unwrap()
}

async fn spawn_fake_upstream(tag: &'static str) -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let app = Router::new().route(
        "/v1/messages",
        post(move |req: Request<Body>| mock_upstream(tag, req)),
    );
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    port
}

fn blocks_of(db: &std::path::Path, session: &str) -> Vec<harness_blocks::HarnessBlock> {
    let store = BlockStore::open(db.to_string_lossy().to_string()).unwrap();
    harness_blocks::list_blocks_by_session(&store, session).unwrap_or_default()
}

#[tokio::test]
async fn entry_gateway_prefix_routing_capture_and_transparent_auth() {
    // ── 环境隔离(HOME 指向临时目录; /cc 走 ZAP_UPSTREAM_BASE 显式覆盖) ──
    let tmp = tempfile::tempdir().unwrap();
    let orig_home = std::env::var("HOME").ok();
    let orig_upstream = std::env::var("ZAP_UPSTREAM_BASE").ok();
    std::env::set_var("HOME", tmp.path());
    std::env::remove_var("ZAP_UPSTREAM_BASE");

    let cc_port = spawn_fake_upstream("cc").await;
    let omp_port = spawn_fake_upstream("omp").await;

    // /omp、/pi 出口: ~/.config/zap/omp-upstream.json(orchestration 侧写入口径)
    std::fs::create_dir_all(tmp.path().join(".config/zap")).unwrap();
    let omp_cfg = format!(
        r#"{{"api_base":"http://127.0.0.1:{omp_port}","api_key_env":"ZAP_OMP_KEY","response_format":"anthropic"}}"#
    );
    std::fs::write(tmp.path().join(".config/zap/omp-upstream.json"), omp_cfg).unwrap();
    // /cc 出口: 显式覆盖优先
    std::env::set_var("ZAP_UPSTREAM_BASE", format!("http://127.0.0.1:{cc_port}"));

    let blocks_db = tmp.path().join("blocks.db");
    let raw_db = tmp.path().join("raw.db");
    let gw = EntryGateway::start(0, Some(&blocks_db), Some(&raw_db))
        .await
        .unwrap();
    assert_ne!(gw.port(), 0);

    let result = async {
        let client = reqwest::Client::new();

        // 1. /cc: 明文 HTTP + 客户端自带凭据 → 透明转发到 cc 上游。
        let resp = client
            .post(format!("http://127.0.0.1:{}/cc/v1/messages", gw.port()))
            .header("x-api-key", "sk-cc-client-key")
            .body(r#"{"model":"claude-3"}"#)
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = resp.text().await.unwrap();
        assert!(body.contains("hello-from-cc"), "cc 前缀路由到 cc 上游: {body}");

        // 2. /omp: 同一端口, 不同前缀 → omp 上游。
        let resp = client
            .post(format!("http://127.0.0.1:{}/omp/v1/messages", gw.port()))
            .header("x-api-key", "sk-omp-client-key")
            .body(r#"{"model":"glm-5.2"}"#)
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = resp.text().await.unwrap();
        assert!(body.contains("hello-from-omp"), "omp 前缀路由到 omp 上游: {body}");

        // 3. 未知前缀 → 404(不猜测目的地)。
        let resp = client
            .post(format!("http://127.0.0.1:{}/nope/v1/messages", gw.port()))
            .body("{}")
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);

        // 4. 透明 auth: 客户端凭据原样到上游(zap 不注不剥)。
        {
            let seen = SEEN.lock().unwrap();
            assert!(seen.contains(&("cc".to_string(), "sk-cc-client-key".to_string())));
            assert!(seen.contains(&("omp".to_string(), "sk-omp-client-key".to_string())));
        }

        // 5. 归并 session: cc/omp 各自 Spawn + 请求块; pi 零流量零块(懒发)。
        //    (forwarder/processor 异步, 留窗口)
        tokio::time::sleep(Duration::from_millis(300)).await;
        for session in ["external-cc", "external-omp"] {
            let blocks = blocks_of(&blocks_db, session);
            assert!(
                blocks.iter().any(|b| b.block_type == BlockType::Spawn),
                "{session} must have a Spawn block (lazy, post-traffic)"
            );
            assert!(
                !blocks.is_empty(),
                "{session} must have captured request blocks"
            );
        }
        assert!(
            blocks_of(&blocks_db, "external-pi").is_empty(),
            "zero-traffic prefix stays invisible (lazy spawn)"
        );

        // 6. 快照: 三行, 端口一致。
        let snap = gw.snapshot();
        assert_eq!(snap.len(), 3);
        assert!(snap.iter().all(|s| s.port == gw.port()));
    }
    .await;

    // ── stop: 活跃 session 落 Exit, 端口释放 ─────────────────────────────
    let port = gw.port();
    gw.stop();
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        std::net::TcpStream::connect(("127.0.0.1", port)).is_err(),
        "entry port must be released after stop"
    );
    for session in ["external-cc", "external-omp"] {
        let blocks = blocks_of(&blocks_db, session);
        assert!(
            blocks.iter().any(|b| b.block_type == BlockType::Exit),
            "{session} must record Exit on stop"
        );
    }

    // ── 环境恢复 ─────────────────────────────────────────────────────────
    match orig_home {
        Some(h) => std::env::set_var("HOME", h),
        None => std::env::remove_var("HOME"),
    }
    match orig_upstream {
        Some(v) => std::env::set_var("ZAP_UPSTREAM_BASE", v),
        None => std::env::remove_var("ZAP_UPSTREAM_BASE"),
    }
    result
}
