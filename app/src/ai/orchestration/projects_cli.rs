//! Project management for the orchestration CLI (orch-caps-v2).
//!
//! Two execution contexts share one semantic:
//! - **GUI process** (command forwarded via L2 runtime RPC): mutations go
//!   through [`crate::projects::ProjectManagementModel`] — the DB write and
//!   the [`ProjectEvent`] happen together, so the project rail refreshes
//!   event-driven (the workspace subscribes at view construction; no
//!   polling).
//! - **Headless CLI** (no live workspace): direct Diesel writes against the
//!   shared SQLite (`warp.sqlite`, WAL + busy_timeout — multi-process safe).
//!   The GUI picks the change up on its next startup / project reload.
//!
//! Tab-association guard (project-remove / worktree-remove): the `tabs`
//! table stores each tab's `project_path`. A project with referencing tabs
//! is refused unless `--force`, which resets those rows to NULL ("no
//! project") — the same end state the GUI's own removal produces
//! (`ProjectManagementModel::remove_project` leaves tabs open under "All").

use std::path::{Path, PathBuf};

use diesel::connection::SimpleConnection;
use diesel::prelude::*;
use diesel::SqliteConnection;

use crate::persistence::schema::{projects, tabs};

/// Open the shared app DB with the same pragmas the orchestration store uses.
pub(crate) fn open_db() -> anyhow::Result<SqliteConnection> {
    let db_path = crate::persistence::database_file_path();
    let url = db_path.to_string_lossy().to_string();
    let mut conn =
        SqliteConnection::establish(&url).map_err(|e| anyhow::anyhow!("open {url}: {e}"))?;
    conn.batch_execute("PRAGMA foreign_keys = ON; PRAGMA busy_timeout = 2000;")
        .map_err(|e| anyhow::anyhow!("pragmas: {e}"))?;
    Ok(conn)
}

/// Outcome of an upsert.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpsertOutcome {
    Added,
    /// Path already registered; last_opened_ts refreshed.
    Existing,
}

/// Direct-DB upsert into `projects`. Mirrors
/// `ProjectManagementModel::upsert_project` (timestamps = now).
pub fn upsert_project_db(path: &Path, conn: &mut SqliteConnection) -> anyhow::Result<UpsertOutcome> {
    let now = chrono::Utc::now().naive_utc();
    let path_str = path.to_string_lossy().to_string();
    let existing: Option<String> = projects::table
        .filter(projects::path.eq(&path_str))
        .select(projects::path)
        .first::<String>(conn)
        .optional()
        .map_err(|e| anyhow::anyhow!("query projects: {e}"))?;
    diesel::replace_into(projects::table)
        .values((
            projects::path.eq(&path_str),
            projects::added_ts.eq(now),
            projects::last_opened_ts.eq(now),
        ))
        .execute(conn)
        .map_err(|e| anyhow::anyhow!("upsert project: {e}"))?;
    Ok(if existing.is_some() {
        UpsertOutcome::Existing
    } else {
        UpsertOutcome::Added
    })
}

/// Direct-DB delete from `projects`. Returns whether the row existed.
pub fn remove_project_db(path: &Path, conn: &mut SqliteConnection) -> anyhow::Result<bool> {
    let path_str = path.to_string_lossy().to_string();
    let n = diesel::delete(projects::table.filter(projects::path.eq(&path_str)))
        .execute(conn)
        .map_err(|e| anyhow::anyhow!("delete project: {e}"))?;
    Ok(n > 0)
}

/// One row of `project-list` output.
pub struct ProjectRow {
    pub path: String,
    pub added_ts: chrono::NaiveDateTime,
    pub last_opened_ts: Option<chrono::NaiveDateTime>,
}

/// Direct-DB read of all projects, ordered by path (stable output).
pub fn list_projects_db(conn: &mut SqliteConnection) -> anyhow::Result<Vec<ProjectRow>> {
    let rows = projects::table
        .order(projects::path.asc())
        .select((projects::path, projects::added_ts, projects::last_opened_ts))
        .load::<(String, chrono::NaiveDateTime, Option<chrono::NaiveDateTime>)>(conn)
        .map_err(|e| anyhow::anyhow!("list projects: {e}"))?;
    Ok(rows
        .into_iter()
        .map(|(path, added_ts, last_opened_ts)| ProjectRow {
            path,
            added_ts,
            last_opened_ts,
        })
        .collect())
}

