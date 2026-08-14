//! DieselOrchestrationStore — SQLite-backed `OrchestrationStore` implementation.
//!
//! All Diesel queries are synchronous (SQLite is in-process). Async callers use
//! the `*_async` convenience methods which wrap each call in
//! `tokio::task::spawn_blocking`.

#![allow(clippy::explicit_auto_deref)]
use std::str::FromStr;
use std::sync::Arc;

use parking_lot::Mutex;

use chrono::Utc;
use diesel::prelude::*;
use diesel::sqlite::SqliteConnection;
use diesel::connection::SimpleConnection;
use diesel::dsl::sql;
use diesel::sql_types::Integer;

use persistence::schema::{
    decision_gates, dispatch_contexts, messages, orchestration_waiters, runs, tasks,
    worker_dispatches, worker_terminal_archives,
};

use super::db::{
    DecisionGate, DispatchContext, Message, OrchestrationError, OrchestrationResult, Run, Task,
    WorkerDispatch,
};
use super::output::WorkerTerminalArchive;
use super::types::*;
use super::worker::is_valid_transition;
use super::OrchestrationStore;

// ─── ID generation ────────────────────────────────────────────────────────

fn generate_id(prefix: &str) -> String {
    let uuid = uuid::Uuid::new_v4().simple().to_string();
    format!("{}_{}", prefix, &uuid[..12])
}

// ─── Insertable structs (module-level — derive cannot be inside fn) ─────────

#[derive(Insertable)]
#[diesel(table_name = runs)]
struct NewRun<'a> {
    id: &'a str,
    objective: &'a str,
}

#[derive(Insertable)]
#[diesel(table_name = tasks)]
struct NewTask<'a> {
    id: &'a str,
    run_id: &'a str,
    spec: &'a str,
    deps: &'a str,
}

#[derive(Insertable)]
#[diesel(table_name = dispatch_contexts)]
struct NewDispatchContext<'a> {
    id: &'a str,
    run_id: &'a str,
    task_id: &'a str,
}

#[derive(Insertable)]
#[diesel(table_name = worker_dispatches)]
struct NewWorkerDispatch<'a> {
    dispatch_id: &'a str,
}

#[derive(Insertable)]
#[diesel(table_name = messages)]
struct NewMessage<'a> {
    id: &'a str,
    run_id: &'a str,
    from_handle: &'a str,
    to_handle: &'a str,
    subject: &'a str,
    body: &'a str,
    message_type: &'a str,
    priority: &'a str,
    delivery_contract: &'a str,
    payload: Option<&'a str>,
}

#[derive(Insertable)]
#[diesel(table_name = decision_gates)]
struct NewDecisionGate<'a> {
    id: &'a str,
    run_id: &'a str,
    task_id: &'a str,
    question: &'a str,
    options: &'a str,
}

// ─── Store ────────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct DieselOrchestrationStore {
    conn: Arc<Mutex<SqliteConnection>>,
}

impl DieselOrchestrationStore {
    /// Wrap an existing connection.
    pub fn new(conn: SqliteConnection) -> Self {
        Self {
            conn: Arc::new(Mutex::new(conn)),
        }
    }

    /// In-memory store with orchestration tables — for tests.
    pub fn in_memory() -> OrchestrationResult<Self> {
        let mut conn = SqliteConnection::establish(":memory:")
            .map_err(|e| OrchestrationError::Connection(e.to_string()))?;
        conn.batch_execute(include_str!(
            "../../../../../crates/persistence/migrations/2026-08-13-000000_add_orchestration_core/up.sql"
        ))?;
        conn.batch_execute(include_str!(
            "../../../../../crates/persistence/migrations/2026-08-13-000100_add_decision_gates/up.sql"
        ))?;
        conn.batch_execute(include_str!(
            "../../../../../crates/persistence/migrations/2026-08-13-000200_add_worker_terminal_archives/up.sql"
        ))?;
        conn.batch_execute(include_str!(
            "../../../../../crates/persistence/migrations/2026-08-13-000300_add_orchestration_waiters/up.sql"
        ))?;
        Ok(Self::new(conn))
    }

