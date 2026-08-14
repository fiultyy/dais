//! Process-wide singleton for the orchestration `DieselOrchestrationStore`.
//!
//! Mirrors `warp_ssh_manager::db`: a second write connection to the same
//! `warp.sqlite`. SQLite WAL mode supports concurrent write connections
//! (writes are serialized by busy_timeout). Orchestration writes are
//! infrequent (CLI invocations, background router), so contention with the
//! main writer thread is negligible.
//!
//! Lifecycle:
//! 1. `set_database_path(path)` — called once at app launch, after migrations.
//! 2. `store()` — lazily opens the connection on first access, returns `&'static`.

use std::path::PathBuf;
use std::sync::OnceLock;

use diesel::connection::SimpleConnection;
use diesel::prelude::*;
use diesel::sqlite::SqliteConnection;

use super::store::DieselOrchestrationStore;

static DB_PATH: OnceLock<PathBuf> = OnceLock::new();
static STORE: OnceLock<DieselOrchestrationStore> = OnceLock::new();

/// Set the database path. Called once at app launch. Subsequent calls are
/// silently ignored (OnceLock semantics).
pub fn set_database_path(path: PathBuf) {
    let _ = DB_PATH.set(path);
}

fn open_store() -> DieselOrchestrationStore {
    let path = DB_PATH
        .get()
        .expect("orchestration::connection: database path not initialized");
    let url = path.to_string_lossy();
    let mut conn = SqliteConnection::establish(&url)
        .expect("orchestration::connection: establish failed");
    conn.batch_execute(
        "PRAGMA foreign_keys = ON; \
         PRAGMA busy_timeout = 2000; \
         PRAGMA journal_mode = WAL;",
    )
    .expect("orchestration::connection: PRAGMA failed");
    DieselOrchestrationStore::new(conn)
}

/// Lazily initialize and return the process-wide store.
///
/// Returns `&'static DieselOrchestrationStore` — the connection lives for the
/// entire process lifetime once opened. Panics on init if the DB path was not
/// set via `set_database_path` before first access (same pattern as
/// `warp_ssh_manager::db::with_conn`).
pub fn store() -> &'static DieselOrchestrationStore {
    STORE.get_or_init(open_store)
}
