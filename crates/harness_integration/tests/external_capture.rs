//! T2b 外部会话登记与入块链路集成测试。
//!
//! 单个 `#[tokio::test]` 串行跑完整生命周期（HOME/ZAP_UPSTREAM_BASE 都是
//! 进程级环境变量，拆多测试会并行竞态）：
//! 1. 两个并行登记 → 各自 TLS proxy 转发 curl 的 anthropic 请求 →
//!    blocks.db 出现各自 session 的 Spawn(mode=external) + SystemPrompt；
//!    session_id 各自正确、seq 各自从 0 单调、互不串。
//! 2. hook 归属：带 reg1 token POST reg1 hook URL → 块落 reg1 的 session；
//!    reg1 token 打 reg2 的 hook → 401（token 按登记隔离）。
//! 3. 显式 stop_registration → Exit(reason=stopped) + 登记消失。
//! 4. 注入时钟推进 31min → reap_idle → 两个登记自动回收 + Exit(reason=
//!    idle_timeout) + proxy/hook 端口释放（连接拒绝）。

use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use futures_util::StreamExt;

use harness_integration::{
    BlockStore, BlockType, ExternalCaptureManager, HarnessType, HOOK_SERVER_URL_ENV,
    HOOK_TOKEN_ENV, IDLE_TIMEOUT_MS,
};

// ── 假上游（同 acceptance.rs 模式：任意路径流式回 chunk） ────────────────

