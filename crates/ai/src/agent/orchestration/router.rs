//! Background message router — polls the messages table and dispatches to
//! `route_message` for lifecycle transitions.
//!
//! Runs in a dedicated `std::thread` (the store is synchronous — no async
//! benefit). Polls `drain_inbox` every 500ms, backing off to 2s when empty.
//! The thread is a daemon: it runs for the process lifetime and exits when
//! the `shutdown` flag is set.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;


use super::db::OrchestrationResult;
use super::messaging;
use super::store::DieselOrchestrationStore;
use super::OrchestrationStore;

const POLL_INTERVAL: Duration = Duration::from_millis(500);
const BACKOFF_INTERVAL: Duration = Duration::from_millis(2000);
const EMPTY_BACKOFF_THRESHOLD: u32 = 3;

/// Background message router. Owns its own DB connection (separate from the
/// CLI store singleton) so message routing doesn't block CLI operations.
pub struct MessageRouter {
    store: DieselOrchestrationStore,
    handle: String,
    shutdown: Arc<AtomicBool>,
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
        }
    }

    /// Spawn the router as a daemon thread. Returns a `JoinHandle` for
    /// graceful shutdown; in practice the handle is usually dropped (detached).
    pub fn spawn(self) -> JoinHandle<()> {
        let shutdown = self.shutdown.clone();
        let store = self.store;
        let handle = self.handle;

        thread::Builder::new()
            .name("orch-msg-router".into())
            .spawn(move || {
                let mut empty_count: u32 = 0;
                while !shutdown.load(Ordering::Relaxed) {
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
            .expect("spawn orchestration router thread")
    }

    /// Drain the inbox for `handle` and route each message.
    /// Returns `true` if any messages were processed.
    fn drain_and_route(
        store: &DieselOrchestrationStore,
        handle: &str,
    ) -> OrchestrationResult<bool> {
        let messages = store.drain_inbox(handle)?;
        if messages.is_empty() {
            return Ok(false);
        }

        for msg in &messages {
            match messaging::route_message(store, msg) {
                Ok(result) => {
                    log::debug!(
                        "orchestration: routed msg seq={} -> {:?}",
                        msg.sequence,
                        result
                    );
                }
                Err(e) => {
                    log::warn!(
                        "orchestration: route_message failed for seq={}: {e}",
                        msg.sequence
                    );
                    // Continue processing remaining messages.
                }
            }
        }
        Ok(true)
    }

    /// Signal the router to stop. The thread will exit after the next poll.
    pub fn shutdown(&self) {
        self.shutdown.store(true, Ordering::Relaxed);
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

        // Second drain should be empty.
        let processed = MessageRouter::drain_and_route(&store, "orchestrator").unwrap();
        assert!(!processed);
    }

    #[test]
    fn test_router_empty_inbox_returns_false() {
        let store = DieselOrchestrationStore::in_memory().unwrap();
        let processed = MessageRouter::drain_and_route(&store, "nobody").unwrap();
        assert!(!processed);
    }
}
