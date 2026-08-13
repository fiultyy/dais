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
///
/// `cx` is the app-wide context: the CLI dispatch path runs in-process on the
/// GPUI main thread, so terminal reads use the direct `_with_cx` flavours
/// (the channel flavours would deadlock on this thread).
pub fn run(
    _global_options: GlobalOptions,
    cx: &mut warpui::AppContext,
    command: OrchestrationCommand,
) -> anyhow::Result<()> {
    // ── Socket fast path (L1) ──
    // For commands that can be served by a running GUI, try forwarding
    // through the Unix-domain socket first.  On failure (no GUI, timeout,
    // L1 fallback stub), degrade to the existing direct-DB path — zero
    // regression risk.
    #[cfg(unix)]
    if try_socket_fast_path(&command) {
        // The command was handled (or deliberately skipped) via socket;
        // still terminate the event loop before returning.
        cx.terminate_app(warpui::platform::TerminationMode::ForceTerminate, None);
        return Ok(());
    }

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
            // Auto-promote: a fresh task with completed (or no) deps goes
            // straight to ready instead of stranding in pending.
            let promoted = store
                .promote_ready_tasks(&run_id)
                .map_err(|e| anyhow!("{e}"))?;
            if promoted.contains(&id) {
                eprintln!("promoted {id} -> ready");
            }
            println!("{id}");
        }

        OrchestrationCommand::StartWorker { task_id, command } => {
            // Look up the task to get its run_id, then create a linked
            // dispatch_context + worker_dispatch pair.
            let task = store
                .get_task(&task_id)
                .map_err(|e| anyhow!("{e}"))?
                .ok_or_else(|| anyhow!("task not found: {task_id}"))?;

            // Build start_options: include command if provided for block-driven settlement.
            let start_options = match &command {
                Some(cmd) => serde_json::json!({"command": cmd}).to_string(),
                None => "{}".to_string(),
            };

            let dispatch_id = store
                .create_dispatch(&task.run_id, &task_id, &start_options)
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
                        let spec_preview: String = t.spec.chars().take(60).collect();
                        println!("  {} [{}] {}", t.id, t.status, spec_preview);
                    }
                }
                None => {
                    let runs = store.list_runs().map_err(|e| anyhow!("{e}"))?;
                    println!("{} runs", runs.len());
                    for r in &runs {
                        let obj_preview: String = r.objective.chars().take(60).collect();
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

        OrchestrationCommand::PromoteTasks { run_id } => {
            let promoted = store
                .promote_ready_tasks(&run_id)
                .map_err(|e| anyhow!("{e}"))?;
            if promoted.is_empty() {
                println!("no tasks promoted");
            } else {
                for id in &promoted {
                    println!("promoted {id} -> ready");
                }
            }
        }

        OrchestrationCommand::MarkReady {
            dispatch_id,
            effects,
        } => {
            store
                .mark_worker_dispatch_ready(&dispatch_id, effects.as_deref())
                .map_err(|e| anyhow!("{e}"))?;
            println!("{dispatch_id} ready (dispatch + task -> dispatched)");
        }

        OrchestrationCommand::FailDispatch {
            dispatch_id,
            error,
        } => {
            let broken = store
                .fail_dispatch(&dispatch_id, &error)
                .map_err(|e| anyhow!("{e}"))?;
            if broken {
                println!("{dispatch_id} circuit_broken");
            } else {
                println!("{dispatch_id} failed");
            }
        }

        OrchestrationCommand::CreateGate {
            task_id,
            question,
            options,
        } => {
            let option_refs: Vec<&str> = options.iter().map(|s| s.as_str()).collect();
            let gate_id = store
                .create_gate(&task_id, &question, &option_refs)
                .map_err(|e| anyhow!("{e}"))?;
            println!("{gate_id}");
        }

        OrchestrationCommand::ResolveGate { gate_id, resolution } => {
            store
                .resolve_gate(&gate_id, &resolution)
                .map_err(|e| anyhow!("{e}"))?;
            println!("gate {gate_id} resolved: {resolution}");
        }

        OrchestrationCommand::ExpireGate { gate_id } => {
            store
                .expire_gate(&gate_id)
                .map_err(|e| anyhow!("{e}"))?;
            println!("gate {gate_id} expired");
        }
        OrchestrationCommand::InjectPrompt {
            dispatch_id,
            text,
            force,
        } => {
            let summary = crate::ai::orchestration::dispatch_send::inject_prompt(
                &dispatch_id, &text, force, cx,
            )?;
            println!("{summary}");
        }

        OrchestrationCommand::ReadWorker {
            dispatch_id,
            lines,
            after,
        } => {
            use ::ai::agent::orchestration::output::{ArchiveKind, TerminalTailContent};

            // We are on the GPUI main thread (in-process CLI dispatch): use
            // the direct flavour. The channel flavour would deadlock here.
            //
            // When `--after N` is given, use the cursor variant for incremental
            // reads. Backward compatible: without --after, behaviour is
            // identical to before.
            let max_bytes = 64 * 1024;

            // Always use the cursor variant — it degrades to full-tail when
            // after is None, and we need the total count for the cursor line.
            let cursor_result = crate::ai::orchestration::terminal_tail::terminal_tail_with_cursor_with_cx(
                &dispatch_id,
                lines,
                max_bytes,
                after,
                cx,
            );

            match cursor_result {
                Some((text, total, _reset)) => {
                    // Emit cursor line to stderr (machine-parseable) when
                    // --after was used.
                    if after.is_some() {
                        eprintln!("cursor: {total}");
                    }

                    println!("{text}");

                    // Persist as a terminal_tail archive.
                    let tail_struct = TerminalTailContent {
                        lines: text.lines().map(|l| l.to_string()).collect(),
                        truncated: text.len() >= max_bytes,
                        terminal_status: String::new(), // not detectable here
                        warnings: vec![],
                    };
                    let json = serde_json::to_string(&tail_struct).unwrap_or_default();
                    store
                        .store_archive(
                            &dispatch_id,
                            "",
                            ArchiveKind::TerminalTail.as_str(),
                            &json,
                        )
                        .map_err(|e| anyhow!("store_archive: {e}"))?;
                }
                None => {
                    anyhow::bail!(
                        "no terminal content for dispatch '{dispatch_id}': \
                         no terminal view is registered for this dispatch (ViewRegistry)"
                    );
                }
            }
        }

        OrchestrationCommand::ScanWaitBlocked { dispatch_id } => {
            let reason =
                crate::ai::orchestration::interactive::scan_wait_blocked(&dispatch_id, cx)?;
            match reason {
                Some(r) => println!("{r}"),
                None => println!("no wait-blocked signal"),
            }
        }


        OrchestrationCommand::Answer {
            dispatch_id,
            text,
            enter,
            interrupt,
        } => {
            crate::ai::orchestration::interactive::answer(
                &dispatch_id,
                text.as_deref(),
                enter,
                interrupt,
            )?;
            println!("action sent to {dispatch_id}");
        }

        OrchestrationCommand::Assign { dispatch_id } => {
            let summary =
                crate::ai::orchestration::dispatch_assign::assign_to_active_pane(&dispatch_id, cx)?;
            println!("{summary}");
        }

        OrchestrationCommand::CheckMessages {
            handle,
            wait,
            timeout_ms,
            message_type,
        } => {
            // The pull path — authoritative consumer of the mailbox. The
            // push pointer (delivery.rs) only accelerates this.
            let matches_filter = |m: &::ai::agent::orchestration::db::Message| {
                message_type.is_empty() || message_type.contains(&m.message_type)
            };

            let mut timed_out = false;
            if wait {
                // Orca waitForMessage semantics (31959): register a claim so
                // the push plane skips this mailbox's matching types (mutual
                // exclusion), poll until match/timeout, always finish with a
                // final same-filter re-read (timeout is not an error).
                let waiter_id = format!("wtr_{}", uuid::Uuid::new_v4().simple());
                let types_json = serde_json::to_string(&message_type).unwrap_or_else(|_| "[]".into());
                let deadline = std::time::Instant::now()
                    + std::time::Duration::from_millis(timeout_ms.max(1));
                let claim_ttl: i64 = 15; // refreshed every poll tick

                let mut matched = false;
                while !matched {
                    store
                        .upsert_waiter(&waiter_id, &handle, &types_json, claim_ttl)
                        .map_err(|e| anyhow!("{e}"))?;
                    let unread = store.drain_inbox(&handle).map_err(|e| anyhow!("{e}"))?;
                    if unread.iter().any(|m| matches_filter(m)) {
                        matched = true;
                    } else if std::time::Instant::now() >= deadline {
                        timed_out = true;
                        break;
                    } else {
                        std::thread::sleep(std::time::Duration::from_millis(500));
                    }
                }
                // Always drop the claim before pulling (all Orca waiter
                // resolutions route through removeMessageWaiter).
                store
                    .delete_waiter(&waiter_id)
                    .map_err(|e| anyhow!("{e}"))?;
                if timed_out {
                    eprintln!("timed_out");
                }
            }

            // Final pull with the same filter (covers the timed-out case —
            // Orca re-reads on timeout so a waiter-filtered row is still
            // returned, 31926-31928).
            let messages = store
                .drain_inbox(&handle)
                .map_err(|e| anyhow!("{e}"))?
                .into_iter()
                .filter(|m| matches_filter(m))
                .collect::<Vec<_>>();
            if messages.is_empty() {
                println!("no unread messages for {handle}");
            } else {
                let mut sequences = Vec::with_capacity(messages.len());
                for m in &messages {
                    println!(
                        "--- seq {} from {} [{}] {} ---\n{}\n",
                        m.sequence, m.from_handle, m.message_type, m.subject, m.body
                    );
                    sequences.push(m.sequence);
                }
                store
                    .mark_messages_read(&sequences)
                    .map_err(|e| anyhow!("{e}"))?;
            }
        }
    }

    // CLI dispatch lives inside the app event loop; without an explicit
    // terminate the loop keeps waiting for windows (headless hang).
    cx.terminate_app(warpui::platform::TerminationMode::ForceTerminate, None);

    Ok(())
}

