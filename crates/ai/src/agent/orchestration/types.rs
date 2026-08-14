//! Orchestration domain types — enums ported from Orca `types.ts`.
//!
//! String serializations match the SQLite CHECK constraint values exactly,
//! enabling round-trip storage via `strum` Display / EnumString / AsRefStr.

use strum_macros::{AsRefStr, Display, EnumString};

// ---------------------------------------------------------------------------
// Message bus
// ---------------------------------------------------------------------------

/// 9 message types driving the orchestration message bus.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Display, EnumString, AsRefStr)]
pub enum MessageType {
    #[strum(serialize = "status")]
    Status,
    #[strum(serialize = "dispatch")]
    Dispatch,
    #[strum(serialize = "worker_done")]
    WorkerDone,
    #[strum(serialize = "merge_ready")]
    MergeReady,
    #[strum(serialize = "escalation")]
    Escalation,
    #[strum(serialize = "handoff")]
    Handoff,
    #[strum(serialize = "decision_gate")]
    DecisionGate,
    #[strum(serialize = "question")]
    Question,
    #[strum(serialize = "heartbeat")]
    Heartbeat,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Display, EnumString, AsRefStr)]
pub enum MessagePriority {
    #[strum(serialize = "normal")]
    Normal,
    #[strum(serialize = "high")]
    High,
    #[strum(serialize = "urgent")]
    Urgent,
}

/// How a message participates in the delivery subsystem.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Display, EnumString, AsRefStr)]
pub enum MessageDeliveryContract {
    /// Pre-delivery-era: delivered directly at enqueue time (legacy path).
    #[strum(serialize = "legacy_direct")]
    LegacyDirect,
    /// Routed through the crash-safe `deliveries` batch table.
    #[strum(serialize = "current_delivery")]
    CurrentDelivery,
    /// Stored for audit only — never delivered.
    #[strum(serialize = "audit_only")]
    AuditOnly,
}

// ---------------------------------------------------------------------------
// Task DAG
// ---------------------------------------------------------------------------

/// Task lifecycle: pending → ready → dispatched → completed/failed/blocked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Display, EnumString, AsRefStr)]
pub enum TaskStatus {
    #[strum(serialize = "pending")]
    Pending,
    #[strum(serialize = "ready")]
    Ready,
    #[strum(serialize = "dispatched")]
    Dispatched,
    #[strum(serialize = "completed")]
    Completed,
    #[strum(serialize = "failed")]
    Failed,
    #[strum(serialize = "blocked")]
    Blocked,
}

// ---------------------------------------------------------------------------
// Dispatch
// ---------------------------------------------------------------------------

/// Dispatch context lifecycle: pending → dispatched → completed/failed/circuit_broken/unknown_dispatch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Display, EnumString, AsRefStr)]
pub enum DispatchStatus {
    #[strum(serialize = "pending")]
    Pending,
    #[strum(serialize = "dispatched")]
    Dispatched,
    #[strum(serialize = "completed")]
    Completed,
    #[strum(serialize = "failed")]
    Failed,
    #[strum(serialize = "circuit_broken")]
    CircuitBroken,
    #[strum(serialize = "unknown_dispatch")]
    UnknownDispatch,
}

// ---------------------------------------------------------------------------
// Worker state machine (9 states)
// ---------------------------------------------------------------------------

/// Worker 9-state machine:
/// ```text
/// starting → ready → (start_unknown) → succeeded/failed
///                        ↘ starting (retry)
/// succeeded/failed → stopping → (stop_unknown) → stopped → abandoned
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Display, EnumString, AsRefStr)]
pub enum WorkerDispatchState {
    #[strum(serialize = "starting")]
    Starting,
    #[strum(serialize = "ready")]
    Ready,
    #[strum(serialize = "start_unknown")]
    StartUnknown,
    #[strum(serialize = "failed")]
    Failed,
    #[strum(serialize = "succeeded")]
    Succeeded,
    #[strum(serialize = "stopping")]
    Stopping,
    #[strum(serialize = "stop_unknown")]
    StopUnknown,
    #[strum(serialize = "stopped")]
    Stopped,
    #[strum(serialize = "abandoned")]
    Abandoned,
}

