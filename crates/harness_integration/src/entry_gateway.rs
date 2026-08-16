//! T5 entry gateway: 单端口入口的捕获接线。
//!
//! [`proxy_interceptor::EntryServer`] 提供网络面(明文 HTTP 单端口, 路径前缀
//! 分流); 本模块补数据面: 每前缀一条旁路捕获流归并到**一个常驻观测
//! session**(用户拍板取舍: 单端口无连接身份, 前缀即 harness 标识):
//! - `/cc` → session `external-cc`(harness 串 `claude-code`)
//! - `/omp` → session `external-omp`(harness 串 `omp`)
//! - `/pi` → session `external-pi`(harness 串 `pi`)
//!
//! 生命周期 = 外部捕获开关(常驻, 不做 idle reap): 开 → [`EntryGateway::start`],
//! 关/退出 → [`EntryGateway::stop`](`Exit` block reason=stopped + 端口释放)。
//! Spawn 懒发沿用外部捕获既有语义: 注册时只预留 seq, 首个 RawEvent(该
//! session 首个真实请求)才落 Spawn block — 零流量 session 不在观测台堆积。
//!
//! DB 路径由调用方注入(app 传观测台的 `harness_blocks.db`/`harness_raw_cache.db`,
//! 测试传临时文件), 块落库即对观测台可见。

use std::path::Path;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use harness_blocks::{BlockStore, BlockType, HarnessBlock, RawCache};
use parking_lot::Mutex;
use proxy_interceptor::EntryServer;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::raw_processor::run_raw_processor;
use crate::session::SessionContext;

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// 前缀 → (观测 session id, harness 串)。
const PREFIXES: [(&str, &str, &str); 3] = [
    ("/cc", "external-cc", "claude-code"),
    ("/omp", "external-omp", "omp"),
    ("/pi", "external-pi", "pi"),
];

/// 一条前缀的常驻捕获通道(纯数据面)。
struct EntryCapture {
    prefix: &'static str,
    session_id: String,
    harness: &'static str,
    ctx: Arc<SessionContext>,
    store: Arc<Mutex<BlockStore>>,
    pending_spawn: Arc<Mutex<Option<HarnessBlock>>>,
    activity: Arc<AtomicI64>,
    born_at: i64,
    forwarder: JoinHandle<()>,
    processor: JoinHandle<()>,
}

impl EntryCapture {
    /// Teardown: 活跃过(懒 Spawn 已物化)则落 `Exit`; 从未活跃零块。
    fn finalize(&self) {
        if self.materialize(false) {
            self.record_exit("stopped");
        }
    }

    /// `force=false` 时仅探测活跃性: 懒 Spawn 仍在(从未有 RawEvent)→
    /// 未活跃; 已被物化 → 活跃。
    fn materialize(&self, force: bool) -> bool {
        let pending = self.pending_spawn.lock();
        match pending.as_ref() {
            Some(_) => !force,
            None => true,
        }
    }

    fn record_exit(&self, reason: &str) {
        let mut b = HarnessBlock::new(
            &self.ctx.session_id,
            &self.ctx.harness_type,
            BlockType::Exit,
            self.ctx.next_seq(),
            Vec::new(),
            self.ctx.now_ms(),
        );
        b.metadata = serde_json::json!({ "exit_code": 0, "reason": reason });
        let s = self.store.lock();
        let _ = s.insert_block(&b);
    }
}

/// 观测 session 快照行(观测台 UI 数据源)。
#[derive(Debug, Clone)]
pub struct EntrySessionInfo {
    pub prefix: &'static str,
    pub session_id: String,
    pub harness: &'static str,
    pub port: u16,
    pub last_activity_ms: i64,
    pub born_at_ms: i64,
}

/// T5 入口网关: [`EntryServer`] + 三前缀捕获归并。常驻, 随外部捕获开关
/// 启停; 显式 [`Self::stop`] 才落 Exit block。
pub struct EntryGateway {
    port: u16,
    captures: Vec<EntryCapture>,
    server: Option<EntryServer>,
}