/// Attempt to forward eligible orchestration commands to a running GUI via the
/// runtime RPC socket.
#[cfg(unix)]
fn try_socket_fast_path(command: &OrchestrationCommand) -> bool {
    use crate::ai::orchestration::runtime_rpc;

    let (cmd_name, args) = match command {
        OrchestrationCommand::CheckStatus { run_id } => {
            ("check-status", serde_json::json!({ "run_id": run_id }))
        }
        OrchestrationCommand::SendMessage {
            run_id,
            from,
            to,
            message_type,
            subject,
            body,
        } => (
            "send-message",
            serde_json::json!({
                "run_id": run_id,
                "from": from,
                "to": to,
                "message_type": message_type,
                "subject": subject,
                "body": body,
            }),
        ),
        OrchestrationCommand::CheckMessages {
            handle,
            wait,
            timeout_ms,
            message_type,
        } => (
            "check-messages",
            serde_json::json!({
                "handle": handle,
                "wait": wait,
                "timeout_ms": timeout_ms,
                "message_type": message_type,
            }),
        ),
        _ => return false,
    };

    match runtime_rpc::try_socket_forward(cmd_name, &args) {
        Ok(Some(response)) => {
            // Socket returned a real result — print it and we're done.
            println!("{response}");
            true
        }
        Ok(None) => {
            // No GUI / fallback stub / timeout — degrade to direct DB.
            false
        }
        Err(e) => {
            // RPC error — log but degrade to direct DB.
            log::warn!("runtime_rpc socket error, degrading to direct DB: {e:#}");
            false
        }
    }
}