    pub(crate) fn lock(&self) -> parking_lot::MutexGuard<'_, SqliteConnection> {
        self.conn.lock()
    }

    // ── TX helpers (take &mut SqliteConnection, no locking) ───────────

    fn get_task_tx(conn: &mut SqliteConnection, id: &str) -> OrchestrationResult<Option<Task>> {
        tasks::table
            .filter(tasks::id.eq(id))
            .first::<Task>(conn)
            .optional()
            .map_err(Into::into)
    }

    fn get_worker_dispatch_tx(
        conn: &mut SqliteConnection,
        dispatch_id: &str,
    ) -> OrchestrationResult<Option<WorkerDispatch>> {
        worker_dispatches::table
            .filter(worker_dispatches::dispatch_id.eq(dispatch_id))
            .first::<WorkerDispatch>(conn)
            .optional()
            .map_err(Into::into)
    }

    fn get_dispatch_context_by_id_tx(
        conn: &mut SqliteConnection,
        id: &str,
    ) -> OrchestrationResult<Option<DispatchContext>> {
        dispatch_contexts::table
            .filter(dispatch_contexts::id.eq(id))
            .first::<DispatchContext>(conn)
            .optional()
            .map_err(Into::into)
    }

    /// Latest dispatch context for a task (by insertion order / rowid).
    fn get_dispatch_context_for_task_tx(
        conn: &mut SqliteConnection,
        task_id: &str,
    ) -> OrchestrationResult<Option<DispatchContext>> {
        dispatch_contexts::table
            .filter(dispatch_contexts::task_id.eq(task_id))
            .order(sql::<Integer>("rowid").desc())
            .first::<DispatchContext>(conn)
            .optional()
            .map_err(Into::into)
    }

    fn promote_ready_tasks_tx(
        conn: &mut SqliteConnection,
        run_id: &str,
    ) -> OrchestrationResult<Vec<String>> {
        let candidates: Vec<Task> = tasks::table
            .filter(tasks::run_id.eq(run_id))
            .filter(tasks::status.eq("pending"))
            .load(conn)?;

        let mut promoted = Vec::new();
        for task in candidates {
            let deps: Vec<String> = serde_json::from_str(&task.deps).unwrap_or_default();
            let all_completed = deps.iter().all(|dep_id| {
                tasks::table
                    .filter(tasks::id.eq(dep_id))
                    .filter(tasks::status.eq("completed"))
                    .select(tasks::id)
                    .first::<String>(conn)
                    .is_ok()
            });
            if all_completed {
                diesel::update(tasks::table.filter(tasks::id.eq(&task.id)))
                    .set(tasks::status.eq("ready"))
                    .execute(conn)?;
                promoted.push(task.id);
            }
        }
        Ok(promoted)
    }

    /// Core settlement logic — runs inside a transaction.
    /// Ported from Orca `settleWorkerReportInTransaction`.
    fn settle_worker_report_tx(
        conn: &mut SqliteConnection,
        task_id: &str,
        dispatch_id: &str,
        outcome: WorkerReportOutcome,
        result: &str,
    ) -> OrchestrationResult<WorkerReportSettlement> {
        let task = match Self::get_task_tx(conn, task_id)? {
            None => {
                return Ok(WorkerReportSettlement::Rejected {
                    code: WorkerReportRejectionCode::UnknownTask,
                    reason: format!("Unknown task {}.", task_id),
                })
            }
            Some(t) => t,
        };

        let dispatch = match Self::get_dispatch_context_by_id_tx(conn, dispatch_id)? {
            None => {
                return Ok(WorkerReportSettlement::Rejected {
                    code: WorkerReportRejectionCode::UnknownDispatch,
                    reason: format!("Unknown dispatch {}.", dispatch_id),
                })
            }
            Some(d) => d,
        };

        if dispatch.task_id != task_id {
            return Ok(WorkerReportSettlement::Rejected {
                code: WorkerReportRejectionCode::TaskDispatchMismatch,
                reason: format!(
                    "Dispatch {} belongs to task {}, not {}.",
                    dispatch_id, dispatch.task_id, task_id
                ),
            });
        }

        let (expected_dispatch_status, expected_task_status) = match outcome {
            WorkerReportOutcome::Succeeded => ("completed", "completed"),
            WorkerReportOutcome::Failed => ("failed", "failed"),
        };

        // Already settled → duplicate
        if dispatch.status == expected_dispatch_status && task.status == expected_task_status {
            return Ok(WorkerReportSettlement::Settled {
                outcome,
                duplicate: true,
            });
        }

        // Must both be 'dispatched' to settle
        if dispatch.status != "dispatched" || task.status != "dispatched" {
            return Ok(WorkerReportSettlement::Rejected {
                code: WorkerReportRejectionCode::InactiveDispatch,
                reason: format!(
                    "Inactive dispatch {}: it or task {} is already settled.",
                    dispatch_id, task_id
                ),
            });
        }

        // Must be the latest dispatch for the task
        let latest = Self::get_dispatch_context_for_task_tx(conn, task_id)?;
        if latest.as_ref().map(|d| d.id.as_str()) != Some(dispatch_id) {
            return Ok(WorkerReportSettlement::Rejected {
                code: WorkerReportRejectionCode::StaleDispatch,
                reason: format!(
                    "Dispatch {} is not the current dispatch for task {}.",
                    dispatch_id, task_id
                ),
            });
        }

        // ── Settlement writes ──────────────────────────────────────────
        let now = Utc::now().naive_utc();

        diesel::update(dispatch_contexts::table.filter(dispatch_contexts::id.eq(dispatch_id)))
            .set((
                dispatch_contexts::status.eq(expected_dispatch_status),
                dispatch_contexts::completed_at.eq(now),
                dispatch_contexts::capability_revoked_at.eq(now),
            ))
            .execute(conn)?;

        diesel::update(tasks::table.filter(tasks::id.eq(task_id)))
            .set((
                tasks::status.eq(expected_task_status),
                tasks::result.eq(result),
                tasks::completed_at.eq(now),
            ))
            .execute(conn)?;

        let target_state = match outcome {
            WorkerReportOutcome::Succeeded => WorkerDispatchState::Succeeded,
            WorkerReportOutcome::Failed => WorkerDispatchState::Failed,
        };
        let worker = Self::get_worker_dispatch_tx(conn, dispatch_id)?
            .ok_or_else(|| {
                OrchestrationError::NotFound(format!("worker dispatch {}", dispatch_id))
            })?;
        let worker_state = target_state.as_ref();
        diesel::update(
            worker_dispatches::table.filter(worker_dispatches::dispatch_id.eq(dispatch_id)),
        )
        .set((
            worker_dispatches::state.eq(worker_state),
            worker_dispatches::stage.eq("settled"),
            worker_dispatches::updated_at.eq(now),
        ))
        .execute(conn)?;

        // Promote dependent tasks on success
        if outcome == WorkerReportOutcome::Succeeded {
            Self::promote_ready_tasks_tx(conn, &task.run_id)?;
        }

        Ok(WorkerReportSettlement::Settled {
            outcome,
            duplicate: false,
        })
    }

    // ── Public settlement & lifecycle methods ──────────────────────────

    /// Settle a `worker_done` report. Ported from Orca `settleWorkerReport`.
    pub fn settle_worker_report(
        &self,
        task_id: &str,
        dispatch_id: &str,
        outcome: WorkerReportOutcome,
        result: &str,
    ) -> OrchestrationResult<WorkerReportSettlement> {
        let mut conn = self.lock();
        conn.transaction(|conn| {
            Self::settle_worker_report_tx(conn, task_id, dispatch_id, outcome, result)
        })
    }

    /// Transition worker starting → ready and dispatch pending → dispatched.
    /// Ported from Orca `markWorkerDispatchReady`.
    pub fn mark_worker_dispatch_ready(
        &self,
        dispatch_id: &str,
        effects: Option<&str>,
    ) -> OrchestrationResult<()> {
        let mut conn = self.lock();
        conn.transaction::<_, OrchestrationError, _>(|conn| {
            let worker = Self::get_worker_dispatch_tx(conn, dispatch_id)?
                .ok_or_else(|| {
                    OrchestrationError::NotFound(format!("worker dispatch {}", dispatch_id))
                })?;
            if worker.state != "starting" {
                return Err(OrchestrationError::Task(format!(
                    "Worker {} is not starting (state: {}).",
                    dispatch_id, worker.state
                )));
            }

            let ctx = Self::get_dispatch_context_by_id_tx(conn, dispatch_id)?
                .ok_or_else(|| {
                    OrchestrationError::NotFound(format!("dispatch context {}", dispatch_id))
                })?;
            if ctx.status != "pending" {
                return Err(OrchestrationError::Task(format!(
                    "Dispatch {} is not pending (status: {}).",
                    dispatch_id, ctx.status
                )));
            }
            diesel::update(
                dispatch_contexts::table.filter(dispatch_contexts::id.eq(dispatch_id)),
            )
            .set(dispatch_contexts::status.eq("dispatched"))
            .execute(conn)?;

            // Fold in the coordinator step: the task must be `dispatched`
            // for `settle_worker_report` to accept a later worker_done.
            // Guard on `ready` so a concurrent decision gate (`blocked`) or
            // an already-settled task is never overwritten.
            diesel::update(
                tasks::table
                    .filter(tasks::id.eq(&ctx.task_id))
                    .filter(tasks::status.eq("ready")),
            )
            .set(tasks::status.eq("dispatched"))
            .execute(conn)?;
            let task_now = Self::get_task_tx(conn, &ctx.task_id)?
                .ok_or_else(|| OrchestrationError::NotFound(format!("task {}", ctx.task_id)))?;
            if task_now.status != "dispatched" {
                return Err(OrchestrationError::Task(format!(
                    "Task {} is {} (not ready) — refusing to mark dispatch ready.",
                    ctx.task_id, task_now.status
                )));
            }
            let now = Utc::now().naive_utc();
            diesel::update(
                worker_dispatches::table.filter(worker_dispatches::dispatch_id.eq(dispatch_id)),
            )
            .set((
                worker_dispatches::state.eq("ready"),
                worker_dispatches::stage.eq("input_accepted"),
                worker_dispatches::effects.eq(effects.unwrap_or("[]")),
                worker_dispatches::updated_at.eq(now),
            ))
            .execute(conn)?;

            Ok(())
        })
    }

    /// Settle a worker stop: stopping → stopped, dispatch → failed.
    /// Ported from Orca `settleWorkerStop`.
    pub fn settle_worker_stop(&self, dispatch_id: &str) -> OrchestrationResult<()> {
        let mut conn = self.lock();
        conn.transaction::<_, OrchestrationError, _>(|conn| {
            let worker = Self::get_worker_dispatch_tx(conn, dispatch_id)?
                .ok_or_else(|| {
                    OrchestrationError::NotFound(format!("worker dispatch {}", dispatch_id))
                })?;
            let current = WorkerDispatchState::from_str(&worker.state)
                .map_err(|_| OrchestrationError::Task(format!("invalid state: {}", worker.state)))?;
            let target = WorkerDispatchState::Stopped;
            if !is_valid_transition(current, target) {
                return Err(OrchestrationError::Task(format!(
                    "Invalid worker transition: {} → {}",
                    worker.state, target
                )));
            }

            let now = Utc::now().naive_utc();
            diesel::update(
                worker_dispatches::table.filter(worker_dispatches::dispatch_id.eq(dispatch_id)),
            )
            .set((
                worker_dispatches::state.eq("stopped"),
                worker_dispatches::stage.eq("process_stopped"),
                worker_dispatches::updated_at.eq(now),
            ))
            .execute(conn)?;

            diesel::update(
                dispatch_contexts::table
                    .filter(dispatch_contexts::id.eq(dispatch_id))
                    .filter(dispatch_contexts::status.eq_any(["pending", "dispatched"])),
            )
            .set((
                dispatch_contexts::status.eq("failed"),
                dispatch_contexts::completed_at.eq(now),
                dispatch_contexts::last_failure.eq("stopped"),
            ))
            .execute(conn)?;

            Ok(())
        })
    }

    /// Record a heartbeat for a dispatched context.
    /// Ported from Orca `recordHeartbeat`.
    pub fn record_heartbeat(&self, dispatch_id: &str) -> OrchestrationResult<()> {
        let mut conn = self.lock();
        diesel::update(
            dispatch_contexts::table
                .filter(dispatch_contexts::id.eq(dispatch_id))
                .filter(dispatch_contexts::status.eq("dispatched")),
        )
        .set(dispatch_contexts::last_heartbeat_at.eq(Utc::now().naive_utc()))
        .execute(&mut *conn)?;
        Ok(())
    }

    // ── Public lookups ────────────────────────────────────────────────

    pub fn get_task(&self, id: &str) -> OrchestrationResult<Option<Task>> {
        let mut conn = self.lock();
        Self::get_task_tx(&mut *conn, id)
    }

    pub fn get_worker_dispatch(
        &self,
        dispatch_id: &str,
    ) -> OrchestrationResult<Option<WorkerDispatch>> {
        let mut conn = self.lock();
        Self::get_worker_dispatch_tx(&mut *conn, dispatch_id)
    }

    pub fn get_dispatch_context_by_id(
        &self,
        id: &str,
    ) -> OrchestrationResult<Option<DispatchContext>> {
        let mut conn = self.lock();
        Self::get_dispatch_context_by_id_tx(&mut *conn, id)
    }

    pub fn get_dispatch_context_for_task(
        &self,
        task_id: &str,
    ) -> OrchestrationResult<Option<DispatchContext>> {
        let mut conn = self.lock();
        Self::get_dispatch_context_for_task_tx(&mut *conn, task_id)
    }

    // ── Convenience: create dispatch context + worker together ─────────

    /// Create a linked dispatch_context + worker_dispatch pair sharing one id.
    /// Ported from Orca `createStartingWorkerDispatch` (simplified — no
    /// mutation receipts, federation, or retry logic).
    pub fn create_dispatch(
        &self,
        run_id: &str,
        task_id: &str,
        start_options: &str,
    ) -> OrchestrationResult<String> {
        let mut conn = self.lock();
        conn.transaction::<_, OrchestrationError, _>(|conn| {
            let id = generate_id("ctx");

            diesel::insert_into(dispatch_contexts::table)
                .values(&NewDispatchContext {
                    id: &id,
                    run_id,
                    task_id,
                })
                .execute(conn)?;

            diesel::insert_into(worker_dispatches::table)
                .values(&NewWorkerDispatch { dispatch_id: &id })
                .execute(conn)?;

            if !start_options.is_empty() && start_options != "{}" {
                diesel::update(
                    worker_dispatches::table
                        .filter(worker_dispatches::dispatch_id.eq(&id)),
                )
                .set(worker_dispatches::start_options.eq(start_options))
                .execute(conn)?;
            }

            Ok(id)
        })
    }

    // ── Decision Gates ────────────────────────────────────────────────

    /// Create a gate for a task, blocking it until resolved.
    /// Ported from Orca `createGate`: inserts gate, completes active dispatch,
    /// sets task to `blocked`.
    pub fn create_gate(
        &self,
        task_id: &str,
        question: &str,
        options: &[&str],
    ) -> OrchestrationResult<String> {
        let mut conn = self.lock();
        conn.transaction::<_, OrchestrationError, _>(|conn| {
            let task = Self::get_task_tx(conn, task_id)?
                .ok_or_else(|| OrchestrationError::NotFound(format!("task {}", task_id)))?;

            let id = generate_id("gate");
            let options_json = serde_json::to_string(options)?;
            diesel::insert_into(decision_gates::table)
                .values(&NewDecisionGate {
                    id: &id,
                    run_id: &task.run_id,
                    task_id,
                    question,
                    options: &options_json,
                })
                .execute(conn)?;

            // Complete any active dispatch for the task
            let active = dispatch_contexts::table
                .filter(dispatch_contexts::task_id.eq(task_id))
                .filter(dispatch_contexts::status.eq_any(["pending", "dispatched"]))
                .order(sql::<Integer>("rowid").desc())
                .first::<DispatchContext>(conn)
                .optional()?;
            if let Some(d) = active {
                let now = Utc::now().naive_utc();
                diesel::update(dispatch_contexts::table.filter(dispatch_contexts::id.eq(&d.id)))
                    .set((
                        dispatch_contexts::status.eq("completed"),
                        dispatch_contexts::completed_at.eq(now),
                    ))
                    .execute(conn)?;
            }

            // Block the task
            diesel::update(tasks::table.filter(tasks::id.eq(task_id)))
                .set(tasks::status.eq("blocked"))
                .execute(conn)?;

            Ok(id)
        })
    }

    /// Resolve a gate with a decision, unblocking its task.
    /// Ported from Orca `resolveGate`: sets gate to `resolved`, task to `ready`.
    pub fn resolve_gate(&self, gate_id: &str, resolution: &str) -> OrchestrationResult<()> {
        let mut conn = self.lock();
        conn.transaction::<_, OrchestrationError, _>(|conn| {
            let gate = decision_gates::table
                .filter(decision_gates::id.eq(gate_id))
                .filter(decision_gates::status.eq("pending"))
                .first::<DecisionGate>(conn)
                .optional()?
                .ok_or_else(|| OrchestrationError::NotFound(format!("gate {}", gate_id)))?;

            let now = Utc::now().naive_utc();
            diesel::update(decision_gates::table.filter(decision_gates::id.eq(gate_id)))
                .set((
                    decision_gates::status.eq("resolved"),
                    decision_gates::resolution.eq(resolution),
                    decision_gates::resolved_at.eq(now),
                ))
                .execute(conn)?;

            // Set task to 'ready' (not previous status) so the coordinator re-dispatches
            // with the resolution context.
            // Only unblock tasks that are actually blocked — a task that
            // was already resolved/expired must not be downgraded to ready
            // by a stale or duplicate gate resolution (#9b).
            diesel::update(
                tasks::table
                    .filter(tasks::id.eq(&gate.task_id))
                    .filter(tasks::status.eq("blocked")),
            )
            .set(tasks::status.eq("ready"))
            .execute(conn)?;

            Ok(())
        })
    }

    /// Expire a gate due to timeout.
    /// Ported from Orca `timeoutGate`: sets gate to `timeout` and fails the
    /// blocked task so it doesn't stay stranded forever.
    pub fn expire_gate(&self, gate_id: &str) -> OrchestrationResult<()> {
        let mut conn = self.lock();
        let now = Utc::now().naive_utc();
        conn.transaction::<_, OrchestrationError, _>(|conn| {
            let gate = decision_gates::table
                .filter(decision_gates::id.eq(gate_id))
                .filter(decision_gates::status.eq("pending"))
                .first::<DecisionGate>(conn)
                .optional()?;
            let gate = gate
                .ok_or_else(|| OrchestrationError::NotFound(format!("gate {}", gate_id)))?;

            diesel::update(decision_gates::table.filter(decision_gates::id.eq(gate_id)))
                .set((
                    decision_gates::status.eq("timeout"),
                    decision_gates::resolved_at.eq(now),
                ))
                .execute(conn)?;

            // Fail the blocked task so it doesn't stay stranded.
            diesel::update(tasks::table.filter(tasks::id.eq(&gate.task_id)))
                .set((
                    tasks::status.eq("failed"),
                    tasks::result.eq("gate_timeout"),
                    tasks::completed_at.eq(now),
                ))
                .execute(conn)?;

            Ok(())
        })
    }

    /// List gates with optional filters.
    /// Ported from Orca `listGates`.
    pub fn list_gates(
        &self,
        task_id: Option<&str>,
        status: Option<&str>,
    ) -> OrchestrationResult<Vec<DecisionGate>> {
        let mut conn = self.lock();
        let mut query = decision_gates::table.into_boxed();
        if let Some(tid) = task_id {
            query = query.filter(decision_gates::task_id.eq(tid));
        }
        if let Some(s) = status {
            query = query.filter(decision_gates::status.eq(s));
        }
        query
            .order(decision_gates::created_at.asc())
            .load(&mut *conn)
            .map_err(Into::into)
    }

    // ── Run queries ──────────────────────────────────────────────────

    /// List all runs, newest first.
    pub fn list_runs(&self) -> OrchestrationResult<Vec<Run>> {
        let mut conn = self.lock();
        runs::table
            .order(runs::created_at.desc())
            .load(&mut *conn)
            .map_err(Into::into)
    }

    // ── Task DAG ──────────────────────────────────────────────────────

    /// List tasks by status filter.
    /// Ported from Orca `listTasks`.
    pub fn list_tasks(
        &self,
        run_id: Option<&str>,
        status: Option<&str>,
    ) -> OrchestrationResult<Vec<Task>> {
        let mut conn = self.lock();
        let mut query = tasks::table.into_boxed();
        if let Some(rid) = run_id {
            query = query.filter(tasks::run_id.eq(rid));
        }
        if let Some(s) = status {
            query = query.filter(tasks::status.eq(s));
        }
        query
            .order(tasks::created_at.asc())
            .load(&mut *conn)
            .map_err(Into::into)
    }

    /// Resolve all dispatchable tasks in topological order.
    ///
    /// Returns task ids whose deps are all `completed`, in dependency order.
    /// Tasks already `dispatched`/`completed`/`failed` are excluded.
    ///
    /// This implements the Orca `dispatchReadyTasks` selection: any task
    /// currently in `ready` status is dispatchable. The topological sort
    /// orders them by dependency depth so a parent completes before its
    /// children are considered.
    pub fn resolve_ready_tasks(&self, run_id: &str) -> OrchestrationResult<Vec<String>> {
        let all_tasks = self.list_tasks(Some(run_id), None)?;

        // Build id → task lookup and dep graph
        use std::collections::HashMap;
        let task_map: HashMap<&str, &Task> =
            all_tasks.iter().map(|t| (t.id.as_str(), t)).collect();

        // Only consider tasks in 'ready' state (deps already promoted)
        let ready: Vec<&Task> = all_tasks.iter().filter(|t| t.status == "ready").collect();

        // Compute dependency depth for each ready task.
        // A visiting sentinel (usize::MAX) guards against circular deps;
        // if we re-enter a node currently being visited, the cycle is
        // broken by returning depth 0 instead of recursing infinitely.
        const VISITING: usize = usize::MAX;
        fn depth(
            task_id: &str,
            task_map: &HashMap<&str, &Task>,
            cache: &mut HashMap<String, usize>,
        ) -> usize {
            if let Some(&d) = cache.get(task_id) {
                return if d == VISITING { 0 } else { d };
            }
            let task = match task_map.get(task_id) {
                Some(t) => *t,
                None => return 0,
            };
            cache.insert(task_id.to_string(), VISITING);
            let deps: Vec<String> = serde_json::from_str(&task.deps).unwrap_or_default();
            let max_dep = deps
                .iter()
                .map(|d| depth(d, task_map, cache))
                .max()
                .unwrap_or(0);
            let d = max_dep + 1;
            cache.insert(task_id.to_string(), d);
            d
        }

        let mut cache: HashMap<String, usize> = HashMap::new();
        let mut sorted: Vec<(&Task, usize)> = ready
            .iter()
            .map(|t| (*t, depth(&t.id, &task_map, &mut cache)))
            .collect();
        sorted.sort_by_key(|(_, d)| *d);

        Ok(sorted.iter().map(|(t, _)| t.id.clone()).collect())
    }

    // ── Async wrappers (spawn_blocking) ───────────────────────────────

    pub async fn create_run_async(&self, objective: String) -> OrchestrationResult<String> {
        let store = self.clone();
        tokio::task::spawn_blocking(move || store.create_run(&objective))
            .await
            .map_err(|e| OrchestrationError::Join(e.to_string()))?
    }

    pub async fn create_task_async(
        &self,
        run_id: String,
        spec: String,
        deps: Vec<String>,
    ) -> OrchestrationResult<String> {
        let store = self.clone();
        tokio::task::spawn_blocking(move || {
            let deps: Vec<&str> = deps.iter().map(|s| s.as_str()).collect();
            store.create_task(&run_id, &spec, &deps)
        })
        .await
        .map_err(|e| OrchestrationError::Join(e.to_string()))?
    }

    pub async fn settle_worker_report_async(
        &self,
        task_id: String,
        dispatch_id: String,
        outcome: WorkerReportOutcome,
        result: String,
    ) -> OrchestrationResult<WorkerReportSettlement> {
        let store = self.clone();
        tokio::task::spawn_blocking(move || {
            store.settle_worker_report(&task_id, &dispatch_id, outcome, &result)
        })
        .await
        .map_err(|e| OrchestrationError::Join(e.to_string()))?
    }

    pub async fn transition_worker_async(
        &self,
        dispatch_id: String,
        next: WorkerDispatchState,
    ) -> OrchestrationResult<()> {
        let store = self.clone();
        tokio::task::spawn_blocking(move || store.transition_worker(&dispatch_id, next))
            .await
            .map_err(|e| OrchestrationError::Join(e.to_string()))?
    }

    // ── Worker terminal archives ─────────────────────────────────────

    /// Store (upsert) a worker terminal output archive.
    /// Ported from Orca `storeWorkerTerminalArchive`.
    pub fn store_archive(
        &self,
        dispatch_id: &str,
        resource_id: &str,
        kind: &str,
        content: &str,
    ) -> OrchestrationResult<()> {
        let mut conn = self.lock();
        diesel::replace_into(worker_terminal_archives::table)
            .values((
                worker_terminal_archives::dispatch_id.eq(dispatch_id),
                worker_terminal_archives::resource_id.eq(resource_id),
                worker_terminal_archives::kind.eq(kind),
                worker_terminal_archives::content.eq(content),
            ))
            .execute(&mut *conn)?;
        Ok(())
    }

    /// Retrieve a worker terminal output archive.
    /// Ported from Orca `getWorkerTerminalArchive`.
    pub fn get_archive(&self, dispatch_id: &str) -> OrchestrationResult<Option<WorkerTerminalArchive>> {
        let mut conn = self.lock();
        worker_terminal_archives::table
            .filter(worker_terminal_archives::dispatch_id.eq(dispatch_id))
            .first::<WorkerTerminalArchive>(&mut *conn)
            .optional()
            .map_err(Into::into)
    }
}

