//! Multi-signal idle detector — classifies whether a terminal is idle, busy,
//! or in an unknown state by fusing multiple observable signals.
//!
//! ## Signal sources
//!
//! | Signal                | Source                           | Accessible from     |
//! |-----------------------|----------------------------------|---------------------|
//! | `title`               | OSC 0/1/2 terminal title        | `terminal_tail`     |
//! | `alt_screen_active`   | `TerminalModel.alt_screen_active`| `terminal_tail`     |
//! | `output_silent_for_ms`| Time since last rendered block   | `terminal_tail`     |
//! | `since_last_precmd_ms`| Time since last OSC 133 A marker | bridge (future)     |
//!
//! ## Classification rules
//!
//! 1. **Title-primary path**: title-based detection (`detect_agent_status_from_title`)
//!    is the first check. `Working` / `Permission` → `Busy`; `Idle` → `Idle`;
//!    `None` (no agent signal in title) falls through to multi-signal.
//!
//! 2. **Multi-signal fallback** (when title carries no agent signal):
//!    - `alt_screen_active` → `Busy` (TUI full-screen rendering).
//!    - `since_last_precmd_ms` in `[0, 60_000]` AND `output_silent_for_ms >= 800`
//!      → `Idle` (command finished + output settled).
//!    - All signals absent → `Unknown`.

use super::prompt_injection::{detect_agent_status_from_title, AgentTerminalStatus};

/// Collected idle-related signals from a terminal view.
#[derive(Debug, Clone, Default)]
pub struct IdleSignal {
    /// Terminal title (OSC 0/1/2).
    pub title: Option<String>,
    /// Whether the alternate screen buffer is active (TUI full-screen).
    pub alt_screen_active: bool,
    /// Milliseconds since the last rendered block completed (None if no blocks).
    pub output_silent_for_ms: Option<u64>,
    /// Milliseconds since the last OSC 133 Precmd marker (None if not tracked).
    /// Future: wired from ShellEventBridge.last_activity via SessionDispatchMap.
    pub since_last_precmd_ms: Option<u64>,
}

/// Verdict of the idle classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdleVerdict {
    /// Agent is idle and ready to receive messages.
    Idle,
    /// Agent is busy (working, TUI, etc.).
    Busy,
    /// Not enough signal to determine state.
    Unknown,
}

/// Classify terminal state from collected signals.
///
/// Merges title-based detection with multi-signal fallback:
///
/// 1. **Title-primary**: If the title contains a known agent status signal,
///    use it directly. `Working` / `Permission` → `Busy`; `Idle` → `Idle`.
///
/// 2. **Multi-signal fallback** (title has no agent signal):
///    - `alt_screen_active` → `Busy` (TUI rendering).
///    - `since_last_precmd_ms` in `[0, 60_000]` AND `output_silent_for_ms >= 800`
///      → `Idle`.
///    - Otherwise → `Unknown`.
pub fn classify_idle(sig: &IdleSignal) -> IdleVerdict {
    // Phase 1: title-primary detection.
    // A title that contains *any* agent signal (Working, Idle, Permission, etc.)
    // takes priority. If the title has NO agent signal (detect returns None),
    // we fall through to multi-signal.
    if let Some(title) = &sig.title {
        if let Some(status) = detect_agent_status_from_title(title) {
            return match status {
                AgentTerminalStatus::Idle => IdleVerdict::Idle,
                AgentTerminalStatus::Working | AgentTerminalStatus::Permission => IdleVerdict::Busy,
            };
        }
        // Title exists but carries no agent signal — fall through to multi-signal.
    }

    // Phase 2: multi-signal fallback.
    // alt_screen_active → Busy (TUI full-screen rendering).
    if sig.alt_screen_active {
        return IdleVerdict::Busy;
    }

    // Command completed recently AND output has settled → Idle.
    // since_last_precmd_ms in [0, 60_000]: a precmd was seen within the last
    // 60 seconds, meaning the shell is at a prompt after a command.
    // Silence threshold is split: a title exists (agent-attributed but no
    // status signal) → 800 ms; no title at all (bare shell) → 2 000 ms.
    // Rationale: a bare shell's inter-command gap is easily ≥ 800 ms; a
    // false Idle there injects a pointer line the shell tries to execute
    // ("command not found") — harmless but noisy, so demand more silence.
    let silence_threshold_ms: u64 = if sig.title.is_some() { 800 } else { 2_000 };
    if let Some(precmd_ms) = sig.since_last_precmd_ms {
        if precmd_ms <= 60_000 {
            if let Some(silent_ms) = sig.output_silent_for_ms {
                if silent_ms >= silence_threshold_ms {
                    return IdleVerdict::Idle;
                }
            }
        }
    }

    IdleVerdict::Unknown
}

