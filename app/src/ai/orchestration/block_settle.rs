//! Block-driven settlement — settle a worker dispatch when a terminal block
//! completes with a command matching the dispatch's `start_options.command`.
//!
//! This decouples settlement from preamble detection: if a dispatch was created
//! with `start_options = {"command": "make test"}`, then when the shell finishes
//! a block whose user command matches `"make test"`, the dispatch is settled
//! automatically — no preamble parsing required.
//!
//! ## Matching rule
//!
//! The comparison is **prefix-free full-string match after whitespace trim**:
//! ```text
//! actual_command.trim() == expected_command.trim()
//! ```
//!
//! This works even when the shell wraps the command (e.g. `make test` from a
//! subshell prompt that already trimmed). For shell-wrapper prefixes the caller
//! can store the wrapper-inclusive form in `start_options.command`.

use ai::agent::orchestration::store::DieselOrchestrationStore;
use ai::agent::orchestration::{
    OrchestrationStore, WorkerReportOutcome, WorkerReportSettlement, MessageType,
};

/// Try to settle a dispatch from a completed block.
///
/// Returns `true` if settlement occurred, `false` if the block didn't match
/// or the dispatch had no `command` configured.
///
/// # Steps
/// 1. Read the dispatch context + worker dispatch to get `start_options`.
/// 2. Parse the `"command"` field from `start_options` JSON.
/// 3. Compare `command_text.trim() == expected_command.trim()`.
/// 4. Match → `settle_worker_report` + enqueue `worker_done` message.
/// 5. Return settlement outcome.
pub fn try_settle_from_block(
    dispatch_id: &str,
    command_text: &str,
    exit_code: i32,
    store: &DieselOrchestrationStore,
) -> bool {
    // 1. Load dispatch context + worker dispatch
    let ctx = match store.get_dispatch_context_by_id(dispatch_id) {
        Ok(Some(c)) => c,
        _ => return false,
    };

    let worker = match store.get_worker_dispatch(dispatch_id) {
        Ok(Some(w)) => w,
        _ => return false,
    };

    // 2. Parse expected command from start_options
    let start_options = worker.start_options.as_str();
    if start_options.is_empty() || start_options == "{}" {
        return false;
    }

    let expected_command = match parse_command_from_start_options(start_options) {
        Some(cmd) => cmd,
        None => return false,
    };

    // 3. Match: trimmed full-string equality
    if command_text.trim() != expected_command.trim() {
        return false;
    }

    // 4. Settle
    let task_id = &ctx.task_id;
    let outcome = if exit_code == 0 {
        WorkerReportOutcome::Succeeded
    } else {
        WorkerReportOutcome::Failed
    };

    let result_json = serde_json::json!({
        "task_id": task_id,
        "dispatch_id": dispatch_id,
        "outcome": if exit_code == 0 { "succeeded" } else { "failed" },
        "command": command_text,
        "exit_code": exit_code,
        "provenance": "block"
    })
    .to_string();

    let settled = match store.settle_worker_report(task_id, dispatch_id, outcome, &result_json) {
        Ok(WorkerReportSettlement::Settled { duplicate: false, .. }) => true,
        // Already settled, rejected, or error → no-op (D-18: log the reason
        // — a silent rejection here strands the task with no trace).
        other => {
            log::debug!(
                "orchestration: block_settle no-op for {dispatch_id} (cmd={command_text:?} \
                 vs expected={expected_command:?}, exit={exit_code}): {other:?}"
            );
            return false;
        }
    };

    // 5. Enqueue a worker_done message to keep the message bus semantically complete.
    // enqueue_message already mirrors the payload into the message row.
    if let Err(e) = store.enqueue_message(
        &ctx.run_id,
        &format!("worker_{}", dispatch_id),
        "orchestrator",
        MessageType::WorkerDone,
        "worker_done",
        &result_json,
    ) {
        log::warn!(
            "orchestration: block_settle failed to enqueue worker_done for {}: {e}",
            dispatch_id
        );
    }

    settled
}

