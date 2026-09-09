-- 三个 pane 表在裁剪后不再持久化（welcome / code_review / ai_memory pane 均已移除）。
-- 先清 pane_leaves 中 legacy kind 行及由此产生的孤儿 pane_nodes，防止恢复期
-- read_node 因未知 kind 整 tab 静默丢弃（含同 tab 内无辜的 terminal pane）。
DELETE FROM pane_leaves WHERE kind IN ('welcome', 'get_started', 'code_review', 'ai_fact', 'ai_memory');
DELETE FROM pane_nodes WHERE is_leaf = 1 AND id NOT IN (SELECT pane_node_id FROM pane_leaves);
-- 一次性 drop 三表。
DROP TABLE IF EXISTS welcome_panes;
DROP TABLE IF EXISTS code_review_panes;
DROP TABLE IF EXISTS ai_memory_panes;
