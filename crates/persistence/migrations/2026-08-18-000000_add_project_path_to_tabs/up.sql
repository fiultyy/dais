-- 项目栏: tab 归属项目路径。NULL = 无项目(兼容旧快照, 恒可见)。
ALTER TABLE tabs ADD COLUMN project_path TEXT NULL;
