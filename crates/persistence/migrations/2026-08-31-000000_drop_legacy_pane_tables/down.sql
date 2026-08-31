-- 回滚 2026-08-31-000000_drop_legacy_pane_tables：
-- 按 add_code_review_pane / add_ai_memory_pane 的原 up.sql 反向重建（welcome_panes
-- 的建表迁移不在本仓迁移历史中，按同构 schema 补齐）。
CREATE TABLE welcome_panes (
  id INTEGER PRIMARY KEY NOT NULL,
  kind TEXT NOT NULL DEFAULT 'welcome' CHECK (kind = 'welcome'),

  FOREIGN KEY (id, kind) REFERENCES pane_leaves (pane_node_id, kind)
);
CREATE TABLE code_review_panes (
  id INTEGER PRIMARY KEY NOT NULL,
  kind TEXT NOT NULL DEFAULT 'code_review' CHECK (kind = 'code_review'),

  -- The UUID of the terminal this code review pane is associated with, for attaching context.
  terminal_uuid BLOB NOT NULL,

  -- The repository path the code review pane was created for.
  repo_path TEXT NOT NULL,

  FOREIGN KEY (id, kind) REFERENCES pane_leaves (pane_node_id, kind)
);
CREATE TABLE ai_memory_panes (
  id INTEGER PRIMARY KEY NOT NULL,
  kind TEXT NOT NULL DEFAULT 'ai_memory' CHECK (kind = 'ai_memory'),

  FOREIGN KEY (id, kind) REFERENCES pane_leaves (pane_node_id, kind)
);
