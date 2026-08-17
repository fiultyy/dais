//! T8 entry gateway 集成测试: session 粒度 = 每次实例启动。
//!
//! 实例身份 = `x-dais-instance` 请求头(dais 别名铸造)。断言:
//! 1. **按实例分 session**: 同前缀不同标记 → `external-omp-<tag>` 各自
//!    独立 session, 各恰一个 Spawn(懒发), 请求块互不串;
//! 2. **无标记回落**: 未携带标记的流量归并默认 `external-omp`(T5 行为);
//! 3. **透明管道不动**: 标记头不进上游(转发前剥), 其余头/体原样;
//! 4. **stop**: 活跃 session(默认+各实例)各落 Exit。

use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::response::Response;
use axum::routing::post;
use axum::Router;
use futures_util::{stream, StreamExt};
use harness_integration::{BlockStore, BlockType, EntryGateway};
use parking_lot::Mutex as PlMutex;

/// mock 上游记录: (tag, x-dais-instance 是否出现, x-api-key, body)。
static SEEN: PlMutex<Vec<(String, bool, String, String)>> = PlMutex::new(Vec::new());

/// HOME 是进程级全局 — 与其它改 HOME 的集成测试串行靠各自独占端口 +
/// env 恢复; 这里同样全量恢复。
static ENV_LOCK: PlMutex<()> = PlMutex::new(());

async fn mock_upstream(req: Request<Body>) -> Response {
    let dais_instance = req.headers().contains_key("x-dais-instance");
    let key = req
        .headers()
        .get("x-api-key")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let body = axum::body::to_bytes(req.into_body(), 1024 * 1024)
        .await
        .unwrap();
    let body_s = String::from_utf8_lossy(&body).to_string();
    let model = serde_json::from_str::<serde_json::Value>(&body_s)
        .ok()
        .and_then(|v| {
            v.get("model")
                .and_then(|m| m.as_str())
                .map(str::to_string)
        })
        .unwrap_or_default();
    let rec_model = model.clone();
    SEEN.lock().push((
        rec_model,
        dais_instance,
        key,
        body_s,
    ));
    let json = serde_json::json!({
        "id": "chatcmpl-t8", "object": "chat.completion", "model": model,
        "choices": [{"index": 0,
                     "message": {"role": "assistant", "content": "ok"},
                     "finish_reason": "stop"}],
        "usage": {"prompt_tokens": 1, "completion_tokens": 2, "total_tokens": 3}
    });
    let s = stream::iter(vec![Ok::<bytes::Bytes, std::io::Error>(
        bytes::Bytes::from(json.to_string().into_bytes()),
    )]);
    Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "application/json")
        .body(Body::from_stream(s))
        .unwrap()
}

async fn spawn_fake_upstream() -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let app = Router::new().route("/v1/chat/completions", post(mock_upstream));
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    port
}

fn blocks_of(db: &std::path::Path, session: &str) -> Vec<harness_blocks::HarnessBlock> {
    let store = BlockStore::open(db.to_string_lossy().to_string()).unwrap();
    harness_blocks::list_blocks_by_session(&store, session).unwrap_or_default()
}

fn openai_body(model: &str) -> String {
    serde_json::json!({
        "model": model,
        "messages": [{"role": "user", "content": format!("u-{model}")}]
    })
    .to_string()
}

