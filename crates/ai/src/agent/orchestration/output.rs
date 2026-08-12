//! Transcript storage — worker output capture and retrieval.
//!
//! Ported from Orca `worker-output-archive.ts` + `storeWorkerTerminalArchive` /
//! `getWorkerTerminalArchive`.
//!
//! Two archive kinds:
//! - `transcript_pin`: structured provider transcript snapshot (JSON messages)
//! - `terminal_tail`: bounded redacted raw terminal output
//!
//! The archive is frozen at dispatch teardown — one durable copy per dispatch,
//! upserted by dispatch_id.

use chrono::NaiveDateTime;
use diesel::prelude::*;

use super::db::{OrchestrationError, OrchestrationResult};

// ---------------------------------------------------------------------------
// DB model
// ---------------------------------------------------------------------------

/// `worker_terminal_archives` table — frozen output snapshot per dispatch.
#[derive(Debug, Clone, Queryable)]
pub struct WorkerTerminalArchive {
    pub dispatch_id: String,
    pub resource_id: String,
    /// "transcript_pin" or "terminal_tail"
    pub kind: String,
    /// JSON content (format depends on `kind`)
    pub content: String,
    pub created_at: NaiveDateTime,
}

/// Archive kind discriminator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveKind {
    /// Structured provider transcript snapshot.
    TranscriptPin,
    /// Bounded redacted terminal output.
    TerminalTail,
}

impl ArchiveKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            ArchiveKind::TranscriptPin => "transcript_pin",
            ArchiveKind::TerminalTail => "terminal_tail",
        }
    }

    pub fn parse_kind(s: &str) -> OrchestrationResult<Self> {
        match s {
            "transcript_pin" => Ok(ArchiveKind::TranscriptPin),
            "terminal_tail" => Ok(ArchiveKind::TerminalTail),
            other => Err(OrchestrationError::InvalidEnum {
                context: "WorkerTerminalArchive.kind",
                value: other.to_string(),
            }),
        }
    }
}

impl WorkerTerminalArchive {
    pub fn typed_kind(&self) -> OrchestrationResult<ArchiveKind> {
        ArchiveKind::parse_kind(&self.kind)
    }
}

// ---------------------------------------------------------------------------
// Content structs (JSON payloads stored in the `content` column)
// ---------------------------------------------------------------------------

/// Content of a `terminal_tail` archive — bounded redacted terminal output.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TerminalTailContent {
    pub lines: Vec<String>,
    pub truncated: bool,
    pub terminal_status: String,
    pub warnings: Vec<String>,
}

/// Content of a `transcript_pin` archive — structured provider messages.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TranscriptPinContent {
    pub agent: String,
    pub process_incarnation: String,
    /// Arbitrary JSON messages (provider-specific schema).
    pub messages: serde_json::Value,
    pub limited: bool,
    pub warnings: Vec<String>,
}

// ---------------------------------------------------------------------------
// Bounding (ported from Orca `boundArchiveLines`)
// ---------------------------------------------------------------------------

/// Maximum chars in a terminal tail archive (Orca: 262_144).
pub const TERMINAL_ARCHIVE_MAX_CHARS: usize = 262_144;

/// Bound terminal lines to `TERMINAL_ARCHIVE_MAX_CHARS`, keeping the tail.
/// Ported from Orca `boundArchiveLines`.
pub fn bound_archive_lines(lines: Vec<String>) -> (Vec<String>, bool) {
    let total: usize = lines.iter().map(|l| l.len() + 1).sum();
    if total <= TERMINAL_ARCHIVE_MAX_CHARS {
        return (lines, false);
    }

    let mut kept: Vec<String> = Vec::new();
    let mut budget = TERMINAL_ARCHIVE_MAX_CHARS;

    for line in lines.iter().rev() {
        let cost = line.len() + 1;
        if cost > budget {
            // Partial keep of the last line if we still have budget
            if kept.is_empty() && budget > 1 {
                kept.insert(0, line[line.len().saturating_sub(budget - 1)..].to_string());
            }
            break;
        }
        kept.insert(0, line.clone());
        budget -= cost;
    }

    (kept, true)
}

// ---------------------------------------------------------------------------
// Cursor-based transcript reading
// ---------------------------------------------------------------------------