// ─── OrchestrationStore trait impl ────────────────────────────────────────

impl OrchestrationStore for DieselOrchestrationStore {
    fn create_run(&self, objective: &str) -> OrchestrationResult<String> {
        let mut conn = self.lock();
        let id = generate_id("run");
        diesel::insert_into(runs::table)
            .values(&NewRun { id: &id, objective })
            .execute(&mut *conn)?;
        Ok(id)
    }

    fn create_task(&self, run_id: &str, spec: &str, deps: &[&str]) -> OrchestrationResult<String> {
        let mut conn = self.lock();
        let id = generate_id("task");
        let deps_json = serde_json::to_string(deps)?;
        diesel::insert_into(tasks::table)
            .values(&NewTask {
                id: &id,
                run_id,
                spec,
                deps: &deps_json,
            })
            .execute(&mut *conn)?;
        Ok(id)
    }

    fn promote_ready_tasks(&self, run_id: &str) -> OrchestrationResult<Vec<String>> {
        let mut conn = self.lock();
        Self::promote_ready_tasks_tx(&mut *conn, run_id)
    }

    fn create_dispatch_context(
        &self,
        run_id: &str,
        task_id: &str,
    ) -> OrchestrationResult<String> {
        let mut conn = self.lock();
        let id = generate_id("ctx");
        diesel::insert_into(dispatch_contexts::table)
            .values(&NewDispatchContext {
                id: &id,
                run_id,
                task_id,
            })
            .execute(&mut *conn)?;
        Ok(id)
    }

