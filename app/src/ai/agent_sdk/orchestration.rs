//! Dispatch handler for orchestration CLI commands.
//!
//! Opens the process-wide orchestration store via `connection::store()` and
//! delegates to the appropriate store method.

use std::str::FromStr;
use ::ai::agent::orchestration;
use ::ai::agent::orchestration::OrchestrationStore;
use ::ai::agent::orchestration::types::{MessageType, WorkerDispatchState};
use ::ai::agent::orchestration::store::DieselOrchestrationStore;
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
    // ── Socket fast path ──
    // When a GUI runtime owns the orchestration plane, forward the command
    // through its Unix-domain RPC socket. A forwarded *execution* error
    // (or a refused invocation) propagates as Err → stderr + exit 1 (D-03:
    // previously both printed to stdout with exit 0). Transport-level
    // failures (no runtime, timeout, serve-mode stub) degrade to the
    // existing direct-DB path — zero regression risk.
    #[cfg(unix)]
    if try_socket_fast_path(&command)? {
        // new-terminal: the GUI created the tab (output printed above);
        // resolve session_<sid> by polling the L1 latest-session probe —
        // bootstrap is async, and the GUI main thread must not wait for it.
        #[cfg(unix)]
        // v2-fix-13 票1: tab 已建成功(GUI 输出在上方), 此处等 bootstrap
        // 注册 session 邮箱。权威源 = shell_event_bridge 注册点的打点
        // (L1 latest-session), 不猜 DB/log。只开 tab 返回 sid; 启动
        // harness 交给调用方注入(票2: 别名本就武装在每个新 shell)。
        if let OrchestrationCommand::NewTerminal { ref project_path, .. } = command {
            // CLI 侧预检: 路径不存在直接报错, 不进 12s bootstrap 等待
            // (GUI 侧错误响应也会打印, 但 poll 文案会误导)。
            let p = std::path::Path::new(project_path);
            if !p.is_dir() {
                anyhow::bail!("path does not exist or is not a directory: {project_path}");
            }
            if let Some(handle) = poll_latest_session_mailbox() {
                println!("{handle}");
            } else {
                eprintln!(
                    "tab created but bootstrap not observed within timeout — \
                     the shell is still starting; find its handle later in the \
                     GUI log (\"orchestration: session mailbox session_<sid> registered\")"
                );
            }
        }
        // worktree-create --agent/--prompt: the GUI created the worktree +
        // a terminal tab in it (output printed above); resolve the session
        // and run the one-shot spawn from here (the CLI process may sleep;
        // the GUI main thread must not).
        #[cfg(unix)]
        if let OrchestrationCommand::WorktreeCreate {
            ref agent,
            ref prompt,
            ..
        } = command
        {
            if agent.is_some() || prompt.is_some() {
                let handle = poll_latest_session_mailbox().ok_or_else(|| {
                    anyhow!(
                        "worktree + terminal created but session bootstrap not observed \
                         within timeout — inject manually into the new tab"
                    )
                })?;
                println!("{handle}");
                if let Some(agent_cmd) = agent {
                    forward_session_inject(&handle, agent_cmd, false)?;
                }
                if let Some(prompt_text) = prompt {
                    if agent.is_some() {
                        // Give the agent TUI a beat to take over the TTY
                        // before handing it the prompt.
                        std::thread::sleep(std::time::Duration::from_millis(
                            WORKTREE_AGENT_SETTLE_MS,
                        ));
                    }
                    // We just launched the agent ourselves — force past the
                    // idle check (its title is not settled yet).
                    forward_session_inject(&handle, prompt_text, true)?;
                }
            }
        }
        // The command was handled via socket; terminate the event loop.
        cx.terminate_app(warpui::platform::TerminationMode::ForceTerminate, None);
        return Ok(());
    }

    let store = orchestration::connection::store();
    let result = execute_command(&command, store, cx);
    if result.is_ok() {
        // CLI dispatch lives inside the app event loop; without an explicit
        // terminate the loop keeps waiting for windows (headless hang).
        cx.terminate_app(warpui::platform::TerminationMode::ForceTerminate, None);
    }
    result
}

