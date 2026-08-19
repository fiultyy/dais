-- 项目栏: 每项目上次聚焦 tab(对快照 tabs 数组的索引), JSON 对象 {path: index}。
ALTER TABLE windows ADD COLUMN last_active_tab_index_by_project TEXT NULL;