    fn fail_dispatch(&self, id: &str, error: &str) -> OrchestrationResult<bool> {
        let mut conn = self.lock();
        conn.transaction::<_, OrchestrationError, _>(|conn| {
            let ctx = Self::get_dispatch_context_by_id_tx(conn, id)?
                .ok_or_else(|| OrchestrationError::NotFound(format!("dispatch context {}", id)))?;

            let new_failure_count = ctx.failure_count + 1;
            let new_status = if new_failure_count >= CIRCUIT_BREAKER_FAILURE_THRESHOLD {
                "circuit_broken"
            } else {
                "failed"
            };

            let now = Utc::now().naive_utc();
            diesel::update(dispatch_contexts::table.filter(dispatch_contexts::id.eq(id)))
                .set((
                    dispatch_contexts::status.eq(new_status),
                    dispatch_contexts::failure_count.eq(new_failure_count),
                    dispatch_contexts::last_failure.eq(error),
                    dispatch_contexts::completed_at.eq(now),
                    dispatch_contexts::capability_revoked_at.eq(now),
                ))
                .execute(conn)?;

            // Back to 'ready' (not 'pending') — 'pending' would strand since
            // promoteReadyTasks only runs when a dep completes.
            let task_status = if new_status == "circuit_broken" {
                "failed"
            } else {
                "ready"
            };
            diesel::update(tasks::table.filter(tasks::id.eq(&ctx.task_id)))
                .set(tasks::status.eq(task_status))
                .execute(conn)?;

            Ok(new_status == "circuit_broken")
        })
    }

