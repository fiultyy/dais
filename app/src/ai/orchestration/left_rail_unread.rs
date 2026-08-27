//! 左栏未读角标数据源 — 轮询编排 messages 表,写入 LeftRailStatusModel。
//!
//! ## 架构
//!
//! 后台 `std::thread` 每 2s 调用 store 的 peek 方法(不标记 read/delivered),
//! 数值变化时通过 `async_channel` 推送到 GPUI 主线程的
//! `UnreadPollConsumer` 单例,由其调用 `LeftRailStatusModel::set_unread`。
//!
//! feature 关闭时零编译产物(整文件在 `app/src/ai/mod.rs` 的
//! `#[cfg(feature = "orchestration")]` 门控下)。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::workspace::view::left_rail_status::LeftRailStatusModel;
use warpui::{Entity, ModelContext, SingletonEntity};

/// 轮询间隔: 2 秒(与 router 退避间隔对齐)。
const POLL_INTERVAL: Duration = Duration::from_secs(2);

/// 后台线程推给 GPUI 主线程的快照。
#[derive(Debug, Clone)]
struct UnreadSnapshot {
    total: u32,
    by_project: HashMap<PathBuf, u32>,
}

// ---------------------------------------------------------------------------
// 进程全局 channel — 初始化时一次性拆分
// ---------------------------------------------------------------------------

static CHANNEL: OnceLock<(
    async_channel::Sender<UnreadSnapshot>,
    async_channel::Receiver<UnreadSnapshot>,
)> = OnceLock::new();

/// 初始化 channel 并返回 (sender, receiver)。
/// 仅在 app 启动时调用一次;重复调用返回同一对。
fn init_channel() -> (
    async_channel::Sender<UnreadSnapshot>,
    async_channel::Receiver<UnreadSnapshot>,
) {
    CHANNEL
        .get_or_init(|| async_channel::bounded(1))
        .clone()
}

// ---------------------------------------------------------------------------
// UnreadPollConsumer — GPUI 单例,消费 channel 并写入 LeftRailStatusModel
// ---------------------------------------------------------------------------

/// GPUI 单例:消费后台线程推送的未读快照,写入 LeftRailStatusModel。
pub struct UnreadPollConsumer;

impl UnreadPollConsumer {
    pub fn new(ctx: &mut ModelContext<Self>) -> Self {
        let (_tx, rx) = init_channel();
        // 用 spawn_stream_local 消费 channel(与 PtyBridgeConsumer 同模式)。
        ctx.spawn_stream_local(
            rx,
            move |_me, snapshot, ctx| {
                let model = LeftRailStatusModel::handle(ctx);
                model.update(ctx, |m, ctx| {
                    m.set_unread(snapshot.total, snapshot.by_project, ctx);
                });
            },
            |_, _| {},
        );
        Self
    }
}

impl Entity for UnreadPollConsumer {
    type Event = ();
}

impl SingletonEntity for UnreadPollConsumer {}
// ---------------------------------------------------------------------------
// 后台轮询线程
// ---------------------------------------------------------------------------

/// 启动未读轮询线程。返回 (shutdown flag, JoinHandle)——
/// shutdown 在 Drop 时置 true,线程退出。
///
/// 调用方应 `std::mem::forget` 返回值以获得进程生命周期(与 router 同模式)。
pub fn spawn_unread_poller() -> (Arc<AtomicBool>, JoinHandle<()>) {
    let shutdown = Arc::new(AtomicBool::new(false));
    let (sender, _rx) = init_channel();

    let handle = thread::Builder::new()
        .name("orch-unread-poll".into())
        .spawn({
            let shutdown = shutdown.clone();
            move || {
                // 首次调 connection::store() 触发 lazy init;
                // lib.rs 已在此之前调 set_database_path。
                let store = ::ai::agent::orchestration::connection::store();
                let mut last_total: u32 = u32::MAX; // 首轮必推

                // 首次 sleep:给 GUI 启动留时间,避免首轮空转。
                thread::sleep(POLL_INTERVAL);

                while !shutdown.load(Ordering::Relaxed) {
                    let total = match store.count_unread_all() {
                        Ok(n) => n,
                        Err(e) => {
                            log::warn!("orch-unread-poll: count_unread_all failed: {e}");
                            thread::sleep(POLL_INTERVAL);
                            continue;
                        }
                    };
                    let by_project = store
                        .count_unread_by_project()
                        .unwrap_or_default();

                    // 数值未变 → 跳过推送(model 内部也有去重,此处提前截断通道流量)。
                    if total != last_total {
                        last_total = total;
                        let _ = sender.try_send(UnreadSnapshot { total, by_project });
                    }

                    thread::sleep(POLL_INTERVAL);
                }
            }
        })
        .expect("spawn orch-unread-poll thread");

    (shutdown, handle)
}

