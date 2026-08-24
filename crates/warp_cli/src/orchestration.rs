//! Orchestration CLI command types.
//!
//! These types live in `warp_cli` (not `ai::agent::orchestration::cli`) because
//! `warp_cli` does not depend on `ai`. The dispatch handler in `agent_sdk`
//! converts these to concrete store calls using `ai::agent::orchestration`.

use clap::Subcommand;

/// Orchestration subcommands.
#[derive(Debug, Clone, Subcommand, serde::Serialize, serde::Deserialize)]
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
        /// Optional command that marks completion of this worker.
        /// When a shell block finishes with a matching command, the dispatch
        /// is settled automatically (block-driven settlement).
        #[arg(long)]
        command: Option<String>,
        /// Session mailbox handle (`session_<sid>`, as printed by
        /// new-terminal) of the terminal this worker should own. Binds the
        /// dispatch to that pane and persists the assignment (D-04); a
        /// binding failure is an error (explicit target). Without it the
        /// dispatch binds to the active pane when one exists (best effort).
        #[arg(long)]
        #[serde(default)]
        session: Option<String>,
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
        /// Only return lines after this cursor position (cumulative line count
        /// from a prior read). Enables incremental polling without re-reading
        /// the full scrollback.
        #[arg(long)]
        after: Option<usize>,
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

    /// Add a project (absolute path) to the project list. Idempotent for an
    /// existing path (refreshes last_opened_ts). A live GUI refreshes its
    /// project rail immediately via the project event.
    ProjectAdd {
        /// Absolute path of the project root.
        path: String,
    },

    /// Remove a project from the project list. Refuses when tabs/terminals
    /// still reference the project (reports them). `--force` closes those
    /// tabs outright (interrupt harness → PTY shutdown → tab close → session
    /// mailbox retire; late-bootstrapping tabs swept too) before removal.
    ProjectRemove {
        /// Absolute path of the project root.
        path: String,
        /// Detach referencing tabs (reset to no-project) and remove anyway.
        #[arg(long)]
        force: bool,
    },

    /// List all registered projects. Machine-parseable: one line per project
    /// `path<TAB>added_ts<TAB>last_opened_ts`.
    ProjectList,

    /// Create a git worktree for a project at `<project>/../<repo>-<name>`
    /// (new branch `<name>` from HEAD) and register it as a project.
    /// Prints the worktree path.
    ///
    /// `--agent`/`--prompt` (Orca-parity one-shot spawn): after the worktree
    /// is created, opens a terminal tab in it, waits for its session, then
    /// types the agent launch command and pastes the prompt. Requires the
    /// GUI runtime (a terminal tab is a GUI resource).
    WorktreeCreate {
        /// Existing project path (main checkout) to create the worktree from.
        project_path: String,
        /// Worktree name: suffix for the sibling directory and the new branch.
        name: String,
        /// Agent launch command typed into the fresh terminal (e.g. `omp`,
        /// `pi`, `cc` or any shell command). Submitted as its own line.
        #[arg(long)]
        #[serde(default)]
        agent: Option<String>,
        /// Prompt text handed to the agent after it starts (bracketed paste
        /// + submit, after a short settle delay). Implies `--agent`'s
        /// terminal when given alone (pastes into a bare shell).
        #[arg(long)]
        #[serde(default)]
        prompt: Option<String>,
    },

    /// Garbage-collect finished orchestration runs older than the cutoff
    /// (D-05: the runs registry previously grew without bound). A run is
    /// finished when none of its tasks is pending/ready/dispatched/blocked.
    /// Prints each deleted run id.
    GcRuns {
        /// Age cutoff in days.
        #[arg(long, default_value_t = 7)]
        days: i64,
        /// Report what would be deleted without deleting.
        #[arg(long)]
        dry_run: bool,
    },

    /// List git worktrees (porcelain `git worktree list` wrapped). Without a
    /// path, lists worktrees of every registered project that is a git repo.
    WorktreeList {
        /// Optional project path to scope the listing to.
        project_path: Option<String>,
    },

    /// Remove a git worktree. Refuses when terminals reference it (reports
    /// them). `--force` closes those tabs outright (same reclaim semantics
    /// as project-remove --force), then `git worktree remove --force`.
    WorktreeRemove {
        /// Absolute path of the worktree to remove.
        path: String,
        /// Detach referencing tabs and force-remove even if dirty.
        #[arg(long)]
        force: bool,
    },

    /// Open a new terminal tab in a project's active window (GUI action —
    /// runs on the GUI main thread via the runtime RPC; errors without a
    /// running GUI). Prints the new terminal's session mailbox handle
    /// (`session_<sid>`).
    NewTerminal {
        /// Project path whose window/tab the terminal opens in.
        project_path: String,
        /// Working directory override (default: the project path).
        #[arg(long)]
        cwd: Option<String>,
    },

    /// Close the terminal tab owning a session mailbox (`session_<sid>`).
    /// Reclaims the pane; the session mailbox retires naturally on shell
    /// exit (shell_event_bridge). `--force` interrupts a lingering harness
    /// (Ctrl-C) and shuts the PTY down before closing the tab.
    CloseTerminal {
        /// Session mailbox handle of the terminal to close.
        handle: String,
        /// Interrupt + PTY shutdown before close (harness still running).
        #[arg(long)]
        force: bool,
    },
}