    fn create_worker_dispatch(&self) -> OrchestrationResult<String> {
        let mut conn = self.lock();
        let id = generate_id("wkr");
        diesel::insert_into(worker_dispatches::table)
            .values(&NewWorkerDispatch { dispatch_id: &id })
            .execute(&mut *conn)?;
        Ok(id)
    }

    fn transition_worker(
        &self,
        dispatch_id: &str,
        next: WorkerDispatchState,
    ) -> OrchestrationResult<()> {
        let mut conn = self.lock();
        let worker = Self::get_worker_dispatch_tx(&mut *conn, dispatch_id)?
            .ok_or_else(|| {
                OrchestrationError::NotFound(format!("worker dispatch {}", dispatch_id))
            })?;

        let current = worker.typed_state()?;
        if !is_valid_transition(current, next) {
            return Err(OrchestrationError::Task(format!(
                "Invalid worker state transition: {} → {}",
                current.as_ref(),
                next.as_ref()
            )));
        }

        diesel::update(
            worker_dispatches::table.filter(worker_dispatches::dispatch_id.eq(dispatch_id)),
        )
        .set((
            worker_dispatches::state.eq(next.as_ref()),
            worker_dispatches::updated_at.eq(Utc::now().naive_utc()),
        ))
        .execute(&mut *conn)?;

        Ok(())
    }

    fn enqueue_message(
        &self,
        run_id: &str,
        from_handle: &str,
        to_handle: &str,
        message_type: MessageType,
        subject: &str,
        body: &str,
    ) -> OrchestrationResult<i32> {
        let mut conn = self.lock();
        let id = generate_id("msg");
        // Lifecycle messages carry their structured payload in `body` (the
        // CLI's single input). Mirror valid JSON bodies into the `payload`
        // column so `reconcile_worker_done` / `reconcile_heartbeat` can
        // parse them; other message types leave `payload` NULL.
        let payload: Option<&str> = match message_type {
            MessageType::WorkerDone | MessageType::Heartbeat
                if serde_json::from_str::<serde_json::Value>(body).is_ok() =>
            {
                Some(body)
            }
            _ => None,
        };
        diesel::insert_into(messages::table)
            .values(&NewMessage {
                id: &id,
                run_id,
                from_handle,
                to_handle,
                subject,
                body,
                message_type: message_type.as_ref(),
                priority: MessagePriority::Normal.as_ref(),
                delivery_contract: MessageDeliveryContract::CurrentDelivery.as_ref(),
                payload,
            })
            .execute(&mut *conn)?;
        let seq: i32 = diesel::select(sql::<Integer>("last_insert_rowid()"))
            .get_result(&mut *conn)?;

        Ok(seq)
    }

    fn drain_inbox(&self, handle: &str) -> OrchestrationResult<Vec<Message>> {
        let mut conn = self.lock();

        // Atomically select-and-mark: load unread rows for this handle
        // and set read=1 in the same transaction. This closes the TOCTOU
        // window where two concurrent consumers (router thread + CLI
        // check-messages) could SELECT the same unread rows and each
        // process them — double consumption with side effects.
        conn.transaction::<Vec<Message>, OrchestrationError, _>(|conn| {
            let unread: Vec<Message> = messages::table
                .filter(messages::to_handle.eq(handle))
                .filter(messages::read.eq(0))
                .order(messages::sequence.asc())
                .load(conn)?;

            if !unread.is_empty() {
                let sequences: Vec<i32> = unread.iter().map(|m| m.sequence).collect();
                diesel::update(messages::table.filter(messages::sequence.eq_any(sequences)))
                    .set(messages::read.eq(1))
                    .execute(conn)?;
            }

            Ok(unread)
        })
    }

    fn mark_messages_read(&self, sequences: &[i32]) -> OrchestrationResult<()> {
        if sequences.is_empty() {
            return Ok(());
        }
        let mut conn = self.lock();
        diesel::update(
            messages::table.filter(messages::sequence.eq_any(sequences)),
        )
        .set(messages::read.eq(1))
        .execute(&mut *conn)?;
        Ok(())
    }

    /// Undelivered unread messages for a mailbox — the push-delivery source.
    /// Ported from Orca `getUndeliveredUnreadMessages` (db.ts:3487):
    /// `to_handle = ? AND read = 0 AND delivered_at IS NULL AND
    ///  delivery_contract = 'current_delivery' ORDER BY sequence`.
    fn get_undelivered_unread(&self, handle: &str) -> OrchestrationResult<Vec<Message>> {
        let mut conn = self.lock();
        messages::table
            .filter(messages::to_handle.eq(handle))
            .filter(messages::read.eq(0))
            .filter(messages::delivered_at.is_null())
            .filter(messages::delivery_contract.eq("current_delivery"))
            .order(messages::sequence.asc())
            .load(&mut *conn)
            .map_err(Into::into)
    }

    /// Mark messages delivered (pointer successfully written to the PTY).
    fn mark_delivered(&self, sequences: &[i32]) -> OrchestrationResult<()> {
        if sequences.is_empty() {
            return Ok(());
        }
        let mut conn = self.lock();
        diesel::update(
            messages::table.filter(messages::sequence.eq_any(sequences)),
        )
        .set(messages::delivered_at.eq(Utc::now().naive_utc()))
        .execute(&mut *conn)?;
        Ok(())
    }

