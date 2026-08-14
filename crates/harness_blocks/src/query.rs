//! Read-oriented query helpers on top of [`BlockStore`].

use crate::schema::{BlockType, HarnessBlock};
use crate::store::BlockStore;

/// All blocks of a session, ordered by sequence.
pub fn list_blocks_by_session(
    store: &BlockStore,
    session_id: &str,
) -> rusqlite::Result<Vec<HarnessBlock>> {
    store.list_blocks(session_id, None, None)
}

/// All blocks of a session with the given type, ordered by sequence.
pub fn list_blocks_by_type(
    store: &BlockStore,
    session_id: &str,
    block_type: BlockType,
) -> rusqlite::Result<Vec<HarnessBlock>> {
    store.list_blocks(session_id, Some(block_type), None)
}

/// All blocks whose parent is `parent_id` (across sessions), ordered by sequence.
pub fn list_child_blocks(
    store: &BlockStore,
    parent_id: &str,
) -> rusqlite::Result<Vec<HarnessBlock>> {
    store.list_children(parent_id)
}

/// The most recent `system_prompt` block of a session, if any.
pub fn get_system_prompt(
    store: &BlockStore,
    session_id: &str,
) -> rusqlite::Result<Option<HarnessBlock>> {
    Ok(list_blocks_by_type(store, session_id, BlockType::SystemPrompt)?
        .pop())
}

/// Aggregate counts for a session.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct SessionSummary {
    pub session_id: String,
    pub block_count: u64,
    pub first_timestamp: Option<i64>,
    pub last_timestamp: Option<i64>,
    pub block_type_counts: std::collections::BTreeMap<String, u64>,
}

pub fn get_session_summary(
    store: &BlockStore,
    session_id: &str,
) -> rusqlite::Result<SessionSummary> {
    let blocks = list_blocks_by_session(store, session_id)?;
    let mut block_type_counts = std::collections::BTreeMap::new();
    for b in &blocks {
        *block_type_counts.entry(b.block_type.to_string()).or_insert(0) += 1;
    }
    let first_timestamp = blocks.first().map(|b| b.timestamp);
    let last_timestamp = blocks.last().map(|b| b.timestamp);
    Ok(SessionSummary {
        session_id: session_id.to_string(),
        block_count: blocks.len() as u64,
        first_timestamp,
        last_timestamp,
        block_type_counts,
    })
}
