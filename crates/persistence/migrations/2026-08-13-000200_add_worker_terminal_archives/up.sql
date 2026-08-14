-- Worker terminal archives: frozen transcript/output snapshots for dispatched workers.
-- Ported from Orca db.ts worker_terminal_archives DDL.

CREATE TABLE worker_terminal_archives (
  dispatch_id   TEXT PRIMARY KEY NOT NULL,
  resource_id   TEXT NOT NULL,
  kind          TEXT NOT NULL CHECK(kind IN ('transcript_pin', 'terminal_tail')),
  content       TEXT NOT NULL,
  created_at    TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP
);
