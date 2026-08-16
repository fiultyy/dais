//! T4-E2E 外部捕获全链自动化断言(固化 T5 人工验收, 防回归)。
//!
//! 端到端: 假上游(全量记录请求) ← EntryGateway(随机端口, 前缀分流) ←
//! 三前缀真形状请求(anthropic 形 + openai 形各一)。断言四件事:
//! 1. **URL 改写**: 前缀剥除、query 保留 — 上游收到正确路径;
//! 2. **auth 透明管道**: `x-api-key`/`authorization` 原样转发, 不剥不注
//!    (客户端零凭据时上游也零 auth 头, 即使出口配了 api_key_env);
//! 3. **session 归并**: `/cc` `/omp` `/pi` 各自归并 `external-cc/omp/pi`,
//!    Spawn(懒发) + harness 串正确;
//! 4. **blocks 成对**: SystemPrompt + UserPrompt + Response,
//!    `metadata.source` = `anthropic_request`/`anthropic_response`
//!    (anthropic 形) 与 `openai_request`/`openai_response` (openai 形,
//!    T6 起真解析: system 消息提取 SystemPrompt, 回包提取
//!    content/usage/finish_reason)。
//!
//! 注: 全链只走 loopback(入口/上游均 127.0.0.1 随机端口), 不出网 —
//! 不落 `#[ignore]` 慢标记; 出真网的测试才需标记供 CI 选跑。

use parking_lot::Mutex;
use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::response::Response;
use axum::routing::post;
use axum::Router;
use futures_util::{stream, StreamExt};
use harness_integration::{BlockStore, BlockType, EntryGateway};

/// 假上游记录的一条请求(改写/auth 断言的数据源)。
#[derive(Debug, Clone)]
struct Recorded {
    tag: &'static str,
    method: String,
    /// 入口改写后上游实际收到的 path+query。
    path: String,
    x_api_key: Option<String>,
    authorization: Option<String>,
    content_type: Option<String>,
    body: String,
}

static RECORDED: Mutex<Vec<Recorded>> = Mutex::new(Vec::new());

fn record(
    tag: &'static str,
    method: &str,
    path: &str,
    headers: &axum::http::HeaderMap,
    body: &[u8],
) {
    let get = |name: &str| {
        headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string)
    };
    RECORDED.lock().push(Recorded {
        tag,
        method: method.to_string(),
        path: path.to_string(),
        x_api_key: get("x-api-key"),
        authorization: get("authorization"),
        content_type: get("content-type"),
        body: String::from_utf8_lossy(body).to_string(),
    });
}

/// anthropic 形上游: 记录后回 SSE(message_start / text_delta / message_delta)。
/// 回包文本回显请求里的最后一条用户消息, 供 Response 块内容断言精确对位。
async fn anthropic_upstream(tag: &'static str, req: Request<Body>) -> Response {
    let method = req.method().as_str().to_string();
    let path = req
        .uri()
        .path_and_query()
        .map(|pq| pq.as_str().to_string())
        .unwrap_or_default();
    let headers = req.headers().clone();
    let body = axum::body::to_bytes(req.into_body(), 1024 * 1024)
        .await
        .unwrap();
    record(tag, &method, &path, &headers, &body);

    let v: serde_json::Value = serde_json::from_slice(&body).unwrap_or_default();
    let model = v
        .get("model")
        .and_then(|m| m.as_str())
        .unwrap_or("t4-model")
        .to_string();
    let user = v
        .get("messages")
        .and_then(|m| m.as_array())
        .and_then(|a| a.last().cloned())
        .and_then(|m| m.get("content").and_then(|c| c.as_str()).map(str::to_string))
        .unwrap_or_default();
    let reply = format!("reply[{user}]");

    let start = serde_json::json!({
        "type": "message_start",
        "message": {"id": "msg_t4", "role": "assistant", "model": model,
                    "usage": {"input_tokens": 12, "output_tokens": 1}}
    });
    let delta = serde_json::json!({
        "type": "content_block_delta", "index": 0,
        "delta": {"type": "text_delta", "text": reply}
    });
    let end = serde_json::json!({
        "type": "message_delta",
        "delta": {"stop_reason": "end_turn"},
        "usage": {"output_tokens": 34}
    });
    let sse = format!(
        "event: message_start\ndata: {start}\n\n\
         event: content_block_delta\ndata: {delta}\n\n\
         event: message_delta\ndata: {end}\n\n\
         data: [DONE]\n\n"
    );

    // 两个 chunk: 驱动 ResponseChunk 序列重排路径(seq 排序拼接)。
    let bytes = sse.into_bytes();
    let (a, b) = bytes.split_at(bytes.len() / 2);
    let chunks = vec![
        bytes::Bytes::from(a.to_vec()),
        bytes::Bytes::from(b.to_vec()),
    ];
    let stream = stream::iter(chunks).then(|c| async move {
        tokio::time::sleep(Duration::from_millis(10)).await;
        Ok::<bytes::Bytes, std::io::Error>(c)
    });
    Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "text/event-stream")
        .body(Body::from_stream(stream))
        .unwrap()
}

