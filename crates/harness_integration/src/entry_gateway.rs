//! T5/T8 entry gateway: 单端口入口的捕获接线。
//!
//! [`proxy_interceptor::EntryServer`] 提供网络面(明文 HTTP 单端口, 路径前缀
//! 分流); 本模块补数据面: 每前缀一条旁路捕获流按**实例**归并观测 session
//! (T8: 一实例 = 一次 CLI 启动, 各自 Spawn):
//! - `/cc` → session `external-cc[-<instance>]`(harness 串 `claude-code`)
//! - `/omp` → session `external-omp[-<instance>]`(harness 串 `omp`)
//! - `/pi` → session `external-pi[-<instance>]`(harness 串 `pi`)
//!
//! 实例身份 = 请求头 [`INSTANCE_HEADER`](`x-zap-instance`), 由 zap 别名
//! (`cc-zap`/`omp-zap`/`pi-zap`)在调用时铸造(pid+随机+epoch, 每次调用一次
//! CLI 实例启动)。网络面转发前剥该头(透明管道字节不变); 无标记流量(裸
//! 客户端/未升级的模型配置)回落**默认 session**(T5 行为, 零回归)。
//!
//! 生命周期 = 外部捕获开关(常驻, 不做 idle reap): 开 → [`EntryGateway::start`],
//! 关/退出 → [`EntryGateway::stop`](`Exit` block reason=stopped + 端口释放)。
//! Spawn 懒发沿用外部捕获既有语义: 建 lane 只预留 seq, 首个 RawEvent(该
//! 实例首个真实请求)才落 Spawn block — 零流量 session 不在观测台堆积。
//! 实例 lane 数每前缀上限 [`MAX_LANES_PER_PREFIX`], 超限回落默认 session
//! (防异常客户端无界铸造)。实例没有可靠的生命周期结束信号(连接复用/
//! 重连不可见), lane 随网关常驻 — 与 T5 前缀 session 同口径。
//!
//! DB 路径由调用方注入(app 传观测台的 `harness_blocks.db`/`harness_raw_cache.db`,
//! 测试传临时文件), 块落库即对观测台可见。

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use harness_blocks::{BlockStore, BlockType, HarnessBlock, RawCache};
use parking_lot::Mutex;
use proxy_interceptor::{EntryServer, RawEvent, ResponseFormat};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use uuid::Uuid;

use crate::raw_processor::run_raw_processor;
use crate::session::SessionContext;

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// 前缀 → (默认 session id, harness 串)。实例 session id 在默认 id 后
/// 追加 `-{instance}`。
const PREFIXES: [(&str, &str, &str); 3] = [
    ("/cc", "external-cc", "claude-code"),
    ("/omp", "external-omp", "omp"),
    ("/pi", "external-pi", "pi"),
];

/// 实例标记头: zap 别名铸造, 网络面(`proxy_interceptor::handler`)转发前
/// 剥除, 数据面在此读取做 session 键控。
pub const INSTANCE_HEADER: &str = "x-zap-instance";

/// 单前缀 lane 上限(默认 lane 计入): 超限的新标记回落默认 session。
const MAX_LANES_PER_PREFIX: usize = 64;

/// 从 `RawEvent::Request.headers`(JSON 对象)提取合法实例标记:
/// 1..=64 字符且仅 `[A-Za-z0-9._-]`; 非法/缺失 → None(默认 session)。
fn instance_marker(headers: &serde_json::Value) -> Option<String> {
    let v = headers.get(INSTANCE_HEADER)?.as_str()?;
    let ok = !v.is_empty()
        && v.len() <= 64
        && v.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'_' || b == b'-');
    if ok {
        Some(v.to_string())
    } else {
        None
    }
}

/// 一个实例(=一次 CLI 启动, 或无标记流量的默认通道)的观测 lane。
struct InstanceLane {
    session_id: String,
    ctx: Arc<SessionContext>,
    store: Arc<Mutex<BlockStore>>,
    pending_spawn: Arc<Mutex<Option<HarnessBlock>>>,
    activity: Arc<AtomicI64>,
    born_at: i64,
    tx: mpsc::Sender<RawEvent>,
    processor: JoinHandle<()>,
}

