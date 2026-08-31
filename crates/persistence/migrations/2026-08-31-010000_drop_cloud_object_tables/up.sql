-- 云同步裁剪:drop 8 张云表。
-- object_permissions -> object_metadata、team_members/team_settings -> teams 存在 FK 引用,
-- 按"先子后父"顺序 drop(沿用 legacy_pane_tables 的防外键残留模式)。
DROP TABLE IF EXISTS object_permissions;
DROP TABLE IF EXISTS object_metadata;
DROP TABLE IF EXISTS object_actions;
DROP TABLE IF EXISTS cloud_objects_refreshes;
DROP TABLE IF EXISTS team_members;
DROP TABLE IF EXISTS team_settings;
DROP TABLE IF EXISTS teams;
DROP TABLE IF EXISTS current_user_information;
