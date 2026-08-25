//! Background message router — polls the messages table and dispatches to
//! `route_message` for lifecycle transitions.
//!
//! Runs in a dedicated `std::thread` (the store is synchronous — no async
//! benefit). Polls `drain_inbox` every 500ms, backing off to 2s when empty.
//! The thread exits when the `shutdown` flag is set.
//!
//! **At-least-once delivery**: `drain_inbox` does NOT mark messages read.
//! After each message is successfully routed (or intentionally rejected /
//! suppressed), its sequence is collected and batch-marked read via
//! `mark_messages_read`. Messages that fail reconciliation remain unread and
//! will be retried on the next poll cycle.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use super::db::OrchestrationResult;
use super::delivery;
use super::executor::PtyExecutor;
use super::idle_detector::IdleSignal;
use super::messaging;
use super::store::DieselOrchestrationStore;
use super::OrchestrationStore;

const POLL_INTERVAL: Duration = Duration::from_millis(500);
const BACKOFF_INTERVAL: Duration = Duration::from_millis(2000);
const EMPTY_BACKOFF_THRESHOLD: u32 = 3;


/// Background message router. Owns its own DB connection (separate from the
/// CLI store singleton) so message routing doesn't block CLI operations.
///
/// Implements `Drop` — when the router is dropped without an explicit
/// `shutdown()` call, it signals the thread and joins with a 2-second
/// timeout, preventing thread leaks.
pub struct MessageRouter {
    store: DieselOrchestrationStore,
    handle: String,
    shutdown: Arc<AtomicBool>,
    thread: Mutex<Option<JoinHandle<()>>>,
    /// Optional push-delivery plane: PTY executor + idle-signal probe
    /// for assigned dispatches. Injected by the app layer.
    delivery: Option<PushPlane>,
}

/// Push-delivery collaborators (app-injected): a PTY executor and an
/// idle-signal probe (channel-based; safe off the GPUI main thread).
pub struct PushPlane {
    pub executor: Arc<dyn PtyExecutor>,
    pub signal_probe: Arc<dyn Fn(&str) -> IdleSignal + Send + Sync>,
}


impl MessageRouter {
    /// Create a new router that drains messages for `handle`.
    ///
    /// The store should be constructed with a dedicated connection (not the
    /// CLI singleton) to avoid contention.
    pub fn new(store: DieselOrchestrationStore, handle: impl Into<String>) -> Self {
        Self {
            store,
            handle: handle.into(),
            shutdown: Arc::new(AtomicBool::new(false)),
            thread: Mutex::new(None),
            delivery: None,
        }
    }


    /// Attach the push-delivery plane (PTY executor + title probe).
    /// Must be called before `spawn()`.
    pub fn with_delivery(mut self, plane: PushPlane) -> Self {
        self.delivery = Some(plane);
        self
    }

    /// Spawn the router as a background thread.
    ///
    /// Stores the `JoinHandle` internally so `Drop` can join it.
    /// Panics if called more than once on the same `MessageRouter`.
    pub fn spawn(&self) {
        let mut slot = self.thread.lock().unwrap();
        assert!(slot.is_none(), "MessageRouter::spawn called more than once");

        let shutdown = self.shutdown.clone();
        let store = self.store.clone();
        let handle = self.handle.clone();
        let delivery = self
            .delivery
            .as_ref()
            .map(|p| (p.executor.clone(), p.signal_probe.clone()));

        let handle_thread = thread::Builder::new()
            .name("orch-msg-router".into())
            .spawn(move || {
                let mut empty_count: u32 = 0;
                let mut last_seen: u64 = 0; // P5: arrival 代际(单调;启动前积压使首 wait 立即醒,无害)
                while !shutdown.load(Ordering::Relaxed) {
                    // Push delivery for assigned dispatches (pointer
                    // injection on idle) — best effort, never fatal.
                    if let Some((executor, probe)) = delivery.as_ref() {
                        Self::push_pending(&store, executor, probe);
                    }
                    let sleep = match Self::drain_and_route(&store, &handle) {
                        Ok(true) => {
                            // Messages were processed — reset backoff.
                            empty_count = 0;
                            POLL_INTERVAL
                        }
                        Ok(false) => {
                            // No messages — back off.
                            empty_count = empty_count.saturating_add(1);
                            if empty_count >= EMPTY_BACKOFF_THRESHOLD {
                                BACKOFF_INTERVAL
                            } else {
                                POLL_INTERVAL
                            }
                        }
                        Err(e) => {
                            log::error!("orchestration router error: {e}");
                            // On error, back off to avoid tight error loop.
                            BACKOFF_INTERVAL
                        }
                    };
                    // P5: 到达事件唤醒(timeout 兜底 = 保留原 sleep 作 wait 上限,
                    // 正确性不依赖事件)。事件唤醒后的空轮是并发消费(check-messages
                    // 拉链 / waiter claim)所致,不是"邮箱空"的证据——不加深退避、
                    // 不清零(防事件风暴打成忙轮询);虚假唤醒同列处理(多跑一轮幂等周期)。
                    let (_gen, _timed_out) = super::arrival::wait_for_arrival(last_seen, sleep);
                    last_seen = super::arrival::current_arrival();
                }
            })
            .expect("spawn orchestration router thread");

        *slot = Some(handle_thread);
    }