async fn start_fake_upstream() -> u16 {
    let app = axum::Router::new().fallback(|| async {
        let stream = futures_util::stream::iter(0..4u32).then(|i| async move {
            tokio::time::sleep(Duration::from_millis(10)).await;
            Ok::<bytes::Bytes, std::io::Error>(format!("chunk-{i}\n").into())
        });
        axum::response::Response::new(axum::body::Body::from_stream(stream))
    });
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

fn anthropic_body() -> serde_json::Value {
    serde_json::json!({
        "model": "claude-3-5-sonnet",
        "max_tokens": 64,
        "system": "You are a helpful assistant.",
        "messages": [{"role": "user", "content": "external capture test"}]
    })
}

/// 等端口连接被拒（drop 后释放的判据，A6 同款轮询）。
async fn assert_port_released(port: u16) {
    for _ in 0..50 {
        if std::net::TcpStream::connect(("127.0.0.1", port)).is_err() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("port {port} still accepting after teardown");
}

/// 读库辅助：按 sequence 升序取某 session 全部块（第二个连接，验证持久化落盘）。
fn blocks_of(db: &std::path::Path, session_id: &str) -> Vec<harness_integration::HarnessBlock> {
    let store = BlockStore::open(db.to_string_lossy().to_string()).unwrap();
    let mut blocks = store.list_blocks(session_id, None, None).unwrap();
    blocks.sort_by_key(|b| b.sequence);
    blocks
}

#[tokio::test]
async fn external_capture_registration_lifecycle() {
    // ── 环境隔离：CA 进 tempdir HOME；upstream 指假服务 ──────────────────
    let home_tmp = tempfile::tempdir().unwrap();
    let db_tmp = tempfile::tempdir().unwrap();
    let blocks_db = db_tmp.path().join("harness_blocks.db");
    let raw_db = db_tmp.path().join("harness_raw_cache.db");
    let upstream_port = start_fake_upstream().await;

    let orig_home = std::env::var("HOME").ok();
    let orig_upstream = std::env::var("ZAP_UPSTREAM_BASE").ok();
    std::env::set_var("HOME", home_tmp.path());
    std::env::set_var("ZAP_UPSTREAM_BASE", format!("http://127.0.0.1:{upstream_port}"));

    // 注入时钟：原子计数，测试推进。
    let fake_now = Arc::new(AtomicI64::new(1_000_000_000));
    let mut mgr = ExternalCaptureManager::new()
        .with_db_paths(&blocks_db, &raw_db)
        .with_clock({
        let fake_now = fake_now.clone();
        move || fake_now.load(Ordering::Relaxed)
    });

    let result = async {
        // ── 0. 懒 Spawn：登记后无流量 → 库里无该 session ────────────────────
        let reg0 = mgr
            .register_external_session(HarnessType::Omp)
            .await
            .unwrap();
        assert!(
            blocks_of(&blocks_db, &reg0.session_id).is_empty(),
            "lazy Spawn: registered-but-idle session must be invisible"
        );
        // tick 也不物化（无任何 RawEvent/hook 活动）。
        let reaped0 = mgr.tick();
        assert!(reaped0.is_empty());
        assert!(
            blocks_of(&blocks_db, &reg0.session_id).is_empty(),
            "tick must not materialize an idle session's Spawn"
        );

        // ── 1. 两个并行登记 + curl 入块 ───────────────────────────────────
        let reg1 = mgr
            .register_external_session(HarnessType::ClaudeCode)
            .await
            .unwrap();
        let reg2 = mgr
            .register_external_session(HarnessType::ClaudeCode)
            .await
            .unwrap();
        assert_ne!(reg1.session_id, reg2.session_id);
        assert_ne!(reg0.session_id, reg1.session_id);
        assert_ne!(reg1.proxy_port, reg2.proxy_port, "proxy ports distinct");
        assert_ne!(reg1.hook_base_url, reg2.hook_base_url);
        assert_ne!(reg1.hook_token, reg2.hook_token, "per-registration tokens");
        assert_eq!(mgr.registrations().len(), 3);

        // env 交付：per-registration 完整变量集，指向各自 proxy/hook。
        let env1: std::collections::HashMap<_, _> =
            reg1.env.iter().cloned().collect();
        assert_eq!(
            env1.get("ANTHROPIC_BASE_URL").unwrap(),
            &format!("https://127.0.0.1:{}", reg1.proxy_port)
        );
        assert_eq!(env1.get(HOOK_SERVER_URL_ENV).unwrap(), &reg1.hook_base_url);
        assert_eq!(env1.get(HOOK_TOKEN_ENV).unwrap(), &reg1.hook_token);

        // curl 向登记端口发 anthropic 请求（经各自 TLS proxy → 假上游）。
        let client = tls_client(mgr.ca_cert_path().unwrap());
        for reg in [&reg1, &reg2] {
            let resp = client
                .post(format!("https://127.0.0.1:{}/v1/messages", reg.proxy_port))
                .json(&anthropic_body())
                .send()
                .await
                .unwrap();
            assert!(resp.status().is_success(), "proxy status {}", resp.status());
        }
        tokio::time::sleep(Duration::from_millis(400)).await;

        let b1 = blocks_of(&blocks_db, &reg1.session_id);
        let b2 = blocks_of(&blocks_db, &reg2.session_id);
        for (blocks, reg, tag) in [(&b1, &reg1, "reg1"), (&b2, &reg2, "reg2")] {
            assert!(
                blocks.iter().any(|b| b.block_type == BlockType::Spawn),
                "{tag}: Spawn block missing"
            );
            assert!(
                blocks
                    .iter()
                    .any(|b| b.block_type == BlockType::SystemPrompt),
                "{tag}: parsed SystemPrompt block missing"
            );
            // seq 各自从 0 单调。
            let seqs: Vec<u32> = blocks.iter().map(|b| b.sequence).collect();
            let mut expected = 0;
            for s in &seqs {
                assert_eq!(*s, expected, "{tag}: seq must be dense+monotonic");
                expected += 1;
            }
            // session 归属正确（互不串）。
            assert!(
                blocks.iter().all(|b| b.session_id == reg.session_id),
                "{tag}: cross-session leak"
            );
            // Spawn 是首块且 mode=external。
            assert_eq!(blocks[0].block_type, BlockType::Spawn);
            assert_eq!(
                blocks[0].metadata.get("mode").and_then(|m| m.as_str()),
                Some("external"),
                "{tag}: spawn metadata mode"
            );
        }

        // ── 2. hook 归属：带 reg1 token POST reg1 hook → 块落 reg1 session ──
        let pc = plain_client();
        let hook_resp = pc
            .post(format!("{}/hooks/user_prompt_submit", reg1.hook_base_url))
            .header("x-zap-hook-token", &reg1.hook_token)
            .json(&serde_json::json!({"prompt": "hook ownership test"}))
            .send()
            .await
            .unwrap();
        assert_eq!(hook_resp.status().as_u16(), 200);
        tokio::time::sleep(Duration::from_millis(200)).await;
        let b1_after = blocks_of(&blocks_db, &reg1.session_id);
        assert!(
            b1_after
                .iter()
                .any(|b| b.block_type == BlockType::UserPrompt
                    && b.content == b"hook ownership test"),
            "hook block must land in reg1's session"
        );

        // reg1 token 打 reg2 hook → 401（token 隔离 = 归属隔离）。
        let cross = pc
            .post(format!("{}/hooks/user_prompt_submit", reg2.hook_base_url))
            .header("x-zap-hook-token", &reg1.hook_token)
            .json(&serde_json::json!({"prompt": "should be rejected"}))
            .send()
            .await
            .unwrap();
        assert!(
            cross.status().as_u16() == 401 || cross.status().as_u16() == 403,
            "cross-registration token must be rejected, got {}",
            cross.status()
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(
            !blocks_of(&blocks_db, &reg2.session_id)
                .iter()
                .any(|b| b.content == b"should be rejected"),
            "rejected hook post must not create blocks"
        );

        // ── 3. 显式 stop_registration（激活后再停 → Spawn+Exit 成对） ────
        let reg3 = mgr
            .register_external_session(HarnessType::Codex)
            .await
            .unwrap();
        assert_eq!(mgr.registrations().len(), 4);
        // 激活 reg3（hook 块 → 懒 Spawn 物化）。
        let hook3 = pc
            .post(format!("{}/hooks/user_prompt_submit", reg3.hook_base_url))
            .header("x-zap-hook-token", &reg3.hook_token)
            .json(&serde_json::json!({"prompt": "activate reg3"}))
            .send()
            .await
            .unwrap();
        assert_eq!(hook3.status().as_u16(), 200);
        tokio::time::sleep(Duration::from_millis(200)).await;
        // tick 让 hook 路径的懒 Spawn 物化（seq_count 已越保留号）。
        let _ = mgr.tick();
        let b3 = blocks_of(&blocks_db, &reg3.session_id);
        assert!(
            b3.iter().any(|b| b.block_type == BlockType::Spawn),
            "hook-activated session must materialize its lazy Spawn on tick"
        );

        assert!(mgr.stop_registration(reg3.id));
        assert!(!mgr.stop_registration(reg3.id), "second stop is a no-op");
        assert_eq!(mgr.registrations().len(), 3);
        let b3 = blocks_of(&blocks_db, &reg3.session_id);
        let exit3 = b3
            .iter()
            .find(|b| b.block_type == BlockType::Exit)
            .expect("explicit stop after activity must record Exit");
        assert_eq!(
            exit3.metadata.get("reason").and_then(|r| r.as_str()),
            Some("stopped")
        );
        let hook3_port: u16 = reg3
            .hook_base_url
            .rsplit(':')
            .next()
            .unwrap()
            .parse()
            .unwrap();
        assert_port_released(reg3.proxy_port).await;
        assert_port_released(hook3_port).await;

        // ── 4. 闲置回收（注入时钟推进 31min，不真等） ─────────────────────
        fake_now.store(1_000_000_000 + IDLE_TIMEOUT_MS + 60_000, Ordering::Relaxed);
        let reaped = mgr.reap_idle();
        assert_eq!(
            reaped.len(),
            3,
            "all idle registrations reaped, got {reaped:?}"
        );
        assert!(mgr.registrations().is_empty());
        // reg0 never showed activity: reap leaves no blocks at all.
        assert!(
            blocks_of(&blocks_db, &reg0.session_id).is_empty(),
            "never-active session must stay fully invisible after reap"
        );
        for reg in [&reg1, &reg2] {
            let blocks = blocks_of(&blocks_db, &reg.session_id);
            let exit = blocks
                .iter()
                .find(|b| b.block_type == BlockType::Exit)
                .unwrap_or_else(|| panic!("idle reap must record Exit for {}", reg.session_id));
            assert_eq!(
                exit.metadata.get("reason").and_then(|r| r.as_str()),
                Some("idle_timeout")
            );
            // blocks 留库不删（观测台历史可见）。
            assert!(
                blocks.iter().any(|b| b.block_type == BlockType::Spawn),
                "history must survive reap"
            );
            let hook_port: u16 = reg
                .hook_base_url
                .rsplit(':')
                .next()
                .unwrap()
                .parse()
                .unwrap();
            assert_port_released(reg.proxy_port).await;
            assert_port_released(hook_port).await;
        }

        // 未到时不再回收（重新登记一个，时钟不动 → reap 空手而归）。
        let reg4 = mgr
            .register_external_session(HarnessType::Generic)
            .await
            .unwrap();
        assert!(mgr.reap_idle().is_empty(), "fresh registration is not idle");
        assert_eq!(mgr.registrations().len(), 1);

        // ── 5. 保护式回收（T3 pane 武装: pane 存活=通道存活）────────────
        // 时钟推进到超时后, tick_except(protected) 不回收受保护登记;
        // 同一登记在无保护 tick 下照常回收 → 两条路径语义独立成立。
        fake_now.store(2_000_000_000 + IDLE_TIMEOUT_MS + 60_000, Ordering::Relaxed);
        let reaped = mgr.tick_except(&[reg4.id]);
        assert!(reaped.is_empty(), "protected registration must survive tick_except");
        assert_eq!(mgr.registrations().len(), 1, "protected registration stays registered");

        let reaped = mgr.tick_except(&[]);
        assert_eq!(reaped, vec![reg4.id], "unprotected tick reaps the same idle registration");
        assert!(mgr.registrations().is_empty());
    }
    .await;

    // ── 环境恢复 ──────────────────────────────────────────────────────────
    match orig_home {
        Some(h) => std::env::set_var("HOME", h),
        None => std::env::remove_var("HOME"),
    }
    match orig_upstream {
        Some(u) => std::env::set_var("ZAP_UPSTREAM_BASE", u),
        None => std::env::remove_var("ZAP_UPSTREAM_BASE"),
    }
    result
}