    fn upsert_waiter(
        &self,
        id: &str,
        handle: &str,
        type_filter: &str,
        ttl_secs: i64,
    ) -> OrchestrationResult<()> {
        let mut conn = self.lock();
        let expires_at = (Utc::now() + chrono::Duration::seconds(ttl_secs)).naive_utc();
        diesel::replace_into(orchestration_waiters::table)
            .values((
                orchestration_waiters::id.eq(id),
                orchestration_waiters::handle.eq(handle),
                orchestration_waiters::type_filter.eq(type_filter),
                orchestration_waiters::expires_at.eq(expires_at),
            ))
            .execute(&mut *conn)?;
        Ok(())
    }

    fn delete_waiter(&self, id: &str) -> OrchestrationResult<()> {
        let mut conn = self.lock();
        diesel::delete(orchestration_waiters::table.filter(orchestration_waiters::id.eq(id)))
            .execute(&mut *conn)?;
        Ok(())
    }

    fn has_live_waiter(&self, handle: &str, message_type: &str) -> OrchestrationResult<bool> {
        let mut conn = self.lock();
        // `[]` claims all types; otherwise JSON-array substring match on the
        // quoted type is exact enough (types are plain snake_case words).
        let claimed = orchestration_waiters::table
            .filter(orchestration_waiters::handle.eq(handle))
            .filter(orchestration_waiters::expires_at.gt(Utc::now().naive_utc()))
            .filter(
                orchestration_waiters::type_filter
                    .eq("[]")
                    .or(orchestration_waiters::type_filter.like(format!("%\"{message_type}\"%"))),
            )
            .select(orchestration_waiters::id)
            .first::<String>(&mut *conn)
            .optional()?;
        Ok(claimed.is_some())
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────


#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> DieselOrchestrationStore {
        DieselOrchestrationStore::in_memory().expect("in_memory store")
    }

    /// Full dispatch lifecycle: create → promote → dispatch → mark_ready → settle.
    #[test]
    fn test_full_lifecycle_succeeded() {
        let store = setup();

        // 1. Run + root task (no deps)
        let run_id = store.create_run("test objective").unwrap();
        let task_id = store.create_task(&run_id, "do thing", &[]).unwrap();

        // 2. Promote: root task has no deps → ready
        let promoted = store.promote_ready_tasks(&run_id).unwrap();
        assert_eq!(promoted, vec![task_id.clone()]);
        let task = store.get_task(&task_id).unwrap().unwrap();
        assert_eq!(task.status, "ready");

        // 3. Create linked dispatch (context + worker)
        let dispatch_id = store.create_dispatch(&run_id, &task_id, "{}").unwrap();

        // 4. Mark worker ready: worker starting→ready, context pending→dispatched, task ready→dispatched
        store.mark_worker_dispatch_ready(&dispatch_id, None).unwrap();
        // Task status doesn't change in mark_ready (Orca doesn't update task either)
        // The settle_worker_report expects task in 'dispatched' state.
        // In Orca, the coordinator sets task to 'dispatched' before calling markWorkerDispatchReady.
        // For this test, we set it manually.
        {
            let mut conn = store.lock();
            diesel::update(tasks::table.filter(tasks::id.eq(&task_id)))
                .set(tasks::status.eq("dispatched"))
                .execute(&mut *conn)
                .unwrap();
        }

        let worker = store.get_worker_dispatch(&dispatch_id).unwrap().unwrap();
        assert_eq!(worker.state, "ready");
        let ctx = store.get_dispatch_context_by_id(&dispatch_id).unwrap().unwrap();
        assert_eq!(ctx.status, "dispatched");

        // 5. Settle: worker_done succeeded
        let settlement = store
            .settle_worker_report(
                &task_id,
                &dispatch_id,
                WorkerReportOutcome::Succeeded,
                "{\"outcome\":\"succeeded\"}",
            )
            .unwrap();

        match settlement {
            WorkerReportSettlement::Settled { outcome, duplicate } => {
                assert_eq!(outcome, WorkerReportOutcome::Succeeded);
                assert!(!duplicate);
            }
            other => panic!("expected Settled, got {:?}", other),
        }

        // 6. Verify final state
        let task = store.get_task(&task_id).unwrap().unwrap();
        assert_eq!(task.status, "completed");
        let ctx = store.get_dispatch_context_by_id(&dispatch_id).unwrap().unwrap();
        assert_eq!(ctx.status, "completed");
        let worker = store.get_worker_dispatch(&dispatch_id).unwrap().unwrap();
        assert_eq!(worker.state, "succeeded");
    }

    #[test]
    fn test_full_lifecycle_failed() {
        let store = setup();
        let run_id = store.create_run("fail test").unwrap();
        let task_id = store.create_task(&run_id, "fail thing", &[]).unwrap();
        store.promote_ready_tasks(&run_id).unwrap();
        let dispatch_id = store.create_dispatch(&run_id, &task_id, "{}").unwrap();
        store.mark_worker_dispatch_ready(&dispatch_id, None).unwrap();
        {
            let mut conn = store.lock();
            diesel::update(tasks::table.filter(tasks::id.eq(&task_id)))
                .set(tasks::status.eq("dispatched"))
                .execute(&mut *conn)
                .unwrap();
        }

        let settlement = store
            .settle_worker_report(
                &task_id,
                &dispatch_id,
                WorkerReportOutcome::Failed,
                "{\"outcome\":\"failed\"}",
            )
            .unwrap();

        assert!(matches!(
            settlement,
            WorkerReportSettlement::Settled { outcome: WorkerReportOutcome::Failed, duplicate: false }
        ));

        assert_eq!(
            store.get_task(&task_id).unwrap().unwrap().status,
            "failed"
        );
        assert_eq!(
            store
                .get_dispatch_context_by_id(&dispatch_id)
                .unwrap()
                .unwrap()
                .status,
            "failed"
        );
        assert_eq!(
            store
                .get_worker_dispatch(&dispatch_id)
                .unwrap()
                .unwrap()
                .state,
            "failed"
        );
    }

    #[test]
    fn test_duplicate_settlement() {
        let store = setup();
        let run_id = store.create_run("dup test").unwrap();
        let task_id = store.create_task(&run_id, "do thing", &[]).unwrap();
        store.promote_ready_tasks(&run_id).unwrap();
        let dispatch_id = store.create_dispatch(&run_id, &task_id, "{}").unwrap();
        store.mark_worker_dispatch_ready(&dispatch_id, None).unwrap();
        {
            let mut conn = store.lock();
            diesel::update(tasks::table.filter(tasks::id.eq(&task_id)))
                .set(tasks::status.eq("dispatched"))
                .execute(&mut *conn)
                .unwrap();
        }

        // First settlement
        store
            .settle_worker_report(&task_id, &dispatch_id, WorkerReportOutcome::Succeeded, "{}")
            .unwrap();

        // Second settlement → duplicate
        let settlement = store
            .settle_worker_report(&task_id, &dispatch_id, WorkerReportOutcome::Succeeded, "{}")
            .unwrap();

        assert!(matches!(
            settlement,
            WorkerReportSettlement::Settled { duplicate: true, .. }
        ));
    }

