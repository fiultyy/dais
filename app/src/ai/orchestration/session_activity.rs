//! Session activity registry — process-wide timestamps of shell lifecycle
//! events per session, consumed by the idle-detector signals probe.
//!
//! ## Why a side table
//!
//! The idle probe (`terminal_tail::extract_signals`) reads a
//! `TerminalView`, but OSC 133 timing (precmd / preexec / command-finished)
//! is only observable from the `ShellEventBridge`'s model-event
//! subscription. The view itself carries no `Instant` for these events, so
//! the bridge publishes timestamps here, keyed by `SessionId`, and the
//! probe joins them in.
//!
//! ## Writers / readers
//!
//! - Writers: `ShellEventBridge` subscription (GPUI main thread).
//! - Readers: `terminal_tail::extract_signals` (GPUI main thread).
//!
//! All access is behind a `parking_lot::Mutex`; entries are removed on
//! `ExitShell` so a recycled session id never sees stale timings.

use std::collections::HashMap;
use std::sync::OnceLock;
use std::time::Instant;

use crate::terminal::model::session::SessionId;

#[derive(Debug, Clone, Default)]
struct SessionActivity {
    /// Last OSC 133 Precmd (prompt shown after a command finished).
    last_precmd: Option<Instant>,
    /// Last shell lifecycle event of any kind (precmd / preexec / finished).
    last_event: Option<Instant>,
}

fn registry() -> &'static parking_lot::Mutex<HashMap<u64, SessionActivity>> {
    static REGISTRY: OnceLock<parking_lot::Mutex<HashMap<u64, SessionActivity>>> = OnceLock::new();
    REGISTRY.get_or_init(|| parking_lot::Mutex::new(HashMap::new()))
}

/// Record a Precmd (prompt at rest) for `session_id`.
pub fn record_precmd(session_id: SessionId) {
    let now = Instant::now();
    let mut reg = registry().lock();
    let entry = reg.entry(session_id.as_u64()).or_default();
    entry.last_precmd = Some(now);
    entry.last_event = Some(now);
}

/// Record a shell lifecycle event (preexec / command finished) for `session_id`.
pub fn record_event(session_id: SessionId) {
    let now = Instant::now();
    let mut reg = registry().lock();
    reg.entry(session_id.as_u64()).or_default().last_event = Some(now);
}

/// Drop all timing state for `session_id` (shell exit).
pub fn remove(session_id: SessionId) {
    registry().lock().remove(&session_id.as_u64());
}

/// Idle-timing signals for a session.
#[derive(Debug, Clone, Copy, Default)]
pub struct TimingSignals {
    /// Milliseconds since the last Precmd, if one was ever observed.
    pub since_last_precmd_ms: Option<u64>,
    /// Milliseconds since the last shell event of any kind, if observed.
    pub silent_for_ms: Option<u64>,
}

/// Read the timing signals for `session_id`.
///
/// Returns `None`-populated signals when the session has no record (the
/// idle detector treats that as `Unknown`).
pub fn signals_for(session_id: SessionId) -> TimingSignals {
    let reg = registry().lock();
    match reg.get(&session_id.as_u64()) {
        Some(entry) => {
            let now = Instant::now();
            TimingSignals {
                since_last_precmd_ms: entry.last_precmd.map(|t| now.duration_since(t).as_millis() as u64),
                silent_for_ms: entry.last_event.map(|t| now.duration_since(t).as_millis() as u64),
            }
        }
        None => TimingSignals::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sid(n: u64) -> SessionId {
        n.into()
    }

    #[test]
    fn unknown_session_yields_default() {
        let s = signals_for(sid(999_001));
        assert_eq!(s.since_last_precmd_ms, None);
        assert_eq!(s.silent_for_ms, None);
    }

    #[test]
    fn precmd_populates_both_signals() {
        let id = sid(999_002);
        record_precmd(id);
        let s = signals_for(id);
        assert!(s.since_last_precmd_ms.is_some());
        assert!(s.silent_for_ms.is_some());
        remove(id);
    }

    #[test]
    fn event_only_populates_silence() {
        let id = sid(999_003);
        record_event(id);
        let s = signals_for(id);
        assert_eq!(s.since_last_precmd_ms, None);
        assert!(s.silent_for_ms.is_some());
        remove(id);
    }

    #[test]
    fn remove_clears_state() {
        let id = sid(999_004);
        record_precmd(id);
        remove(id);
        let s = signals_for(id);
        assert_eq!(s.since_last_precmd_ms, None);
        assert_eq!(s.silent_for_ms, None);
    }
}