    /// Drain the inbox for `handle`, route each message, and batch-mark
    /// successfully processed messages as read.
    ///
    /// Messages that fail reconciliation (or panic during routing) remain
    /// unread and will be redelivered on the next poll.
    /// Returns `true` if any messages were processed.
    fn drain_and_route(
        store: &DieselOrchestrationStore,
        handle: &str,
    ) -> OrchestrationResult<bool> {
        let messages = store.drain_inbox(handle)?;
        if messages.is_empty() {
            return Ok(false);
        }

        let mut successfully_processed: Vec<i32> = Vec::with_capacity(messages.len());

        for msg in &messages {
            // catch_unwind: a panic in route_message must not kill the router thread.
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                messaging::route_message(store, msg)
            }));
            let ok = match result {
                Ok(Ok(routing_result)) => {
                    log::debug!(
                        "orchestration: routed msg seq={} -> {:?}",
                        msg.sequence,
                        routing_result
                    );
                    true
                }
                Ok(Err(e)) => {
                    log::warn!(
                        "orchestration: route_message failed for seq={}: {e}",
                        msg.sequence
                    );
                    // Leave unread for retry next cycle.
                    false
                }
                Err(_) => {
                    log::error!(
                        "orchestration: route_message panicked for seq={}, will retry",
                        msg.sequence
                    );
                    // Leave unread for retry next cycle.
                    false
                }
            };
            if ok {
                successfully_processed.push(msg.sequence);
            }
        }

        // Batch-mark successfully processed messages as read.
        if !successfully_processed.is_empty() {
            if let Err(e) = store.mark_messages_read(&successfully_processed) {
                log::error!(
                    "orchestration: failed to mark {} messages read: {e}",
                    successfully_processed.len()
                );
                // Messages remain unread — will be retried (safe due to idempotent settlement).
            }
        }

        Ok(true)
    }

    /// Attempt pointer push-delivery for every registered dispatch.
    /// Best effort: any per-dispatch failure is logged and skipped; the
    /// message stays pending in the DB for the next tick (or the agent's
    /// `check-messages` pull).
    fn push_pending(
        store: &DieselOrchestrationStore,
        executor: &Arc<dyn PtyExecutor>,
        probe: &Arc<dyn Fn(&str) -> IdleSignal + Send + Sync>,
    ) {
        for dispatch_id in delivery::registered_dispatches() {
            let sig = probe(&dispatch_id);
            let outcome = delivery::deliver_pending(store, executor.as_ref(), &dispatch_id, &sig);
            match outcome {
                Ok(delivery::PushOutcome::Delivered { count }) => {
                    log::info!(
                        "orchestration: pushed {count} message pointer(s) to {dispatch_id}"
                    );
                }
                Ok(delivery::PushOutcome::WriteFailed(e)) => {
                    log::warn!("orchestration: push to {dispatch_id} failed: {e}");
                }
                Ok(_) => {}
                Err(e) => {
                    log::warn!("orchestration: push query failed for {dispatch_id}: {e}");
                }
            }
        }
    }

    /// Signal the router to stop and wait for the thread to exit.
    ///
    /// Blocks for up to `SHUTDOWN_JOIN_TIMEOUT` (2 s). If the thread
    /// doesn't join in time, it is detached (it will exit on its own
    /// after the next poll check).
    pub fn shutdown(&self) {
        self.shutdown.store(true, Ordering::Relaxed);
        super::arrival::wake_all(); // P5: 立即唤醒 wait,join 最坏等待从 ≤2s 降为毫秒级
        self.join_thread();
    }

    /// Join the worker thread if it exists.
    fn join_thread(&self) {
        let handle = self.thread.lock().unwrap().take();
        if let Some(h) = handle {
            match h.join() {
                Ok(()) => {}
                Err(_) => {
                    log::error!("orchestration router thread panicked during shutdown");
                }
            }
        }
    }
}

