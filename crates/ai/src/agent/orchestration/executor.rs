//! Adaptation traits — bridge orchestration to Warp's PTY and DCS hook systems.
//!
//! These traits decouple the orchestration plane from concrete Warp implementations.
//! Production wiring (future P4+):
//! - `PtyExecutor` → `pty_controller::write_bytes()` via proto streaming
//!   (`WriteToLongRunningShellCommand`)
//! - `WorkerStatusDetector` → Warp DCS hook callbacks (13+1 variants)
//!
//! Tests use `MockPtyExecutor` / `MockWorkerStatusDetector`.

use super::types::WorkerDispatchState;
use super::db::OrchestrationResult;

// ---------------------------------------------------------------------------
// PtyExecutor — command injection into worker terminals
// ---------------------------------------------------------------------------

/// Writes raw bytes into a terminal's PTY.
///
/// Adaptation layer for Orca's paste-bytes mechanism. In Warp, this maps to
/// `WriteToLongRunningShellCommand` proto streaming, which calls
/// `pty_controller.write_bytes(block_id, input)`.
///
/// The `handle` identifies the target terminal (Warp pane handle).
pub trait PtyExecutor: Send + Sync {
    /// Write `bytes` to the PTY of terminal `handle`.
    fn write_to_pty(&self, handle: &str, bytes: &[u8]) -> OrchestrationResult<()>;

    /// Convenience: write a UTF-8 string + newline (line mode).
    fn write_command(&self, handle: &str, command: &str) -> OrchestrationResult<()> {
        let mut bytes = command.as_bytes().to_vec();
        bytes.push(b'\n');
        self.write_to_pty(handle, &bytes)
    }
}

// ---------------------------------------------------------------------------
// WorkerStatusDetector — DCS hook → state mapping
// ---------------------------------------------------------------------------

/// DCS hook events that drive worker state transitions.
///
/// These correspond to Warp's DCS hook variants. The orchestration plane
/// maps each hook to a worker state transition:
///
/// | DCS Hook | Worker State | Notes |
/// |---|---|---|
/// | `Bootstrapped` | `Starting` | Shell ready, worker not yet ready |
/// | `Precmd` | `Ready` (idle) | Waiting for next command |
/// | `CommandFinished(exit=0)` | `Succeeded` | Task completed successfully |
/// | `CommandFinished(exit≠0)` | `Failed` | Task failed |
/// | `PromptStarted` | `Starting` | New command being typed |
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DcsHookEvent {
    /// Shell bootstrapped — worker process is starting.
    Bootstrapped {
        shell_path: Option<String>,
    },
    /// Shell is at a prompt, waiting for input (idle).
    Precmd,
    /// A command finished with an exit code.
    CommandFinished {
        exit_code: i32,
    },
    /// A new prompt started (user/agent began typing a command).
    PromptStarted,
}

impl DcsHookEvent {
    /// Map a DCS hook event to the target worker dispatch state.
    ///
    /// Returns `None` for events that don't trigger a state transition
    /// (e.g., `PromptStarted` when already in `Ready`).
    pub fn target_state(&self) -> Option<WorkerDispatchState> {
        match self {
            DcsHookEvent::Bootstrapped { .. } => Some(WorkerDispatchState::Starting),
            DcsHookEvent::Precmd => Some(WorkerDispatchState::Ready),
            DcsHookEvent::CommandFinished { exit_code } => {
                if *exit_code == 0 {
                    Some(WorkerDispatchState::Succeeded)
                } else {
                    Some(WorkerDispatchState::Failed)
                }
            }
            DcsHookEvent::PromptStarted => None,
        }
    }
}

/// Receives DCS hook events and drives worker state transitions.
///
/// The production implementation will subscribe to Warp's shell event bus
/// and forward relevant hooks. This trait allows the orchestration plane to
/// remain agnostic of the event delivery mechanism.
pub trait WorkerStatusDetector: Send + Sync {
    /// Process a DCS hook event for a specific dispatch.
    ///
    /// Returns the new worker state if a transition occurred, or `None` if
    /// the event was ignored (invalid transition, wrong dispatch, etc.).
    fn on_dcs_hook(
        &self,
        dispatch_id: &str,
        event: &DcsHookEvent,
    ) -> OrchestrationResult<Option<WorkerDispatchState>>;
}

// ---------------------------------------------------------------------------
// Mock implementations (for testing)
// ---------------------------------------------------------------------------

/// Records all writes for assertion in tests.
#[derive(Debug, Default)]
pub struct MockPtyExecutor {
    pub writes: parking_lot::Mutex<Vec<(String, Vec<u8>)>>,
}

impl MockPtyExecutor {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn writes_snapshot(&self) -> Vec<(String, Vec<u8>)> {
        self.writes.lock().clone()
    }
}

impl PtyExecutor for MockPtyExecutor {
    fn write_to_pty(&self, handle: &str, bytes: &[u8]) -> OrchestrationResult<()> {
        self.writes
            .lock()
            .push((handle.to_string(), bytes.to_vec()));
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dcs_hook_bootstrapped_to_starting() {
        let event = DcsHookEvent::Bootstrapped {
            shell_path: Some("/usr/bin/zsh".into()),
        };
        assert_eq!(
            event.target_state(),
            Some(WorkerDispatchState::Starting)
        );
    }

    #[test]
    fn test_dcs_hook_precmd_to_ready() {
        let event = DcsHookEvent::Precmd;
        assert_eq!(event.target_state(), Some(WorkerDispatchState::Ready));
    }

    #[test]
    fn test_dcs_hook_command_finished_success() {
        let event = DcsHookEvent::CommandFinished { exit_code: 0 };
        assert_eq!(
            event.target_state(),
            Some(WorkerDispatchState::Succeeded)
        );
    }

    #[test]
    fn test_dcs_hook_command_finished_failure() {
        let event = DcsHookEvent::CommandFinished { exit_code: 1 };
        assert_eq!(event.target_state(), Some(WorkerDispatchState::Failed));
    }

    #[test]
    fn test_dcs_hook_prompt_started_no_transition() {
        let event = DcsHookEvent::PromptStarted;
        assert_eq!(event.target_state(), None);
    }

    #[test]
    fn test_mock_pty_executor_records_writes() {
        let executor = MockPtyExecutor::new();
        executor.write_command("term_1", "echo hello").unwrap();

        let writes = executor.writes_snapshot();
        assert_eq!(writes.len(), 1);
        assert_eq!(writes[0].0, "term_1");
        assert_eq!(
            String::from_utf8_lossy(&writes[0].1),
            "echo hello\n"
        );
    }
}
