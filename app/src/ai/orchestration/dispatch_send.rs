//! Use-case 1: prompt injection and task dispatch sending.
//!
//! Sends text into a dispatched worker's terminal via bracketed paste + submit.
//! Checks agent idle status (from terminal title) before injecting, unless `force`.

use ::ai::agent::orchestration::connection::store;
use ::ai::agent::orchestration::prompt_injection::{
    build_dispatch_preamble, detect_agent_status_from_title, send_agent_prompt,
    AgentTerminalStatus, PreambleParams, WorkerKind,
};

use crate::ai::orchestration::global_pty_sender;
use crate::ai::orchestration::terminal_tail::terminal_title_with_cx;

use warpui::AppContext;

// ---------------------------------------------------------------------------
// inject_prompt
// ---------------------------------------------------------------------------

/// Inject a prompt into a dispatched worker's terminal.
///
/// Flow:
/// 1. Verify the dispatch exists in the store.
/// 2. Read the terminal title (if available) and check idle status.
/// 3. Reject if agent is working and `force` is false.
/// 4. Send the prompt via bracketed paste + delayed submit.
pub fn inject_prompt(
    dispatch_id: &str,
    text: &str,
    force: bool,
    cx: &AppContext,
) -> anyhow::Result<String> {
    // 1. Validate dispatch exists. Session mailboxes (`session_<sid>`) are
    // valid terminal addresses without a dispatch row — cross-harness
    // direct sends inject into them directly.
    let is_session_handle = dispatch_id.starts_with("session_");
    let ctx = if is_session_handle {
        None
    } else {
        Some(
            store()
                .get_dispatch_context_by_id(dispatch_id)
                .map_err(|e| anyhow::anyhow!("store error: {e}"))?
                .ok_or_else(|| anyhow::anyhow!("dispatch not found: {dispatch_id}"))?,
        )
    };

    // 2. Try to get terminal title for idle check.
    let title = terminal_title_with_cx(dispatch_id, cx);

    // 3. Idle check.
    if let Some(t) = &title {
        match detect_agent_status_from_title(t) {
            Some(AgentTerminalStatus::Idle) | None => {
                // Good to go — idle or no agent signal.
            }
            Some(status) => {
                if !force {
                    anyhow::bail!(
                        "agent is {status:?} (dispatch {dispatch_id}, title: \"{t}\"). \
                         Use --force to inject anyway.",
                    );
                }
            }
        }
    } else if !force {
        // No title available — can't confirm idle. Only allow with force.
        anyhow::bail!(
            "could not read terminal title for dispatch {dispatch_id} \
             (terminal may not exist). Use --force to inject anyway."
        );
    }

    // 4. Get the PTY sender and send.
    let sender = global_pty_sender().ok_or_else(|| {
        anyhow::anyhow!(
            "global PTY sender not initialized — is the orchestration PTY bridge running?"
        )
    })?;

    send_agent_prompt(sender, dispatch_id, text)
        .map_err(|e| anyhow::anyhow!("PTY write failed: {e}"))?;

    let bytes_len = text.len();
    let summary = match &ctx {
        Some(ctx) => format!(
            "injected {bytes_len} bytes into dispatch {dispatch_id} (task {}, force={force})",
            ctx.task_id,
        ),
        None => format!(
            "injected {bytes_len} bytes into session mailbox {dispatch_id} (force={force})"
        ),
    };
    Ok(summary)
}

// ---------------------------------------------------------------------------
// send_task_dispatch
// ---------------------------------------------------------------------------

/// Send a task dispatch preamble to a worker terminal.
///
/// Builds the dispatch preamble from the stored task spec and sends it as a
/// prompt injection. Requires the agent to be idle.
pub fn send_task_dispatch(
    task_id: &str,
    dispatch_id: &str,
    coordinator_handle: &str,
    cli_command: &str,
    cx: &AppContext,
) -> anyhow::Result<String> {
    // 1. Load the task.
    let task = store()
        .get_task(task_id)
        .map_err(|e| anyhow::anyhow!("store error: {e}"))?
        .ok_or_else(|| anyhow::anyhow!("task not found: {task_id}"))?;

    // 2. Validate the dispatch exists.
    store()
        .get_dispatch_context_by_id(dispatch_id)
        .map_err(|e| anyhow::anyhow!("store error: {e}"))?
        .ok_or_else(|| anyhow::anyhow!("dispatch not found: {dispatch_id}"))?;

    // 3. Check idle status.
    let title = terminal_title_with_cx(dispatch_id, cx);
    if let Some(t) = &title {
        match detect_agent_status_from_title(t) {
            Some(AgentTerminalStatus::Idle) | None => {}
            Some(status) => {
                anyhow::bail!(
                    "agent is {status:?} (dispatch {dispatch_id}, title: \"{t}\"). \
                     Cannot dispatch task while agent is busy.",
                );
            }
        }
    } else {
        anyhow::bail!(
            "could not read terminal title for dispatch {dispatch_id}. \
             Cannot confirm agent idle — aborting dispatch."
        );
    }

    // 4. Build preamble.
    let preamble = build_dispatch_preamble(&PreambleParams {
        task_id,
        dispatch_id,
        task_spec: &task.spec,
        coordinator_handle,
        worker_handle: dispatch_id,
        cli_command,
        worker_kind: WorkerKind::PromptReturningAgent,
    });

    // 5. Send.
    let sender = global_pty_sender().ok_or_else(|| {
        anyhow::anyhow!(
            "global PTY sender not initialized — is the orchestration PTY bridge running?"
        )
    })?;

    send_agent_prompt(sender, dispatch_id, &preamble)
        .map_err(|e| anyhow::anyhow!("PTY write failed: {e}"))?;

    let spec_preview: String = task.spec.chars().take(60).collect();
    let summary = format!(
        "dispatched task {task_id} ({spec_preview}) to {dispatch_id} via preamble",
    );
    Ok(summary)
}
