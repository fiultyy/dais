//! SQL schema creation for the harness block store.

use rusqlite::Connection;

/// Create all tables and indexes if they do not exist yet.
pub fn run(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS harness_blocks (
            id TEXT PRIMARY KEY,
            session_id TEXT NOT NULL,
            parent_id TEXT,
            harness_type TEXT NOT NULL,
            block_type TEXT NOT NULL,
            sequence INTEGER NOT NULL,
            content BLOB,
            metadata TEXT,
            timestamp INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_blocks_session ON harness_blocks(session_id, sequence);
        CREATE INDEX IF NOT EXISTS idx_blocks_parent ON harness_blocks(parent_id);
        CREATE INDEX IF NOT EXISTS idx_blocks_type ON harness_blocks(session_id, block_type);

        CREATE TABLE IF NOT EXISTS raw_cache (
            id TEXT PRIMARY KEY,
            session_id TEXT NOT NULL,
            direction TEXT NOT NULL,
            content BLOB,
            timestamp INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_raw_session ON raw_cache(session_id, timestamp);
        ",
    )
}
