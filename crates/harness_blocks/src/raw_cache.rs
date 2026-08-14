//! Write-through cache of raw harness request/response payloads.

use crate::migration;
use rusqlite::{params, Connection, Row};

/// One cached raw payload.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RawEntry {
    pub id: String,
    pub session_id: String,
    /// "request" or "response" (free-form, kept as stored).
    pub direction: String,
    pub content: Vec<u8>,
    pub timestamp: i64,
}

fn row_to_entry(row: &Row<'_>) -> rusqlite::Result<RawEntry> {
    Ok(RawEntry {
        id: row.get("id")?,
        session_id: row.get("session_id")?,
        direction: row.get("direction")?,
        content: row.get::<_, Option<Vec<u8>>>("content")?.unwrap_or_default(),
        timestamp: row.get("timestamp")?,
    })
}

pub struct RawCache {
    conn: Connection,
}

impl RawCache {
    pub fn open(path: impl AsRef<str>) -> rusqlite::Result<Self> {
        let conn = Connection::open(path.as_ref())?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        migration::run(&conn)?;
        Ok(Self { conn })
    }

    pub fn open_in_memory() -> rusqlite::Result<Self> {
        let conn = Connection::open_in_memory()?;
        migration::run(&conn)?;
        Ok(Self { conn })
    }

    /// Store one raw payload; returns the generated entry id.
    pub fn insert_raw(
        &self,
        session_id: &str,
        direction: &str,
        content: &[u8],
        timestamp: i64,
    ) -> rusqlite::Result<String> {
        let id = uuid::Uuid::new_v4().to_string();
        self.conn.execute(
            "INSERT INTO raw_cache (id, session_id, direction, content, timestamp)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![id, session_id, direction, content, timestamp],
        )?;
        Ok(id)
    }

    /// Read all cached payloads for a session (oldest first) without removing them.
    pub fn peek(&self, session_id: &str) -> rusqlite::Result<Vec<RawEntry>> {
        let mut stmt = self
            .conn
            .prepare("SELECT * FROM raw_cache WHERE session_id = ?1 ORDER BY timestamp ASC")?;
        let rows = stmt.query_map(params![session_id], row_to_entry)?;
        rows.collect()
    }

    /// Take all cached payloads for a session (oldest first) and delete them.
    pub fn drain(&self, session_id: &str) -> rusqlite::Result<Vec<RawEntry>> {
        let entries = self.peek(session_id)?;
        self.conn
            .execute("DELETE FROM raw_cache WHERE session_id = ?1", params![session_id])?;
        Ok(entries)
    }
}
