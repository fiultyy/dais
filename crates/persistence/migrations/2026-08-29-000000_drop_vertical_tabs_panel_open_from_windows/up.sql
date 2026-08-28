-- V3b 僵尸布尔裁决: vertical_tabs_panel_open 语义已死 (面板常驻, 收起退役),
-- 删除 windows 表遗留列。列由 2026-03-27-075600 迁移引入, 此后从未再读。
ALTER TABLE windows DROP COLUMN vertical_tabs_panel_open;
