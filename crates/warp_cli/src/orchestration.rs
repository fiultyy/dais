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

    /// Promote pending tasks whose deps are all completed to ready.
    PromoteTasks {
        /// Run id.
        run_id: String,
    },

    /// Mark a starting worker dispatch ready (worker → ready, dispatch →
    /// dispatched, task → dispatched).
    MarkReady {
        dispatch_id: String,
        /// Optional JSON effects recorded on the worker dispatch.
        #[arg(long)]
        effects: Option<String>,
    },

    /// Record a dispatch failure (increments circuit-breaker counter).
    FailDispatch {
        dispatch_id: String,
        /// Failure description.
        error: String,
    },

    /// Create a decision gate blocking a task until resolved.
    CreateGate {
        /// Task id to block.
        task_id: String,
        /// Question presented to the decider.
        #[arg(long)]
        question: String,
        /// Selectable options.
        #[arg(long = "option")]
        options: Vec<String>,
    },

    /// Resolve a pending decision gate.
    ResolveGate {
        gate_id: String,
        /// Chosen resolution.
        resolution: String,
    },

    /// Expire a pending decision gate (fails its blocked task).
    ExpireGate { gate_id: String },

    /// Inject a prompt into a dispatched worker's terminal (bracketed paste
    /// + delayed submit). Checks terminal idle status first.
    InjectPrompt {
        /// Dispatch id whose terminal receives the prompt.
        dispatch_id: String,
        /// Prompt text to inject.
        text: String,
        /// Skip the idle-status check (inject even if agent is working).
        #[arg(long)]
        force: bool,
    },

    /// Read a dispatched worker's terminal output tail.
    ReadWorker {
        /// Dispatch id to read.
        dispatch_id: String,
        /// Max lines to return.
        #[arg(long, default_value_t = 40)]
        lines: usize,
    },

    /// Scan a dispatched worker's terminal tail for wait-blocked signals.
    ScanWaitBlocked {
        /// Dispatch id to scan.
        dispatch_id: String,
    },

    /// Answer an interactive prompt in a dispatched worker's terminal.
    Answer {
        /// Dispatch id whose terminal is blocked.
        dispatch_id: String,
        /// Text to type (e.g. "y"). Omit for enter/interrupt-only actions.
        #[arg(long)]
        text: Option<String>,
        /// Press Enter after the text (500ms delay).
        #[arg(long)]
        enter: bool,
        /// Send Ctrl-C instead of Enter.
        #[arg(long)]
        interrupt: bool,
    },

    /// Assign a dispatch to the active terminal pane. Registers the pane's
    /// view + session in the orchestration registries so prompt injection,
    /// output reading, and shell-event bridging target this terminal.
    Assign {
        /// Dispatch id to assign to the active pane.
        dispatch_id: String,
    },

    /// Pull unread messages for a mailbox (the agent-side check command the
    /// pushed pointer tells the agent to run).
    CheckMessages {
        /// Mailbox handle (dispatch id, session_N, or "orchestrator").
        handle: String,
        /// Block until a matching message arrives (default timeout 2 min).
        #[arg(long)]
        wait: bool,
        /// Wait timeout in milliseconds (with --wait).
        #[arg(long, default_value_t = 120_000)]
        timeout_ms: u64,
        /// Only wait for / pull this message type (repeatable).
        #[arg(long = "type")]
        message_type: Vec<String>,
    },
}