    #[test]
    fn test_circuit_breaker() {
        let store = setup();
        let run_id = store.create_run("breaker test").unwrap();
        let task_id = store.create_task(&run_id, "fail 3x", &[]).unwrap();
        store.promote_ready_tasks(&run_id).unwrap();
        let ctx_id = store.create_dispatch_context(&run_id, &task_id).unwrap();

        // Fail 1: failure_count=1, status=failed, task back to ready
        let broken = store.fail_dispatch(&ctx_id, "error 1").unwrap();
        assert!(!broken);

        // Fail 2: failure_count=2, status=failed
        let broken = store.fail_dispatch(&ctx_id, "error 2").unwrap();
        assert!(!broken);

        // Fail 3: failure_count=3, status=circuit_broken, task=failed
        let broken = store.fail_dispatch(&ctx_id, "error 3").unwrap();
        assert!(broken);

        let ctx = store.get_dispatch_context_by_id(&ctx_id).unwrap().unwrap();
        assert_eq!(ctx.status, "circuit_broken");
        assert_eq!(ctx.failure_count, 3);
        assert_eq!(store.get_task(&task_id).unwrap().unwrap().status, "failed");
    }

    #[test]
    fn test_worker_state_transitions_db() {
        let store = setup();
        let dispatch_id = store.create_worker_dispatch().unwrap();

        // starting → ready
        store
            .transition_worker(&dispatch_id, WorkerDispatchState::Ready)
            .unwrap();
        assert_eq!(
            store
                .get_worker_dispatch(&dispatch_id)
                .unwrap()
                .unwrap()
                .state,
            "ready"
        );

        // ready → succeeded
        store
            .transition_worker(&dispatch_id, WorkerDispatchState::Succeeded)
            .unwrap();

        // succeeded → stopping
        store
            .transition_worker(&dispatch_id, WorkerDispatchState::Stopping)
            .unwrap();

        // stopping → stopped
        store
            .transition_worker(&dispatch_id, WorkerDispatchState::Stopped)
            .unwrap();
        assert_eq!(
            store
                .get_worker_dispatch(&dispatch_id)
                .unwrap()
                .unwrap()
                .state,
            "stopped"
        );
    }

    #[test]
    fn test_invalid_transition_rejected() {
        let store = setup();
        let dispatch_id = store.create_worker_dispatch().unwrap();

        // starting → succeeded is invalid (must go through ready first)
        let result = store.transition_worker(&dispatch_id, WorkerDispatchState::Succeeded);
        assert!(result.is_err());
    }

    #[test]
    fn test_promote_ready_tasks_dag() {
        let store = setup();
        let run_id = store.create_run("dag test").unwrap();

        // parent → child1, child2 (both depend on parent)
        let parent = store.create_task(&run_id, "parent", &[]).unwrap();
        let child1 = store.create_task(&run_id, "child1", &[&parent]).unwrap();
        let child2 = store.create_task(&run_id, "child2", &[&parent]).unwrap();

        // Initial promotion: only parent (no deps)
        let promoted = store.promote_ready_tasks(&run_id).unwrap();
        assert_eq!(promoted, vec![parent.clone()]);

        // Children should still be pending
        assert_eq!(
            store.get_task(&child1).unwrap().unwrap().status,
            "pending"
        );
        assert_eq!(
            store.get_task(&child2).unwrap().unwrap().status,
            "pending"
        );

        // Complete parent
        {
            let mut conn = store.lock();
            diesel::update(tasks::table.filter(tasks::id.eq(&parent)))
                .set(tasks::status.eq("completed"))
                .execute(&mut *conn)
                .unwrap();
        }

        // Promote again: children should now be ready
        let promoted = store.promote_ready_tasks(&run_id).unwrap();
        assert_eq!(promoted.len(), 2);
        assert!(promoted.contains(&child1));
        assert!(promoted.contains(&child2));
    }

    #[test]
    fn test_settle_worker_stop() {
        let store = setup();
        let run_id = store.create_run("stop test").unwrap();
        let task_id = store.create_task(&run_id, "do thing", &[]).unwrap();
        store.promote_ready_tasks(&run_id).unwrap();
        let dispatch_id = store.create_dispatch(&run_id, &task_id, "{}").unwrap();
        store.mark_worker_dispatch_ready(&dispatch_id, None).unwrap();

        // ready → succeeded → stopping (must report done before stopping)
        store
            .transition_worker(&dispatch_id, WorkerDispatchState::Succeeded)
            .unwrap();
        store
            .transition_worker(&dispatch_id, WorkerDispatchState::Stopping)
            .unwrap();

        // Settle stop
        store.settle_worker_stop(&dispatch_id).unwrap();

        assert_eq!(
            store
                .get_worker_dispatch(&dispatch_id)
                .unwrap()
                .unwrap()
                .state,
            "stopped"
        );
        let ctx = store.get_dispatch_context_by_id(&dispatch_id).unwrap().unwrap();
        assert_eq!(ctx.status, "failed");
        assert_eq!(ctx.last_failure.unwrap(), "stopped");
    }