/// Tab ids whose `project_path` equals `path` (open terminal/tab guard).
pub fn tabs_for_project_db(path: &Path, conn: &mut SqliteConnection) -> anyhow::Result<Vec<i32>> {
    let path_str = path.to_string_lossy().to_string();
    tabs::table
        .filter(tabs::project_path.eq(&path_str))
        .select(tabs::id)
        .order(tabs::id.asc())
        .load::<i32>(conn)
        .map_err(|e| anyhow::anyhow!("query tabs: {e}"))
}

/// Reset the given tabs' project ownership to "no project" (NULL) — the
/// GUI equivalent is removing a project while its tabs stay open under "All".
pub fn detach_tabs_db(tab_ids: &[i32], conn: &mut SqliteConnection) -> anyhow::Result<usize> {
    if tab_ids.is_empty() {
        return Ok(0);
    }
    diesel::update(tabs::table.filter(tabs::id.eq_any(tab_ids)))
        .set(tabs::project_path.eq::<Option<String>>(None))
        .execute(conn)
        .map_err(|e| anyhow::anyhow!("detach tabs: {e}"))
}

/// Validate + canonicalize a project path (must exist, be a directory).
pub fn canonical_project_path(path: &str) -> anyhow::Result<PathBuf> {
    let p = Path::new(path);
    if !p.exists() {
        anyhow::bail!("path does not exist: {path}");
    }
    if !p.is_dir() {
        anyhow::bail!("path is not a directory: {path}");
    }
    let abs = p
        .canonicalize()
        .map_err(|e| anyhow::anyhow!("canonicalize {path}: {e}"))?;
    Ok(abs)
}

/// `project-add` body. Path must exist and be a directory.
pub fn project_add(path: &str, cx: &mut warpui::AppContext) -> anyhow::Result<String> {
    let abs = canonical_project_path(path)?;
    let outcome = upsert_gui_or_db(&abs, cx)?;
    Ok(match outcome {
        UpsertOutcome::Added => format!("project added: {}", abs.display()),
        UpsertOutcome::Existing => format!("project exists (refreshed): {}", abs.display()),
    })
}

