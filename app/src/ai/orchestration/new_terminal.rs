//! GUI new-terminal for the orchestration CLI (orch-caps-v2).
//!
//! Opens a terminal tab in the project's active window on the GUI main
//! thread (this module only runs inside the GUI process — the L2 runtime
//! RPC dispatcher routes `new-terminal` there; headless callers get a clear
//! error from the workspace check below).
//!
//! Flow:
//! 1. Find the workspace whose `active_project` is `project_path`
//!    (fallback: the first live workspace — `switch_project` then makes it
//!    the active project, matching GUI selection).
//! 2. `switch_project` + `add_tab_with_pane_layout(SingleTerminal(
//!    NewTerminalOptions { initial_directory }))` — the same path the GUI's
//!    own "new tab" uses, so shell/bootstrap/intercept wiring is identical.
//! 3. Read the new tab's session id (`pending_session_id` is set at
//!    terminal creation, before bootstrap completes — the bounded poll only
//!    covers the spawn gap).
//! 4. With `--alias`, type the alias command into the fresh shell via the
//!    global PTY sender (`session_<sid>` mailbox, registered at shell
//!    bootstrap): payload write, 500 ms, lone `\r` — the established
//!    split-submit discipline. A fresh prompt is idle by construction, so
//!    no title-based idle check applies.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use ::ai::agent::orchestration::executor::PtyExecutor;
use warpui::AppContext;
use warpui::SingletonEntity as _;

use crate::ai::orchestration::shell_event_bridge::ShellEventBridge;
use crate::terminal::model::session::SessionId;
use crate::workspace::{Workspace, WorkspaceRegistry};

/// Poll budget for the session id (the L2 dispatcher timeout is 4.5 s; keep
/// well under). 60 × 50 ms = 3 s worst case; typically zero iterations.
const SESSION_POLL_ITERS: usize = 60;
const SESSION_POLL_INTERVAL: Duration = Duration::from_millis(50);

/// Known intercept-style harness launch aliases (documentation surface; the
/// injection itself types the alias string into the fresh shell — resolution
/// happens in the user's shell, exactly like typing it by hand).
pub const KNOWN_ALIASES: &[(&str, &str)] = &[
    ("omp", "oh-my-pi agent TUI (omp)"),
    ("pi", "pi agent TUI (pi)"),
    ("cc", "claude code (claude)"),
];

/// `new-terminal` body. Requires the GUI process (live workspaces).
pub fn new_terminal(
    project_path: &str,
    alias: Option<&str>,
    cwd: Option<&str>,
    cx: &mut AppContext,
) -> anyhow::Result<String> {
    let project = super::projects_cli::canonical_project_path(project_path)?;
    let workdir: PathBuf = match cwd {
        Some(c) => {
            let p = PathBuf::from(c);
            if !p.is_dir() {
                anyhow::bail!("--cwd is not a directory: {c}");
            }
            p.canonicalize()
                .map_err(|e| anyhow::anyhow!("canonicalize {c}: {e}"))?
        }
        None => project.clone(),
    };
    if let Some(a) = alias {
        if a.contains('\n') || a.contains('\r') || a.is_empty() {
            anyhow::bail!("invalid alias: {a:?}");
        }
    }

    let all = WorkspaceRegistry::handle(cx).read(cx, |r, app| r.all_workspaces(app));
    let Some((window_id, workspace)) = all
        .iter()
        .find(|(_, ws)| ws.read(cx, |w, _| w.active_project.as_ref() == Some(&project)))
        .or_else(|| all.first())
        .cloned()
    else {
        anyhow::bail!(
            "no GUI window is running — new-terminal requires the dais GUI \
             (start it first, then retry)"
        );
    };

    let mut session_id: Option<SessionId> = None;
    workspace.update(cx, |ws, ctx| {
        // Project view first (same chain as GUI selection), then the tab —
        // the tab inherits the active project for rail grouping.
        ws.switch_project(Some(project.clone()), ctx);
        let layout = crate::pane_group::PanesLayout::SingleTerminal(Box::new(
            crate::pane_group::NewTerminalOptions {
                initial_directory: Some(workdir.clone()),
                hide_homepage: true,
                ..Default::default()
            },
        ));
        ws.add_tab_with_pane_layout(
            layout,
            std::sync::Arc::new(std::collections::HashMap::new()),
            None,
            ctx,
        );
        ctx.notify();

        // The new tab is the active one; its pane group's active terminal is
        // the fresh session.
        let pg = ws.active_tab_pane_group().clone();
        pg.update(ctx, |pg, ctx| {
            if let Some(tv) = pg.active_session_view(ctx) {
                tv.read(ctx, |view, _| {
                    // pending_session_id exists from creation; the
                    // active-block id appears after the first prompt.
                    let sid = view
                        .model
                        .lock()
                        .pending_session_id()
                        .or_else(|| view.active_block_session_id());
                    if let Some(sid) = sid {
                        session_id = Some(sid);
                    }
                });
            }
        });
    });

    // Bounded poll for the spawn gap (creation → pending set). Runs on the
    // GUI main thread; sleeps only while the id is genuinely not yet there.
    let deadline = Instant::now() + SESSION_POLL_INTERVAL * SESSION_POLL_ITERS as u32;
    while session_id.is_none() && Instant::now() < deadline {
        workspace.read(cx, |ws, app| {
            let pg = ws.active_tab_pane_group();
            pg.read(app, |pg, ctx| {
                if let Some(tv) = pg.active_session_view(ctx) {
                    tv.read(ctx, |view, _| {
                        session_id = view
                            .model
                            .lock()
                            .pending_session_id()
                            .or_else(|| view.active_block_session_id());
                    });
                }
            });
        });
        if session_id.is_none() {
            std::thread::sleep(SESSION_POLL_INTERVAL);
        }
    }

    let Some(sid) = session_id else {
        anyhow::bail!("terminal tab created but session id unavailable (bootstrap did not start?)");
    };
    let handle = ShellEventBridge::session_mailbox_handle(sid);

    if let Some(alias) = alias {
        // Fresh prompt: write the alias, then a lone CR after the submit
        // delay (same discipline as inject_prompt, minus the idle check —
        // a brand-new shell is idle by construction).
        let sender = super::global_pty_sender().ok_or_else(|| {
            anyhow::anyhow!(
                "global PTY sender not initialized — is the orchestration PTY bridge running?"
            )
        })?;
        sender
            .write_to_pty(&handle, alias.as_bytes())
            .map_err(|e| anyhow::anyhow!("alias write failed: {e}"))?;
        let sender_clone = sender.clone();
        let handle_owned = handle.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(
                ::ai::agent::orchestration::prompt_injection::AGENT_PROMPT_SUBMIT_DELAY_MS,
            ));
            let _ = sender_clone.write_to_pty(
                &handle_owned,
                ::ai::agent::orchestration::prompt_injection::AGENT_PROMPT_SUBMIT,
            );
        });
    }

    Ok(format!(
        "{handle} (window {window_id:?}, project {}, cwd {}, alias {})",
        project.display(),
        workdir.display(),
        alias.unwrap_or("<none>")
    ))
}
