//! CRUD store for harness blocks, backed by SQLite (WAL mode).

use crate::migration;
use crate::schema::{BlockType, HarnessBlock};
use rusqlite::{params, Connection, OptionalExtension, Row};

/// Open (or create) a block store at `path`. Use `":memory:"` for an
/// in-memory store, or any filesystem path for a persistent one.
pub struct BlockStore {
    conn: Connection,
}

fn row_to_block(row: &Row<'_>) -> rusqlite::Result<HarnessBlock> {
    let block_type: String = row.get("block_type")?;
    let metadata: Option<String> = row.get("metadata")?;
    Ok(HarnessBlock {
        id: row.get("id")?,
        session_id: row.get("session_id")?,
        parent_id: row.get("parent_id")?,
        harness_type: row.get("harness_type")?,
        block_type: block_type
            .parse()
            .map_err(|e: crate::schema::UnknownBlockType| {
                rusqlite::Error::FromSqlConversionFailure(
                    5,
                    rusqlite::types::Type::Text,
                    Box::new(e),
                )
            })?,
        sequence: row.get::<_, i64>("sequence")? as u32,
        content: row.get::<_, Option<Vec<u8>>>("content")?.unwrap_or_default(),
        metadata: metadata
            .as_deref()
            .and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or(serde_json::Value::Null),
        timestamp: row.get("timestamp")?,
    })
}

impl BlockStore {
    pub fn open(path: impl AsRef<str>) -> rusqlite::Result<Self> {
        let conn = Connection::open(path.as_ref())?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        migration::run(&conn)?;
        Ok(Self { conn })
    }

    /// Create an in-memory store (mainly for tests).
    pub fn open_in_memory() -> rusqlite::Result<Self> {
        let conn = Connection::open_in_memory()?;
        migration::run(&conn)?;
        Ok(Self { conn })
    }

    pub fn insert_block(&self, block: &HarnessBlock) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT INTO harness_blocks
                (id, session_id, parent_id, harness_type, block_type, sequence, content, metadata, timestamp)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                block.id,
                block.session_id,
                block.parent_id,
                block.harness_type,
                block.block_type.as_str(),
                block.sequence as i64,
                &block.content,
                serde_json::to_string(&block.metadata).ok(),
                block.timestamp,
            ],
        )?;
        Ok(())
    }

    pub fn get_block(&self, id: &str) -> rusqlite::Result<Option<HarnessBlock>> {
        self.conn
            .query_row(
                "SELECT * FROM harness_blocks WHERE id = ?1",
                params![id],
                row_to_block,
            )
            .optional()
    }

    /// List blocks of a session ordered by sequence. `block_type` and
    /// `parent_id` are optional filters; `Some(None)`-style parent filtering
    /// is expressed via [`BlockStore::list_root_blocks`].
    pub fn list_blocks(
        &self,
        session_id: &str,
        block_type: Option<BlockType>,
        parent_id: Option<&str>,
    ) -> rusqlite::Result<Vec<HarnessBlock>> {
        // `(?N IS NULL OR col = ?N)` keeps the parameter count fixed so one
        // prepared statement serves every filter combination.
        let mut stmt = self.conn.prepare(
            "SELECT * FROM harness_blocks
             WHERE session_id = ?1
               AND (?2 IS NULL OR block_type = ?2)
               AND (?3 IS NULL OR parent_id = ?3)
             ORDER BY sequence ASC",
        )?;
        let rows = stmt.query_map(
            params![
                session_id,
                block_type.map(|t| t.as_str()),
                parent_id,
            ],
            row_to_block,
        )?;
        rows.collect()
    }

    /// List blocks with no parent (top of a block tree) for a session.
    pub fn list_root_blocks(&self, session_id: &str) -> rusqlite::Result<Vec<HarnessBlock>> {
        let mut stmt = self.conn.prepare(
            "SELECT * FROM harness_blocks WHERE session_id = ?1 AND parent_id IS NULL
             ORDER BY sequence ASC",
        )?;
        let rows = stmt.query_map(params![session_id], row_to_block)?;
        rows.collect()
    }

    /// List all blocks whose parent is `parent_id`, ordered by sequence.
    pub fn list_children(&self, parent_id: &str) -> rusqlite::Result<Vec<HarnessBlock>> {
        let mut stmt = self.conn.prepare(
            "SELECT * FROM harness_blocks WHERE parent_id = ?1 ORDER BY sequence ASC",
        )?;
        let rows = stmt.query_map(params![parent_id], row_to_block)?;
        rows.collect()
    }

    /// Delete all blocks of a session. Returns the number of rows removed.
    pub fn delete_session(&self, session_id: &str) -> rusqlite::Result<usize> {
        self.conn
            .execute("DELETE FROM harness_blocks WHERE session_id = ?1", params![session_id])
    }
}