impl InstanceLane {
    /// 建 lane: 预留懒 Spawn + 起专属 raw processor(每 lane 独立
    /// SessionContext, seq 各自单调)。
    fn spawn_lane(
        session_id: String,
        harness: &'static str,
        store: Arc<Mutex<BlockStore>>,
        raw_cache: Arc<Mutex<RawCache>>,
    ) -> Arc<Self> {
        let ctx = Arc::new(SessionContext::new(session_id.clone(), harness));

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

        let (tx, rx) = mpsc::channel(256);
        // 混合形状通道: 同一实例会并行跑 anthropic/openai 形流量(T4 钉
        // 此场景), 逐事件按线上形状分派; Generic = 不提供通道级倾向。
        let processor = tokio::spawn(run_raw_processor(
            rx,
            store.clone(),
            raw_cache,
            ctx.clone(),
            ResponseFormat::Generic,
        ));

        Arc::new(Self {
            session_id,
            ctx,
            store,
            pending_spawn,
            activity,
            born_at,
            tx,
            processor,
        })
    }

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

/// 一条前缀的数据面: 默认 lane + 实例 lane 表 + 串行分派任务。
struct PrefixPlane {
    prefix: &'static str,
    harness: &'static str,
    /// 键 = 实例标记; `""` = 默认 lane(无标记流量回落, 网关启动即建)。
    lanes: Arc<Mutex<HashMap<String, Arc<InstanceLane>>>>,
    demux: JoinHandle<()>,
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

/// T5/T8 入口网关: [`EntryServer`] + 三前缀按实例归并捕获。常驻, 随外部
/// 捕获开关启停; 显式 [`Self::stop`] 才落 Exit block。
pub struct EntryGateway {
    port: u16,
    planes: Vec<PrefixPlane>,
    server: Option<EntryServer>,
}

impl EntryGateway {
    /// 绑定端口(0 = 未运行/绑定失败)。
    pub fn port(&self) -> u16 {
        self.port
    }

    /// 绑定 `127.0.0.1:port` 并为三前缀各建数据面(默认 lane + 实例分派)。
    /// 端口被占 → Err(调用方降级: 开关开着但入口不可用, 快照 port=0)。
    pub async fn start(
        port: u16,
        blocks_db: Option<&Path>,
        raw_cache_db: Option<&Path>,
    ) -> proxy_interceptor::Result<Self> {
        let mut server = EntryServer::start(port).await?;

        let mut planes = Vec::new();
        // EntryServer.raw_rxs 顺序与 PREFIXES 一致(构造即按此序 push)。
        for ((prefix, default_session, harness), (_, mut raw_rx)) in
            PREFIXES.iter().zip(std::mem::take(&mut server.raw_rxs))
        {
            let prefix: &'static str = prefix;
            let harness: &'static str = harness;
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

            let lanes = Arc::new(Mutex::new(HashMap::new()));
            // 默认 lane 常驻: 无标记流量的归并点(T5 行为回落), 零流量时
            // 保持零块(懒 Spawn 未物化)。
            let default_lane = InstanceLane::spawn_lane(
                default_session.to_string(),
                harness,
                store.clone(),
                raw_cache.clone(),
            );
            lanes.lock().insert(String::new(), default_lane);

            // 实例分派(T8): 单任务串行消费 → 每 lane FIFO 保序。请求事件
            // 读标记建/取 lane, 并登记 请求id→lane; 无头事件(响应 chunk/
            // done)经登记回路由(登记在 done 后清除)。
            let demux = {
                let lanes = lanes.clone();
                let store = store.clone();
                let raw_cache = raw_cache.clone();
                tokio::spawn(async move {
                    let mut ids: HashMap<Uuid, String> = HashMap::new();
                    while let Some(event) = raw_rx.recv().await {
                        let key = match &event {
                            RawEvent::Request { id, headers, .. } => {
                                let key = instance_marker(headers).unwrap_or_default();
                                ids.insert(*id, key.clone());
                                key
                            }
                            // 极端情况(Request 满通道被丢)下 chunk 可能先于
                            // 登记到达 → 回落默认 lane。
                            RawEvent::ResponseChunk { id, .. } => {
                                ids.get(id).cloned().unwrap_or_default()
                            }
                            RawEvent::ResponseDone { id, .. } => {
                                ids.remove(id).unwrap_or_default()
                            }
                        };
                        let lane = {
                            let mut map = lanes.lock();
                            if key.is_empty() {
                                map.get("").cloned()
                            } else if let Some(l) = map.get(&key) {
                                Some(l.clone())
                            } else if map.len() < MAX_LANES_PER_PREFIX {
                                let session_id = format!("{default_session}-{key}");
                                let l = InstanceLane::spawn_lane(
                                    session_id,
                                    harness,
                                    store.clone(),
                                    raw_cache.clone(),
                                );
                                map.insert(key.clone(), l.clone());
                                Some(l)
                            } else {
                                tracing::warn!(
                                    "entry gateway: lane cap ({MAX_LANES_PER_PREFIX}) \
                                     reached on {prefix}, new marker falls back to \
                                     default session"
                                );
                                map.get("").cloned()
                            }
                        };
                        let Some(lane) = lane else { continue };
                        lane.activity.store(now_ms(), Ordering::Relaxed);
                        // 首个事件 = 该 session 真实 → 物化懒 Spawn(立即写库,
                        // 观测台可见先于响应流完结)。
                        if let Some(block) = lane.pending_spawn.lock().take() {
                            let s = lane.store.lock();
                            let _ = s.insert_block(&block);
                        }
                        if lane.tx.send(event).await.is_err() {
                            // processor 消亡(仅 teardown 时): 放弃该前缀。
                            break;
                        }
                    }
                })
            };

            planes.push(PrefixPlane {
                prefix,
                harness,
                lanes,
                demux,
            });
        }

        Ok(Self {
            port: server.port,
            planes,
            server: Some(server),
        })
    }