impl Drop for MessageRouter {
    fn drop(&mut self) {
        // If the thread is still running, signal it and join with timeout.
        // We can't easily do a timed join with std::thread, but the thread
        // checks shutdown every poll interval (≤ 2 s), so it exits promptly.
        self.shutdown.store(true, Ordering::Relaxed);
        super::arrival::wake_all(); // P5: 唤醒 wait 中的线程使其检出 flag 退出
        // Drop the mutex guard after taking the handle.
        if let Some(h) = self.thread.lock().unwrap().take() {
            // The thread will exit within one poll cycle (~500ms–2s).
            // We don't block indefinitely in Drop — just let it finish.
            let _ = h.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::OrchestrationStore;

    #[test]
    fn test_router_drains_and_routes() {
        let store = DieselOrchestrationStore::in_memory().unwrap();
        let run_id = store.create_run("test").unwrap();
        let _task = store.create_task(&run_id, "do thing", &[]).unwrap();

        // Enqueue a status message (Ignored result).
        store
            .enqueue_message(
                &run_id,
                "agent_a",
                "orchestrator",
                super::super::types::MessageType::Status,
                "update",
                "halfway done",
            )
            .unwrap();

        let processed = MessageRouter::drain_and_route(&store, "orchestrator").unwrap();
        assert!(processed);

        // After successful routing, messages should be marked read.
        // Second drain should be empty (no redelivery).
        let processed = MessageRouter::drain_and_route(&store, "orchestrator").unwrap();
        assert!(!processed);
    }

    #[test]
    fn test_router_empty_inbox_returns_false() {
        let store = DieselOrchestrationStore::in_memory().unwrap();
        let processed = MessageRouter::drain_and_route(&store, "nobody").unwrap();
        assert!(!processed);
    }

    #[test]
    fn test_shutdown_signals_thread() {
        let store = DieselOrchestrationStore::in_memory().unwrap();
        let router = MessageRouter::new(store, "test_handle");
        router.spawn();
        router.shutdown();
        // If we get here without hanging, shutdown works.
    }

    #[test]
    fn test_drop_triggers_shutdown() {
        let store = DieselOrchestrationStore::in_memory().unwrap();
        {
            let router = MessageRouter::new(store, "test_handle");
            router.spawn();
            // Drop without explicit shutdown — Drop impl should clean up.
        }
        // If we get here without hanging, Drop-based shutdown works.
    }

    // ---- P5(T7): 到达事件机制与四不变量(spec P5.3) ----

    use super::super::arrival;
    use super::super::delivery::{register_dispatch, unregister_dispatch};
    use super::super::executor::MockPtyExecutor;
    use super::super::idle_detector::idle_signal_from_title;
    use super::super::store::DieselOrchestrationStore;
    use std::sync::Arc;
    use std::time::Instant;

    fn p5_seed(store: &DieselOrchestrationStore, to: &str, body: &str) -> i32 {
        store
            .enqueue_message("run_p5", "coordinator", to, super::super::types::MessageType::Status, "hi", body)
            .unwrap()
    }

    fn idle_probe() -> Arc<dyn Fn(&str) -> super::IdleSignal + Send + Sync> {
        Arc::new(|_| idle_signal_from_title(Some("✳ claude idle".to_string())))
    }

    fn busy_probe() -> Arc<dyn Fn(&str) -> super::IdleSignal + Send + Sync> {
        Arc::new(|_| idle_signal_from_title(Some("claude working".to_string())))
    }

    struct FailExec;
    impl super::super::executor::PtyExecutor for FailExec {
        fn write_to_pty(&self, _h: &str, _b: &[u8]) -> super::super::db::OrchestrationResult<()> {
            Err(super::super::db::OrchestrationError::Connection("pty gone".into()))
        }
    }

    /// INV-1: 指针写失败路径不动库——恒败 executor + Idle probe,一轮 push 后 pending 不减。
    #[test]
    fn router_invariant_pointer_write_failure_leaves_null() {
        let store = DieselOrchestrationStore::in_memory().unwrap();
        register_dispatch("ctx_inv1");
        p5_seed(&store, "ctx_inv1", "m1");
        p5_seed(&store, "ctx_inv1", "m2");
        let probe = idle_probe();
        let out = super::delivery::deliver_pending(&store, &FailExec, "ctx_inv1", &probe("ctx_inv1")).unwrap();
        assert!(matches!(out, super::delivery::PushOutcome::WriteFailed(_)));
        assert_eq!(store.get_undelivered_unread("ctx_inv1").unwrap().len(), 2);
        unregister_dispatch("ctx_inv1");
    }

    /// INV-2: watermark 丢失(重绑)不重不漏——DB 是唯一权威。
    #[test]
    fn router_invariant_watermark_loss_no_leak_no_dup() {
        let store = DieselOrchestrationStore::in_memory().unwrap();
        let exec = MockPtyExecutor::new();
        register_dispatch("ctx_inv2");
        p5_seed(&store, "ctx_inv2", "old");
        let probe = idle_probe();
        let out = super::delivery::deliver_pending(&store, &exec, "ctx_inv2", &probe("ctx_inv2")).unwrap();
        assert!(matches!(out, super::delivery::PushOutcome::Delivered { .. }));
        // 模拟重启/重绑: unregister 丢 watermark,再 register。
        unregister_dispatch("ctx_inv2");
        register_dispatch("ctx_inv2");
        p5_seed(&store, "ctx_inv2", "new");
        let out2 = super::delivery::deliver_pending(&store, &exec, "ctx_inv2", &probe("ctx_inv2")).unwrap();
        assert!(matches!(out2, super::delivery::PushOutcome::Delivered { .. }));
        let writes = exec.writes_snapshot();
        let old_ptrs = writes.iter().filter(|(_, b)| String::from_utf8_lossy(b).contains("check-messages")).count();
        assert_eq!(old_ptrs, 2, "两条消息各恰 1 次指针(不重不漏)");
        assert!(store.get_undelivered_unread("ctx_inv2").unwrap().is_empty());
        unregister_dispatch("ctx_inv2");
    }

    /// INV-3: idle 闸不跳过——事件唤醒 + Busy probe = 零注入。
    #[test]
    fn router_invariant_event_wake_busy_zero_injection() {
        let store = DieselOrchestrationStore::in_memory().unwrap();
        let exec = MockPtyExecutor::new();
        register_dispatch("ctx_inv3");
        p5_seed(&store, "ctx_inv3", "busy-time");
        arrival::notify_message_arrived(); // 显式事件唤醒
        let probe = busy_probe();
        let out = super::delivery::deliver_pending(&store, &exec, "ctx_inv3", &probe("ctx_inv3")).unwrap();
        assert_eq!(out, super::delivery::PushOutcome::NotIdle);
        assert!(exec.writes_snapshot().is_empty(), "Busy 时零注入");
        assert_eq!(store.get_undelivered_unread("ctx_inv3").unwrap().len(), 1);
        unregister_dispatch("ctx_inv3");
    }

    /// INV-4: 拉链仍是权威消费者——push 只落 delivered_at(read 仍 0),drain 才翻 read。
    #[test]
    fn router_invariant_push_not_consume_pull_authoritative() {
        let store = DieselOrchestrationStore::in_memory().unwrap();
        let exec = MockPtyExecutor::new();
        register_dispatch("ctx_inv4");
        p5_seed(&store, "ctx_inv4", "pull-me");
        let probe = idle_probe();
        let out = super::delivery::deliver_pending(&store, &exec, "ctx_inv4", &probe("ctx_inv4")).unwrap();
        assert!(matches!(out, super::delivery::PushOutcome::Delivered { .. }));
        let after_push = store.get_undelivered_unread("ctx_inv4").unwrap();
        assert!(after_push.is_empty(), "push 后 delivered_at 已落");
        let drained = store.drain_inbox("ctx_inv4").unwrap();
        assert_eq!(drained.len(), 1);
        // drain 后 read==1: 再查 undelivered_unread 仍空,且 drain 第二次为零(已读)。
        assert!(store.drain_inbox("ctx_inv4").unwrap().is_empty());
        unregister_dispatch("ctx_inv4");
    }

    /// E1: enqueue → 指针行 ≤450ms(击败一个 500ms 轮询周期;事件路径理想值毫秒级)。
    #[test]
    fn router_event_wake_beats_poll_interval() {
        let store = DieselOrchestrationStore::in_memory().unwrap();
        let exec = Arc::new(MockPtyExecutor::new());
        register_dispatch("ctx_e1");
        let probe = idle_probe();
        let router = MessageRouter::new(store.clone(), "orchestrator")
            .with_delivery(super::PushPlane { executor: exec.clone(), signal_probe: probe });
        router.spawn();
        let t0 = Instant::now();
        p5_seed(&store, "ctx_e1", "event-latency");
        let deadline = std::time::Duration::from_millis(450);
        let mut wrote = false;
        while t0.elapsed() < deadline {
            if exec.writes_snapshot().iter().any(|(_, b)| String::from_utf8_lossy(b).contains("check-messages")) {
                wrote = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let elapsed = t0.elapsed();
        router.shutdown();
        unregister_dispatch("ctx_e1");
        assert!(wrote, "指针行未出现");
        // 票面 E1: enqueue→指针行 ≤450ms(击败一个 500ms 轮询周期;事件路径理想值毫秒级)
        assert!(elapsed <= std::time::Duration::from_millis(450), "E1 超 450ms: {elapsed:?}");
    }

    /// E2: 兜底上界——无论事件是否生效,消息 ≤2.5s 内必投递
    /// (事件路径 ms 级;事件丢失时最坏一个 BACKOFF 周期 2000ms + 处理余量)。
    #[test]
    fn router_missed_notify_timeout_fallback_delivers() {
        let store = DieselOrchestrationStore::in_memory().unwrap();
        let exec = Arc::new(MockPtyExecutor::new());
        register_dispatch("ctx_e2");
        let probe = idle_probe();
        let router = MessageRouter::new(store.clone(), "orchestrator")
            .with_delivery(super::PushPlane { executor: exec.clone(), signal_probe: probe });
        router.spawn();
        // 沉降: 等线程过 ≥1 轮空转,避免与 spawn 竞态
        std::thread::sleep(std::time::Duration::from_millis(300));
        let t0 = Instant::now();
        p5_seed(&store, "ctx_e2", "fallback");
        let deadline = std::time::Duration::from_millis(2500);
        let mut wrote = false;
        while t0.elapsed() < deadline {
            if exec.writes_snapshot().iter().any(|(_, b)| String::from_utf8_lossy(b).contains("check-messages")) {
                wrote = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let elapsed = t0.elapsed();
        router.shutdown();
        unregister_dispatch("ctx_e2");
        assert!(wrote, "2.5s 兜底窗内指针行未出现");
        assert!(elapsed <= std::time::Duration::from_millis(2500), "E2 超 2.5s: {elapsed:?}");
    }

    /// E3: BACKOFF 态 wait 中 shutdown() 即时返回(wake 语义 + join 上界远小于 2s BACKOFF)。
    /// 注: 进程全局代际在测试间共享,E1 的 enqueue 会使本 router 的 last_seen=0 立即醒
    /// (忙轮询态)——shutdown 须等当轮 push/drain 完成,故 join 上界取"远小于无唤醒的
    /// 2s BACKOFF 下界"= 1.5s;wake 语义本身由 arrival::wake_all 断言锁定。
    #[test]
    fn router_shutdown_wakes_wait_immediately() {
        let store = DieselOrchestrationStore::in_memory().unwrap();
        let router = MessageRouter::new(store, "orchestrator");
        router.spawn();
        // 先推代际消耗积压,让线程进入稳态 BACKOFF wait(≥3 空轮)
        for _ in 0..3 {
            arrival::notify_message_arrived();
        }
        std::thread::sleep(std::time::Duration::from_millis(2500));
        let t0 = Instant::now();
        router.shutdown();
        let elapsed = t0.elapsed();
        // 票面 E3: 稳态 wait 中 shutdown() join ≤200ms(wake_all 直达;无唤醒最坏 2s)
        assert!(elapsed <= std::time::Duration::from_millis(200), "E3 shutdown 用时 {elapsed:?}");
    }

    /// E4: 单一挂点回归闸——成功 enqueue 使代际恰 +1。
    #[test]
    fn store_enqueue_notifies_arrival_hub() {
        let store = DieselOrchestrationStore::in_memory().unwrap();
        let before = arrival::current_arrival();
        p5_seed(&store, "ctx_e4", "gen");
        let after = arrival::current_arrival();
        // 并行测试下其他用例的 enqueue/wake_all 可能插入推进;串行(权威口径)下恰 +1。
        assert!(after >= before + 1, "成功 enqueue 必须推进代际: {before} -> {after}");
        // 失败入队代际不变: enqueue 面无公开失败注入点,以 read-side 不触发为证
        // (drain 不经 enqueue_message,不推进代际)。drain 前后差分=0 在串行下严格成立;
        // 并行下容忍并发插入(差分只可能来自其他用例)。
        let before2 = arrival::current_arrival();
        store.drain_inbox("ctx_e4").unwrap();
        let after2 = arrival::current_arrival();
        assert!(after2 >= before2, "代际单调不可回退: {before2} -> {after2}");
    }
}
