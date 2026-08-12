//! CLI command surface — defines the orchestration CLI command structure.
//!
//! Future integration: these commands will be wired into `warp_cli` to provide
//! a user-facing orchestration interface. The enum structure is complete; the
//! `run()` method dispatches to `DieselOrchestrationStore` and prints results.
//!
//! Design: mirrors Orca's orchestration API surface (createRun, createTask,
//! startWorker, sendMessage, checkStatus) using clap derive for ergonomic
//! command-line parsing.

use clap::{Parser, Subcommand};

use super::db::OrchestrationResult;
use super::store::DieselOrchestrationStore;
use super::types::{MessageType, WorkerDispatchState};
use super::OrchestrationStore;

// ---------------------------------------------------------------------------
// CLI structure
// ---------------------------------------------------------------------------

/// Top-level orchestration CLI parser.
#[derive(Parser, Debug)]
#[command(name = "orchestration", about = "Local orchestration plane")]
pub struct OrchestrationCli {
    #[command(subcommand)]
    pub command: CliCommand,
}

/// Orchestration CLI subcommands.
#[derive(Subcommand, Debug)]
pub enum CliCommand {
    /// Create a new orchestration run.
    CreateRun {
        /// Human-readable objective for the run.
        #[arg(long)]
        objective: String,
    },

    /// Create a task within a run.
    CreateTask {
        /// Run id to create the task in.
        #[arg(long)]
        run_id: String,

        /// Task specification text.
        #[arg(long)]
        spec: String,

        /// Parent task ids this task depends on (all must complete first).
        #[arg(long, value_delimiter = ',')]
        deps: Vec<String>,
    },

    /// Start a worker dispatch for a task.
    StartWorker {
        /// Run id.
        #[arg(long)]
        run_id: String,

        /// Task id to dispatch.
        #[arg(long)]
        task_id: String,

        /// JSON start options (worktree, terminal title, etc.).
        #[arg(long, default_value = "{}")]
        start_options: String,
    },

    /// Send a message between agents.
    SendMessage {
        /// Run id.
        #[arg(long)]
        run_id: String,

        /// Sender agent handle.
        #[arg(long)]
        from: String,

        /// Recipient handle or group address (@all, @idle, @worktree:<id>, @agentName).
        #[arg(long)]
        to: String,

        /// Message type.
        #[arg(long)]
        message_type: String,

        /// Message subject.
        #[arg(long)]
        subject: String,

        /// Message body.
        #[arg(long, default_value = "")]
        body: String,
    },

    /// Check orchestration status (runs, tasks, dispatches).
    CheckStatus {
        /// Optional run id to filter.
        #[arg(long)]
        run_id: Option<String>,
    },

    /// Transition a worker dispatch to a new state.
    TransitionWorker {
        /// Dispatch id.
        #[arg(long)]
        dispatch_id: String,

        /// Target state.
        #[arg(long)]
        state: String,
    },
}

// ---------------------------------------------------------------------------
// Execution
// ---------------------------------------------------------------------------

/// Execute a CLI command against the given store.
pub fn run_command(store: &DieselOrchestrationStore, command: &CliCommand) -> OrchestrationResult<String> {
    match command {
        CliCommand::CreateRun { objective } => {
            let id = store.create_run(objective)?;
            Ok(format!("Created run: {}", id))
        }

        CliCommand::CreateTask { run_id, spec, deps } => {
            let deps: Vec<&str> = deps.iter().map(|s| s.as_str()).collect();
            let id = store.create_task(run_id, spec, &deps)?;
            Ok(format!("Created task: {}", id))
        }

        CliCommand::StartWorker { run_id, task_id, start_options } => {
            let id = store.create_dispatch(run_id, task_id, start_options)?;
            Ok(format!("Started worker dispatch: {}", id))
        }

        CliCommand::SendMessage {
            run_id,
            from,
            to,
            message_type,
            subject,
            body,
        } => {
            let mt: MessageType = message_type
                .parse()
                .map_err(|_| super::db::OrchestrationError::InvalidEnum {
                    context: "MessageType",
                    value: message_type.clone(),
                })?;
            let seq = store.enqueue_message(run_id, from, to, mt, subject, body)?;
            Ok(format!("Sent message (sequence: {})", seq))
        }

        CliCommand::CheckStatus { run_id } => {
            let mut output = String::new();

            let tasks = store.list_tasks(run_id.as_deref(), None)?;
            if tasks.is_empty() {
                output.push_str("No tasks found.\n");
            } else {
                for t in &tasks {
                    output.push_str(&format!(
                        "  task {} [{}] {}\n",
                        t.id, t.status, t.spec
                    ));
                }
            }

            Ok(output)
        }

        CliCommand::TransitionWorker { dispatch_id, state } => {
            let target: WorkerDispatchState = state
                .parse()
                .map_err(|_| super::db::OrchestrationError::InvalidEnum {
                    context: "WorkerDispatchState",
                    value: state.clone(),
                })?;
            store.transition_worker(dispatch_id, target)?;
            Ok(format!("Transitioned {} to {}", dispatch_id, state))
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cli_create_run() {
        let store = DieselOrchestrationStore::in_memory().unwrap();
        let cmd = CliCommand::CreateRun {
            objective: "test objective".into(),
        };
        let result = run_command(&store, &cmd).unwrap();
        assert!(result.starts_with("Created run:"));
    }

    #[test]
    fn test_cli_create_task_and_check_status() {
        let store = DieselOrchestrationStore::in_memory().unwrap();

        let run_id = store.create_run("e2e cli").unwrap();
        let cmd = CliCommand::CreateTask {
            run_id: run_id.clone(),
            spec: "do work".into(),
            deps: vec![],
        };
        let result = run_command(&store, &cmd).unwrap();
        assert!(result.starts_with("Created task:"));

        let cmd = CliCommand::CheckStatus {
            run_id: Some(run_id),
        };
        let result = run_command(&store, &cmd).unwrap();
        assert!(result.contains("do work"));
    }

    #[test]
    fn test_cli_start_worker() {
        let store = DieselOrchestrationStore::in_memory().unwrap();
        let run_id = store.create_run("worker test").unwrap();
        let task_id = store.create_task(&run_id, "work", &[]).unwrap();

        let cmd = CliCommand::StartWorker {
            run_id,
            task_id,
            start_options: "{}".into(),
        };
        let result = run_command(&store, &cmd).unwrap();
        assert!(result.starts_with("Started worker dispatch:"));
    }

    #[test]
    fn test_cli_send_message() {
        let store = DieselOrchestrationStore::in_memory().unwrap();
        let run_id = store.create_run("msg test").unwrap();

        let cmd = CliCommand::SendMessage {
            run_id,
            from: "coordinator".into(),
            to: "worker_1".into(),
            message_type: "dispatch".into(),
            subject: "go".into(),
            body: "do the thing".into(),
        };
        let result = run_command(&store, &cmd).unwrap();
        assert!(result.contains("Sent message"));
    }

    #[test]
    fn test_cli_transition_worker() {
        let store = DieselOrchestrationStore::in_memory().unwrap();
        let dispatch_id = store.create_worker_dispatch().unwrap();

        let cmd = CliCommand::TransitionWorker {
            dispatch_id,
            state: "ready".into(),
        };
        let result = run_command(&store, &cmd).unwrap();
        assert!(result.contains("Transitioned"));
    }
}
