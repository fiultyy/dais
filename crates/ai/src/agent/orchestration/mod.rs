//! Local orchestration plane — ported from Orca's TypeScript orchestration module.
//!
//! Adaptation layers (Orca → zap):
//! 1. **DB**: TS node:sqlite (DatabaseSync) → Diesel + SQLite migrations in `crates/persistence/`.
//! 2. **IPC**: dual-socket NDJSON → tokio mpsc channels (single process).
//! 3. **Agent comms**: direct DB insertion (SendMessageToAgent proto was severed).
//! 4. **PTY injection**: `PtyExecutor` trait → `AIAgentActionType::WriteToLongRunningShellCommand`.
//! 5. **State detection**: OSC 133 prompt markers → `DcsHookEvent` (4 variants).
//!
//! Wiring status: P1 (store + CLI) implemented. P2 (message loop + PTY bridge)
//! and P3 (shell event detection) are pending — their traits/impls compile but
//! have no production consumers yet.

pub mod connection;

pub mod db;
pub mod executor;
pub mod groups;
pub mod messaging;
pub mod delivery;
pub mod idle_detector;
pub mod output;
pub mod prompt_injection;
pub mod reconciliation;
pub mod store;
pub mod types;
pub mod router;
pub mod worker;

pub use db::{
    DecisionGate, Delivery, DispatchContext, Message, OrchestrationError, OrchestrationResult,
    Run, Task, WorkerDispatch,
};
pub use executor::{DcsHookEvent, MockPtyExecutor, PtyExecutor, WorkerStatusDetector};
pub use output::{ArchiveKind, TerminalTailContent, TranscriptPinContent, WorkerTerminalArchive};
pub use types::*;

/// Abstraction over the orchestration persistence layer.
///
/// `DieselOrchestrationStore` (SQLite-backed) is the production implementation.
/// All methods are synchronous — async callers wrap in `spawn_blocking` via
/// the `*_async` convenience methods on `DieselOrchestrationStore`.
pub trait OrchestrationStore: Send + Sync {
    // ── Run lifecycle ──────────────────────────────────────────────────

    /// Create a new run. Returns the generated run id.
    fn create_run(&self, objective: &str) -> OrchestrationResult<String>;

    // ── Task DAG ───────────────────────────────────────────────────────

    /// Create a task. `deps` are parent task ids that must complete first.
    fn create_task(&self, run_id: &str, spec: &str, deps: &[&str]) -> OrchestrationResult<String>;

    /// Promote `pending` tasks whose deps are all `completed` → `ready`.
    /// Returns the promoted task ids.
    fn promote_ready_tasks(&self, run_id: &str) -> OrchestrationResult<Vec<String>>;

    // ── Dispatch ───────────────────────────────────────────────────────

    /// Create a dispatch context for a task. Returns the context id.
    fn create_dispatch_context(&self, run_id: &str, task_id: &str) -> OrchestrationResult<String>;

    /// Record a dispatch failure, increment the circuit-breaker counter.
    /// Returns `true` if the circuit is now broken.
    fn fail_dispatch(&self, id: &str, error: &str) -> OrchestrationResult<bool>;

    // ── Worker dispatch ────────────────────────────────────────────────

    /// Create a worker dispatch in `starting` state. Returns the dispatch id.
    fn create_worker_dispatch(&self) -> OrchestrationResult<String>;

    /// Transition a worker dispatch to a new state.
    fn transition_worker(
        &self,
        dispatch_id: &str,
        next: WorkerDispatchState,
    ) -> OrchestrationResult<()>;

    // ── Messaging ──────────────────────────────────────────────────────

    /// Enqueue a message. Returns the autoincrement sequence number.
    fn enqueue_message(
        &self,
        run_id: &str,
        from_handle: &str,
        to_handle: &str,
        message_type: MessageType,
        subject: &str,
        body: &str,
    ) -> OrchestrationResult<i32>;

    /// Drain all unread messages for a handle.
    fn drain_inbox(&self, handle: &str) -> OrchestrationResult<Vec<Message>>;

    /// Mark messages as read by their sequence numbers.
    /// Call after successful reconciliation to prevent redelivery.
    fn mark_messages_read(&self, sequences: &[i32]) -> OrchestrationResult<()>;

    /// Undelivered unread messages for a mailbox (push-delivery source).
    /// `to_handle = ? AND read = 0 AND delivered_at IS NULL AND
    ///  delivery_contract = 'current_delivery' ORDER BY sequence`.
    fn get_undelivered_unread(&self, handle: &str) -> OrchestrationResult<Vec<Message>>;

    /// Mark messages delivered (pointer written to the target PTY).
    fn mark_delivered(&self, sequences: &[i32]) -> OrchestrationResult<()>;

    // ── Waiters (check --wait claims; push/pull mutual exclusion) ─────

    /// Upsert a waiter claim. `type_filter` is a JSON array of message types
    /// (`[]` claims all). `ttl_secs` refreshes `expires_at`; stale claims
    /// expire on their own (dead waiting process).
    fn upsert_waiter(
        &self,
        id: &str,
        handle: &str,
        type_filter: &str,
        ttl_secs: i64,
    ) -> OrchestrationResult<()>;

    /// Remove a waiter claim (waiter resolved / timed out / cancelled).
    fn delete_waiter(&self, id: &str) -> OrchestrationResult<()>;

    /// Whether a live (non-expired) waiter claims `message_type` for `handle`.
    /// Mirrors Orca `messageTypeHasLiveWaiter` (orca-runtime.ts:32636-32643):
    /// the push plane must skip claimed messages so a blocking check and a
    /// pointer push cannot double-consume the same row.
    fn has_live_waiter(&self, handle: &str, message_type: &str) -> OrchestrationResult<bool>;
}
