//! Zap 拦截体系验收测试（PR #14/#15/#16/#17）。
//! 输出 `RESULT <节点> <状态> :: <证据>` 行，由外部汇总为报告。
//! G3 子进程: 环境变量 ZAP_G3_CHILD=1 时走 crasher 分支。
//! B1/B2: 真实 Claude Code 经本地 TLS proxy 走 BigModel Anthropic 兼容端点。

use std::process::Command;
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::response::Response;
use futures_util::StreamExt;
use harness_blocks::{
    BlockStore, BlockType, HarnessBlock, InterceptMode, RawCache,
};
use harness_integration::{build_spawn_env, parse_anthropic_request, Integration, SessionContext};
use proxy_interceptor::{HarnessType, ProxyManager, RawEvent, UpstreamConfig};

// ── 证据收集 ─────────────────────────────────────────────────────────────

static RESULTS: std::sync::Mutex<Vec<(String, &'static str, String)>> =
    std::sync::Mutex::new(Vec::new());

fn record(node: &str, pass: bool, evidence: String) {
    println!("RESULT {} {} :: {}", node, if pass { "PASS" } else { "FAIL" }, evidence);
    RESULTS
        .lock()
        .unwrap()
        .push((node.to_string(), if pass { "PASS" } else { "FAIL" }, evidence));
}

fn record_skip(node: &str) {
    println!("RESULT {} SKIP :: HUMAN_REVIEW_REQUIRED", node);
    RESULTS
        .lock()
        .unwrap()
        .push((node.to_string(), "SKIP", "HUMAN_REVIEW_REQUIRED".to_string()));
}

// ── 假上游: 流式 SSE 风格响应 ────────────────────────────────────────────

const CHUNKS: u32 = 10;
const CHUNK_DELAY_MS: u64 = 30;

async fn streaming_handler() -> Response {
    let stream = futures_util::stream::iter(0..CHUNKS).then(|i| async move {
        tokio::time::sleep(Duration::from_millis(CHUNK_DELAY_MS)).await;
        Ok::<bytes::Bytes, std::io::Error>(format!("chunk-{i}\n").into())
    });
    Response::new(Body::from_stream(stream))
}

async fn start_fake_upstream() -> u16 {
    let app = axum::Router::new().fallback(streaming_handler);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move { axum::serve(listener, app).await });
    port
}

fn tls_client(ca_cert_path: &std::path::Path) -> reqwest::Client {
    let pem = std::fs::read(ca_cert_path).unwrap();
    let cert = reqwest::Certificate::from_pem(&pem).unwrap();
    reqwest::Client::builder()
        .add_root_certificate(cert)
        .no_proxy()
        .build()
        .unwrap()
}

fn plain_client() -> reqwest::Client {
    reqwest::Client::builder().no_proxy().build().unwrap()
}

fn anthropic_request_body() -> serde_json::Value {
    serde_json::json!({
        "model": "claude-3-5-sonnet",
        "max_tokens": 64,
        "system": "You are a helpful assistant.",
        "messages": [{"role": "user", "content": "hello"}]
    })
}

/// 流式请求，返回 (chunk 数, 首 chunk 毫秒, 末 chunk 毫秒, 合并文本)
async fn stream_request(client: &reqwest::Client, url: &str) -> (usize, u128, u128, String) {
    let start = Instant::now();
    let resp = client.post(url).json(&anthropic_request_body()).send().await.unwrap();
    assert!(resp.status().is_success(), "upstream status {}", resp.status());
    let mut first: Option<u128> = None;
    let mut text = String::new();
    let mut stream = resp.bytes_stream();
    while let Some(item) = stream.next().await {
        let b = item.unwrap();
        if first.is_none() {
            first = Some(start.elapsed().as_millis());
        }
        text.push_str(&String::from_utf8_lossy(&b));
    }
    let last = start.elapsed().as_millis();
    let n = text.matches("chunk-").count();
    (n, first.unwrap(), last, text)
}

fn mtime_nanos(p: &std::path::Path) -> i128 {
    std::fs::metadata(p)
        .unwrap()
        .modified()
        .unwrap()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos() as i128
}

// ── Node A: TLS 代理基础 ─────────────────────────────────────────────────