/// Extract the `"command"` field from a `start_options` JSON string.
fn parse_command_from_start_options(start_options: &str) -> Option<String> {
    let val: serde_json::Value = serde_json::from_str(start_options).ok()?;
    val.get("command")?.as_str().map(|s| s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_command_from_start_options() {
        assert_eq!(
            parse_command_from_start_options(r#"{"command": "make test"}"#),
            Some("make test".to_string())
        );
        assert_eq!(
            parse_command_from_start_options(r#"{"command": "make test", "extra": true}"#),
            Some("make test".to_string())
        );
        // No command field
        assert_eq!(
            parse_command_from_start_options(r#"{"extra": true}"#),
            None
        );
        // Empty start_options
        assert_eq!(parse_command_from_start_options("{}"), None);
        assert_eq!(parse_command_from_start_options(""), None);
        // Command is not a string
        assert_eq!(
            parse_command_from_start_options(r#"{"command": 123}"#),
            None
        );
    }

    /// Helper: create a fully dispatched task with a command in start_options.
    /// Returns (store, run_id, task_id, dispatch_id).
    fn setup_dispatched_with_command(command: &str) -> (
        DieselOrchestrationStore,
        String,
        String,
        String,
    ) {
        let store = DieselOrchestrationStore::in_memory().unwrap();
        let run_id = store.create_run("block_settle test").unwrap();
        let task_id = store.create_task(&run_id, "do thing", &[]).unwrap();
        store.promote_ready_tasks(&run_id).unwrap();

        // Create dispatch with command in start_options
        let start_options = serde_json::json!({"command": command}).to_string();
        let dispatch_id = store
            .create_dispatch(&run_id, &task_id, &start_options)
            .unwrap();

        // Mark ready: sets task→dispatched, dispatch→dispatched
        store.mark_worker_dispatch_ready(&dispatch_id, None).unwrap();

        (store, run_id, task_id, dispatch_id)
    }

    #[test]
    fn test_block_settle_matching_command_success() {
        let (store, _run_id, task_id, dispatch_id) =
            setup_dispatched_with_command("make test");

        let settled = try_settle_from_block(&dispatch_id, "make test", 0, &store);
        assert!(settled, "should settle when command matches and exit_code=0");

        // Verify task completed
        let task = store.get_task(&task_id).unwrap().unwrap();
        assert_eq!(task.status, "completed");

        // Verify dispatch context completed
        let ctx = store.get_dispatch_context_by_id(&dispatch_id).unwrap().unwrap();
        assert_eq!(ctx.status, "completed");

        // Verify worker succeeded
        let worker = store.get_worker_dispatch(&dispatch_id).unwrap().unwrap();
        assert_eq!(worker.state, "succeeded");
    }

    #[test]
    fn test_block_settle_matching_command_trimmed() {
        let (store, _run_id, _task_id, dispatch_id) =
            setup_dispatched_with_command("make test");

        // Command with surrounding whitespace — trim before match
        let settled = try_settle_from_block(&dispatch_id, "  make test  ", 0, &store);
        assert!(settled, "should settle with trimmed command match");
    }

    #[test]
    fn test_block_settle_nonzero_exit_code_fails() {
        let (store, _run_id, task_id, dispatch_id) =
            setup_dispatched_with_command("make test");

        let settled = try_settle_from_block(&dispatch_id, "make test", 1, &store);
        assert!(settled, "should settle (as Failed) when exit_code != 0");

        let task = store.get_task(&task_id).unwrap().unwrap();
        assert_eq!(task.status, "failed");

        let worker = store.get_worker_dispatch(&dispatch_id).unwrap().unwrap();
        assert_eq!(worker.state, "failed");
    }

    #[test]
    fn test_block_settle_nonmatching_command_noop() {
        let (store, _run_id, task_id, dispatch_id) =
            setup_dispatched_with_command("make test");

        let settled = try_settle_from_block(&dispatch_id, "cargo build", 0, &store);
        assert!(!settled, "should NOT settle when command doesn't match");

        // Task still dispatched (unchanged)
        let task = store.get_task(&task_id).unwrap().unwrap();
        assert_eq!(task.status, "dispatched");
    }

    #[test]
    fn test_block_settle_no_command_in_start_options_noop() {
        let store = DieselOrchestrationStore::in_memory().unwrap();
        let run_id = store.create_run("no-cmd test").unwrap();
        let task_id = store.create_task(&run_id, "do thing", &[]).unwrap();
        store.promote_ready_tasks(&run_id).unwrap();
        let dispatch_id = store
            .create_dispatch(&run_id, &task_id, "{}")
            .unwrap();
        store.mark_worker_dispatch_ready(&dispatch_id, None).unwrap();

        let settled = try_settle_from_block(&dispatch_id, "make test", 0, &store);
        assert!(
            !settled,
            "should NOT settle when no command in start_options"
        );

        let task = store.get_task(&task_id).unwrap().unwrap();
        assert_eq!(task.status, "dispatched");
    }

    #[test]
    fn test_block_settle_unknown_dispatch_noop() {
        let store = DieselOrchestrationStore::in_memory().unwrap();

        let settled = try_settle_from_block("nonexistent", "make test", 0, &store);
        assert!(!settled, "should NOT settle for unknown dispatch");
    }

    #[test]
    fn test_block_settle_duplicate_settle_idempotent() {
        let (store, _run_id, _task_id, dispatch_id) =
            setup_dispatched_with_command("make test");

        // First settle
        let settled1 = try_settle_from_block(&dispatch_id, "make test", 0, &store);
        assert!(settled1);

        // Second settle → should return false (already settled, settle returns duplicate)
        let settled2 = try_settle_from_block(&dispatch_id, "make test", 0, &store);
        assert!(!settled2, "second settle should return false (duplicate)");
    }

    /// D-18 regression: without mark-ready (task `ready`, dispatch `pending`)
    /// the settlement is rejected with InactiveDispatch and the task strands
    /// in `ready` — start-worker's auto-bind now marks ready itself; this
    /// test pins the settle-side invariant it relies on.
    #[test]
    fn test_block_settle_without_mark_ready_rejected() {
        let store = DieselOrchestrationStore::in_memory().unwrap();
        let run_id = store.create_run("d-18 repro").unwrap();
        let task_id = store.create_task(&run_id, "do thing", &[]).unwrap();
        store.promote_ready_tasks(&run_id).unwrap();
        let start_options = serde_json::json!({"command": "echo X"}).to_string();
        let dispatch_id = store.create_dispatch(&run_id, &task_id, &start_options).unwrap();
        // NOTE: no mark_worker_dispatch_ready — the exact LB-002-B live shape.

        let settled = try_settle_from_block(&dispatch_id, "echo X", 0, &store);
        assert!(!settled, "settle must reject (task ready / dispatch pending)");

        // Task strands in ready — the documented D-18 symptom.
        let task = store.get_task(&task_id).unwrap().unwrap();
        assert_eq!(task.status, "ready");

        // After mark-ready (what start-worker's auto-bind now does), the
        // same block settles.
        store.mark_worker_dispatch_ready(&dispatch_id, None).unwrap();
        let settled = try_settle_from_block(&dispatch_id, "echo X", 0, &store);
        assert!(settled, "settle must succeed once dispatched");
        let task = store.get_task(&task_id).unwrap().unwrap();
        assert_eq!(task.status, "completed");
    }
}