// ---------------------------------------------------------------------------
// Worker report settlement
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Display, EnumString, AsRefStr)]
pub enum WorkerReportOutcome {
    #[strum(serialize = "succeeded")]
    Succeeded,
    #[strum(serialize = "failed")]
    Failed,
}

/// Outcome of settling a `worker_done` message — settled or rejected with a code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkerReportSettlement {
    Settled {
        outcome: WorkerReportOutcome,
        duplicate: bool,
    },
    Rejected {
        code: WorkerReportRejectionCode,
        reason: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Display, EnumString, AsRefStr)]
pub enum WorkerReportRejectionCode {
    #[strum(serialize = "unknown_task")]
    UnknownTask,
    #[strum(serialize = "unknown_dispatch")]
    UnknownDispatch,
    #[strum(serialize = "task_dispatch_mismatch")]
    TaskDispatchMismatch,
    #[strum(serialize = "inactive_dispatch")]
    InactiveDispatch,
    #[strum(serialize = "stale_dispatch")]
    StaleDispatch,
}

// ---------------------------------------------------------------------------
// Decision gates & questions
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Display, EnumString, AsRefStr)]
pub enum GateStatus {
    #[strum(serialize = "pending")]
    Pending,
    #[strum(serialize = "resolved")]
    Resolved,
    #[strum(serialize = "timeout")]
    Timeout,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Display, EnumString, AsRefStr)]
pub enum QuestionStatus {
    #[strum(serialize = "pending")]
    Pending,
    #[strum(serialize = "answered")]
    Answered,
    #[strum(serialize = "closed")]
    Closed,
}

// ---------------------------------------------------------------------------
// Delivery & coordinator (schema fidelity)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Display, EnumString, AsRefStr)]
pub enum DeliveryStatus {
    #[strum(serialize = "outstanding")]
    Outstanding,
    #[strum(serialize = "acknowledged")]
    Acknowledged,
    #[strum(serialize = "fenced")]
    Fenced,
}

/// Coordinator status — kept for schema fidelity; the autonomous loop is NOT ported
/// (caller-driven model replaces it).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Display, EnumString, AsRefStr)]
pub enum CoordinatorStatus {
    #[strum(serialize = "idle")]
    Idle,
    #[strum(serialize = "running")]
    Running,
    #[strum(serialize = "completed")]
    Completed,
    #[strum(serialize = "failed")]
    Failed,
}

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Circuit-breaker threshold: dispatch fails this many times → `circuit_broken`.
pub const CIRCUIT_BREAKER_FAILURE_THRESHOLD: i32 = 3;

/// Default run id for legacy / ad-hoc tasks (no explicit run).
pub const LEGACY_RUN_ID: &str = "run_legacy";

/// Contract version for dispatch contexts.
pub const CURRENT_CONTRACT_VERSION: i32 = 1;

// ---------------------------------------------------------------------------
// Message payload structs (for reconciliation deserialization)
// ---------------------------------------------------------------------------

/// Payload of a `worker_done` message — drives task settlement.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct WorkerDonePayload {
    pub task_id: String,
    pub dispatch_id: String,
    pub outcome: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
}

/// Payload of a `heartbeat` message — confirms dispatch liveness.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct HeartbeatPayload {
    pub dispatch_id: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dispatch_status_roundtrip() {
        let all = [
            DispatchStatus::Pending,
            DispatchStatus::Dispatched,
            DispatchStatus::Completed,
            DispatchStatus::Failed,
            DispatchStatus::CircuitBroken,
            DispatchStatus::UnknownDispatch,
        ];
        for variant in all {
            let s = variant.as_ref();
            let back: DispatchStatus = s.parse().expect("roundtrip parse");
            assert_eq!(variant, back, "roundtrip failed for {}", s);
        }
    }
}
