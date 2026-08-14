//! Worker 9-state machine logic.
//!
//! P1: state-transition validation on write, `worker_done` message settlement,
//! circuit-breaker integration, DCS-hook → state detection mapping.

use super::types::WorkerDispatchState;

/// Legal transitions for the worker state machine.
///
/// ```text
/// starting ──→ ready ──→ succeeded ──→ stopping ──→ stopped ──→ abandoned
///    │  │        │ ╲         │ ╲           │           │
///    ↓  ↓        ↓  ╲        ↓  ╲          ↓           ↓
/// start_unknown  failed  stop_unknown  (terminal)  (terminal)
///    │           ↑
///    └→ starting (retry)
/// ```
#[rustfmt::skip]
pub fn is_valid_transition(from: WorkerDispatchState, to: WorkerDispatchState) -> bool {
    use WorkerDispatchState as S;
    match from {
        // Bootstrap phase — starting can abort to stopping (timeout/cancel)
        S::Starting      => matches!(to, S::Ready | S::StartUnknown | S::Failed | S::Stopping),
        S::StartUnknown  => matches!(to, S::Ready | S::Starting | S::Failed),

        // Active work phase — ready can report done or be stopped (cancel)
        S::Ready         => matches!(to, S::Succeeded | S::Failed | S::StartUnknown | S::Stopping),

        // Teardown phase
        S::Succeeded | S::Failed => matches!(to, S::Stopping | S::StopUnknown),
        S::Stopping      => matches!(to, S::Stopped | S::StopUnknown),
        S::StopUnknown   => matches!(to, S::Stopped | S::Stopping | S::Failed),

        // Terminal states
        S::Stopped       => matches!(to, S::Abandoned),
        S::Abandoned     => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bootstrap_path() {
        assert!(is_valid_transition(WorkerDispatchState::Starting, WorkerDispatchState::Ready));
        assert!(is_valid_transition(WorkerDispatchState::Starting, WorkerDispatchState::StartUnknown));
        assert!(is_valid_transition(WorkerDispatchState::Starting, WorkerDispatchState::Failed));
        // Starting can abort to stopping (timeout/cancel).
        assert!(is_valid_transition(WorkerDispatchState::Starting, WorkerDispatchState::Stopping));
    }

    #[test]
    fn work_to_teardown() {
        assert!(is_valid_transition(WorkerDispatchState::Ready, WorkerDispatchState::Succeeded));
        assert!(is_valid_transition(WorkerDispatchState::Ready, WorkerDispatchState::Failed));
        assert!(is_valid_transition(WorkerDispatchState::Succeeded, WorkerDispatchState::Stopping));
        assert!(is_valid_transition(WorkerDispatchState::Failed, WorkerDispatchState::Stopping));
        // Ready can be stopped (coordinator cancel).
        assert!(is_valid_transition(WorkerDispatchState::Ready, WorkerDispatchState::Stopping));
    }

    #[test]
    fn terminal_is_sink() {
        assert!(!is_valid_transition(WorkerDispatchState::Abandoned, WorkerDispatchState::Starting));
        assert!(is_valid_transition(WorkerDispatchState::Stopped, WorkerDispatchState::Abandoned));
    }

    #[test]
    fn no_skip_phases() {
        // Cannot jump from starting directly to succeeded
        assert!(!is_valid_transition(WorkerDispatchState::Starting, WorkerDispatchState::Succeeded));
        // Cannot go from ready to stopped (must go through stopping first)
        assert!(!is_valid_transition(WorkerDispatchState::Ready, WorkerDispatchState::Stopped));
    }

    #[test]
    fn stop_unknown_can_fail() {
        // Process confirmed dead during stop → failed.
        assert!(is_valid_transition(WorkerDispatchState::StopUnknown, WorkerDispatchState::Failed));
    }
}
