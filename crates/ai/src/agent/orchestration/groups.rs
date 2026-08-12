//! GroupRouter — `@all` / `@idle` / `@worktree:<id>` / `@agentName` multicast.
//!
//! Ported from Orca `groups.ts`. Resolution happens at send-time: one message
//! per resolved recipient, sharing a `thread_id` for independent read-tracking.
//!
//! Adaptation: Orca uses `RuntimeTerminalSummary[]` (live terminal metadata).
//! Zap uses a simplified `TerminalSnapshot` — the caller provides whatever
//! terminal info is available at send-time. Agent name matching uses a
//! case-insensitive whole-word regex (same semantics as Orca's
//! `buildAgentNameRe`, simplified for our agent set).

use std::collections::HashSet;

/// Known agent-name group prefixes (Orca: `AGENT_NAME_GROUPS`).
/// Matching is case-insensitive on the group token after `@`.
pub const AGENT_NAME_GROUPS: &[&str] = &[
    "claude",
    "codex",
    "mimo",
    "gemini",
    "droid",
    "grok",
    "cursor",
];

/// Lightweight terminal snapshot used for group resolution.
///
/// Orca's `RuntimeTerminalSummary` has many fields; we only need the ones
/// that affect routing: handle, title (for agent-name matching), worktree id,
/// and agent status (for `@idle`).
#[derive(Debug, Clone)]
pub struct TerminalSnapshot {
    pub handle: String,
    pub title: Option<String>,
    pub worktree_id: Option<String>,
    pub status: Option<String>,
}

/// Returns true if `to` is a group address (starts with `@`).
pub fn is_group_address(to: &str) -> bool {
    to.starts_with('@')
}

/// Resolve a group address to concrete agent handles.
///
/// Returns an empty vec for unknown groups or groups with no current members
/// (Orca behavior: distinguish "valid group, no members" from errors).
///
/// # Arguments
/// * `to` — the address (group or direct)
/// * `sender_handle` — excluded from results to avoid self-delivery
/// * `terminals` — active terminal snapshots
pub fn resolve_group_address(
    to: &str,
    sender_handle: &str,
    terminals: &[TerminalSnapshot],
) -> Vec<String> {
    if !is_group_address(to) {
        return vec![to.to_string()];
    }

    let group = to.to_lowercase();

    // @all — broadcast to every terminal except sender
    if group == "@all" {
        return terminals
            .iter()
            .filter(|t| t.handle != sender_handle)
            .map(|t| t.handle.clone())
            .collect();
    }

    // @idle — only agents reporting 'idle' status
    if group == "@idle" {
        return terminals
            .iter()
            .filter(|t| {
                t.handle != sender_handle && t.status.as_deref() == Some("idle")
            })
            .map(|t| t.handle.clone())
            .collect();
    }

    // @worktree:<id> — all handles in a specific worktree
    if group.strip_prefix("@worktree:").is_some() {
        // Preserve original case for worktree id (case-sensitive match)
        let original_wt = &to["@worktree:".len()..];
        return terminals
            .iter()
            .filter(|t| {
                t.handle != sender_handle && t.worktree_id.as_deref() == Some(original_wt)
            })
            .map(|t| t.handle.clone())
            .collect();
    }

    // @agentName — match by terminal title (e.g. @claude → all Claude terminals)
    let agent_name = &group[1..]; // remove @
    if AGENT_NAME_GROUPS.contains(&agent_name) {
        return terminals
            .iter()
            .filter(|t| {
                if t.handle == sender_handle {
                    return false;
                }
                title_matches_agent_name(t.title.as_deref().unwrap_or(""), agent_name)
            })
            .map(|t| t.handle.clone())
            .collect();
    }

    // Unknown group → empty
    Vec::new()
}

/// Expand a message to multiple recipients if `to` is a group address.
/// Returns `(handle, thread_id)` pairs — each gets its own message record.
///
/// For direct addresses, returns a single entry.
pub fn expand_recipients(
    to: &str,
    sender_handle: &str,
    terminals: &[TerminalSnapshot],
    thread_id: Option<&str>,
) -> Vec<(String, Option<String>)> {
    if !is_group_address(to) {
        return vec![(to.to_string(), thread_id.map(|s| s.to_string()))];
    }

    // Group messages share a common thread_id so recipients can correlate.
    // If no thread_id is provided, the group address itself serves as the
    // thread key (Orca uses the same pattern).
    let shared_thread = thread_id.map(|s| s.to_string()).or(Some(to.to_string()));

    let handles = resolve_group_address(to, sender_handle, terminals);

    // Deduplicate (a terminal could match multiple criteria)
    let seen: HashSet<String> = handles.iter().cloned().collect();
    seen.into_iter()
        .map(|h| (h, shared_thread.clone()))
        .collect()
}

