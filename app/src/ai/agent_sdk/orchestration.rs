//! Dispatch handler for orchestration CLI commands.
//!
//! Opens the process-wide orchestration store via `connection::store()` and
//! delegates to the appropriate store method.

use std::str::FromStr;
use ::ai::agent::orchestration;
use ::ai::agent::orchestration::types::{MessageType, WorkerDispatchState};
use ::ai::agent::orchestration::OrchestrationStore;
use anyhow::anyhow;
use warp_cli::orchestration::OrchestrationCommand;
use warp_cli::GlobalOptions;

/// Run an orchestration CLI command.
pub fn run(_global_options: GlobalOptions, command: OrchestrationCommand) -> anyhow::Result<()> {
    let store = orchestration::connection::store();

    match command {
        OrchestrationCommand::CreateRun { objective } => {
            let id = store.create_run(&objective).map_err(|e| anyhow!("{e}"))?;
            println!("{id}");
        }

        OrchestrationCommand::CreateTask {
            run_id,
            spec,
            deps,
        } => {
            let dep_refs: Vec<&str> = deps.iter().map(|s| s.as_str()).collect();
            let id = store
                .create_task(&run_id, &spec, &dep_refs)
                .map_err(|e| anyhow!("{e}"))?;
            println!("{id}");
        }

        OrchestrationCommand::StartWorker { task_id: _ } => {
            let dispatch_id = store
                .create_worker_dispatch()
                .map_err(|e| anyhow!("{e}"))?;
            println!("{dispatch_id}");
        }

        OrchestrationCommand::SendMessage {
            run_id,
            from,
            to,
            message_type,
            subject,
            body,
        } => {
            let mt = MessageType::from_str(&message_type)
                .map_err(|e| anyhow!("invalid message_type '{message_type}': {e}"))?;
            let seq = store
                .enqueue_message(&run_id, &from, &to, mt, &subject, &body)
                .map_err(|e| anyhow!("{e}"))?;
            println!("enqueued seq={seq}");
        }

        OrchestrationCommand::CheckStatus { run_id } => {
            match run_id {
                Some(rid) => {
                    let tasks = store
                        .list_tasks(Some(&rid), None)
                        .map_err(|e| anyhow!("{e}"))?;
                    println!("Run {rid}: {} tasks", tasks.len());
                    for t in &tasks {
                        let spec_preview = &t.spec[..t.spec.len().min(60)];
                        println!("  {} [{}] {}", t.id, t.status, spec_preview);
                    }
                }
                None => {
                    let runs = store.list_runs().map_err(|e| anyhow!("{e}"))?;
                    println!("{} runs", runs.len());
                    for r in &runs {
                        let obj_preview = &r.objective[..r.objective.len().min(60)];
                        println!("  {} {}", r.id, obj_preview);
                    }
                }
            }
        }

        OrchestrationCommand::TransitionWorker {
            dispatch_id,
            state,
        } => {
            let ws = WorkerDispatchState::from_str(&state)
                .map_err(|e| anyhow!("invalid state '{state}': {e}"))?;
            store
                .transition_worker(&dispatch_id, ws)
                .map_err(|e| anyhow!("{e}"))?;
            println!("transitioned {dispatch_id} -> {ws:?}");
        }
    }

    Ok(())
}