/// A cursor into a transcript — line index for terminal_tail, message index
/// for transcript_pin.
#[derive(Debug, Clone, Default)]
pub struct TranscriptCursor {
    pub offset: usize,
    pub limit: usize,
}

/// A page of transcript lines read from an archive.
#[derive(Debug, Clone)]
pub struct TranscriptPage {
    pub lines: Vec<String>,
    pub next_cursor: Option<usize>,
    pub truncated: bool,
}

/// Read a page of transcript lines from a terminal_tail archive content.
///
/// Uses cursor-based pagination: returns up to `cursor.limit` lines starting
/// at `cursor.offset`. `next_cursor` is `Some(offset + lines.len())` if more
/// lines remain.
pub fn read_transcript(
    content: &TerminalTailContent,
    cursor: &TranscriptCursor,
) -> TranscriptPage {
    let limit = if cursor.limit == 0 {
        content.lines.len()
    } else {
        cursor.limit
    };

    let start = cursor.offset.min(content.lines.len());
    let end = (start + limit).min(content.lines.len());

    let lines: Vec<String> = content.lines[start..end].to_vec();
    let has_more = end < content.lines.len();

    TranscriptPage {
        next_cursor: if has_more { Some(end) } else { None },
        truncated: content.truncated,
        lines,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bound_archive_lines_no_truncation() {
        let lines = vec!["line1".into(), "line2".into()];
        let (kept, truncated) = bound_archive_lines(lines);
        assert_eq!(kept.len(), 2);
        assert!(!truncated);
    }

    #[test]
    fn test_bound_archive_lines_truncation() {
        // Create lines that exceed the max
        let big_line = "x".repeat(100_000);
        let lines = vec![big_line.clone(); 10]; // 1M chars > 262_144
        let (kept, truncated) = bound_archive_lines(lines);
        assert!(truncated);
        assert!(kept.iter().map(|l| l.len() + 1).sum::<usize>() <= TERMINAL_ARCHIVE_MAX_CHARS + 1);
    }

    #[test]
    fn test_read_transcript_pagination() {
        let content = TerminalTailContent {
            lines: (0..100).map(|i| format!("line {}", i)).collect(),
            truncated: false,
            terminal_status: "exited".into(),
            warnings: vec![],
        };

        // Page 1: offset 0, limit 10
        let page = read_transcript(&content, &TranscriptCursor { offset: 0, limit: 10 });
        assert_eq!(page.lines.len(), 10);
        assert_eq!(page.lines[0], "line 0");
        assert_eq!(page.lines[9], "line 9");
        assert_eq!(page.next_cursor, Some(10));

        // Page 2: offset 10, limit 10
        let page = read_transcript(&content, &TranscriptCursor { offset: 10, limit: 10 });
        assert_eq!(page.lines.len(), 10);
        assert_eq!(page.lines[0], "line 10");
        assert_eq!(page.next_cursor, Some(20));

        // Last page
        let page = read_transcript(&content, &TranscriptCursor { offset: 90, limit: 10 });
        assert_eq!(page.lines.len(), 10);
        assert_eq!(page.next_cursor, None);

        // Beyond end
        let page = read_transcript(&content, &TranscriptCursor { offset: 100, limit: 10 });
        assert_eq!(page.lines.len(), 0);
        assert_eq!(page.next_cursor, None);
    }

    #[test]
    fn test_read_transcript_no_limit() {
        let content = TerminalTailContent {
            lines: (0..5).map(|i| format!("line {}", i)).collect(),
            truncated: false,
            terminal_status: "exited".into(),
            warnings: vec![],
        };

        let page = read_transcript(&content, &TranscriptCursor { offset: 0, limit: 0 });
        assert_eq!(page.lines.len(), 5);
        assert_eq!(page.next_cursor, None);
    }

    #[test]
    fn test_archive_kind_roundtrip() {
        assert_eq!(ArchiveKind::TranscriptPin.as_str(), "transcript_pin");
        assert_eq!(ArchiveKind::TerminalTail.as_str(), "terminal_tail");
        assert_eq!(
            ArchiveKind::parse_kind("transcript_pin").unwrap(),
            ArchiveKind::TranscriptPin
        );
        assert!(ArchiveKind::parse_kind("invalid").is_err());
    }
}
