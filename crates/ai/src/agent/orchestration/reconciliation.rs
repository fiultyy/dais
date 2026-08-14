//! Message consumption — drives state transitions from `worker_done` and
//! `heartbeat` messages.
//!
//! Ported from Orca `lifecycle-reconciliation.ts`.
//! The caller-driven model replaces Orca's coordinator poll loop: callers
//! invoke `reconcile_*` directly after draining the inbox.

use super::db::{Message, OrchestrationResult};
use super::store::DieselOrchestrationStore;
use super::types::*;

// ---------------------------------------------------------------------------
// Result type
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReconciliationResult {
    /// No lifecycle action needed (e.g., status / dispatch messages).
    Ignored,

    /// Message consumed but suppressed (e.g., stale heartbeat).
    Suppressed,

    /// Message rejected with a structured code.
    Rejected {
        code: String,
        reason: String,
    },

    /// Task completed successfully.
    Completed {
        task_id: String,
        dispatch_id: String,
    },

    /// Task failed.
    Failed {
        task_id: String,
        dispatch_id: String,
    },

    /// Heartbeat recorded for a dispatch.
    HeartbeatRecorded {
        dispatch_id: String,
    },
}

// ---------------------------------------------------------------------------
// Reconciliation functions
// ---------------------------------------------------------------------------

/// Reconcile a `worker_done` message — the core lifecycle driver.
///
/// Ported from Orca `reconcileWorkerDoneMessage`. Parses the JSON payload,
/// validates task/dispatch references, and calls `settle_worker_report`.
pub fn reconcile_worker_done(
    store: &DieselOrchestrationStore,
    msg: &Message,
) -> OrchestrationResult<ReconciliationResult> {
    // Parse payload
    let payload: WorkerDonePayload = match &msg.payload {
        Some(p) if !p.is_empty() => serde_json::from_str(p)?,
        _ => {
            return Ok(ReconciliationResult::Rejected {
                code: "invalid_payload".into(),
                reason: "worker_done requires a JSON object payload.".into(),
            })
        }
    };

    // Validate fields
    if payload.task_id.is_empty() {
        return Ok(ReconciliationResult::Rejected {
            code: "missing_task_id".into(),
            reason: "worker_done requires taskId.".into(),
        });
    }

    if payload.dispatch_id.is_empty() {
        return Ok(ReconciliationResult::Rejected {
            code: "missing_dispatch_id".into(),
            reason: "worker_done requires dispatchId.".into(),
        });
    }

    let outcome = match payload.outcome.as_str() {
        "succeeded" => WorkerReportOutcome::Succeeded,
        "failed" => WorkerReportOutcome::Failed,
        _ => {
            return Ok(ReconciliationResult::Rejected {
                code: "invalid_outcome".into(),
                reason: "worker_done requires outcome=succeeded or outcome=failed.".into(),
            })
        }
    };

    // Build result JSON (provenance metadata for audit)
    let result = serde_json::json!({
        "provenance": "worker_report",
        "outcome": payload.outcome,
        "messageId": msg.id,
        "reportedBy": msg.from_handle,
        "subject": msg.subject,
        "body": msg.body,
        "completedBy": msg.from_handle,
    })
    .to_string();

    // Settle
    let settlement =
        store.settle_worker_report(&payload.task_id, &payload.dispatch_id, outcome, &result)?;

    match settlement {
        WorkerReportSettlement::Settled { outcome, .. } => {
            if outcome == WorkerReportOutcome::Succeeded {
                Ok(ReconciliationResult::Completed {
                    task_id: payload.task_id,
                    dispatch_id: payload.dispatch_id,
                })
            } else {
                Ok(ReconciliationResult::Failed {
                    task_id: payload.task_id,
                    dispatch_id: payload.dispatch_id,
                })
            }
        }
        WorkerReportSettlement::Rejected { code, reason } => Ok(ReconciliationResult::Rejected {
            code: code.to_string(),
            reason,
        }),
    }
}