    /// 观测 session 快照(观测台 UI 数据源): 每前缀按出生序列出默认 +
    /// 各实例 lane。
    pub fn snapshot(&self) -> Vec<EntrySessionInfo> {
        let mut out = Vec::new();
        for p in &self.planes {
            let mut lanes: Vec<Arc<InstanceLane>> =
                p.lanes.lock().values().cloned().collect();
            lanes.sort_by_key(|l| l.born_at);
            for l in lanes {
                out.push(EntrySessionInfo {
                    prefix: p.prefix,
                    session_id: l.session_id.clone(),
                    harness: p.harness,
                    port: self.port,
                    last_activity_ms: l.activity.load(Ordering::Relaxed),
                    born_at_ms: l.born_at,
                });
            }
        }
        out
    }

    /// 显式停止: 落 Exit block(活跃 session) + **同步等端口释放** —
    /// graceful 优先(在途请求自然完结, 上限 2s), 超时 abort serve 任务
    /// 兜底; 返回时端口确定不可连(见 [`EntryServer::stop`])。
    pub async fn stop(mut self) {
        for p in &self.planes {
            p.demux.abort();
            let lanes: Vec<Arc<InstanceLane>> =
                p.lanes.lock().values().cloned().collect();
            for l in lanes {
                l.processor.abort();
                l.finalize();
            }
        }
        if let Some(server) = self.server.take() {
            server.stop().await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 标记提取: 合法值原样, 非法(空/超长/异字符)/缺失 → None。
    #[test]
    fn marker_validation() {
        let obj = |v: Option<&str>| match v {
            Some(s) => serde_json::json!({ INSTANCE_HEADER: s }),
            None => serde_json::json!({}),
        };

        assert_eq!(
            instance_marker(&obj(Some("351209-12345-1786930770"))),
            Some("351209-12345-1786930770".to_string())
        );
        assert_eq!(
            instance_marker(&obj(Some("a.b_c-d"))),
            Some("a.b_c-d".to_string())
        );
        assert_eq!(instance_marker(&obj(Some(""))), None, "空标记非法");
        assert_eq!(instance_marker(&obj(Some("bad marker!"))), None, "空格/! 非法");
        assert_eq!(
            instance_marker(&obj(Some("x".repeat(65).as_str()))),
            None,
            "超长非法"
        );
        // 64 字符上边界合法。
        let m64 = "x".repeat(64);
        assert_eq!(instance_marker(&obj(Some(&m64))), Some(m64));
        assert_eq!(instance_marker(&obj(None)), None, "缺失回落默认");
        // 非字符串值(异常客户端)同样回落。
        assert_eq!(instance_marker(&serde_json::json!({ INSTANCE_HEADER: 42 })), None);
    }
}
