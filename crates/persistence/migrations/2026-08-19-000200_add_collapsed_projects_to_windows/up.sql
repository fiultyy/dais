-- 项目栏: 折叠(chevron 收起)的项目路径列表, JSON 数组 ["path", ...]。
ALTER TABLE windows ADD COLUMN collapsed_projects TEXT NULL;
