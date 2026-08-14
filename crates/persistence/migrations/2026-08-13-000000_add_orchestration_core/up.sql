-- Orchestration core: 6 tables ported from Orca orchestration module.
-- Adapted from TS TEXT-timestamp schema to zap TIMESTAMP convention.
-- Tables: runs, messages, deliveries, worker_dispatches, tasks, dispatch_contexts.

-- ── runs ──────────────────────────────────────────────────────────────────
CREATE TABLE runs (
  id                    TEXT PRIMARY KEY NOT NULL,
  objective             TEXT NOT NULL,
  home_database         TEXT NOT NULL DEFAULT 'this_database',
  coordinator_handle    TEXT,
  coordinator_pane_key  TEXT,
  consumer_generation   INTEGER NOT NULL DEFAULT 0,
  legacy                INTEGER NOT NULL DEFAULT 0,
  created_at            TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
  updated_at            TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP
);

-- ── messages ───────────────────────────────────────────────────────────────
-- sequence is AUTOINCREMENT PK (delivery order); id is logical message id.
CREATE TABLE messages (
  id                TEXT NOT NULL,
  run_id            TEXT NOT NULL DEFAULT 'run_legacy',
  delivery_contract TEXT NOT NULL DEFAULT 'current_delivery'
    CHECK(delivery_contract IN ('legacy_direct', 'current_delivery', 'audit_only')),
  from_handle       TEXT NOT NULL,
  to_handle         TEXT NOT NULL,
  subject           TEXT NOT NULL,
  body              TEXT NOT NULL DEFAULT '',
  message_type      TEXT NOT NULL DEFAULT 'status'
    CHECK(message_type IN (
      'status', 'dispatch', 'worker_done', 'merge_ready',
      'escalation', 'handoff', 'decision_gate', 'question', 'heartbeat'
    )),
  priority          TEXT NOT NULL DEFAULT 'normal'
    CHECK(priority IN ('normal', 'high', 'urgent')),
  thread_id         TEXT,
  payload           TEXT,
  read              INTEGER NOT NULL DEFAULT 0,
  sequence          INTEGER PRIMARY KEY AUTOINCREMENT,
  created_at        TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
  delivered_at      TIMESTAMP,
  sender_pane_key   TEXT
);

CREATE UNIQUE INDEX idx_messages_id ON messages(id);
CREATE INDEX idx_inbox ON messages(to_handle, read);
CREATE INDEX idx_thread ON messages(thread_id);

-- ── deliveries ─────────────────────────────────────────────────────────────
-- Crash-safe delivery batches: exactly one outstanding per run.
CREATE TABLE deliveries (
  id                  TEXT PRIMARY KEY NOT NULL,
  run_id              TEXT NOT NULL,
  consumer_generation INTEGER NOT NULL,
  message_ids         TEXT NOT NULL,
  status              TEXT NOT NULL DEFAULT 'outstanding'
    CHECK(status IN ('outstanding', 'acknowledged', 'fenced')),
  created_at          TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
  acknowledged_at     TIMESTAMP
);

CREATE UNIQUE INDEX idx_deliveries_one_outstanding
  ON deliveries(run_id) WHERE status = 'outstanding';
CREATE INDEX idx_deliveries_run_created ON deliveries(run_id, created_at);

-- ── worker_dispatches ──────────────────────────────────────────────────────
-- 9-state worker state machine.
CREATE TABLE worker_dispatches (
  dispatch_id           TEXT PRIMARY KEY NOT NULL,
  runtime_epoch         TEXT,
  state                 TEXT NOT NULL DEFAULT 'starting'
    CHECK(state IN (
      'starting', 'ready', 'start_unknown', 'failed', 'succeeded',
      'stopping', 'stop_unknown', 'stopped', 'abandoned'
    )),
  stage                 TEXT NOT NULL DEFAULT 'accepted',
  worktree_id           TEXT,
  agent_terminal_handle TEXT,
  setup_state           TEXT NOT NULL DEFAULT 'not_applicable',
  effects               TEXT NOT NULL DEFAULT '[]',
  residual_resources    TEXT NOT NULL DEFAULT '[]',
  start_options         TEXT NOT NULL DEFAULT '{}',
  last_error            TEXT,
  created_at            TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
  updated_at            TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP
);

-- ── tasks ──────────────────────────────────────────────────────────────────
-- Task DAG: spec + status + deps (JSON array of parent ids).
CREATE TABLE tasks (
  id                             TEXT PRIMARY KEY NOT NULL,
  run_id                         TEXT NOT NULL DEFAULT 'run_legacy',
  parent_id                      TEXT,
  created_by_terminal_handle     TEXT,
  created_by_pane_key            TEXT,
  created_by_process_incarnation TEXT,
  created_by_run_generation      INTEGER,
  task_title                     TEXT,
  display_name                   TEXT,
  spec                           TEXT NOT NULL,
  status                         TEXT NOT NULL DEFAULT 'pending'
    CHECK(status IN ('pending', 'ready', 'dispatched', 'completed', 'failed', 'blocked')),
  deps                           TEXT NOT NULL DEFAULT '[]',
  result                         TEXT,
  created_at                     TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
  completed_at                   TIMESTAMP
);

CREATE INDEX idx_tasks_status ON tasks(status);
CREATE INDEX idx_tasks_parent ON tasks(parent_id);

-- ── dispatch_contexts ──────────────────────────────────────────────────────
-- Capability-scoped dispatch with circuit breaker (failure_count >= 3 → circuit_broken).
CREATE TABLE dispatch_contexts (
  id                    TEXT PRIMARY KEY NOT NULL,
  run_id                TEXT NOT NULL DEFAULT 'run_legacy',
  task_id               TEXT NOT NULL,
  contract_version      INTEGER NOT NULL DEFAULT 1,
  launch_token_hash     TEXT,
  assignee_handle       TEXT,
  assignee_pane_key     TEXT,
  capability_hash       TEXT,
  process_incarnation   TEXT,
  capability_revoked_at TIMESTAMP,
  status                TEXT NOT NULL DEFAULT 'pending'
    CHECK(status IN ('pending', 'dispatched', 'completed', 'failed', 'circuit_broken', 'unknown_dispatch')),
  failure_count         INTEGER NOT NULL DEFAULT 0,
  last_failure          TEXT,
  dispatched_at         TIMESTAMP,
  completed_at          TIMESTAMP,
  created_at            TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
  last_heartbeat_at     TIMESTAMP
);

CREATE INDEX idx_dispatch_task ON dispatch_contexts(task_id);
CREATE INDEX idx_dispatch_status ON dispatch_contexts(status);
CREATE INDEX idx_dispatch_assignee_handle ON dispatch_contexts(assignee_handle);
