//! Per-session shared context: monotonic sequence counter + timestamp helper.
//!
//! Shared (via `Arc`) between the raw-event processor and the hook server so
//! that blocks coming from both sources get unique, monotonic sequence numbers
//! within a session.

use std::sync::atomic::{AtomicU32, Ordering};

/// Shared per-session state.
///
/// Cheaply clonable via `Arc`; the sequence counter is a relaxed `AtomicU32`
/// — uniqueness is guaranteed, and cross-source ordering is established by the
/// wall-clock `timestamp` on each block, not by the sequence value.
pub struct SessionContext {
    pub session_id: String,
    pub harness_type: String,
    seq: AtomicU32,
}

impl SessionContext {
    pub fn new(session_id: impl Into<String>, harness_type: impl Into<String>) -> Self {
        Self {
            session_id: session_id.into(),
            harness_type: harness_type.into(),
            seq: AtomicU32::new(0),
        }
    }

    /// Allocate the next monotonic sequence number for this session.
    pub fn next_seq(&self) -> u32 {
        self.seq.fetch_add(1, Ordering::Relaxed)
    }

    /// Current Unix epoch milliseconds.
    pub fn now_ms(&self) -> i64 {
        use std::time::{SystemTime, UNIX_EPOCH};
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0)
    }
}