    #[test]
    fn test_messaging_enqueue_drain() {
        let store = setup();
        let run_id = store.create_run("msg test").unwrap();

        let seq1 = store
            .enqueue_message(&run_id, "alice", "bob", MessageType::Status, "hi", "hello")
            .unwrap();
        let seq2 = store
            .enqueue_message(&run_id, "alice", "bob", MessageType::Dispatch, "go", "work")
            .unwrap();
        assert!(seq2 > seq1);

        let messages = store.drain_inbox("bob").unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].subject, "hi");
        assert_eq!(messages[1].subject, "go");

        // Mark them read explicitly (drain no longer auto-marks).
        let seqs: Vec<i32> = messages.iter().map(|m| m.sequence).collect();
        store.mark_messages_read(&seqs).unwrap();

        // Second drain should be empty (marked read)
        let messages = store.drain_inbox("bob").unwrap();
        assert!(messages.is_empty());
    }

    // ── Decision Gate tests ───────────────────────────────────────────

    #[test]
    fn test_gate_create_blocks_task() {
        let store = setup();
        let run_id = store.create_run("gate test").unwrap();
        let task_id = store.create_task(&run_id, "decide thing", &[]).unwrap();
        store.promote_ready_tasks(&run_id).unwrap();
        assert_eq!(store.get_task(&task_id).unwrap().unwrap().status, "ready");

        let _gate_id = store
            .create_gate(&task_id, "which approach?", &["A", "B"])
            .unwrap();

        // Task should be blocked
        assert_eq!(
            store.get_task(&task_id).unwrap().unwrap().status,
            "blocked"
        );

        // Gate should be pending
        let gates = store.list_gates(Some(&task_id), None).unwrap();
        assert_eq!(gates.len(), 1);
        assert_eq!(gates[0].status, "pending");
        assert_eq!(gates[0].question, "which approach?");
    }

    #[test]
    fn test_gate_resolve_unblocks_task() {
        let store = setup();
        let run_id = store.create_run("gate resolve").unwrap();
        let task_id = store.create_task(&run_id, "decide thing", &[]).unwrap();
        store.promote_ready_tasks(&run_id).unwrap();
        let gate_id = store
            .create_gate(&task_id, "which approach?", &["A", "B"])
            .unwrap();

        // Task is blocked
        assert_eq!(
            store.get_task(&task_id).unwrap().unwrap().status,
            "blocked"
        );

        // Resolve gate
        store.resolve_gate(&gate_id, "option A").unwrap();

        // Task should be ready again
        assert_eq!(
            store.get_task(&task_id).unwrap().unwrap().status,
            "ready"
        );

        // Gate should be resolved
        let gate = store.list_gates(Some(&task_id), None).unwrap();
        assert_eq!(gate[0].status, "resolved");
        assert_eq!(gate[0].resolution.as_deref(), Some("option A"));
        assert!(gate[0].resolved_at.is_some());
    }

    #[test]
    fn test_gate_expire() {
        let store = setup();
        let run_id = store.create_run("gate expire").unwrap();
        let task_id = store.create_task(&run_id, "decide thing", &[]).unwrap();
        store.promote_ready_tasks(&run_id).unwrap();
        let gate_id = store
            .create_gate(&task_id, "which approach?", &["A", "B"])
            .unwrap();

        // Expire gate
        store.expire_gate(&gate_id).unwrap();

        let gate = store.list_gates(Some(&task_id), None).unwrap();
        assert_eq!(gate[0].status, "timeout");
        assert!(gate[0].resolved_at.is_some());
    }

    #[test]
    fn test_gate_filters() {
        let store = setup();
        let run_id = store.create_run("gate filter").unwrap();
        let t1 = store.create_task(&run_id, "task1", &[]).unwrap();
        let t2 = store.create_task(&run_id, "task2", &[]).unwrap();
        store.promote_ready_tasks(&run_id).unwrap();

        let g1 = store.create_gate(&t1, "q1", &[]).unwrap();
        let g2 = store.create_gate(&t2, "q2", &[]).unwrap();

        // Filter by task
        let gates = store.list_gates(Some(&t1), None).unwrap();
        assert_eq!(gates.len(), 1);
        assert_eq!(gates[0].id, g1);

        // Filter by status
        store.resolve_gate(&g1, "answer").unwrap();
        let pending = store.list_gates(None, Some("pending")).unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].id, g2);

        let resolved = store.list_gates(None, Some("resolved")).unwrap();
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].id, g1);
    }

    // ── DAG resolution tests ──────────────────────────────────────────

    #[test]
    fn test_resolve_ready_tasks_topo_order() {
        let store = setup();
        let run_id = store.create_run("topo test").unwrap();

        // Diamond: root → {a, b} → child
        let root = store.create_task(&run_id, "root", &[]).unwrap();
        let a = store.create_task(&run_id, "a", &[&root]).unwrap();
        let b = store.create_task(&run_id, "b", &[&root]).unwrap();
        let child = store.create_task(&run_id, "child", &[&a, &b]).unwrap();

        // Initial promotion: only root
        store.promote_ready_tasks(&run_id).unwrap();
        let ready = store.resolve_ready_tasks(&run_id).unwrap();
        assert_eq!(ready, vec![root.clone()]);

        // Complete root → promote a, b
        {
            use diesel::prelude::*;
            use persistence::schema::tasks;
            let mut conn = store.lock();
            diesel::update(tasks::table.filter(tasks::id.eq(&root)))
                .set(tasks::status.eq("completed"))
                .execute(&mut *conn)
                .unwrap();
        }
        store.promote_ready_tasks(&run_id).unwrap();
        let ready = store.resolve_ready_tasks(&run_id).unwrap();
        assert_eq!(ready.len(), 2);
        assert!(ready.contains(&a));
        assert!(ready.contains(&b));

        // Complete a, b → promote child
        {
            use diesel::prelude::*;
            use persistence::schema::tasks;
            let mut conn = store.lock();
            diesel::update(tasks::table.filter(tasks::id.eq_any([&a, &b])))
                .set(tasks::status.eq("completed"))
                .execute(&mut *conn)
                .unwrap();
        }
        store.promote_ready_tasks(&run_id).unwrap();
        let ready = store.resolve_ready_tasks(&run_id).unwrap();
        assert_eq!(ready, vec![child]);
    }

    #[test]
    fn test_list_tasks_by_status() {
        let store = setup();
        let run_id = store.create_run("list test").unwrap();
        let _t1 = store.create_task(&run_id, "t1", &[]).unwrap();
        let _t2 = store.create_task(&run_id, "t2", &[]).unwrap();
        store.promote_ready_tasks(&run_id).unwrap();

        let pending = store.list_tasks(Some(&run_id), Some("pending")).unwrap();
        assert!(pending.is_empty());

        let ready = store.list_tasks(Some(&run_id), Some("ready")).unwrap();
        assert_eq!(ready.len(), 2);
    }

    // ── End-to-end integration tests ──────────────────────────────────

    /// Full lifecycle: message → reconciliation → store → state transitions → DAG promote → circuit breaker.
    ///
    /// Exercises the complete chain:
    /// 1. Create run + task DAG (root → child)
    /// 2. Promote root → dispatch → mark_ready
    /// 3. Enqueue worker_done message → route_message → reconcile → settle
    /// 4. Verify task completed + child promoted to ready
    /// 5. Create a second task that fails 3x → circuit_broken
    #[test]
    fn test_message_to_lifecycle_e2e() {
        use crate::agent::orchestration::messaging::route_message;
        use crate::agent::orchestration::db::Message;
        use diesel::prelude::*;
        use persistence::schema::tasks;

        let store = setup();

        // 1. Run + DAG: root → child
        let run_id = store.create_run("e2e objective").unwrap();
        let root = store.create_task(&run_id, "root task", &[]).unwrap();
        let child = store.create_task(&run_id, "child task", &[&root]).unwrap();

        // 2. Promote root (no deps)
        let promoted = store.promote_ready_tasks(&run_id).unwrap();
        assert_eq!(promoted, vec![root.clone()]);
        assert_eq!(
            store.get_task(&child).unwrap().unwrap().status,
            "pending"
        );

        // 3. Dispatch root
        let dispatch_id = store.create_dispatch(&run_id, &root, "{}").unwrap();
        store.mark_worker_dispatch_ready(&dispatch_id, None).unwrap();

        // Set task to dispatched (coordinator step)
        {
            let mut conn = store.lock();
            diesel::update(tasks::table.filter(tasks::id.eq(&root)))
                .set(tasks::status.eq("dispatched"))
                .execute(&mut *conn)
                .unwrap();
        }

        // 4. Build worker_done message and route it through the full pipeline
        let payload = serde_json::json!({
            "task_id": root,
            "dispatch_id": dispatch_id,
            "outcome": "succeeded",
            "result": "root done",
        });
        let msg = Message {
            id: "msg_e2e".into(),
            run_id: run_id.clone(),
            delivery_contract: "current_delivery".into(),
            from_handle: "worker_1".into(),
            to_handle: "coordinator".into(),
            subject: "root completed".into(),
            body: "all good".into(),
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

        let result = route_message(&store, &msg).unwrap();
        assert!(matches!(
            result,
            crate::agent::orchestration::reconciliation::ReconciliationResult::Completed { .. }
        ));

        // 5. Verify root completed + worker succeeded
        let root_task = store.get_task(&root).unwrap().unwrap();
        assert_eq!(root_task.status, "completed");
        let worker = store.get_worker_dispatch(&dispatch_id).unwrap().unwrap();
        assert_eq!(worker.state, "succeeded");
        let ctx = store.get_dispatch_context_by_id(&dispatch_id).unwrap().unwrap();
        assert_eq!(ctx.status, "completed");

        // 6. Child should be promoted to ready (settle_worker_report calls promote)
        let child_task = store.get_task(&child).unwrap().unwrap();
        assert_eq!(
            child_task.status, "ready",
            "child should be promoted after root completes"
        );

        // 7. Circuit breaker: create a task, fail its dispatch 3x
        let failing_task = store.create_task(&run_id, "will fail", &[]).unwrap();
        store.promote_ready_tasks(&run_id).unwrap();
        let fail_ctx = store
            .create_dispatch_context(&run_id, &failing_task)
            .unwrap();

        assert!(!store.fail_dispatch(&fail_ctx, "err 1").unwrap());
        assert!(!store.fail_dispatch(&fail_ctx, "err 2").unwrap());
        let broken = store.fail_dispatch(&fail_ctx, "err 3").unwrap();
        assert!(broken, "circuit should break on 3rd failure");

        let ctx = store.get_dispatch_context_by_id(&fail_ctx).unwrap().unwrap();
        assert_eq!(ctx.status, "circuit_broken");
        assert_eq!(
            store.get_task(&failing_task).unwrap().unwrap().status,
            "failed"
        );
    }
}