#[tokio::test]
async fn entry_gateway_instance_keyed_sessions_marker_stripped_fallback_intact() {
    let _env = ENV_LOCK.lock();
    // ── 环境隔离(HOME 指向临时目录; /omp 出口走 omp-upstream.json) ──
    let tmp = tempfile::tempdir().unwrap();
    let orig_home = std::env::var("HOME").ok();
    let orig_upstream = std::env::var("ZAP_UPSTREAM_BASE").ok();
    std::env::set_var("HOME", tmp.path());
    std::env::remove_var("ZAP_UPSTREAM_BASE");

    let upstream_port = spawn_fake_upstream().await;
    std::fs::create_dir_all(tmp.path().join(".config/zap")).unwrap();
    let omp_cfg = format!(
        r#"{{"api_base":"http://127.0.0.1:{upstream_port}","api_key_env":"ZAP_OMP_KEY","response_format":"openai"}}"#
    );
    std::fs::write(tmp.path().join(".config/zap/omp-upstream.json"), omp_cfg).unwrap();

    let blocks_db = tmp.path().join("blocks.db");
    let raw_db = tmp.path().join("raw.db");
    let gw = EntryGateway::start(0, Some(&blocks_db), Some(&raw_db))
        .await
        .unwrap();
    let entry = gw.port();
    assert_ne!(entry, 0);

    let result = async {
        let client = reqwest::Client::new();

        // ── 实例 A(/omp, 标记 inst-a): 一次请求 ──
        let resp = client
            .post(format!("http://127.0.0.1:{entry}/omp/v1/chat/completions"))
            .header("x-api-key", "sk-t8")
            .header("x-dais-instance", "inst-a")
            .body(openai_body("m-a"))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // ── 实例 B(/omp, 标记 inst-b): 两次请求(同实例多次请求归并) ──
        for _ in 0..2 {
            let resp = client
                .post(format!("http://127.0.0.1:{entry}/omp/v1/chat/completions"))
                .header("x-api-key", "sk-t8")
                .header("x-dais-instance", "inst-b")
                .body(openai_body("m-b"))
                .send()
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::OK);
        }

        // ── 无标记流量(未升级客户端/裸 curl): 回落默认 external-omp ──
        let resp = client
            .post(format!("http://127.0.0.1:{entry}/omp/v1/chat/completions"))
            .header("x-api-key", "sk-t8")
            .body(openai_body("m-bare"))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // ── 上游视角: 标记头被剥, 凭据/体原样 ──
        {
            let seen = SEEN.lock();
            assert_eq!(seen.len(), 4, "4 请求全达上游");
            assert!(
                seen.iter().all(|(_, has_marker, _, _)| !has_marker),
                "x-dais-instance 不得进上游(内部信号, 转发前剥): {seen:?}"
            );
            assert!(seen.iter().all(|(_, _, key, _)| key == "sk-t8"));
            assert!(seen.iter().any(|(m, ..)| m == "m-a"));
            assert!(seen.iter().any(|(m, ..)| m == "m-b"));
            assert!(seen.iter().any(|(m, ..)| m == "m-bare"));
        }

        // ── 数据面: 按实例分 session, 各恰一 Spawn, 请求互不串 ──
        //    (demux/processor 异步, 留窗口)
        tokio::time::sleep(Duration::from_millis(400)).await;
        for (session, model, user_count) in [
            ("external-omp-inst-a", "m-a", 1),
            ("external-omp-inst-b", "m-b", 2),
            ("external-omp", "m-bare", 1),
        ] {
            let blocks = blocks_of(&blocks_db, session);
            assert!(!blocks.is_empty(), "{session} 必有捕获块");

            let spawns: Vec<_> = blocks
                .iter()
                .filter(|b| b.block_type == BlockType::Spawn)
                .collect();
            assert_eq!(spawns.len(), 1, "{session} 恰一个 Spawn(一实例一 session)");
            assert_eq!(spawns[0].harness_type, "omp");
            assert_eq!(spawns[0].metadata["mode"], "external");

            // 该 session 的 UserPrompt 只含自己的请求体, 不串别的实例。
            let users: Vec<String> = blocks
                .iter()
                .filter(|b| b.block_type == BlockType::UserPrompt)
                .map(|b| String::from_utf8_lossy(&b.content).to_string())
                .collect();
            let expected = vec![format!("u-{model}"); user_count];
            assert_eq!(users, expected, "{session} 请求互不串");

            // Response 块模型同源(openai 形解析)。
            let models: Vec<String> = blocks
                .iter()
                .filter(|b| b.block_type == BlockType::Response)
                .filter_map(|b| {
                    b.metadata["model"].as_str().map(str::to_string)
                })
                .collect();
            assert_eq!(models, vec![model.to_string(); user_count]);
        }

        // ── 快照: 默认 + 两实例 = 3 行 /omp, session id 各异 ──
        let snap: Vec<_> = gw
            .snapshot()
            .into_iter()
            .filter(|s| s.prefix == "/omp")
            .collect();
        assert_eq!(snap.len(), 3, "/omp 快照 = 默认 + 2 实例: {snap:?}");
        let ids: Vec<&str> = snap.iter().map(|s| s.session_id.as_str()).collect();
        assert!(ids.contains(&"external-omp"));
        assert!(ids.contains(&"external-omp-inst-a"));
        assert!(ids.contains(&"external-omp-inst-b"));
        assert!(snap.iter().all(|s| s.port == entry));
    }
    .await;

    // ── stop: 三 session(默认+两实例)各落 Exit, 端口释放 ──
    let port = gw.port();
    gw.stop().await;
    assert!(
        std::net::TcpStream::connect(("127.0.0.1", port)).is_err(),
        "entry port must be released after stop"
    );
    for session in ["external-omp", "external-omp-inst-a", "external-omp-inst-b"] {
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