/// Extract a minimal title-only `IdleSignal` — backward-compatible helper
/// for callers that only have a title string.
pub fn idle_signal_from_title(title: Option<String>) -> IdleSignal {
    IdleSignal {
        title,
        ..Default::default()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn idle_sig() -> IdleSignal {
        IdleSignal::default()
    }

    // -- Title-primary path --

    #[test]
    fn title_idle_gives_idle() {
        let mut sig = idle_sig();
        sig.title = Some("✳ claude idle".into());
        assert_eq!(classify_idle(&sig), IdleVerdict::Idle);
    }

    #[test]
    fn title_working_gives_busy() {
        let mut sig = idle_sig();
        sig.title = Some("claude working".into());
        assert_eq!(classify_idle(&sig), IdleVerdict::Busy);
    }

    #[test]
    fn title_permission_gives_busy() {
        let mut sig = idle_sig();
        sig.title = Some("claude permission".into());
        assert_eq!(classify_idle(&sig), IdleVerdict::Busy);
    }

    #[test]
    fn title_none_gives_unknown() {
        let sig = idle_sig(); // title = None
        assert_eq!(classify_idle(&sig), IdleVerdict::Unknown);
    }

    #[test]
    fn title_non_agent_gives_unknown() {
        let mut sig = idle_sig();
        sig.title = Some("vim file.txt".into());
        assert_eq!(classify_idle(&sig), IdleVerdict::Unknown);
    }

    // -- Multi-signal fallback --

    #[test]
    fn alt_screen_active_gives_busy() {
        let mut sig = idle_sig();
        sig.alt_screen_active = true;
        assert_eq!(classify_idle(&sig), IdleVerdict::Busy);
    }

    #[test]
    fn alt_screen_overrides_precmd_idle() {
        let mut sig = idle_sig();
        sig.alt_screen_active = true;
        sig.since_last_precmd_ms = Some(5000);
        sig.output_silent_for_ms = Some(2000);
        // alt_screen takes priority → Busy
        assert_eq!(classify_idle(&sig), IdleVerdict::Busy);
    }

    #[test]
    fn precmd_recent_and_output_silent_gives_idle() {
        let mut sig = idle_sig();
        sig.title = Some("claude code".into());
        sig.since_last_precmd_ms = Some(5000); // 5s ago
        sig.output_silent_for_ms = Some(1000);  // 1s silent
        assert_eq!(classify_idle(&sig), IdleVerdict::Idle);
    }

    #[test]
    fn precmd_recent_output_not_silent_gives_unknown() {
        let mut sig = idle_sig();
        sig.since_last_precmd_ms = Some(5000);
        sig.output_silent_for_ms = Some(200); // only 200ms — still rendering
        assert_eq!(classify_idle(&sig), IdleVerdict::Unknown);
    }

    #[test]
    fn precmd_too_old_gives_unknown() {
        let mut sig = idle_sig();
        sig.since_last_precmd_ms = Some(120_000); // 2 minutes ago — stale
        sig.output_silent_for_ms = Some(5000);
        assert_eq!(classify_idle(&sig), IdleVerdict::Unknown);
    }

    #[test]
    fn precmd_zero_with_silent_output_gives_idle() {
        let mut sig = idle_sig();
        sig.title = Some("claude code".into()); // agent-attributed title, no status signal
        sig.since_last_precmd_ms = Some(0); // just happened
        sig.output_silent_for_ms = Some(900);
        assert_eq!(classify_idle(&sig), IdleVerdict::Idle);
    }

    #[test]
    fn precmd_boundary_60k_with_silent_output_gives_idle() {
        let mut sig = idle_sig();
        sig.title = Some("claude code".into());
        sig.since_last_precmd_ms = Some(60_000); // exactly 60s
        sig.output_silent_for_ms = Some(800);      // exactly 800ms (titled fast threshold)
        assert_eq!(classify_idle(&sig), IdleVerdict::Idle);
    }

    #[test]
    fn precmd_60k_plus_one_gives_unknown() {
        let mut sig = idle_sig();
        sig.since_last_precmd_ms = Some(60_001); // just over 60s
        sig.output_silent_for_ms = Some(5000);
        assert_eq!(classify_idle(&sig), IdleVerdict::Unknown);
    }

    #[test]
    fn precmd_none_gives_unknown() {
        let mut sig = idle_sig();
        sig.since_last_precmd_ms = None;
        sig.output_silent_for_ms = Some(5000);
        assert_eq!(classify_idle(&sig), IdleVerdict::Unknown);
    }

    #[test]
    fn output_silent_none_gives_unknown() {
        let mut sig = idle_sig();
        sig.since_last_precmd_ms = Some(5000);
        sig.output_silent_for_ms = None;
        assert_eq!(classify_idle(&sig), IdleVerdict::Unknown);
    }

    // -- Title priority over multi-signal --

    #[test]
    fn title_busy_overrides_alt_screen_idle_signals() {
        let mut sig = idle_sig();
        sig.title = Some("claude working".into());
        sig.alt_screen_active = true;
        sig.since_last_precmd_ms = Some(5000);
        sig.output_silent_for_ms = Some(2000);
        // Title Working → Busy, regardless of other signals
        assert_eq!(classify_idle(&sig), IdleVerdict::Busy);
    }

    #[test]
    fn title_idle_overrides_alt_screen() {
        let mut sig = idle_sig();
        sig.title = Some("✳ claude idle".into());
        sig.alt_screen_active = true;
        // Title Idle → Idle, alt_screen is irrelevant
        assert_eq!(classify_idle(&sig), IdleVerdict::Idle);
    }

    // -- Combined: no signals --

    #[test]
    fn all_signals_absent_gives_unknown() {
        let sig = IdleSignal::default();
        assert_eq!(classify_idle(&sig), IdleVerdict::Unknown);
    }

    // -- Helper --

    #[test]
    fn idle_signal_from_title_builds_correctly() {
        let sig = idle_signal_from_title(Some("✳ claude idle".into()));
        assert_eq!(sig.title.as_deref(), Some("✳ claude idle"));
        assert!(!sig.alt_screen_active);
        assert!(sig.output_silent_for_ms.is_none());
        assert!(sig.since_last_precmd_ms.is_none());
    }
}