/// Poll the GUI's L1 `latest-session` probe until a session mailbox registers
/// after this invocation began (snapshot first, then wait for a change).
/// Runs in the CLI process — safe to sleep here. ~4 s budget.
#[cfg(unix)]
fn poll_latest_session_mailbox() -> Option<String> {
    use crate::ai::orchestration::runtime_rpc;

    let probe = || -> Option<String> {
        let resp = runtime_rpc::try_socket_forward("latest-session", &serde_json::json!({})).ok()?;
        let resp = resp?;
        let v: serde_json::Value = serde_json::from_str(&resp).ok()?;
        v.get("handle")?.as_str().map(String::from)
    };
    let before = probe();
    // 票1: bootstrap 通常 <3s, 窗口 12s 慢机余量; 100ms 轮询。
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(12_000);
    loop {
        let now = probe();
        if let Some(h) = now {
            if before.as_deref() != Some(h.as_str()) {
                return Some(h);
            }
        }
        if std::time::Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}


/// Execute an orchestration command against the store.
///
/// Shared by the CLI dispatch path ([`run`]) and the runtime RPC server's
/// L2 dispatcher (`RpcDispatcher`) — the GUI process executes forwarded
/// commands with the exact same semantics as a local CLI invocation.
pub fn execute_command(
    command: &OrchestrationCommand,
    store: &DieselOrchestrationStore,
    cx: &mut warpui::AppContext,
) -> anyhow::Result<()> {
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

        OrchestrationCommand::StartWorker {
            task_id,
            command,
            session,
        } => {
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

            // D-04: auto-bind the dispatch to a worker terminal. Explicit
            // `--session session_<sid>` is the DAG path — bind exactly that
            // pane (failure is an error, the caller asked for it). Without
            // it, bind the active pane when one exists (best effort —
            // headless dispatch creation stays legal).
            let mut bound = false;
            match session {
                Some(handle) => {
                    let summary =
                        crate::ai::orchestration::dispatch_assign::assign_to_session(
                            &dispatch_id,
                            handle,
                            cx,
                        )
                        .map_err(|e| {
                            anyhow!("dispatch {dispatch_id} created but session binding failed: {e:#}")
                        })?;
                    println!("{summary}");
                    bound = true;
                }
                None => {
                    match crate::ai::orchestration::dispatch_assign::assign_to_active_pane(
                        &dispatch_id, cx,
                    ) {
                        Ok(summary) => {
                            println!("{summary}");
                            bound = true;
                        }
                        Err(e) => eprintln!(
                            "note: dispatch {dispatch_id} not bound to a pane ({e:#}); \
                             re-run with --session session_<sid> to bind explicitly"
                        ),
                    }
                }
            }

            // D-18: a bound dispatch means the worker terminal is in place —
            // advance the state machine (worker starting→ready, dispatch
            // pending→dispatched, task ready→dispatched). Without this the
            // block-driven settlement rejects every matching block with
            // InactiveDispatch (settle requires task+dispatch `dispatched`)
            // and the task strands in `ready` with no log trace (the
            // rejection path was silent). Orca semantics: the coordinator
            // calls markWorkerDispatchReady once the worker is up; here the
            // binding is that signal.
            if bound {
                match store.mark_worker_dispatch_ready(&dispatch_id, None) {
                    Ok(()) => println!("{dispatch_id} ready (dispatch + task -> dispatched)"),
                    Err(e) => eprintln!(
                        "note: dispatch {dispatch_id} bound but not marked ready ({e}); \
                         block settlement requires mark-ready first"
                    ),
                }
            }
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
                &dispatch_id, &text, *force, cx,
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
                *lines,
                max_bytes,
                *after,
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
                *enter,
                *interrupt,
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
            if *wait {
                // Orca waitForMessage semantics (31959): register a claim so
                // the push plane skips this mailbox's matching types (mutual
                // exclusion), poll until match/timeout, always finish with a
                // final same-filter re-read (timeout is not an error).
                let waiter_id = format!("wtr_{}", uuid::Uuid::new_v4().simple());
                let types_json = serde_json::to_string(&message_type).unwrap_or_else(|_| "[]".into());
                let deadline = std::time::Instant::now()
                    + std::time::Duration::from_millis((*timeout_ms).max(1));
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

        // ── orch-caps-v2: project / worktree / new-terminal ──
        OrchestrationCommand::ProjectAdd { path } => {
            println!("{}", crate::ai::orchestration::projects_cli::project_add(path, cx)?);
        }

        OrchestrationCommand::ProjectRemove { path, force } => {
            println!(
                "{}",
                crate::ai::orchestration::projects_cli::project_remove(path, *force, cx)?
            );
        }

        OrchestrationCommand::ProjectList => {
            let out = crate::ai::orchestration::projects_cli::project_list()?;
            if !out.is_empty() {
                println!("{out}");
            }
        }

        OrchestrationCommand::WorktreeCreate {
            project_path,
            name,
            agent,
            prompt,
        } => {
            let path =
                crate::ai::orchestration::worktrees::worktree_create(project_path, name, cx)?;
            println!("{path}");
            // One-shot spawn (Orca parity): open a terminal in the fresh
            // worktree here (GUI action); the CLI process then resolves the
            // session handle and injects agent/prompt (see `run`).
            if agent.is_some() || prompt.is_some() {
                let tab =
                    crate::ai::orchestration::new_terminal::new_terminal(&path, None, cx)?;
                println!("{tab}");
            }
        }

        OrchestrationCommand::GcRuns { days, dry_run } => {
            let victims = store
                .gc_runs(chrono::Duration::days(*days), *dry_run)
                .map_err(|e| anyhow!("{e}"))?;
            if victims.is_empty() {
                if *dry_run {
                    println!("no runs older than {days}d to collect");
                } else {
                    println!("no runs collected");
                }
            } else {
                for id in &victims {
                    if *dry_run {
                        println!("would delete {id}");
                    } else {
                        println!("deleted {id}");
                    }
                }
            }
        }

        OrchestrationCommand::WorktreeList { project_path } => {
            let out = crate::ai::orchestration::worktrees::worktree_list(project_path.as_deref())?;
            if out.is_empty() {
                println!("no worktrees");
            } else {
                println!("{out}");
            }
        }

        OrchestrationCommand::WorktreeRemove { path, force } => {
            println!(
                "{}",
                crate::ai::orchestration::worktrees::worktree_remove(path, *force, cx)?
            );
        }

        OrchestrationCommand::CloseTerminal { handle, force } => {
            println!(
                "{}",
                crate::ai::orchestration::new_terminal::close_terminal(handle, *force, cx)?
            );
        }

        OrchestrationCommand::NewTerminal { project_path, cwd } => {
            // GUI action: the L2 fast path forwards this to the GUI process
            // (try_socket_fast_path). Landing here headless (no GUI) errors
            // inside with a clear message.
            println!(
                "{}",
                crate::ai::orchestration::new_terminal::new_terminal(
                    project_path,
                    cwd.as_deref(),
                    cx,
                )?
            );
        }
    }

    Ok(())
}

/// Attempt to forward an orchestration command to a running GUI via the
/// runtime RPC socket (L2). Returns `Ok(true)` when the command was handled
/// on the GUI side, `Ok(false)` when the caller should degrade to the
/// direct-DB path, and `Err` when the invocation *failed* (refused, or the
/// GUI executed it and reported an error — D-03: both previously printed to
/// stdout with exit 0; errors now surface as stderr + exit 1).
///
/// Blocking commands (`check-messages --wait`) are never forwarded — the
/// waiter loop belongs in this process (its waiter claim must live exactly
/// as long as this invocation); a GUI-side wait would also pin the GPUI
/// main thread against the dispatcher timeout.
fn try_socket_fast_path(command: &OrchestrationCommand) -> anyhow::Result<bool> {
    use crate::ai::orchestration::runtime_rpc;

    // Single-consumer guard: when a GUI runtime is alive, its message-router
    // thread owns the "orchestrator" mailbox. Pulling it from here (forwarded
    // or direct-DB) races the router for the same unread rows. Refuse the
    // invocation outright; headless (no runtime) pulls stay legal.
    if let OrchestrationCommand::CheckMessages { ref handle, .. } = command {
        if handle == "orchestrator" && runtime_rpc::runtime_alive() {
            anyhow::bail!(
                "refused: the orchestrator mailbox is consumed by the running \
                 GUI's message router"
            );
        }
    }

    if let OrchestrationCommand::CheckMessages { wait: true, .. } = command {
        return Ok(false);
    }

    match runtime_rpc::try_socket_forward("orchestration", &serde_json::to_value(command).unwrap_or(serde_json::Value::Null)) {
        Ok(Some(response)) => {
            // The GUI executed the command; the response JSON carries the
            // captured stdout under "output". Print it verbatim (strip the
            // JSON wrapper) so the caller sees exactly what a local CLI
            // invocation would print — unless the envelope reports an
            // execution error, which becomes Err (D-03).
            match classify_forwarded_response(&response) {
                ForwardedOutcome::Failed(err) => Err(anyhow!("{err}")),
                ForwardedOutcome::Output(v) => {
                    print_forwarded_value(&v);
                    Ok(true)
                }
                ForwardedOutcome::Raw(raw) => {
                    print!("{raw}");
                    use std::io::Write as _;
                    let _ = std::io::stdout().flush();
                    Ok(true)
                }
            }
        }
        Ok(None) => {
            // No GUI / fallback stub / timeout — degrade to direct DB.
            Ok(false)
        }
        Err(e) => {
            // RPC error — log but degrade to direct DB.
            log::warn!("runtime_rpc socket error, degrading to direct DB: {e:#}");
            Ok(false)
        }
    }
}

/// How to surface a forwarded command's response envelope.
#[derive(Debug, PartialEq)]
enum ForwardedOutcome {
    /// Success: the pretty-printed `result` JSON carrying `"output"`.
    Output(serde_json::Value),
    /// Error: `{"error": ..., "executed": true}` — the command ran (or timed
    /// out) on the GUI thread and failed. Never printable as stdout + exit 0
    /// (D-03). Recognized by a top-level `"error"` without `"output"` (a
    /// successful result always carries `"output"`).
    Failed(String),
    /// Unrecognized shape — print raw rather than losing information.
    Raw(String),
}

fn classify_forwarded_response(response: &str) -> ForwardedOutcome {
    match serde_json::from_str::<serde_json::Value>(response) {
        Ok(v) => {
            if v.get("output").is_none() {
                if let Some(err) = v.get("error").and_then(|e| e.as_str()) {
                    return ForwardedOutcome::Failed(err.to_string());
                }
            }
            ForwardedOutcome::Output(v)
        }
        Err(_) => ForwardedOutcome::Raw(response.to_string()),
    }
}

/// Print the captured output of a forwarded command.
///
/// `value` is the parsed `result` JSON (`{"output": "..."}`); anything else
/// is printed pretty rather than losing information.
#[cfg(unix)]
fn print_forwarded_value(value: &serde_json::Value) {
    if let Some(output) = value.get("output").and_then(|o| o.as_str()) {
        print!("{output}");
    } else {
        print!("{}", serde_json::to_string_pretty(value).unwrap_or_default());
    }
    use std::io::Write as _;
    let _ = std::io::stdout().flush();
}

/// Settle delay between typing the agent launch command and pasting the
/// prompt into it (worktree-create --agent + --prompt one-shot spawn).
#[cfg(unix)]
const WORKTREE_AGENT_SETTLE_MS: u64 = 6_000;

/// Forward an inject-prompt for a session handle to the running GUI (used by
/// the worktree-create --agent/--prompt flow; PTY writes must happen in the
/// GUI process). Error envelopes surface as Err (D-03).
#[cfg(unix)]
fn forward_session_inject(handle: &str, text: &str, force: bool) -> anyhow::Result<()> {
    use crate::ai::orchestration::runtime_rpc;

    let cmd = OrchestrationCommand::InjectPrompt {
        dispatch_id: handle.to_string(),
        text: text.to_string(),
        force,
    };
    let response = runtime_rpc::try_socket_forward(
        "orchestration",
        &serde_json::to_value(&cmd)?,
    )?
    .ok_or_else(|| anyhow!("inject into {handle}: no GUI runtime (it went away?)"))?;
    match classify_forwarded_response(&response) {
        ForwardedOutcome::Failed(err) => Err(anyhow!("inject into {handle} failed: {err}")),
        ForwardedOutcome::Output(v) => {
            print_forwarded_value(&v);
            Ok(())
        }
        ForwardedOutcome::Raw(raw) => {
            print!("{raw}");
            use std::io::Write as _;
            let _ = std::io::stdout().flush();
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// D-03: error envelopes must classify as Failed — never as printable
    /// output (the bug: exit 0 + `{"error":...}` on stdout).
    #[test]
    fn forwarded_error_envelope_classifies_failed() {
        let resp = r#"{
  "error": "no GUI window is running",
  "executed": true
}"#;
        assert_eq!(
            classify_forwarded_response(resp),
            ForwardedOutcome::Failed("no GUI window is running".to_string())
        );
    }

    /// Success envelopes (carry "output") classify as Output even when the
    /// output text itself mentions errors.
    #[test]
    fn forwarded_success_envelope_classifies_output() {
        let resp = r#"{"output": "ctx_abc123\nnote: error-like text inside output"}"#;
        assert!(matches!(
            classify_forwarded_response(resp),
            ForwardedOutcome::Output(_)
        ));
    }

    /// Non-JSON shapes stay raw (printed, not treated as success or error).
    #[test]
    fn forwarded_non_json_classifies_raw() {
        assert_eq!(
            classify_forwarded_response("not json at all"),
            ForwardedOutcome::Raw("not json at all".to_string())
        );
    }
}
