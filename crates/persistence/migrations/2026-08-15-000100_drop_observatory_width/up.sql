-- 观测台 tab 化后宽度由 pane 布局决定，Resizable 持久化链路整体移除。
-- 回滚 2026-08-15-000000_add_observatory_width（该列在已部署 DB 中存在）。
ALTER TABLE windows DROP COLUMN observatory_width;
