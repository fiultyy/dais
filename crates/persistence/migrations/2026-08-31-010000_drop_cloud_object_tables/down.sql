-- 回滚 2026-08-31-010000_drop_cloud_object_tables:
-- 按历史迁移链(add_cloud_refresh / create_object_actions / restructure_cloud_objects_refreshes /
-- add_processed_at_timestamps / add-object-metadata 及其后列演变 / add_object_permissions 及
-- persist_object_guests / persist_subject_uid / persist_object_link_sharing / create_team_settings /
-- add_uid_to_teams + make_team_server_uid_unique + drop_server_id_from_tables + add_billing_metadata_to_teams /
-- add_members_json_to_teams / add_current_user_information)折叠后的最终形态重建, 列集对照 schema.rs 现定义。
CREATE TABLE cloud_objects_refreshes (
  id INTEGER PRIMARY KEY NOT NULL,
  time_of_next_refresh DATETIME NOT NULL
);

CREATE TABLE object_actions (
  id INTEGER PRIMARY KEY NOT NULL,
  hashed_object_id TEXT NOT NULL,
  timestamp DATETIME,
  -- An enum here would be overly restrictive for future action types.
  action TEXT NOT NULL,
  data TEXT,
  count INTEGER,
  oldest_timestamp DATETIME,
  latest_timestamp DATETIME,
  pending BOOLEAN,
  processed_at_timestamp DATETIME
);

CREATE TABLE object_metadata (
    id INTEGER NOT NULL PRIMARY KEY,
    is_pending BOOLEAN NOT NULL,
    object_type TEXT NOT NULL,
    revision_ts BIGINTEGER,
    server_id TEXT,
    client_id TEXT,
    shareable_object_id INTEGER NOT NULL,
    author_id INTEGER,
    retry_count INTEGER NOT NULL,
    metadata_last_updated_ts BIGINTEGER,
    trashed_ts BIGINTEGER,
    folder_id TEXT,
    is_welcome_object BOOLEAN NOT NULL DEFAULT false,
    creator_uid TEXT,
    last_editor_uid TEXT,
    current_editor TEXT
);

CREATE TABLE object_permissions (
  id INTEGER NOT NULL PRIMARY KEY,
  object_metadata_id INTEGER NOT NULL REFERENCES object_metadata(id) ON DELETE CASCADE,
  subject_type TEXT NOT NULL,
  subject_id TEXT,
  subject_uid TEXT NOT NULL,
  permissions_last_updated_at BIGINTEGER,
  object_guests BLOB,
  anyone_with_link_access_level TEXT,
  anyone_with_link_source BLOB
);

CREATE TABLE teams (
  id integer NOT NULL PRIMARY KEY,
  name TEXT NOT NULL,
  server_uid TEXT NOT NULL UNIQUE,
  billing_metadata_json TEXT
);

CREATE TABLE team_settings (
  id INTEGER PRIMARY KEY NOT NULL,
  team_id INTEGER NOT NULL UNIQUE,
  settings_json TEXT NOT NULL,
  FOREIGN KEY (team_id) REFERENCES teams (id)
);

CREATE TABLE team_members (
  id INTEGER NOT NULL PRIMARY KEY AUTOINCREMENT,
  team_id INTEGER NOT NULL REFERENCES teams(id) ON DELETE CASCADE,
  user_uid TEXT NOT NULL,
  email TEXT NOT NULL,
  role TEXT NOT NULL
);

CREATE TABLE current_user_information (
  email TEXT PRIMARY KEY NOT NULL
);
