-- 项目栏: 当前选中项目路径。NULL = "全部"(无过滤)。
ALTER TABLE windows ADD COLUMN active_project TEXT NULL;
