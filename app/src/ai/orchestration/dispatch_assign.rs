//! Dispatch-to-pane assignment — registers a dispatch ID against a terminal
//! pane's view and session.
//!
//! This is the business link that makes the three injection use-cases
//! operational: until a dispatch is assigned to a pane, `ViewRegistry` (PTY
//! writes + tail/title reads) and `SessionDispatchMap` (shell-event bridging)
//! have no entry for it and every use-case call returns
//! "no terminal view registered".
//!
//! Assignment binds:
//! - `ViewRegistry`: dispatch_id → WeakViewHandle<TerminalView>
//! - `SessionDispatchMap`: session_id → dispatch_id (shell events route back)
//! - DB `dispatch_contexts.assignee_*` (D-04: previously only the in-memory
//!   registries were updated, so the assignee columns stayed NULL)

use ::ai::agent::orchestration::connection::store;

use crate::ai::orchestration::shell_event_bridge::{SessionDispatchMap, ShellEventBridge};
use crate::ai::orchestration::ViewRegistry;
use crate::terminal::model::session::SessionId;
use crate::terminal::view::TerminalView;
use warpui::{AppContext, SingletonEntity as _};

/// Shared bind core for both entry points: register the in-memory maps +
/// push delivery, then persist the assignment (D-04).
fn bind_dispatch_to_view(
    dispatch_id: &str,
    terminal_view: &warpui::ViewHandle<TerminalView>,
    session_id: SessionId,
    cx: &AppContext,
) -> anyhow::Result<String> {
    // 1. Register both maps.
    ViewRegistry::handle(cx).read(cx, |registry, _| {
        registry.register(dispatch_id, terminal_view.downgrade());
    });
    SessionDispatchMap::handle(cx).read(cx, |map, _| {
        map.register(session_id, dispatch_id);
    });

    // 2. Register for push delivery (pointer injection on idle).
    ::ai::agent::orchestration::delivery::register_dispatch(dispatch_id);

    // 3. Persist the assignment (D-04) — assignee_handle is the session
    //    mailbox (the direct-send address other agents use), pane_key the
    //    terminal view id.
    let mailbox = ShellEventBridge::session_mailbox_handle(session_id);
    let pane_key = format!("view_{}", terminal_view.id());
    store()
        .set_dispatch_assignee(dispatch_id, &mailbox, &pane_key)
        .map_err(|e| anyhow::anyhow!("persist assignment: {e}"))?;

    Ok(format!(
        "{dispatch_id} assigned to {mailbox} (pane view {})",
        terminal_view.id()
    ))
}

/// Assign `dispatch_id` to the active terminal pane.
///
/// Returns a human-readable summary of the binding.
///
/// Fails when there is no window / workspace / active terminal pane (e.g. the
/// app is running headless or the focused pane is not a terminal).
pub fn assign_to_active_pane(dispatch_id: &str, cx: &AppContext) -> anyhow::Result<String> {
    // 1. Locate the active terminal view.
    let terminal_view = active_terminal_view(cx).ok_or_else(|| {
        anyhow::anyhow!(
            "no active terminal pane found — focus a terminal pane (or open one) \
             before assigning dispatch {dispatch_id}"
        )
    })?;

    // 2. Resolve its session id (the key shell events carry).
    let session_id = terminal_view
        .read(cx, |tv, _cx| tv.active_block_session_id())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "active terminal pane has no session yet (shell not bootstrapped) — \
                 cannot assign dispatch {dispatch_id}"
            )
        })?;

    bind_dispatch_to_view(dispatch_id, &terminal_view, session_id, cx)
}

/// Assign `dispatch_id` to the terminal owning `session_handle` — the
/// `session_<sid>` mailbox handle new-terminal prints. The DAG entry point
/// (D-04): a coordinator creates a terminal, then binds the worker dispatch
/// to exactly that pane (the "active pane" is whatever the human last
/// focused — wrong target for multi-worker DAGs).
///
/// Accepts the handle with or without the `session_` prefix. Fails when no
/// live terminal owns the session (not bootstrapped yet / already closed) or
/// when the GUI is not running.
pub fn assign_to_session(
    dispatch_id: &str,
    session_handle: &str,
    cx: &AppContext,
) -> anyhow::Result<String> {
    let mailbox = if let Some(sid) = session_handle.strip_prefix("session_") {
        format!("session_{sid}")
    } else {
        format!("session_{session_handle}")
    };

    // Session mailboxes self-register in ViewRegistry at shell bootstrap
    // (shell_event_bridge::ensure_session_mailbox) — resolve the view there.
    let terminal_view =
        ViewRegistry::handle(cx).read(cx, |registry, cx| registry.get(&mailbox, cx));
    let terminal_view = terminal_view.ok_or_else(|| {
        anyhow::anyhow!(
            "no terminal owns session handle {mailbox} — the shell may still be \
             bootstrapping or was closed; cannot assign dispatch {dispatch_id}"
        )
    })?;

    let session_id = terminal_view
        .read(cx, |tv, _cx| tv.active_block_session_id())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "terminal for {mailbox} has no active session — cannot assign \
                 dispatch {dispatch_id}"
            )
        })?;

    bind_dispatch_to_view(dispatch_id, &terminal_view, session_id, cx)
}

/// Find the active window's focused/active terminal view, if any.
fn active_terminal_view(cx: &AppContext) -> Option<warpui::ViewHandle<TerminalView>> {
    let window_id = warpui::windowing::WindowManager::as_ref(cx).active_window()?;
    let workspace = cx
        .views_of_type::<crate::workspace::Workspace>(window_id)?
        .into_iter()
        .next()?;
    workspace.read(cx, |ws, cx| {
        ws.active_tab_pane_group()
            .as_ref(cx)
            .active_session_view(cx)
    })
}

/// Remove an assignment (both maps + push delivery + DB columns).
pub fn unassign(dispatch_id: &str, cx: &AppContext) -> anyhow::Result<()> {
    // Unregister push delivery first: drops the watermark so a re-assignment
    // starts clean (a reborn PTY must never receive a stale Enter).
    ::ai::agent::orchestration::delivery::unregister_dispatch(dispatch_id);
    ViewRegistry::handle(cx).read(cx, |registry, _| {
        registry.unregister(dispatch_id);
    });
    // SessionDispatchMap is keyed by session_id; without a reverse index we
    // can only drop entries whose value matches. Scan is fine (small map).
    SessionDispatchMap::handle(cx).read(cx, |map, _| {
        map.retain(|_sid, did| did != dispatch_id);
    });
    // D-04: clear the persisted assignment too (best effort — the row may
    // predate assignment persistence).
    if let Err(e) = store().clear_dispatch_assignee(dispatch_id) {
        log::warn!("unassign: clearing DB assignment for {dispatch_id}: {e}");
    }
    Ok(())
}
