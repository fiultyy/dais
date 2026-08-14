//! Diesel model structs for the 6 core orchestration tables.
//!
//! Column order in each struct matches the `table!` macro order in
//! `crates/persistence/src/schema.rs`. Diesel `Queryable` maps by position.
//!
//! Enum columns are stored as `String` at the DB layer; typed accessors
//! (`typed_state`, `typed_status`, …) parse to the domain enums in `types.rs`.

use chrono::NaiveDateTime;
use diesel::prelude::*;

use super::types::*;

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum OrchestrationError {
    #[error("database error: {0}")]
    Database(#[from] diesel::result::Error),

    #[error("invalid enum value '{value}' for {context}")]
    InvalidEnum {
        context: &'static str,
        value: String,
    },

    #[error("not found: {0}")]
    NotFound(String),

    #[error("poisoned mutex")]
    PoisonedMutex,

    #[error("connection error: {0}")]
    Connection(String),

    #[error("task error: {0}")]
    Task(String),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("join error: {0}")]
    Join(String),
}

pub type OrchestrationResult<T> = Result<T, OrchestrationError>;

// ---------------------------------------------------------------------------
// Queryable models
// ---------------------------------------------------------------------------

/// `runs` table — one per orchestration objective.
#[derive(Debug, Clone, Queryable)]
pub struct Run {
    pub id: String,
    pub objective: String,
    pub home_database: String,
    pub coordinator_handle: Option<String>,
    pub coordinator_pane_key: Option<String>,
    pub consumer_generation: i32,
    pub legacy: i32,
    pub created_at: NaiveDateTime,
    pub updated_at: NaiveDateTime,
}

/// `messages` table — the orchestration message bus.
///
/// `sequence` is the AUTOINCREMENT PK (delivery order); `id` is the logical
/// message identifier (unique index).
#[derive(Debug, Clone, Queryable)]
pub struct Message {
    pub id: String,
    pub run_id: String,
    pub delivery_contract: String,
    pub from_handle: String,
    pub to_handle: String,
    pub subject: String,
    pub body: String,
    /// SQL column `type` (reserved word) — position 8 in the table.
    pub message_type: String,
    pub priority: String,
    pub thread_id: Option<String>,
    pub payload: Option<String>,
    pub read: i32,
    pub sequence: i32,
    pub created_at: NaiveDateTime,
    pub delivered_at: Option<NaiveDateTime>,
    pub sender_pane_key: Option<String>,
}

impl Message {
    pub fn typed(&self) -> OrchestrationResult<MessageType> {
        self.message_type
            .parse()
            .map_err(|_| OrchestrationError::InvalidEnum {
                context: "Message.type",
                value: self.message_type.clone(),
            })
    }
}

/// `deliveries` table — crash-safe delivery batches (one outstanding per run).
///
/// **Reserved, zero live methods**: the local plane keeps flight state in
/// memory (`delivery::DispatchPushState`) plus the `messages.delivered_at`
/// watermark, so nothing reads this table yet. It is the porting target for
/// Orca's federation relay ack/fencing (consumer_generation + the
/// one-outstanding-per-run unique index) — do not repurpose or drop it.
#[derive(Debug, Clone, Queryable)]
pub struct Delivery {
    pub id: String,
    pub run_id: String,
    pub consumer_generation: i32,
    pub message_ids: String,
    pub status: String,
    pub created_at: NaiveDateTime,
    pub acknowledged_at: Option<NaiveDateTime>,
}

/// `worker_dispatches` table — the 9-state worker state machine.
#[derive(Debug, Clone, Queryable)]
pub struct WorkerDispatch {
    pub dispatch_id: String,
    pub runtime_epoch: Option<String>,
    pub state: String,
    pub stage: String,
    pub worktree_id: Option<String>,
    pub agent_terminal_handle: Option<String>,
    pub setup_state: String,
    pub effects: String,
    pub residual_resources: String,
    pub start_options: String,
    pub last_error: Option<String>,
    pub created_at: NaiveDateTime,
    pub updated_at: NaiveDateTime,
}

impl WorkerDispatch {
    pub fn typed_state(&self) -> OrchestrationResult<WorkerDispatchState> {
        self.state
            .parse()
            .map_err(|_| OrchestrationError::InvalidEnum {
                context: "WorkerDispatch.state",
                value: self.state.clone(),
            })
    }
}

/// `tasks` table — the task DAG (spec + status + deps JSON).
#[derive(Debug, Clone, Queryable)]
pub struct Task {
    pub id: String,
    pub run_id: String,
    pub parent_id: Option<String>,
    pub created_by_terminal_handle: Option<String>,
    pub created_by_pane_key: Option<String>,
    pub created_by_process_incarnation: Option<String>,
    pub created_by_run_generation: Option<i32>,
    pub task_title: Option<String>,
    pub display_name: Option<String>,
    pub spec: String,
    pub status: String,
    pub deps: String,
    pub result: Option<String>,
    pub created_at: NaiveDateTime,
    pub completed_at: Option<NaiveDateTime>,
}

impl Task {
    pub fn typed_status(&self) -> OrchestrationResult<TaskStatus> {
        self.status
            .parse()
            .map_err(|_| OrchestrationError::InvalidEnum {
                context: "Task.status",
                value: self.status.clone(),
            })
    }
}

/// `dispatch_contexts` table — capability-scoped dispatch with circuit breaker.
#[derive(Debug, Clone, Queryable)]
pub struct DispatchContext {
    pub id: String,
    pub run_id: String,
    pub task_id: String,
    pub contract_version: i32,
    pub launch_token_hash: Option<String>,
    pub assignee_handle: Option<String>,
    pub assignee_pane_key: Option<String>,
    pub capability_hash: Option<String>,
    pub process_incarnation: Option<String>,
    pub capability_revoked_at: Option<NaiveDateTime>,
    pub status: String,
    pub failure_count: i32,
    pub last_failure: Option<String>,
    pub dispatched_at: Option<NaiveDateTime>,
    pub completed_at: Option<NaiveDateTime>,
    pub created_at: NaiveDateTime,
    pub last_heartbeat_at: Option<NaiveDateTime>,
}

impl DispatchContext {
    pub fn typed_status(&self) -> OrchestrationResult<DispatchStatus> {
        self.status
            .parse()
            .map_err(|_| OrchestrationError::InvalidEnum {
                context: "DispatchContext.status",
                value: self.status.clone(),
            })
    }

    /// True when `failure_count` has reached the circuit-breaker threshold.
    pub fn is_circuit_broken(&self) -> bool {
        self.failure_count >= CIRCUIT_BREAKER_FAILURE_THRESHOLD
    }
}

/// `decision_gates` table — 3-state gate (pending/resolved/timeout).
#[derive(Debug, Clone, Queryable)]
pub struct DecisionGate {
    pub id: String,
    pub run_id: String,
    pub task_id: String,
    pub question: String,
    pub options: String,
    pub status: String,
    pub resolution: Option<String>,
    pub created_at: NaiveDateTime,
    pub resolved_at: Option<NaiveDateTime>,
}

impl DecisionGate {
    pub fn typed_status(&self) -> OrchestrationResult<GateStatus> {
        self.status
            .parse()
            .map_err(|_| OrchestrationError::InvalidEnum {
                context: "DecisionGate.status",
                value: self.status.clone(),
            })
    }
}
