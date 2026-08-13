//! Orchestration CLI command types.
//!
//! These types live in `warp_cli` (not `ai::agent::orchestration::cli`) because
//! `warp_cli` does not depend on `ai`. The dispatch handler in `agent_sdk`
//! converts these to concrete store calls using `ai::agent::orchestration`.

use clap::Subcommand;

/// Orchestration subcommands.
#[derive(Debug, Clone, Subcommand)]
pub enum OrchestrationCommand {
    /// Create a new orchestration run.
    CreateRun {
        /// Objective / goal description.
        #[arg(long)]
        objective: String,
    },

    /// Create a task within a run.
    CreateTask {
        /// Parent run id.
        run_id: String,
        /// Task specification (what to do).
        spec: String,
        /// Parent task ids that must complete first.
        #[arg(long = "dep")]
        deps: Vec<String>,
    },

    /// Start a worker dispatch for a task.
    StartWorker {
        /// Task id to dispatch.
        task_id: String,
    },

    /// Send a message between agents.
    SendMessage {
        run_id: String,
        /// Sender handle.
        from: String,
        /// Recipient handle.
        to: String,
        /// Message type (broadcast, direct, group, etc.).
        #[arg(long)]
        message_type: String,
        #[arg(long)]
        subject: String,
        #[arg(long)]
        body: String,
    },

    /// Check orchestration status.
    CheckStatus {
        /// Filter by run id. If omitted, lists all runs.
        #[arg(long)]
        run_id: Option<String>,
    },

    /// Transition a worker dispatch to a new state.
    TransitionWorker {
        dispatch_id: String,
        /// Target state (starting, ready, running, succeeded, failed, etc.).
        state: String,
    },
}