/// `project-remove` body: guard (tabs) → maybe detach → remove.
pub fn project_remove(path: &str, force: bool, cx: &mut warpui::AppContext) -> anyhow::Result<String> {
    let abs = canonical_project_path(path)?;
    let mut conn = open_db()?;
    let refs = tabs_for_project_db(&abs, &mut conn)?;
    if !refs.is_empty() && !force {
        anyhow::bail!(
            "project {} still referenced by {} tab(s) [{}]: close them first or pass --force",
            abs.display(),
            refs.len(),
            refs.iter()
                .map(|i| format!("tab#{i}"))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    let detached = detach_tabs_db(&refs, &mut conn)?;
    // GUI process: route through the live model so the rail refreshes via
    // ProjectEvent (the model also deletes the DB row). Headless: plain row
    // delete above via remove_project_db.
    let removed = if in_gui_process(cx) {
        use crate::projects::ProjectManagementModel;
        use warpui::SingletonEntity as _;
        let existed = ProjectManagementModel::handle(cx)
            .read(cx, |m, _| m.all_projects().any(|p| p.path == abs.to_string_lossy()));
        if existed {
            ProjectManagementModel::handle(cx).update(cx, |m, ctx| {
                m.remove_project(abs.clone(), ctx)
            });
            true
        } else {
            false
        }
    } else {
        remove_project_db(&abs, &mut conn)?
    };
    if !removed {
        return Ok(format!("project not registered: {}", abs.display()));
    }
    Ok(format!(
        "project removed: {}{}",
        abs.display(),
        if detached > 0 {
            format!(" ({detached} tab(s) detached to no-project)")
        } else {
            String::new()
        }
    ))
}

/// `project-list` body: TSV lines (machine-parseable).
pub fn project_list() -> anyhow::Result<String> {
    let mut conn = open_db()?;
    let rows = list_projects_db(&mut conn)?;
    if rows.is_empty() {
        return Ok(String::new());
    }
    Ok(rows
        .iter()
        .map(|r| {
            format!(
                "{}\t{}\t{}",
                r.path,
                r.added_ts.format("%Y-%m-%dT%H:%M:%S"),
                r.last_opened_ts
                    .map(|t| t.format("%Y-%m-%dT%H:%M:%S").to_string())
                    .unwrap_or_else(|| "-".into())
            )
        })
        .collect::<Vec<_>>()
        .join("\n"))
}

/// Whether this process hosts live workspace views (the GUI). Used to pick
/// the event-emitting model path over the headless direct-DB path.
pub(crate) fn in_gui_process(cx: &warpui::AppContext) -> bool {
    use crate::workspace::WorkspaceRegistry;
    use warpui::SingletonEntity as _;
    !WorkspaceRegistry::handle(cx)
        .read(cx, |registry, app| registry.all_workspaces(app))
        .is_empty()
}

/// Upsert through the GUI's `ProjectManagementModel` when we're in the GUI
/// process (its upsert writes the DB via the persistence sender AND emits
/// `ProjectEvent::Added/Updated` — the workspace rail is event-driven, no
/// polling). Headless CLI (no live workspace) falls back to a direct-DB
/// write: same rows, no event needed (no rail exists).
pub(crate) fn upsert_gui_or_db(
    path: &Path,
    cx: &mut warpui::AppContext,
) -> anyhow::Result<UpsertOutcome> {
    if in_gui_process(cx) {
        use crate::projects::ProjectManagementModel;
        use warpui::SingletonEntity as _;
        let existed = ProjectManagementModel::handle(cx).read(cx, |m, _| {
            m.all_projects().any(|p| p.path == path.to_string_lossy())
        });
        ProjectManagementModel::handle(cx).update(cx, |m, ctx| {
            m.upsert_project(path.to_path_buf(), ctx)
        });
        return Ok(if existed {
            UpsertOutcome::Existing
        } else {
            UpsertOutcome::Added
        });
    }
    let mut conn = open_db()?;
    upsert_project_db(path, &mut conn)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mem_conn() -> SqliteConnection {
        let mut conn = SqliteConnection::establish(":memory:").unwrap();
        conn.batch_execute(
            "CREATE TABLE projects (path TEXT NOT NULL PRIMARY KEY, added_ts DATETIME NOT NULL, last_opened_ts DATETIME);
             CREATE TABLE tabs (id INTEGER PRIMARY KEY AUTOINCREMENT, project_path TEXT);",
        )
        .unwrap();
        conn
    }

    #[test]
    fn upsert_add_then_existing() {
        let mut conn = mem_conn();
        let p = Path::new("/tmp/proj-a");
        assert_eq!(upsert_project_db(p, &mut conn).unwrap(), UpsertOutcome::Added);
        assert_eq!(
            upsert_project_db(p, &mut conn).unwrap(),
            UpsertOutcome::Existing
        );
        let rows = list_projects_db(&mut conn).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].path, "/tmp/proj-a");
    }

    #[test]
    fn remove_with_tab_guard() {
        let mut conn = mem_conn();
        let p = Path::new("/tmp/proj-b");
        upsert_project_db(p, &mut conn).unwrap();
        diesel::insert_into(tabs::table)
            .values(vec![
                (tabs::project_path.eq(Some("/tmp/proj-b")),),
                (tabs::project_path.eq(Some("/tmp/proj-b")),),
                (tabs::project_path.eq(Some("/tmp/other")),),
            ])
            .execute(&mut conn)
            .unwrap();
        // guard lists exactly the referencing tabs
        assert_eq!(tabs_for_project_db(p, &mut conn).unwrap().len(), 2);
        // detach resets only those rows
        let ids = tabs_for_project_db(p, &mut conn).unwrap();
        assert_eq!(detach_tabs_db(&ids, &mut conn).unwrap(), 2);
        assert_eq!(tabs_for_project_db(p, &mut conn).unwrap().len(), 0);
        assert!(remove_project_db(p, &mut conn).unwrap());
        assert!(list_projects_db(&mut conn).unwrap().is_empty());
    }

    #[test]
    fn remove_missing_is_false() {
        let mut conn = mem_conn();
        assert!(!remove_project_db(Path::new("/tmp/nope"), &mut conn).unwrap());
    }
}
