-- 回滚 2026-08-29-000000: 恢复遗留列 (可空, 无数据语义)。
ALTER TABLE windows ADD COLUMN vertical_tabs_panel_open BOOLEAN;
