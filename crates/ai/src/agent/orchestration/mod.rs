//! Local orchestration plane — ported from Orca's TypeScript orchestration module.
//!
//! Adaptation layers (Orca → zap):
//! 1. **DB**: TS node:sqlite (DatabaseSync) → Diesel + SQLite migrations in `crates/persistence/`.
//! 2. **IPC**: dual-socket NDJSON → tokio mpsc channels (single process).
//! 3. **Agent comms**: HTTP loopback → proto streaming `SendMessageToAgent`.
//! 4. **PTY injection**: paste bytes → `pty_controller::write_bytes()`.
//! 5. **State detection**: window-title / OSC → DCS hook (13 variants).
//!
//! What is NOT ported: Coordinator autonomous loop (Orca dead code, fenced by
//! `RETIRED_ORCHESTRATION_METHODS`), agent-hook HTTP server, daemon fork,
//! node-pty. The caller-driven model replaces all of these.

#![allow(dead_code)]

pub mod db;
pub mod cli;
pub mod executor;
pub mod groups;
pub mod messaging;
pub mod output;
pub mod reconciliation;
pub mod store;
pub mod types;
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

    /// Drain all unread messages for a handle, marking them read.
    fn drain_inbox(&self, handle: &str) -> OrchestrationResult<Vec<Message>>;
}