/// Check if a terminal title matches an agent name group.
///
/// Uses case-insensitive whole-word matching. The Orca version has special
/// handling for `cursor` (via `isCursorAgentTitle`) because "cursor" is also
/// ordinary vocabulary in another agent's task-summary title ("fix the text
/// cursor blink"). We handle this the same way: `cursor` only matches when
/// the title starts with it (agent-title pattern), while other agent names
/// match anywhere as a whole word.
fn title_matches_agent_name(title: &str, agent_name: &str) -> bool {
    let title_lower = title.to_lowercase();
    let agent_lower = agent_name.to_lowercase();

    // cursor is ambiguous — only match as an agent identity (title prefix),
    // not as a common noun anywhere in the title.
    if agent_lower == "cursor" {
        // Match "Cursor: ...", "cursor.exe ...", etc. but not "fix the text cursor"
        let first_word = title_lower.split_whitespace().next().unwrap_or("");
        let cleaned = first_word
            .trim_end_matches(|c: char| !c.is_alphanumeric())
            .trim_end_matches(".exe");
        return cleaned == "cursor";
    }

    // Other agent names: whole-word, case-insensitive match anywhere in title.
    for word in title_lower.split_whitespace() {
        let cleaned = word
            .trim_end_matches(|c: char| !c.is_alphanumeric())
            .trim_end_matches(".exe");
        if cleaned == agent_lower {
            return true;
        }
    }
    false
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn term(handle: &str, title: Option<&str>, worktree: Option<&str>, status: Option<&str>) -> TerminalSnapshot {
        TerminalSnapshot {
            handle: handle.to_string(),
            title: title.map(|s| s.to_string()),
            worktree_id: worktree.map(|s| s.to_string()),
            status: status.map(|s| s.to_string()),
        }
    }

    #[test]
    fn test_is_group_address() {
        assert!(is_group_address("@all"));
        assert!(is_group_address("@idle"));
        assert!(is_group_address("@worktree:abc"));
        assert!(is_group_address("@claude"));
        assert!(!is_group_address("term_1"));
        assert!(!is_group_address(""));
    }

    #[test]
    fn test_direct_address_passthrough() {
        let terminals = vec![term("t1", None, None, None)];
        let result = resolve_group_address("term_1", "sender", &terminals);
        assert_eq!(result, vec!["term_1"]);
    }

    #[test]
    fn test_all_excludes_sender() {
        let terminals = vec![
            term("alice", None, None, None),
            term("bob", None, None, None),
            term("sender", None, None, None),
        ];
        let result = resolve_group_address("@all", "sender", &terminals);
        assert_eq!(result.len(), 2);
        assert!(result.contains(&"alice".to_string()));
        assert!(result.contains(&"bob".to_string()));
        assert!(!result.contains(&"sender".to_string()));
    }

    #[test]
    fn test_idle_filters_by_status() {
        let terminals = vec![
            term("alice", None, None, Some("idle")),
            term("bob", None, None, Some("busy")),
            term("carol", None, None, Some("idle")),
        ];
        let result = resolve_group_address("@idle", "sender", &terminals);
        assert_eq!(result, vec!["alice", "carol"]);
    }

    #[test]
    fn test_worktree_filter() {
        let terminals = vec![
            term("alice", None, Some("wt_1"), None),
            term("bob", None, Some("wt_2"), None),
            term("carol", None, Some("wt_1"), None),
        ];
        let result = resolve_group_address("@worktree:wt_1", "sender", &terminals);
        assert_eq!(result, vec!["alice", "carol"]);
    }

    #[test]
    fn test_agent_name_match_by_title() {
        let terminals = vec![
            term("t1", Some("Claude Code"), None, None),
            term("t2", Some("Codex Worker"), None, None),
            term("t3", Some("Claude Agent"), None, None),
            term("t4", Some("Gemini CLI"), None, None),
        ];
        let result = resolve_group_address("@claude", "sender", &terminals);
        assert_eq!(result, vec!["t1", "t3"]);
    }

    #[test]
    fn test_unknown_group_empty() {
        let terminals = vec![term("t1", Some("Claude"), None, None)];
        let result = resolve_group_address("@unknown_agent", "sender", &terminals);
        assert!(result.is_empty());
    }

    #[test]
    fn test_expand_recipients_dedup() {
        let terminals = vec![
            term("alice", None, None, Some("idle")),
            term("bob", None, None, Some("idle")),
        ];
        let expanded = expand_recipients("@idle", "sender", &terminals, None);
        assert_eq!(expanded.len(), 2);
        // All share the same thread_id
        let thread = &expanded[0].1;
        for (_, tid) in &expanded {
            assert_eq!(tid, thread);
        }
    }

    #[test]
    fn test_expand_direct_no_thread() {
        let terminals = vec![];
        let expanded = expand_recipients("term_1", "sender", &terminals, None);
        assert_eq!(expanded.len(), 1);
        assert_eq!(expanded[0].0, "term_1");
        assert!(expanded[0].1.is_none());
    }

    #[test]
    fn test_title_match_word_boundary() {
        // "cursor" in "fix the text cursor blink" should NOT match
        assert!(!title_matches_agent_name("fix the text cursor blink", "cursor"));
        // "Cursor" as standalone word should match
        assert!(title_matches_agent_name("Cursor: editing files", "cursor"));
        // Case insensitive
        assert!(title_matches_agent_name("CLAUDE session", "claude"));
        // With .exe suffix
        assert!(title_matches_agent_name("grok.exe - working", "grok"));
    }
}
