-- Decision gates: 3-state (pending/resolved/timeout) gate for blocking task decisions.
-- Ported from Orca db.ts createTables().

CREATE TABLE decision_gates (
  id            TEXT PRIMARY KEY NOT NULL,
  run_id        TEXT NOT NULL DEFAULT 'run_legacy',
  task_id       TEXT NOT NULL,
  question      TEXT NOT NULL,
  options       TEXT NOT NULL DEFAULT '[]',
  status        TEXT NOT NULL DEFAULT 'pending'
    CHECK(status IN ('pending', 'resolved', 'timeout')),
  resolution    TEXT,
  created_at    TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
  resolved_at   TIMESTAMP
);

CREATE INDEX idx_gates_task ON decision_gates(task_id);
CREATE INDEX idx_gates_status ON decision_gates(status);
