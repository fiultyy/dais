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
//! 3. `session_<sid>` cannot be awaited on the GUI main thread
//!    (`pending_session_id` is set by the async DCS handshake; any sync wait
//!    there freezes the event loop and starves the very spawn it waits for).
//!    Instead the command returns after tab creation, and the **CLI process**
//!    polls the L1 `latest-session` probe (runtime_rpc records every session
//!    mailbox registration with a timestamp; see `note_session_mailbox`) until
//!    a mailbox registers after the invocation, then prints it.
//! 4. Harness launch is the caller's business: intercept aliases (omp-dais
//!    等) are armed in every new shell's bootstrap, so the caller injects the
//!    launch command itself (inject-prompt) after receiving `session_<sid>`
//!    on stdout.')


use std::path::{Path, PathBuf};

use ::ai::agent::orchestration::executor::PtyExecutor;
use warpui::AppContext;
use warpui::SingletonEntity as _;

use crate::workspace::{Workspace, WorkspaceRegistry};
use anyhow::anyhow;

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
    });
    // session_<sid> 由 CLI 端轮询 L1 latest-session 解析(见模块 doc 第 3 条);
    // --alias 的注入同样由 CLI 端拿到 handle 后经 PTY 桥下发。

    // --alias 的注入由 CLI 端在解析出 session handle 后, 经既有
    // inject-prompt L2 链路转发回 GUI 执行(PTY 写必须发生在 GUI 进程)。

    Ok(format!(
        "terminal tab created (window {window_id:?}, project {}, cwd {})",
        project.display(),
        workdir.display()
    ))
}

// ── v2-fix-13 票3: close-terminal / project --force 全回收 ─────────────

/// Find the workspace tab index containing a terminal view id.
fn find_tab_by_terminal(
    cx: &AppContext,
    target_id: warpui::EntityId,
) -> Option<(warpui::ViewHandle<Workspace>, usize)> {
    let all = WorkspaceRegistry::handle(cx).read(cx, |r, app| r.all_workspaces(app));
    all.into_iter().find_map(|(_, ws)| {
        let idx = ws.read(cx, |w, _| {
            w.tabs.iter().position(|tab| {
                tab.pane_group.read(cx, |pg, ctx| {
                    pg.terminal_views(ctx).iter().any(|tv| tv.id() == target_id)
                })
            })
        })?;
        Some((ws, idx))
    })
}

/// One tab's full reclaim when closing under `--force` semantics: interrupt
/// the foreground process (Ctrl-C), shut the PTY down (the pty server's
/// child reaper SIGHUPs + reaps the shell and its group), then close the tab.
/// The session mailbox retires naturally on shell exit (shell_event_bridge).
fn force_close_tab(ws: &warpui::ViewHandle<Workspace>, idx: usize, cx: &mut AppContext) {
    // Interrupt + PTY shutdown for every terminal pane in the tab.
    let views: Vec<warpui::ViewHandle<crate::terminal::view::TerminalView>> = ws
        .read(cx, |w, _| {
            w.tabs
                .get(idx)
                .map(|tab| {
                    tab.pane_group.read(cx, |pg, ctx| pg.terminal_views(ctx))
                })
                .unwrap_or_default()
        });
    for tv in views {
        tv.update(cx, |v, ctx| {
            v.shutdown_pty(ctx);
        });
    }
    ws.update(cx, |ws, ctx| {
        ws.remove_tab_without_undo(idx, ctx);
        ctx.notify();
    });
}

/// SIGTERM → grace → SIGKILL sweep of processes whose cwd is inside `root`
/// (v2-fix-13: 驻留 harness 兜底; SIGHUP 通常已覆盖, 此处是孤儿兜网)。
/// Never signals the caller's own process.
fn kill_cwd_sweep(root: &Path) -> usize {
    let me = std::process::id() as i32;
    let mut pids = Vec::new();
    if let Ok(dir) = std::fs::read_dir("/proc") {
        for entry in dir.flatten() {
            let name = entry.file_name();
            let Some(pid) = name.to_str().and_then(|n| n.parse::<i32>().ok()) else {
                continue;
            };
            if pid == me {
                continue;
            }
            if let Ok(cwd) = std::fs::read_link(format!("/proc/{pid}/cwd")) {
                if cwd.starts_with(root) {
                    pids.push(pid);
                }
            }
        }
    }
    let mut hit = 0;
    for &pid in &pids {
        unsafe { libc::kill(pid, libc::SIGTERM) };
        hit += 1;
    }
    if !pids.is_empty() {
        std::thread::sleep(std::time::Duration::from_millis(1000));
        for &pid in &pids {
            unsafe { libc::kill(pid, libc::SIGKILL) };
        }
    }
    hit
}

/// pub wrapper for sibling modules (project --force sweep).
pub(crate) fn kill_cwd_sweep_pub(root: &Path) -> usize {
    kill_cwd_sweep(root)
}

/// `close-terminal` body: close the tab owning `session_<sid>` (manual
/// single-instance reclaim). `--force` interrupts + PTY-shuts first.
pub fn close_terminal(handle: &str, force: bool, cx: &mut AppContext) -> anyhow::Result<String> {
    use crate::ai::orchestration::ViewRegistry;
    use warpui::SingletonEntity as _;

    if !handle.starts_with("session_") {
        anyhow::bail!("handle must be a session mailbox (session_<sid>): {handle}");
    }
    let view = ViewRegistry::handle(cx)
        .read(cx, |r, app| r.get(handle, app))
        .ok_or_else(|| anyhow!("no terminal registered for mailbox {handle} (already closed?)"))?;
    let target_id = view.id();
    let (ws, idx) = find_tab_by_terminal(cx, target_id)
        .ok_or_else(|| anyhow!("no tab owns terminal {handle} (pane closed already?)"))?;
    if force {
        // Ctrl-C 前台进程组(SIGHUP+reap 由 shutdown 内部完成)。
        if let Some(sender) = super::global_pty_sender() {
            use ::ai::agent::orchestration::executor::PtyExecutor as _;
            let _ = sender.write_to_pty(handle, b"\x03");
        }
        force_close_tab(&ws, idx, cx);
    } else {
        ws.update(cx, |ws, ctx| {
            ws.remove_tab_without_undo(idx, ctx);
            ctx.notify();
        });
    }
    Ok(format!(
        "closed {handle} (tab#{idx}{})",
        if force { ", force" } else { "" }
    ))
}

/// project/worktree `--force` 共用: close every GUI tab whose project_path
/// is `path` — full reclaim per tab (interrupt + PTY shutdown + close), loop
/// until none remain (remove 期间新 bootstrap 到达的 tab 也会被后续轮次收
/// 掉 — 竞态清扫, 见 v2-fix-13)。Returns closed count. GUI-only.
pub(crate) fn close_project_tabs(path: &Path, cx: &mut AppContext) -> usize {
    use warpui::SingletonEntity as _;
    let all = WorkspaceRegistry::handle(cx).read(cx, |r, app| r.all_workspaces(app));
    let mut closed = 0usize;
    for (_, ws) in all {
        loop {
            let idx = ws.read(cx, |w, _| {
                w.tabs.iter().position(|tab| tab.project_path.as_deref() == Some(path))
            });
            match idx {
                Some(i) => {
                    force_close_tab(&ws, i, cx);
                    closed += 1;
                }
                None => break,
            }
        }
    }
    closed
}
