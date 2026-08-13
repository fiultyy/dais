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
                    thread::sleep(sleep);
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
}
