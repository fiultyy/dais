//! MessageType routing — dispatches messages to the appropriate handler.
//!
//! `worker_done` and `heartbeat` are fully wired to reconciliation. The
//! remaining 7 types are no-ops (`Ignored`) until later phases.

use super::db::{Message, OrchestrationResult};
use super::reconciliation::{self, ReconciliationResult};
use super::store::DieselOrchestrationStore;
use super::types::MessageType;

/// Route a message to its lifecycle handler based on `MessageType`.
///
/// Returns `Ignored` for message types that have no lifecycle side-effects
/// (status, dispatch, merge_ready, escalation, handoff, decision_gate, question).
/// These are stored for audit / UI but don't trigger state transitions.
pub fn route_message(
    store: &DieselOrchestrationStore,
    msg: &Message,
) -> OrchestrationResult<ReconciliationResult> {
    let msg_type = msg.typed()?;

    match msg_type {
        MessageType::WorkerDone => reconciliation::reconcile_worker_done(store, msg),
        MessageType::Heartbeat => reconciliation::reconcile_heartbeat(store, msg),

        // Later phases: routing for these types will be added as features land.
        MessageType::Status
        | MessageType::Dispatch
        | MessageType::MergeReady
        | MessageType::Escalation
        | MessageType::Handoff
        | MessageType::DecisionGate
        | MessageType::Question => Ok(ReconciliationResult::Ignored),
    }
}

/// Helper: build a `Message` struct for testing.
#[cfg(test)]
fn make_message(
    msg_type: &str,
    from_handle: &str,
    to_handle: &str,
    payload: Option<&str>,
) -> Message {
    Message {
        id: format!("msg_{}", msg_type),
        run_id: "run_test".into(),
        delivery_contract: "current_delivery".into(),
        from_handle: from_handle.into(),
        to_handle: to_handle.into(),
        subject: "test".into(),
        body: "".into(),
        message_type: msg_type.into(),
        priority: "normal".into(),
        thread_id: None,
        payload: payload.map(|s| s.to_string()),
        read: 0,
        sequence: 1,
        created_at: chrono::Utc::now().naive_utc(),
        delivered_at: None,
        sender_pane_key: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::orchestration::OrchestrationStore;
    use diesel::prelude::*;
    use persistence::schema::tasks;

    /// Setup a dispatched task for lifecycle message testing.
    fn setup_dispatched_task() -> (DieselOrchestrationStore, String, String, String) {
        let store = DieselOrchestrationStore::in_memory().unwrap();
        let run_id = store.create_run("routing test").unwrap();
        let task_id = store.create_task(&run_id, "do work", &[]).unwrap();
        store.promote_ready_tasks(&run_id).unwrap();
        let dispatch_id = store.create_dispatch(&run_id, &task_id, "{}").unwrap();
        store.mark_worker_dispatch_ready(&dispatch_id, None).unwrap();

        // Set task to dispatched (simulating coordinator step)
        {
            let mut conn = store.lock();
            diesel::update(tasks::table.filter(tasks::id.eq(&task_id)))
                .set(tasks::status.eq("dispatched"))
                .execute(&mut *conn)
                .unwrap();
        }

        (store, run_id, task_id, dispatch_id)
    }

    #[test]
    fn test_route_status_ignored() {
        let store = DieselOrchestrationStore::in_memory().unwrap();
        let msg = make_message("status", "a", "b", None);
        assert_eq!(
            route_message(&store, &msg).unwrap(),
            ReconciliationResult::Ignored
        );
    }

    #[test]
    fn test_route_worker_done_triggers_reconcile() {
        let (store, _run_id, task_id, dispatch_id) = setup_dispatched_task();

        let payload = serde_json::json!({
            "task_id": task_id.clone(),
            "dispatch_id": dispatch_id.clone(),
            "outcome": "succeeded",
        });
        let msg = make_message("worker_done", "worker_1", "coordinator", Some(&payload.to_string()));

        let result = route_message(&store, &msg).unwrap();
        match result {
            ReconciliationResult::Completed {
                task_id: ref tid,
                dispatch_id: ref did,
            } => {
                assert_eq!(tid, &task_id);
                assert_eq!(did, &dispatch_id);
            }
            other => panic!("expected Completed, got {:?}", other),
        }

        // Verify task completed in DB
        let task = store.get_task(&task_id).unwrap().unwrap();
        assert_eq!(task.status, "completed");
    }

    #[test]
    fn test_route_heartbeat_triggers_reconcile() {
        let (store, _run_id, _task_id, dispatch_id) = setup_dispatched_task();

        let payload = serde_json::json!({ "dispatch_id": dispatch_id });
        let msg = make_message("heartbeat", "worker_1", "coordinator", Some(&payload.to_string()));

        let result = route_message(&store, &msg).unwrap();
        assert!(matches!(
            result,
            ReconciliationResult::HeartbeatRecorded { .. }
        ));

        // Verify heartbeat was recorded
        let ctx = store
            .get_dispatch_context_by_id(&dispatch_id)
            .unwrap()
            .unwrap();
        assert!(ctx.last_heartbeat_at.is_some());
    }

    #[test]
    fn test_route_escalation_ignored() {
        let store = DieselOrchestrationStore::in_memory().unwrap();
        let msg = make_message("escalation", "worker", "coordinator", None);
        assert_eq!(
            route_message(&store, &msg).unwrap(),
            ReconciliationResult::Ignored
        );
    }

    #[test]
    fn test_route_decision_gate_ignored() {
        let store = DieselOrchestrationStore::in_memory().unwrap();
        let msg = make_message("decision_gate", "worker", "coordinator", None);
        assert_eq!(
            route_message(&store, &msg).unwrap(),
            ReconciliationResult::Ignored
        );
    }

    #[test]
    fn test_route_dispatch_ignored() {
        let store = DieselOrchestrationStore::in_memory().unwrap();
        let msg = make_message("dispatch", "coordinator", "worker", None);
        assert_eq!(
            route_message(&store, &msg).unwrap(),
            ReconciliationResult::Ignored
        );
    }

    #[test]
    fn test_route_handoff_ignored() {
        let store = DieselOrchestrationStore::in_memory().unwrap();
        let msg = make_message("handoff", "a", "b", None);
        assert_eq!(
            route_message(&store, &msg).unwrap(),
            ReconciliationResult::Ignored
        );
    }

    #[test]
    fn test_route_merge_ready_ignored() {
        let store = DieselOrchestrationStore::in_memory().unwrap();
        let msg = make_message("merge_ready", "a", "b", None);
        assert_eq!(
            route_message(&store, &msg).unwrap(),
            ReconciliationResult::Ignored
        );
    }

    #[test]
    fn test_route_question_ignored() {
        let store = DieselOrchestrationStore::in_memory().unwrap();
        let msg = make_message("question", "a", "b", None);
        assert_eq!(
            route_message(&store, &msg).unwrap(),
            ReconciliationResult::Ignored
        );
    }
}