async fn node_a() {
    // A1: 删 CA 目录 → ProxyManager::new()（内部 ensure_local_ca）→ 4 个 pem 生成
    let home = std::env::var("HOME").unwrap();
    let ca_dir = std::path::Path::new(&home).join(".config/dais/proxy-ca");
    if ca_dir.exists() {
        std::fs::remove_dir_all(&ca_dir).unwrap();
    }
    let ca_cert_path = ca_dir.join("ca-cert.pem");
    let server_cert_path = ca_dir.join("server-cert.pem");
    let _mgr = ProxyManager::new().unwrap(); // 触发 ensure_local_ca
    let files = ["ca-cert.pem", "ca-key.pem", "server-cert.pem", "server-key.pem"];
    let present: Vec<bool> = files.iter().map(|f| ca_dir.join(f).is_file()).collect();
    let a1 = present.iter().all(|b| *b) && ca_cert_path.is_file();
    record("A1", a1, format!("dir={} files_present={}/4 (trigger: ProxyManager::new → ensure_local_ca)", ca_dir.display(), present.iter().filter(|b| **b).count()));

    // A2: 二次调用不重新生成
    let m1 = mtime_nanos(&ca_cert_path);
    let _mgr2 = ProxyManager::new().unwrap();
    let m2 = mtime_nanos(&ca_cert_path);
    record("A2", m1 == m2, format!("ca-cert.pem mtime {m1} -> {m2} ({})", if m1 == m2 { "unchanged" } else { "REGENERATED" }));

    // A3: SAN 检查
    let out = Command::new("openssl")
        .args(["x509", "-in", server_cert_path.to_str().unwrap(), "-noout", "-text"])
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    let san_ok = text.contains("IP Address:127.0.0.1") && text.contains("DNS:localhost");
    let san_line = text
        .lines()
        .skip_while(|l| !l.contains("Alternative"))
        .take(2)
        .collect::<Vec<_>>()
        .join(" | ");
    record("A3", san_ok, format!("openssl x509 -text: {}", san_line.trim()));

    // 假上游
    let upstream_port = start_fake_upstream().await;
    let upstream = UpstreamConfig::resolve(
        HarnessType::ClaudeCode,
        Some(&format!("http://127.0.0.1:{upstream_port}")),
    )
    .unwrap();

    // A4: 两个 ProxyServer 端口不同
    let manager = ProxyManager::new().unwrap();
    let mut h1 = manager.allocate(upstream.clone()).await.unwrap();
    let mut h2 = manager.allocate(upstream).await.unwrap();
    let a4 = h1.port != h2.port && h1.port != 0 && h2.port != 0;
    record("A4", a4, format!("proxy ports: {} / {} ({})", h1.port, h2.port, if a4 { "distinct" } else { "COLLISION" }));

    // A5: 流式透传，逐 chunk 到达
    let client = tls_client(&h1.ca_cert_path);
    let proxy_url = format!("https://127.0.0.1:{}/v1/messages", h1.port);
    let (n, first_ms, last_ms, text) = stream_request(&client, &proxy_url).await;
    let spread = last_ms - first_ms;
    let a5 = n == CHUNKS as usize && spread >= 150; // 缓冲式透传 spread≈0
    record("A5", a5, format!("via proxy: chunks={n}/{} first_chunk={}ms last_chunk={}ms spread={spread}ms (expected ~{}ms; buffering would collapse to ~0) sample={:?}", CHUNKS, first_ms, last_ms, CHUNKS as u64 * CHUNK_DELAY_MS, &text[..text.len().min(30)]));

    // A6: drop ProxyHandle → 端口释放
    let dying_port = h2.port;
    drop(h2);
    let mut released = false;
    for _ in 0..30 {
        if std::net::TcpStream::connect(("127.0.0.1", dying_port)).is_err() {
            released = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    record("A6", released, format!("after drop(handle): connect 127.0.0.1:{dying_port} -> {}", if released { "refused (port released)" } else { "STILL ACCEPTING" }));

    // A7: 拦截 request → raw_cache 有 BLOB
    let _ = stream_request(&client, &proxy_url).await;
    let mut req_body: Option<bytes::Bytes> = None;
    let mut chunk_events = 0usize;
    let mut done_events = 0usize;
    while let Ok(ev) = h1.raw_rx.try_recv() {
        match ev {
            RawEvent::Request { body, .. } => req_body = Some(body),
            RawEvent::ResponseChunk { .. } => chunk_events += 1,
            RawEvent::ResponseDone { .. } => done_events += 1,
        }
    }
    let tmp = std::env::temp_dir().join(format!("zap-acc-rawcache-{}.sqlite", std::process::id()));
    let _ = std::fs::remove_file(&tmp);
    let cache = RawCache::open(tmp.to_str().unwrap()).unwrap();
    let blob = req_body.clone().unwrap_or_default();
    cache.insert_raw("acc-a7", "request", &blob, 1).unwrap();
    let peeked = cache.peek("acc-a7").unwrap();
    let parsed_ok = serde_json::from_slice::<serde_json::Value>(&blob)
        .map(|v| v.get("model").and_then(|m| m.as_str()) == Some("claude-3-5-sonnet"))
        .unwrap_or(false);
    let a7 = parsed_ok && peeked.len() == 1 && peeked[0].content == blob.to_vec();
    record("A7", a7, format!("RawEvent::Request body={}B model=parsed_ok:{parsed_ok}, chunk_events={chunk_events} done_events={done_events}, raw_cache peek=1 entry={}B roundtrip_identical={}", blob.len(), peeked[0].content.len(), peeked[0].content == blob.to_vec()));
    let _ = std::fs::remove_file(&tmp);
}

// ── Node B: System Prompt 拦截 ───────────────────────────────────────────

/// 从 ~/.claude/settings.json 读 CC 的 env 配置（BigModel 中转 token）
fn cc_upstream_creds() -> (String, String) {
    let home = std::env::var("HOME").unwrap();
    let settings: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(format!("{home}/.claude/settings.json")).unwrap())
            .unwrap();
    let env = &settings["env"];
    let token = env["ANTHROPIC_AUTH_TOKEN"].as_str().unwrap_or("").to_string();
    let base = env["ANTHROPIC_BASE_URL"].as_str().unwrap_or("").to_string();
    (token, base)
}

/// 提取 Anthropic system 字段纯文本（与 extract_system_text 同逻辑，独立复核用）
fn extract_system_text_for_audit(system: &serde_json::Value) -> String {
    match system {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Array(blocks) => blocks
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

async fn node_b_real() {
    let cc_bin = "/home/yy/.local/bin/claude";
    let (token, real_base) = cc_upstream_creds();
    if token.is_empty() || real_base.is_empty() {
        record("B1", false, "CC settings.json 缺少 ANTHROPIC_AUTH_TOKEN/ANTHROPIC_BASE_URL".into());
        record("B2", false, "同 B1".into());
        return;
    }

    // 上游: BigModel Anthropic 兼容端点, Bearer 模式（与 CC 原生 ANTHROPIC_AUTH_TOKEN 行为一致）
    std::env::set_var("ANTHROPIC_AUTH_TOKEN", &token);
    let upstream = UpstreamConfig {
        api_base: real_base.trim_end_matches('/').to_string(),
        auth_header: "authorization".into(),
        auth_prefix: "Bearer ".into(),
        api_key_env: "ANTHROPIC_AUTH_TOKEN".into(),
        request_path: "/v1/messages".into(),
        response_format: proxy_interceptor::ResponseFormat::AnthropicSSE,
    };

    let manager = ProxyManager::new().unwrap();
    let mut handle = manager.allocate(upstream).await.unwrap();

    // 用 --settings 覆盖 CC 的 BASE_URL（settings.json env 优先级高于进程 env）
    let proxy_base = format!("https://127.0.0.1:{}", handle.port);
    let settings = serde_json::json!({
        "env": {
            "ANTHROPIC_BASE_URL": proxy_base,
            "NODE_EXTRA_CA_CERTS": handle.ca_cert_path.display().to_string(),
            "ANTHROPIC_AUTH_TOKEN": token,
            "API_TIMEOUT_MS": "300000",
            "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC": "1",
            "ANTHROPIC_DEFAULT_HAIKU_MODEL": "glm-5-turbo",
            "ANTHROPIC_DEFAULT_SONNET_MODEL": "glm-5.2",
            "ANTHROPIC_DEFAULT_OPUS_MODEL": "glm-5.2"
        }
    });
    let settings_path = std::env::temp_dir().join(format!("zap-acc-cc-settings-{}.json", std::process::id()));
    std::fs::write(&settings_path, settings.to_string()).unwrap();

    // 注意: std Command::output() 会阻塞 current_thread runtime (proxy 无法服务) → 死锁。
    // 必须 async spawn + timeout。
    let out = tokio::time::timeout(
        Duration::from_secs(150),
        tokio::process::Command::new(cc_bin)
            .arg("-p")
            .arg("Reply with exactly: hello")
            .arg("--settings")
            .arg(&settings_path)
            .env("ANTHROPIC_BASE_URL", &proxy_base)
            .env("NODE_EXTRA_CA_CERTS", handle.ca_cert_path.display().to_string())
            .env("ANTHROPIC_AUTH_TOKEN", &token)
            .output(),
    )
    .await;

    let _ = std::fs::remove_file(&settings_path);

    let out = match out {
        Ok(Ok(o)) => o,
        Ok(Err(e)) => {
            record("B1", false, format!("spawn claude failed: {e}"));
            record("B2", false, "同 B1".into());
            return;
        }
        Err(_) => {
            record("B1", false, "claude -p 超时 (>150s) — proxy 链路未在期限内完成".into());
            record("B2", false, "同 B1".into());
            return;
        }
    };
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();

    // 收 raw events（阻塞收尾: proxy 还活着, channel 未关, 只 try_recv 已缓冲的）
    let mut requests: Vec<(String, bytes::Bytes)> = Vec::new();
    while let Ok(ev) = handle.raw_rx.try_recv() {
        if let RawEvent::Request { path, body, .. } = ev {
            requests.push((path, body));
        }
    }
    drop(handle);

    let exit = out.status.code().unwrap_or(-1);
    // 找第一个带 system 字段的 /v1/messages 请求
    let main_req = requests
        .iter()
        .filter(|(p, _)| p.contains("/v1/messages"))
        .find(|(_, b)| {
            serde_json::from_slice::<serde_json::Value>(b)
                .map(|v| v.get("system").is_some())
                .unwrap_or(false)
        });

    let cc_failed = exit != 0 || stdout.trim().is_empty();

    match main_req {
        None => {
            let detail = format!(
                "claude exit={exit}, requests captured={}, stdout={:?}, stderr(first 400)={:?}",
                requests.len(),
                &stdout[..stdout.len().min(120)],
                &stderr[..stderr.len().min(400)]
            );
            record("B1", false, format!("未捕获到带 system 的请求 — {detail}"));
            record("B2", false, "无 raw request body 可对比".into());
        }
        Some((path, body)) => {
            let raw: serde_json::Value = serde_json::from_slice(body).unwrap();
            let ctx = SessionContext::new("acc-b1", "claude-code");
            let blocks = parse_anthropic_request(body, &ctx);
            let store = BlockStore::open_in_memory().unwrap();
            for b in &blocks {
                store.insert_block(b).unwrap();
            }
            let sys_blocks = store
                .list_blocks("acc-b1", Some(BlockType::SystemPrompt), None)
                .unwrap();

            // B1: SystemPrompt block 存在 + content 含 CC 标识
            let content = sys_blocks
                .first()
                .map(|b| String::from_utf8_lossy(&b.content).to_string())
                .unwrap_or_default();
            let has_cc_id = content.contains("Claude Code");
            let b1 = sys_blocks.len() == 1 && has_cc_id && !cc_failed;
            record("B1", b1, format!(
                "claude exit={exit} stdout={:?}, SystemPrompt blocks={}, content {}B, contains 'Claude Code': {}, model={}",
                &stdout.trim()[..stdout.trim().len().min(60)],
                sys_blocks.len(), content.len(), has_cc_id,
                sys_blocks.first().map(|b| b.metadata["model"].as_str().unwrap_or("?")).unwrap_or("?")
            ));

            // B2: raw body system 提取 vs BlockStore content 逐字节一致（无截断）
            let raw_system = extract_system_text_for_audit(raw.get("system").unwrap());
            let stored = sys_blocks.first().map(|b| b.content.clone()).unwrap_or_default();
            let identical = raw_system.as_bytes() == stored.as_slice();
            record("B2", identical, format!(
                "raw system (joined) {}B vs BlockStore content {}B, byte-identical: {}, request path={}",
                raw_system.len(), stored.len(), identical, path
            ));
        }
    }
}


fn node_b(upstream_port: u16) {
    // B3: system 为 content blocks 数组 → join
    let ctx = SessionContext::new("acc-b3", "claude-code");
    let body = serde_json::json!({
        "system": [
            {"type": "text", "text": "You are an agent"},
            {"type": "text", "text": "for testing"}
        ],
        "messages": [{"role": "user", "content": "hi"}]
    });
    let blocks = parse_anthropic_request(body.to_string().as_bytes(), &ctx);
    let sys = blocks.iter().find(|b| b.block_type == BlockType::SystemPrompt);
    let content = sys.map(|b| String::from_utf8_lossy(&b.content).to_string()).unwrap_or_default();
    record("B3", content.contains("You are an agent") && content.contains("for testing"),
        format!("SystemPrompt content = {:?}", content));

    // B4: PromptSegment children parent_id
    let store = BlockStore::open_in_memory().unwrap();
    let parent = HarnessBlock::new("acc-b4", "claude-code", BlockType::SystemPrompt, 0, b"sys".to_vec(), 1);
    store.insert_block(&parent).unwrap();
    for i in 0..2 {
        let mut seg = HarnessBlock::new("acc-b4", "claude-code", BlockType::PromptSegment, i + 1, format!("seg-{i}").into_bytes(), 2);
        seg.parent_id = Some(parent.id.clone());
        store.insert_block(&seg).unwrap();
    }
    let children = store.list_children(&parent.id).unwrap();
    let b4 = children.len() == 2 && children.iter().all(|c| c.parent_id.as_deref() == Some(parent.id.as_str()));
    record("B4", b4, format!("list_children(SystemPrompt) = {} PromptSegments, parent_id 全部正确: {}", children.len(), children.iter().all(|c| c.parent_id.is_some())));

    // B5: 带 tools 的 request → parse
    let ctx5 = SessionContext::new("acc-b5", "claude-code");
    let body5 = serde_json::json!({
        "system": "sys",
        "tools": [{"name": "bash", "description": "run cmd", "input_schema": {"type": "object"}}],
        "messages": [{"role": "user", "content": "hi"}]
    });
    let blocks5 = parse_anthropic_request(body5.to_string().as_bytes(), &ctx5);
    let has_toolcall = blocks5.iter().any(|b| b.block_type == BlockType::ToolCall);
    let tools_in_meta = blocks5
        .iter()
        .find(|b| b.block_type == BlockType::SystemPrompt)
        .map(|b| b.metadata.get("tools").map(|t| t.as_array().map(|a| a.len()).unwrap_or(0)).unwrap_or(0))
        .unwrap_or(0);
    let types: Vec<String> = blocks5.iter().map(|b| b.block_type.as_str().to_string()).collect();
    record("B5", has_toolcall,
        format!("带 tools 的 request parse → block types = {types:?}; ToolCall(kind=definition) 存在: {has_toolcall}; tools[{}] 同时记录在 SystemPrompt.metadata.tools", tools_in_meta));

    let _ = upstream_port;
}

async fn node_b_async(upstream_port: u16) {
    let mut integ = Integration::in_memory("acc-b6", "claude-code").unwrap();
    integ.start_hooks().await.unwrap();
    let manager = ProxyManager::new().unwrap();
    let upstream = UpstreamConfig::resolve(
        HarnessType::ClaudeCode,
        Some(&format!("http://127.0.0.1:{upstream_port}")),
    )
    .unwrap();
    integ.start_proxy(&manager, upstream).await.unwrap();
    let env = build_spawn_env(
        InterceptMode::Full,
        integ.proxy_handle(),
        integ.hook_url().as_deref(),
        integ.hook_token().as_deref(),
        HarnessType::ClaudeCode,
    );
    let env_has = |k: &str| env.iter().any(|(ek, _)| ek == k);
    let env_ok = env_has("ANTHROPIC_BASE_URL") && env_has("NODE_EXTRA_CA_CERTS") && env_has("ZAP_HOOK_SERVER_URL") && env_has("ZAP_HOOK_TOKEN");

    let client = tls_client(integ.ca_cert_path().unwrap());
    let url = format!("https://127.0.0.1:{}/v1/messages", integ.proxy_port().unwrap());
    let _ = stream_request(&client, &url).await;
    tokio::time::sleep(Duration::from_millis(500)).await; // 等 raw_processor 落库

    let sys_count = integ.with_store(|s| {
        s.list_blocks("acc-b6", Some(BlockType::SystemPrompt), None).unwrap().len()
    });
    record("B6", env_ok && sys_count >= 1,
        format!("BYOP: spawn_env keys={:?}, 1 req via proxy → SystemPrompt×{sys_count} (raw_processor 管道落库)", env.iter().map(|(k, _)| k).collect::<Vec<_>>()));
}

// ── Node C: Agent Hooks ──────────────────────────────────────────────────

async fn node_c() {
    let mut integ = Integration::in_memory("acc-c", "claude-code").unwrap();
    integ.start_hooks().await.unwrap();
    let base = integ.hook_url().unwrap();
    let token = integ.hook_token().unwrap().to_string();
    let client = plain_client();

    // C1: 可连
    let health = client.get(format!("{base}/health")).send().await.unwrap();
    let c1 = health.status().as_u16() == 200;
    record("C1", c1, format!("GET {base}/health -> {} (health 不鉴权)", health.status()));

    let post = |path: &str, body: serde_json::Value| {
        let client = client.clone();
        let url = format!("{base}{path}");
        let token = token.clone();
        async move {
            client
                .post(url)
                .header("x-zap-hook-token", &token)
                .json(&body)
                .send()
                .await
                .unwrap()
                .status()
        }
    };

    // C2: UserPromptSubmit → UserPrompt block
    let st = post("/hooks/user_prompt_submit", serde_json::json!({"prompt": "acceptance hello", "session_id": "acc-c"})).await;
    tokio::time::sleep(Duration::from_millis(150)).await;
    let up = integ.with_store(|s| s.list_blocks("acc-c", Some(BlockType::UserPrompt), None).unwrap());
    let c2 = st.as_u16() == 200 && up.len() == 1 && up[0].content == b"acceptance hello";
    record("C2", c2, format!("POST user_prompt_submit -> {st}, UserPrompt blocks={}, content={:?}", up.len(), up.first().map(|b| String::from_utf8_lossy(&b.content).to_string())));

    // C3: Pre/PostToolUse → ToolCall + ToolResult 成对
    let s1 = post("/hooks/pre_tool_use", serde_json::json!({"tool_name": "Bash", "tool_input": {"command": "ls"}})).await;
    let s2 = post("/hooks/post_tool_use", serde_json::json!({"tool_name": "Bash", "tool_output": "file1"})).await;
    tokio::time::sleep(Duration::from_millis(150)).await;
    let (tc, tr) = integ.with_store(|s| {
        (
            s.list_blocks("acc-c", Some(BlockType::ToolCall), None).unwrap(),
            s.list_blocks("acc-c", Some(BlockType::ToolResult), None).unwrap(),
        )
    });
    let c3 = s1.is_success() && s2.is_success() && tc.len() == 1 && tr.len() == 1;
    record("C3", c3, format!("pre={s1} post={s2}, ToolCall×{} content={:?}, ToolResult×{} content={:?}", tc.len(), tc.first().map(|b| String::from_utf8_lossy(&b.content).to_string()), tr.len(), tr.first().map(|b| String::from_utf8_lossy(&b.content).to_string())));

    // C4: Stop → Exit block
    let s4 = post("/hooks/stop", serde_json::json!({"exit_code": 0})).await;
    tokio::time::sleep(Duration::from_millis(150)).await;
    let ex = integ.with_store(|s| s.list_blocks("acc-c", Some(BlockType::Exit), None).unwrap());
    let c4 = s4.is_success() && ex.len() == 1 && ex[0].content == b"0";
    record("C4", c4, format!("POST stop -> {s4}, Exit blocks={}, content={:?}", ex.len(), ex.first().map(|b| String::from_utf8_lossy(&b.content).to_string())));

    // C5: 错误 token → 401/403
    let resp = client
        .post(format!("{base}/hooks/stop"))
        .header("Authorization", "Bearer invalid-token")
        .json(&serde_json::json!({"exit_code": 0}))
        .send()
        .await
        .unwrap();
    let code = resp.status().as_u16();
    record("C5", code == 401 || code == 403,
        format!("POST with Authorization: Bearer invalid-token -> {code} (shared token 鉴权: Bearer / x-zap-hook-token / ?token= 三选一)"));
}

// ── Node E: Upstream 配置 ────────────────────────────────────────────────

fn node_e() {
    std::env::remove_var("ZAP_UPSTREAM_BASE");
    let cfg = UpstreamConfig::resolve(HarnessType::ClaudeCode, None).unwrap();
    record("E1", cfg.api_base == "https://api.anthropic.com",
        format!("resolve(ClaudeCode, None) -> api_base={} auth_header={} key_env={} path={}", cfg.api_base, cfg.auth_header, cfg.api_key_env, cfg.request_path));

    let cfg2 = UpstreamConfig::resolve(HarnessType::ClaudeCode, Some("https://custom.com")).unwrap();
    record("E2", cfg2.api_base == "https://custom.com",
        format!("resolve(ClaudeCode, Some(custom.com)) -> api_base={} (形状保留: auth_header={}, path={})", cfg2.api_base, cfg2.auth_header, cfg2.request_path));
}

// ── Node F: Block 数据层 ─────────────────────────────────────────────────

fn node_f() {
    let tmp = std::env::temp_dir().join(format!("zap-acc-blocks-{}.sqlite", std::process::id()));
    let _ = std::fs::remove_file(&tmp);
    let store = BlockStore::open(tmp.to_str().unwrap()).unwrap();
    let ctx = SessionContext::new("acc-f", "claude-code");

    // F1: 完整 session 序列
    let seq_types = [BlockType::Spawn, BlockType::SystemPrompt, BlockType::UserPrompt, BlockType::Response, BlockType::Exit];
    for t in seq_types {
        let b = HarnessBlock::new("acc-f", "claude-code", t, ctx.next_seq(), b"x".to_vec(), ctx.now_ms());
        store.insert_block(&b).unwrap();
    }
    let all = store.list_blocks("acc-f", None, None).unwrap();
    let got: Vec<String> = all.iter().map(|b| b.block_type.as_str().to_string()).collect();
    let want: Vec<String> = seq_types.iter().map(|t| t.as_str().to_string()).collect();
    record("F1", got == want, format!("sequence = {got:?} (expected {want:?})"));

    // F2: children parent_id (ResponseChunk)
    let resp = HarnessBlock::new("acc-f", "claude-code", BlockType::Response, ctx.next_seq(), b"r".to_vec(), 1);
    store.insert_block(&resp).unwrap();
    for i in 0..3 {
        let mut c = HarnessBlock::new("acc-f", "claude-code", BlockType::ResponseChunk, ctx.next_seq(), format!("c{i}").into_bytes(), 2);
        c.parent_id = Some(resp.id.clone());
        store.insert_block(&c).unwrap();
    }
    let kids = store.list_children(&resp.id).unwrap();
    let f2 = kids.len() == 3 && kids.iter().all(|k| k.parent_id.as_deref() == Some(resp.id.as_str()));
    record("F2", f2, format!("Response ×1 + ResponseChunk children ×{}, parent_id 全部指向 Response: {}", kids.len(), kids.iter().all(|k| k.parent_id.is_some())));

    // F3: 多轮 sequence 严格递增
    let seqs: Vec<u32> = store.list_blocks("acc-f", None, None).unwrap().iter().map(|b| b.sequence).collect();
    let strict = seqs.windows(2).all(|w| w[0] < w[1]);
    record("F3", strict, format!("sequences = {seqs:?}, strictly increasing: {strict}"));

    // F4: RawCache drain 后为空
    let tmp_raw = std::env::temp_dir().join(format!("zap-acc-raw-{}.sqlite", std::process::id()));
    let _ = std::fs::remove_file(&tmp_raw);
    let cache = RawCache::open(tmp_raw.to_str().unwrap()).unwrap();
    for i in 0..3 {
        cache.insert_raw("acc-f", "request", format!("raw-{i}").as_bytes(), i).unwrap();
    }
    let drained = cache.drain("acc-f").unwrap();
    let after = cache.peek("acc-f").unwrap();
    let f4 = drained.len() == 3 && after.is_empty();
    record("F4", f4, format!("drain -> {} entries, peek after = {}", drained.len(), after.len()));

    // F5: delete_session → blocks + raw 全清（两 store 各自的 delete_session）
    let raw_cache2 = RawCache::open(tmp_raw.to_str().unwrap()).unwrap();
    raw_cache2.insert_raw("acc-f", "response", b"resp-bytes", 9).unwrap();
    let removed = store.delete_session("acc-f").unwrap();
    let raw_removed = raw_cache2.delete_session("acc-f").unwrap();
    let blocks_left = store.list_blocks("acc-f", None, None).unwrap().len();
    let raw_left = raw_cache2.peek("acc-f").unwrap().len();
    let f5 = removed >= 8 && blocks_left == 0 && raw_removed == 1 && raw_left == 0;
    record("F5", f5, format!("store.delete_session removed {removed} blocks, blocks_left={blocks_left}; raw_cache.delete_session removed {raw_removed}, entries_left={raw_left}"));

    // F6: content 是原始 Vec<u8> BLOB
    let binary: Vec<u8> = vec![0x00, 0xFF, 0xFE, 0x01, 0x80, 0x7F, 0xC3, 0x28];
    let nb = HarnessBlock::new("acc-f2", "claude-code", BlockType::PtyRaw, 0, binary.clone(), 1);
    store.insert_block(&nb).unwrap();
    let back = store.get_block(&nb.id).unwrap().unwrap();
    let f6 = back.content == binary && back.content.iter().any(|b| *b > 0x7F);
    record("F6", f6, format!("插入非 UTF-8 字节 {:02X?} ({}B), 读回完全一致: {}, 含 >0x7F 字节: {}", &binary[..4], binary.len(), back.content == binary, back.content.iter().any(|b| *b > 0x7F)));
    let _ = std::fs::remove_file(&tmp);
    let _ = std::fs::remove_file(&tmp_raw);
}

// ── Node G: 端到端 ───────────────────────────────────────────────────────

async fn node_g() {
    // G2: proxy 开/关流式延迟对比
    let upstream_port = start_fake_upstream().await;
    let direct_url = format!("http://127.0.0.1:{upstream_port}/v1/messages");
    let client_plain = plain_client();
    let (_, d_first, d_last, _) = stream_request(&client_plain, &direct_url).await;

    let manager = ProxyManager::new().unwrap();
    let upstream = UpstreamConfig::resolve(
        HarnessType::ClaudeCode,
        Some(&format!("http://127.0.0.1:{upstream_port}")),
    )
    .unwrap();
    let handle = manager.allocate(upstream).await.unwrap();
    let client_tls = tls_client(&handle.ca_cert_path);
    let proxy_url = format!("https://127.0.0.1:{}/v1/messages", handle.port);
    // 预热一次（TLS 握手 + 代理链路），测第二次排除建连噪声
    let _ = stream_request(&client_tls, &proxy_url).await;
    let (_, p_first, p_last, _) = stream_request(&client_tls, &proxy_url).await;
    drop(handle);

    let overhead_first = p_first as i64 - d_first as i64;
    let overhead_last = p_last as i64 - d_last as i64;
    let spread_ok = (p_last - p_first) >= (d_last - d_first) / 2;
    record("G2", spread_ok && overhead_last < 500,
        format!("direct: first={d_first}ms last={d_last}ms spread={}ms | via proxy(2nd req): first={p_first}ms last={p_last}ms spread={}ms | overhead first=+{overhead_first}ms last=+{overhead_last}ms", d_last - d_first, p_last - p_first));
}

fn node_g3() {
    // G3: 模拟 crash (SIGKILL) → 重开 raw_cache 数据保留
    let exe = std::env::current_exe().unwrap();
    let db = std::env::temp_dir().join(format!("zap-acc-g3-{}.sqlite", std::process::id()));
    let _ = std::fs::remove_file(&db);

    let mut child = Command::new(exe)
        .args(["all_nodes", "--exact", "--nocapture"])
        .env("ZAP_G3_CHILD", "1")
        .env("ZAP_G3_DB", &db)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    std::thread::sleep(Duration::from_millis(2000)); // 等子进程完成 insert（含二进制链接时间余量）
    child.kill().unwrap(); // SIGKILL 模拟 crash
    let _ = child.wait();

    let cache = RawCache::open(db.to_str().unwrap()).unwrap();
    let entries = cache.peek("g3-crash").unwrap();
    let ok = entries.len() == 3 && entries.iter().all(|e| !e.content.is_empty());
    record("G3", ok, format!("child SIGKILLed after 3 inserts; reopened raw_cache {}: entries={}, content sizes={:?}", db.display(), entries.len(), entries.iter().map(|e| e.content.len()).collect::<Vec<_>>()));
    let _ = std::fs::remove_file(&db);
}

// ── 主流程 ───────────────────────────────────────────────────────────────

fn g3_child_branch() -> ! {
    let db = std::env::var("ZAP_G3_DB").unwrap();
    let cache = RawCache::open(&db).unwrap();
    for i in 0..3 {
        cache.insert_raw("g3-crash", "request", format!("crash-payload-{i}").as_bytes(), i as i64).unwrap();
    }
    eprintln!("G3_CHILD_READY");
    loop {
        std::thread::sleep(Duration::from_secs(1));
    }
}

#[tokio::test]
async fn all_nodes() {
    if std::env::var("ZAP_G3_CHILD").is_ok() {
        g3_child_branch();
    }

    println!("==== NODE A ====");
    node_a().await;

    println!("==== NODE B (real CC) ====");
    node_b_real().await;

    println!("==== NODE B (synthetic) ====");
    let upstream_port = start_fake_upstream().await;
    node_b(upstream_port);
    node_b_async(upstream_port).await;

    println!("==== NODE C ====");
    node_c().await;

    println!("==== NODE E ====");
    node_e();

    println!("==== NODE F ====");
    node_f();

    println!("==== NODE G ====");
    record_skip("G1");
    node_g().await;
    node_g3();

    // 汇总
    let results = RESULTS.lock().unwrap().clone();
    let pass = results.iter().filter(|(_, s, _)| *s == "PASS").count();
    let fail = results.iter().filter(|(_, s, _)| *s == "FAIL").count();
    println!("\n==== SUMMARY ====");
    println!("TOTAL={} PASS={} FAIL={}", results.len(), pass, fail);
    for (node, status, _) in &results {
        if *status == "FAIL" {
            println!("FAILED: {node}");
        }
    }
}