impl EntryGateway {
    /// 绑定端口(0 = 未运行/绑定失败)。
    pub fn port(&self) -> u16 {
        self.port
    }

    /// 绑定 `127.0.0.1:port` 并为三前缀各建捕获通道。端口被占 → Err
    /// (调用方降级: 开关开着但入口不可用, 快照 port=0)。
    pub async fn start(
        port: u16,
        blocks_db: Option<&Path>,
        raw_cache_db: Option<&Path>,
    ) -> proxy_interceptor::Result<Self> {
        let mut server = EntryServer::start(port).await?;

        let mut captures = Vec::new();
        // EntryServer.raw_rxs 顺序与 PREFIXES 一致(构造即按此序 push)。
        for ((prefix, session_id, harness), (_, mut raw_rx)) in
            PREFIXES.iter().zip(std::mem::take(&mut server.raw_rxs))
        {
            let prefix: &'static str = prefix;
            let store = match blocks_db {
                Some(p) => Arc::new(Mutex::new(BlockStore::open(
                    p.to_string_lossy().to_string(),
                )?)),
                None => Arc::new(Mutex::new(BlockStore::open_in_memory()?)),
            };
            let raw_cache = match raw_cache_db {
                Some(p) => Arc::new(Mutex::new(RawCache::open(
                    p.to_string_lossy().to_string(),
                )?)),
                None => Arc::new(Mutex::new(RawCache::open_in_memory()?)),
            };
            let ctx = Arc::new(SessionContext::new(*session_id, *harness));

            // 懒 Spawn: 只预留 seq 与 block 体, 首个 RawEvent 才插入。
            let mut spawn_block = HarnessBlock::new(
                &ctx.session_id,
                &ctx.harness_type,
                BlockType::Spawn,
                ctx.next_seq(),
                Vec::new(),
                ctx.now_ms(),
            );
            spawn_block.metadata = serde_json::json!({
                "mode": "external",
                "harness_type": ctx.harness_type,
            });
            let pending_spawn = Arc::new(Mutex::new(Some(spawn_block)));

            let activity = Arc::new(AtomicI64::new(now_ms()));
            let born_at = activity.load(Ordering::Relaxed);

            let (fwd_tx, proc_rx) = mpsc::channel(256);
            let forwarder = {
                let activity = activity.clone();
                let pending_spawn = pending_spawn.clone();
                let store = store.clone();
                tokio::spawn(async move {
                    while let Some(event) = raw_rx.recv().await {
                        activity.store(now_ms(), Ordering::Relaxed);
                        // 首个事件 = session 真实 → 物化懒 Spawn(立即写库,
                        // 观测台可见先于响应流完结)。
                        let spawned = pending_spawn.lock().take();
                        if let Some(block) = spawned {
                            let s = store.lock();
                            let _ = s.insert_block(&block);
                        }
                        if fwd_tx.send(event).await.is_err() {
                            break;
                        }
                    }
                })
            };
            let processor = tokio::spawn(run_raw_processor(
                proc_rx,
                store.clone(),
                raw_cache,
                ctx.clone(),
            ));

            captures.push(EntryCapture {
                prefix,
                session_id: session_id.to_string(),
                harness,
                ctx,
                store,
                pending_spawn,
                activity,
                born_at,
                forwarder,
                processor,
            });
        }

        Ok(Self {
            port: server.port,
            captures,
            server: Some(server),
        })
    }

    /// 观测 session 快照(观测台 UI 数据源)。
    pub fn snapshot(&self) -> Vec<EntrySessionInfo> {
        self.captures
            .iter()
            .map(|c| EntrySessionInfo {
                prefix: c.prefix,
                session_id: c.session_id.clone(),
                harness: c.harness,
                port: self.port,
                last_activity_ms: c.activity.load(Ordering::Relaxed),
                born_at_ms: c.born_at,
            })
            .collect()
    }

    /// 显式停止: 落 Exit block(活跃 session) + 端口释放。
    pub fn stop(mut self) {
        for c in &self.captures {
            c.forwarder.abort();
            c.processor.abort();
            c.finalize();
        }
        if let Some(server) = self.server.take() {
            server.stop();
        }
    }
}
