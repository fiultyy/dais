//! Interactive injection — scan wait-blocked signals and send answers.
//!
//! Use-case 3 of the orchestration CLI surface.
//!
//! - `scan_wait_blocked` reads the terminal tail for a dispatch and applies
//!   `find_wait_blocked_signal` from the shared pure-function layer.
//! - `answer` sends a terminal action (text + optional enter/interrupt) through
//!   the global PTY sender.

use ::ai::agent::orchestration::prompt_injection::{
    find_wait_blocked_signal, send_terminal_action, TAIL_SCAN_MAX_BYTES, TAIL_SCAN_MAX_LINES,
};
use warpui::AppContext;
// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Scan the terminal tail for the given dispatch and return the reason string
/// if a wait-blocked signal is detected.
///
/// Returns `Ok(None)` when no signal is found.
pub fn scan_wait_blocked(
    dispatch_id: &str,
    cx: &AppContext,
) -> anyhow::Result<Option<&'static str>> {
    // GPUI main thread: direct flavour (channel flavour would deadlock).
    let tail = crate::ai::orchestration::terminal_tail::terminal_tail_with_cx(
        dispatch_id,
        TAIL_SCAN_MAX_LINES,
        TAIL_SCAN_MAX_BYTES,
        cx,
    )
    .ok_or_else(|| anyhow::anyhow!("no terminal view registered for dispatch {dispatch_id}"))?;
    Ok(find_wait_blocked_signal(&tail).map(|r| r.as_str()))
}
/// - `enter` — append `\r` after a 500 ms delay
/// - `interrupt` — append `\x03` (Ctrl-C) instead of `\r`
pub fn answer(
    dispatch_id: &str,
    text: Option<&str>,
    enter: bool,
    interrupt: bool,
) -> anyhow::Result<()> {
    let sender = super::global_pty_sender()
        .ok_or_else(|| anyhow::anyhow!("no global PTY sender available"))?;
    send_terminal_action(sender, dispatch_id, text, enter, interrupt)
        .map_err(|e| anyhow::anyhow!("{e}"))
}