/// openai 形上游: 记录后回 chat.completion JSON(回显 model 供块断言对位)。
async fn openai_upstream(tag: &'static str, req: Request<Body>) -> Response {
    let method = req.method().as_str().to_string();
    let path = req
        .uri()
        .path_and_query()
        .map(|pq| pq.as_str().to_string())
        .unwrap_or_default();
    let headers = req.headers().clone();
    let body = axum::body::to_bytes(req.into_body(), 1024 * 1024)
        .await
        .unwrap();
    record(tag, &method, &path, &headers, &body);

    let v: serde_json::Value = serde_json::from_slice(&body).unwrap_or_default();
    let model = v
        .get("model")
        .and_then(|m| m.as_str())
        .unwrap_or("zap/glm-5.2")
        .to_string();
    let json = serde_json::json!({
        "id": "chatcmpl-t4", "object": "chat.completion", "model": model,
        "choices": [{"index": 0,
                     "message": {"role": "assistant", "content": "ok-openai"},
                     "finish_reason": "stop"}],
        "usage": {"prompt_tokens": 3, "completion_tokens": 4, "total_tokens": 7}
    });
    Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "application/json")
        .body(Body::from(json.to_string()))
        .unwrap()
}

async fn spawn_recording_upstream(tag: &'static str) -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let app = Router::new()
        .route(
            "/v1/messages",
            post(move |req: Request<Body>| anthropic_upstream(tag, req)),
        )
        .route(
            "/v1/chat/completions",
            post(move |req: Request<Body>| openai_upstream(tag, req)),
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

fn anthropic_body(p: &str) -> serde_json::Value {
    serde_json::json!({
        "model": format!("t4-anthropic-model-{p}"),
        "system": format!("t4-system-{p}"),
        "max_tokens": 64,
        "messages": [{"role": "user", "content": format!("t4-user-{p}")}]
    })
}

fn openai_body(p: &str) -> serde_json::Value {
    serde_json::json!({
        "model": "zap/glm-5.2",
        "messages": [
            {"role": "system", "content": "t4-openai-system"},
            {"role": "user", "content": format!("t4-openai-user-{p}")}
        ]
    })
}

#[tokio::test]
async fn t4_full_chain_rewrite_transparent_auth_sessions_and_paired_blocks() {
    // ── 环境隔离: HOME→临时目录; cc 走 ZAP_UPSTREAM_BASE 显式覆盖 ──────
    let tmp = tempfile::tempdir().unwrap();
    let orig_home = std::env::var("HOME").ok();
    let orig_upstream = std::env::var("ZAP_UPSTREAM_BASE").ok();
    let orig_omp_key = std::env::var("ZAP_OMP_KEY").ok();
    std::env::set_var("HOME", tmp.path());
    std::env::remove_var("ZAP_UPSTREAM_BASE");

    let cc_port = spawn_recording_upstream("cc").await;
    let omppi_port = spawn_recording_upstream("omppi").await;

    // /omp、/pi 出口: ~/.config/zap/omp-upstream.json(两前缀共用同一出口)。
    // api_key_env 指向已装载的 env — 用于证明"不注"(透明管道零注入)。
    std::fs::create_dir_all(tmp.path().join(".config/zap")).unwrap();
    let omp_cfg = format!(
        r#"{{"api_base":"http://127.0.0.1:{omppi_port}","api_key_env":"ZAP_OMP_KEY","response_format":"anthropic"}}"#
    );
    std::fs::write(tmp.path().join(".config/zap/omp-upstream.json"), omp_cfg).unwrap();
    std::env::set_var("ZAP_UPSTREAM_BASE", format!("http://127.0.0.1:{cc_port}"));
    std::env::set_var("ZAP_OMP_KEY", "t4-side-loaded-key");

    let blocks_db = tmp.path().join("blocks.db");
    let raw_db = tmp.path().join("raw.db");
    let gw = EntryGateway::start(0, Some(&blocks_db), Some(&raw_db))
        .await
        .unwrap();
    let entry = gw.port();
    assert_ne!(entry, 0);

    let result = async {
        let client = reqwest::Client::new();

        // ── 三前缀 × 真形状各一(anthropic /v1/messages + openai /v1/chat/completions) ──
        for p in ["cc", "omp", "pi"] {
            let resp = client
                .post(format!("http://127.0.0.1:{entry}/{p}/v1/messages?beta=true"))
                .header("x-api-key", format!("t4-ant-key-{p}"))
                .header("content-type", "application/json")
                .body(anthropic_body(p).to_string())
                .send()
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::OK, "{p} anthropic 形状态");
            let text = resp.text().await.unwrap();
            assert!(
                text.contains(&format!("reply[t4-user-{p}]")),
                "{p} anthropic 形 SSE 透传到客户端: {text}"
            );

            let resp = client
                .post(format!("http://127.0.0.1:{entry}/{p}/v1/chat/completions"))
                .header("authorization", format!("Bearer t4-bearer-{p}"))
                .header("content-type", "application/json")
                .body(openai_body(p).to_string())
                .send()
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::OK, "{p} openai 形状态");
            let text = resp.text().await.unwrap();
            assert!(
                text.contains("ok-openai"),
                "{p} openai 形 JSON 透传到客户端: {text}"
            );
        }

        // ── 不注: 客户端零凭据 → 上游零 auth 头(即使 api_key_env 已配已装载) ──
        let resp = client
            .post(format!("http://127.0.0.1:{entry}/omp/v1/messages"))
            .header("content-type", "application/json")
            .body(
                serde_json::json!({
                    "model": "t4-noauth",
                    "messages": [{"role": "user", "content": "t4-noauth-user"}]
                })
                .to_string(),
            )
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // ── 上游视角: URL 改写 + auth 透明管道 ──────────────────────────
        {
            let rec = RECORDED.lock();
            assert_eq!(rec.len(), 7, "6 形状请求 + 1 零凭据请求全达上游");

            for p in ["cc", "omp", "pi"] {
                // cc 前缀 → cc 出口; omp/pi 前缀 → omp-upstream.json 出口。
                let tag = if p == "cc" { "cc" } else { "omppi" };
                let model_marker = format!("t4-anthropic-model-{p}");

                let ant = rec
                    .iter()
                    .find(|r| r.tag == tag
                        && r.path == "/v1/messages?beta=true"
                        && r.body.contains(&model_marker))
                    .unwrap_or_else(|| panic!("{p} anthropic 形未达 {tag} 上游: {rec:?}"));
                assert_eq!(ant.method, "POST");
                assert_eq!(
                    ant.x_api_key.as_deref(),
                    Some(format!("t4-ant-key-{p}").as_str()),
                    "{p} x-api-key 原样到上游(不剥不改)"
                );
                assert_eq!(
                    ant.authorization, None,
                    "{p} 客户端未带 authorization → 不得注入"
                );
                assert_eq!(ant.content_type.as_deref(), Some("application/json"));
                let got: serde_json::Value = serde_json::from_str(&ant.body).unwrap();
                assert_eq!(got, anthropic_body(p), "{p} 请求体逐字节透传");

                let oai = rec
                    .iter()
                    .find(|r| r.tag == tag
                        && r.path == "/v1/chat/completions"
                        && r.body.contains(&format!("t4-openai-user-{p}")))
                    .unwrap_or_else(|| panic!("{p} openai 形未达 {tag} 上游: {rec:?}"));
                assert_eq!(oai.method, "POST");
                assert_eq!(
                    oai.authorization.as_deref(),
                    Some(format!("Bearer t4-bearer-{p}").as_str()),
                    "{p} authorization 原样到上游(不剥不改)"
                );
                assert_eq!(oai.x_api_key, None, "{p} 不得混注 x-api-key");
                let got: serde_json::Value = serde_json::from_str(&oai.body).unwrap();
                assert_eq!(got, openai_body(p), "{p} openai 体逐字节透传");
            }

            let noauth = rec
                .iter()
                .find(|r| r.path == "/v1/messages" && r.body.contains("t4-noauth"))
                .expect("零凭据请求未达上游");
            assert_eq!(
                noauth.x_api_key, None,
                "客户端零凭据 → 上游不得出现 x-api-key(不注)"
            );
            assert_eq!(noauth.authorization, None, "同上, authorization");
        }

        // ── 数据面: 三前缀各自归并 session + blocks 成对 ─────────────────
        //    (forwarder/processor 异步, 留窗口)
        tokio::time::sleep(Duration::from_millis(300)).await;
        for (p, session, harness) in [
            ("cc", "external-cc", "claude-code"),
            ("omp", "external-omp", "omp"),
            ("pi", "external-pi", "pi"),
        ] {
            let blocks = blocks_of(&blocks_db, session);
            assert!(!blocks.is_empty(), "{session} 必有捕获块");

            // Spawn(懒发): 恰一个, harness 归并正确。
            let spawns: Vec<_> = blocks
                .iter()
                .filter(|b| b.block_type == BlockType::Spawn)
                .collect();
            assert_eq!(spawns.len(), 1, "{session} 恰一个 Spawn");
            assert_eq!(spawns[0].harness_type, harness);
            assert_eq!(spawns[0].metadata["mode"], "external");
            assert_eq!(spawns[0].metadata["harness_type"], harness);

            // SystemPrompt 成对: anthropic 形(顶层 system 字段) + openai 形
            // (messages 内 system role, T6 起同样提取) 各一。
            let sys: Vec<_> = blocks
                .iter()
                .filter(|b| b.block_type == BlockType::SystemPrompt)
                .collect();
            assert_eq!(sys.len(), 2, "{session} SystemPrompt 恰两个(anthropic+openai)");
            let ant_sys = sys
                .iter()
                .find(|b| b.content == format!("t4-system-{p}").into_bytes())
                .unwrap();
            assert_eq!(ant_sys.metadata["source"], "anthropic_request");
            assert_eq!(
                ant_sys.metadata["model"],
                format!("t4-anthropic-model-{p}")
            );
            let oai_sys = sys
                .iter()
                .find(|b| b.content == b"t4-openai-system".to_vec())
                .unwrap();
            assert_eq!(oai_sys.metadata["source"], "openai_request");
            assert_eq!(oai_sys.metadata["model"], "zap/glm-5.2");

            // UserPrompt 成对: anthropic 用户消息 + openai 用户消息,
            // 来源各自按形标注。/omp 另收零凭据请求 → 多一条。
            let mut users: Vec<String> = blocks
                .iter()
                .filter(|b| b.block_type == BlockType::UserPrompt)
                .map(|b| String::from_utf8_lossy(&b.content).to_string())
                .collect();
            let mut expected = vec![
                format!("t4-user-{p}"),
                format!("t4-openai-user-{p}"),
            ];
            if p == "omp" {
                expected.push("t4-noauth-user".to_string());
            }
            users.sort();
            expected.sort();
            assert_eq!(users, expected, "{session} UserPrompt 成对");
            for b in blocks.iter().filter(|b| b.block_type == BlockType::UserPrompt) {
                let content = String::from_utf8_lossy(&b.content);
                let expected_source = if content.starts_with("t4-openai-user-") {
                    "openai_request"
                } else {
                    "anthropic_request"
                };
                assert_eq!(
                    b.metadata["source"], expected_source,
                    "{session} UserPrompt {content} 来源"
                );
            }

            // Response 成对: anthropic SSE 回包 + openai JSON 回包
            // (+/omp 零凭据回包)。两形各自解析: anthropic 提
            // content/usage/stop_reason, openai 提 content/usage
            // (prompt/completion→input/output)/finish_reason。
            let mut models: Vec<String> = blocks
                .iter()
                .filter(|b| b.block_type == BlockType::Response)
                .filter_map(|b| {
                    b.metadata["model"].as_str().map(str::to_string)
                })
                .collect();
            let mut expected_models =
                vec![format!("t4-anthropic-model-{p}"), "zap/glm-5.2".to_string()];
            if p == "omp" {
                expected_models.push("t4-noauth".to_string());
            }
            models.sort();
            expected_models.sort();
            assert_eq!(models, expected_models, "{session} Response 成对(按模型对位)");

            let ant_resp = blocks
                .iter()
                .find(|b| b.block_type == BlockType::Response
                    && b.metadata["model"] == format!("t4-anthropic-model-{p}"))
                .unwrap();
            assert_eq!(ant_resp.metadata["source"], "anthropic_response");
            assert_eq!(
                ant_resp.content,
                format!("reply[t4-user-{p}]").into_bytes(),
                "{session} anthropic 回包文本"
            );
            assert_eq!(ant_resp.metadata["usage"]["input_tokens"], 12);
            assert_eq!(ant_resp.metadata["usage"]["output_tokens"], 34);
            assert_eq!(ant_resp.metadata["stop_reason"], "end_turn");

            let oai_resp = blocks
                .iter()
                .find(|b| b.block_type == BlockType::Response
                    && b.metadata["model"] == "zap/glm-5.2")
                .unwrap();
            assert_eq!(oai_resp.metadata["source"], "openai_response");
            assert_eq!(
                oai_resp.content,
                b"ok-openai".to_vec(),
                "{session} openai 回包文本"
            );
            assert_eq!(oai_resp.metadata["usage"]["input_tokens"], 3);
            assert_eq!(oai_resp.metadata["usage"]["output_tokens"], 4);
            assert_eq!(oai_resp.metadata["finish_reason"], "stop");

            // 除 Spawn/Exit 外, 所有捕获块 source ∈ {anthropic_request,
            // anthropic_response, openai_request, openai_response} —
            // 无来源不明的块。
            for b in &blocks {
                if matches!(b.block_type, BlockType::Spawn | BlockType::Exit) {
                    continue;
                }
                let src = b.metadata["source"].as_str().unwrap_or_default();
                assert!(
                    src == "anthropic_request"
                        || src == "anthropic_response"
                        || src == "openai_request"
                        || src == "openai_response",
                    "{session} 块 {} source 异常: {src}",
                    b.block_type
                );
            }
        }
    }
    .await;

    // ── stop: 三 session 落 Exit, 端口释放 ─────────────────────────────
    let port = gw.port();
    gw.stop();
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        std::net::TcpStream::connect(("127.0.0.1", port)).is_err(),
        "entry 端口 stop 后必须释放"
    );
    for session in ["external-cc", "external-omp", "external-pi"] {
        let blocks = blocks_of(&blocks_db, session);
        assert!(
            blocks.iter().any(|b| b.block_type == BlockType::Exit),
            "{session} stop 必落 Exit"
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
    match orig_omp_key {
        Some(v) => std::env::set_var("ZAP_OMP_KEY", v),
        None => std::env::remove_var("ZAP_OMP_KEY"),
    }
    result
}
