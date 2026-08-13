//! Dispatch-to-pane assignment — registers a dispatch ID against the active
//! terminal pane's view and session.
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

use crate::ai::orchestration::shell_event_bridge::SessionDispatchMap;
use crate::ai::orchestration::ViewRegistry;
use crate::terminal::view::TerminalView;
use warpui::{AppContext, SingletonEntity as _};

/// Assign `dispatch_id` to the active terminal pane.
///
/// Returns the Warp session id bound by the assignment.
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

    // 3. Register both maps.
    ViewRegistry::handle(cx)
        .read(cx, |registry, _| {
            registry.register(dispatch_id, terminal_view.downgrade());
        });
    SessionDispatchMap::handle(cx)
        .read(cx, |map, _| {
            map.register(session_id, dispatch_id);
        });

    Ok(format!("{dispatch_id} assigned to session {session_id:?} (pane view {})", terminal_view.id()))
}

/// Find the active window's focused/active terminal view, if any.
fn active_terminal_view(cx: &AppContext) -> Option<warpui::ViewHandle<TerminalView>> {
    let window_id = warpui::windowing::WindowManager::as_ref(cx)
        .active_window()?;
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

/// Remove an assignment (both maps).
pub fn unassign(dispatch_id: &str, cx: &AppContext) -> anyhow::Result<()> {
    ViewRegistry::handle(cx).read(cx, |registry, _| {
        registry.unregister(dispatch_id);
    });
    // SessionDispatchMap is keyed by session_id; without a reverse index we
    // can only drop entries whose value matches. Scan is fine (small map).
    SessionDispatchMap::handle(cx).read(cx, |map, _| {
        map.retain(|_sid, did| did != dispatch_id);
    });
    Ok(())
}
