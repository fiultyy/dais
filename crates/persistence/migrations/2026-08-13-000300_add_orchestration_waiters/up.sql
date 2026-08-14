-- Orchestration waiters: live `check --wait` claims.
-- A waiter claims message types for a mailbox; the push-delivery plane
-- skips claimed messages so a blocking check and a pointer push cannot
-- double-consume the same row (Orca: messageWaitersByHandle +
-- messageTypeHasLiveWaiter, orca-runtime.ts:32636-32643).
-- `type_filter` is a JSON array of message types; `[]` claims ALL types.
-- `expires_at` is the claim TTL — the waiting process refreshes it every
-- poll tick; stale claims (dead process) expire on their own.
CREATE TABLE orchestration_waiters (
  id           TEXT PRIMARY KEY NOT NULL,
  handle       TEXT NOT NULL,
  type_filter  TEXT NOT NULL DEFAULT '[]',
  expires_at   TIMESTAMP NOT NULL
);

CREATE INDEX idx_waiters_handle ON orchestration_waiters(handle);