/// Reconcile a `heartbeat` message — records liveness for a dispatched context.
///
/// Ported from Orca `reconcileHeartbeatMessage`.
pub fn reconcile_heartbeat(
    store: &DieselOrchestrationStore,
    msg: &Message,
) -> OrchestrationResult<ReconciliationResult> {
    let payload: HeartbeatPayload = match &msg.payload {
        Some(p) if !p.is_empty() => serde_json::from_str(p)?,
        _ => {
            return Ok(ReconciliationResult::Rejected {
                code: "invalid_payload".into(),
                reason: "heartbeat requires a JSON object payload.".into(),
            })
        }
    };

    if payload.dispatch_id.is_empty() {
        return Ok(ReconciliationResult::Rejected {
            code: "missing_dispatch_id".into(),
            reason: "heartbeat requires dispatchId.".into(),
        });
    }

    store.record_heartbeat(&payload.dispatch_id)?;

    Ok(ReconciliationResult::HeartbeatRecorded {
        dispatch_id: payload.dispatch_id,
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::orchestration::OrchestrationStore;
    use diesel::prelude::*;
    use persistence::schema::tasks;
    fn setup_with_dispatched_task() -> (DieselOrchestrationStore, String, String) {
        let store = DieselOrchestrationStore::in_memory().unwrap();
        let run_id = store.create_run("recon test").unwrap();
        let task_id = store.create_task(&run_id, "do work", &[]).unwrap();
        store.promote_ready_tasks(&run_id).unwrap();
        let dispatch_id = store.create_dispatch(&run_id, &task_id, "{}").unwrap();
        store.mark_worker_dispatch_ready(&dispatch_id, None).unwrap();

        // Set task to dispatched (Orca coordinator does this; we simulate)
        {
            let mut conn = store.lock();
            diesel::update(tasks::table.filter(tasks::id.eq(&task_id)))
                .set(tasks::status.eq("dispatched"))
                .execute(&mut *conn)
                .unwrap();
        }

        (store, task_id, dispatch_id)
    }

    #[test]
    fn test_reconcile_worker_done_succeeded() {
        let (store, task_id, dispatch_id) = setup_with_dispatched_task();

        let payload = serde_json::json!({
            "task_id": task_id,
            "dispatch_id": dispatch_id,
            "outcome": "succeeded",
        });

        let msg = Message {
            id: "msg_1".into(),
            run_id: "run_test".into(),
            delivery_contract: "current_delivery".into(),
            from_handle: "worker_1".into(),
            to_handle: "coordinator".into(),
            subject: "done".into(),
            body: "task finished".into(),
            message_type: "worker_done".into(),
            priority: "normal".into(),
            thread_id: None,
            payload: Some(payload.to_string()),
            read: 0,
            sequence: 1,
            created_at: chrono::Utc::now().naive_utc(),
            delivered_at: None,
            sender_pane_key: None,
        };

        let result = reconcile_worker_done(&store, &msg).unwrap();
        assert_eq!(
            result,
            ReconciliationResult::Completed {
                task_id,
                dispatch_id
            }
        );

        // Verify DB state
        let task = store.get_task(&msg_payload_task_id(&msg)).unwrap().unwrap();
        assert_eq!(task.status, "completed");
    }

    fn msg_payload_task_id(msg: &Message) -> String {
        let payload: WorkerDonePayload = serde_json::from_str(msg.payload.as_deref().unwrap()).unwrap();
        payload.task_id
    }

    #[test]
    fn test_reconcile_worker_done_missing_payload() {
        let store = DieselOrchestrationStore::in_memory().unwrap();

        let msg = Message {
            id: "msg_1".into(),
            run_id: "run_test".into(),
            delivery_contract: "current_delivery".into(),
            from_handle: "worker_1".into(),
            to_handle: "coordinator".into(),
            subject: "done".into(),
            body: "".into(),
            message_type: "worker_done".into(),
            priority: "normal".into(),
            thread_id: None,
            payload: None,
            read: 0,
            sequence: 1,
            created_at: chrono::Utc::now().naive_utc(),
            delivered_at: None,
            sender_pane_key: None,
        };

        let result = reconcile_worker_done(&store, &msg).unwrap();
        assert!(matches!(
            result,
            ReconciliationResult::Rejected { code, .. } if code == "invalid_payload"
        ));
    }

    #[test]
    fn test_reconcile_worker_done_invalid_outcome() {
        let store = DieselOrchestrationStore::in_memory().unwrap();

        let payload = serde_json::json!({
            "task_id": "t1",
            "dispatch_id": "d1",
            "outcome": "maybe",
        });

        let msg = Message {
            id: "msg_1".into(),
            run_id: "run_test".into(),
            delivery_contract: "current_delivery".into(),
            from_handle: "w1".into(),
            to_handle: "c1".into(),
            subject: "done".into(),
            body: "".into(),
            message_type: "worker_done".into(),
            priority: "normal".into(),
            thread_id: None,
            payload: Some(payload.to_string()),
            read: 0,
            sequence: 1,
            created_at: chrono::Utc::now().naive_utc(),
            delivered_at: None,
            sender_pane_key: None,
        };

        let result = reconcile_worker_done(&store, &msg).unwrap();
        assert!(matches!(
            result,
            ReconciliationResult::Rejected { code, .. } if code == "invalid_outcome"
        ));
    }

    #[test]
    fn test_reconcile_heartbeat() {
        let (store, _task_id, dispatch_id) = setup_with_dispatched_task();

        let payload = serde_json::json!({ "dispatch_id": dispatch_id });

        let msg = Message {
            id: "msg_hb".into(),
            run_id: "run_test".into(),
            delivery_contract: "current_delivery".into(),
            from_handle: "w1".into(),
            to_handle: "c1".into(),
            subject: "alive".into(),
            body: "".into(),
            message_type: "heartbeat".into(),
            priority: "normal".into(),
            thread_id: None,
            payload: Some(payload.to_string()),
            read: 0,
            sequence: 2,
            created_at: chrono::Utc::now().naive_utc(),
            delivered_at: None,
            sender_pane_key: None,
        };

        let result = reconcile_heartbeat(&store, &msg).unwrap();
        assert!(matches!(
            result,
            ReconciliationResult::HeartbeatRecorded { .. }
        ));

        // Verify heartbeat recorded
        let ctx = store.get_dispatch_context_by_id(&dispatch_id).unwrap().unwrap();
        assert!(ctx.last_heartbeat_at.is_some());
    }
}
