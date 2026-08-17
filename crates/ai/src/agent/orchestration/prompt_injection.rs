//! Agent prompt injection — pure byte/string construction for driving other
//! harnesses' terminals.
//!
//! Ported from Orca:
//! - `agent-prompt-injection.ts` — bracketed paste framing + submit delay
//! - `agent-title-status.ts` — OSC title → agent status
//! - `preamble.ts` — dispatch preamble template
//! - `orca-runtime.ts` (wait-blocked detection patterns)
//!
//! This module is pure (no GPUI / no I/O) so all logic is unit-testable.
//! App-layer callers pair these constructors with `PtyExecutor::write_bytes`.

use super::executor::PtyExecutor;
use super::db::OrchestrationResult;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Delay between the final paste byte and the submit CR. Agent TUIs can swallow
/// a `\r` that arrives in the same PTY write as the paste frame.
pub const AGENT_PROMPT_SUBMIT_DELAY_MS: u64 = 500;

/// Submit key written after the delay (Enter).
pub const AGENT_PROMPT_SUBMIT: &[u8] = b"\r";

/// Interrupt key (Ctrl-C) for `split_terminal_action`.
pub const TERMINAL_INTERRUPT: &[u8] = b"\x03";

/// Max tail buffer size for wait-blocked scanning.
pub const TAIL_SCAN_MAX_LINES: usize = 2000;
pub const TAIL_SCAN_MAX_BYTES: usize = 256 * 1024;

// ---------------------------------------------------------------------------
// Paste framing
// ---------------------------------------------------------------------------

/// Replace ESC (0x1B) bytes with the literal text `<ESC>` so the target agent
/// cannot interpret injected escape sequences.
pub fn sanitize_agent_prompt_text(text: &str) -> String {
    text.replace('\x1b', "<ESC>")
}

/// Frame text as a bracketed paste: `ESC[200~` + sanitized text + `ESC[201~`.
///
/// Newlines inside the frame are preserved as content (not submits); the final
/// CR is written separately after [`AGENT_PROMPT_SUBMIT_DELAY_MS`].
pub fn build_agent_prompt_paste_bytes(text: &str) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(text.len() + 12);
    bytes.extend_from_slice(b"\x1b[200~");
    bytes.extend_from_slice(sanitize_agent_prompt_text(text).as_bytes());
    bytes.extend_from_slice(b"\x1b[201~");
    bytes
}

/// Write an agent prompt to a terminal: paste frame, delay, then submit CR.
/// Ported from Orca `sendTerminalAgentPrompt`.
pub fn send_agent_prompt(executor: &dyn PtyExecutor, handle: &str, prompt: &str) -> OrchestrationResult<()> {
    executor.write_to_pty(handle, &build_agent_prompt_paste_bytes(prompt))?;
    std::thread::sleep(std::time::Duration::from_millis(AGENT_PROMPT_SUBMIT_DELAY_MS));
    executor.write_to_pty(handle, AGENT_PROMPT_SUBMIT)
}

// ---------------------------------------------------------------------------
// Terminal action (interactive answers)
// ---------------------------------------------------------------------------

/// A terminal input action split into two writes: text payload first, then a
/// suffix (`\r` for enter, `\x03` for interrupt) after the submit delay.
/// Ported from Orca `writeTerminalAction`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalActionBytes {
    pub payload: Vec<u8>,
    pub suffix: Option<Vec<u8>>,
}

/// Split an interactive answer into payload + delayed suffix.
///
/// Typical y/n answer: `{text: "y", enter: true}` → write `y`, wait 500ms,
/// write `\r`.
pub fn split_terminal_action(text: Option<&str>, enter: bool, interrupt: bool) -> TerminalActionBytes {
    let payload = text.map(|t| sanitize_agent_prompt_text(t).into_bytes()).unwrap_or_default();
    let suffix = if interrupt {
        Some(TERMINAL_INTERRUPT.to_vec())
    } else if enter {
        Some(AGENT_PROMPT_SUBMIT.to_vec())
    } else {
        None
    };
    TerminalActionBytes { payload, suffix }
}

/// Write a terminal action with the 500ms text/suffix separation.
pub fn send_terminal_action(
    executor: &dyn PtyExecutor,
    handle: &str,
    text: Option<&str>,
    enter: bool,
    interrupt: bool,
) -> OrchestrationResult<()> {
    let action = split_terminal_action(text, enter, interrupt);
    if !action.payload.is_empty() {
        executor.write_to_pty(handle, &action.payload)?;
    }
    if let Some(suffix) = action.suffix {
        std::thread::sleep(std::time::Duration::from_millis(AGENT_PROMPT_SUBMIT_DELAY_MS));
        executor.write_to_pty(handle, &suffix)?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Agent status from OSC title
// ---------------------------------------------------------------------------

/// Agent status inferred from the terminal title.
/// Ported from Orca `AgentStatus` (title detection subset).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentTerminalStatus {
    /// At prompt, can accept a new prompt injection.
    Idle,
    /// Actively processing a turn.
    Working,
    /// Waiting on a permission / interactive answer.
    Permission,
}

const CLAUDE_IDLE: &str = "✳ claude";
const GEMINI_IDLE: &str = "gemini idle";
const GEMINI_WORKING: &str = "gemini working";
const GEMINI_PERMISSION: &str = "gemini permission";
const GEMINI_SILENT_WORKING: &str = "gemini thinking";
const CURSOR_NATIVE_TITLE_LOWER: &str = "cursor";

const SPINNER_GLYPHS: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏", "◐", "◓", "◑", "◒"];

/// Known agent CLI name fragments that make a title agent-attributed.
const AGENT_NAME_FRAGMENTS: &[&str] = &[
    "claude", "codex", "gemini", "grok", "omp", "pi", "droid", "hermes", "agy", "cursor",
];

fn contains_spinner_glyph(lower: &str) -> bool {
    SPINNER_GLYPHS.iter().any(|g| lower.contains(g))
}

fn contains_any(lower: &str, needles: &[&str]) -> bool {
    needles.iter().any(|n| lower.contains(n))
}

/// Boundary-aware idle keywords (word-ish match to avoid cwd substrings).
fn has_strong_idle_keyword(lower: &str) -> bool {
    // "idle", "ready", "done", "complete(d)", "waiting for input" as words.
    lower.split(|c: char| !c.is_ascii_alphanumeric())
        .any(|w| matches!(w, "idle" | "ready" | "done" | "complete" | "completed" | "finished"))
}

fn has_strong_working_keyword(lower: &str) -> bool {
    lower.split(|c: char| !c.is_ascii_alphanumeric())
        .any(|w| matches!(w, "working" | "running" | "thinking" | "processing" | "executing" | "generating"))
}

/// Detect agent status from an OSC 0/1/2 terminal title.
/// Ported from Orca `detectAgentStatusFromTitle` (core paths; hook-based and
/// provider-specific native titles are future work).
///
/// Returns `None` when the title carries no agent signal.
pub fn detect_agent_status_from_title(title: &str) -> Option<AgentTerminalStatus> {
    let trimmed = title.trim();
    if trimmed.is_empty() {
        return None;
    }
    let lower = trimmed.to_lowercase();

    if lower == CURSOR_NATIVE_TITLE_LOWER {
        return None;
    }

    // Gemini explicit markers
    if lower.contains(GEMINI_PERMISSION) {
        return Some(AgentTerminalStatus::Permission);
    }
    if lower.contains(GEMINI_WORKING) || lower.contains(GEMINI_SILENT_WORKING) {
        return Some(AgentTerminalStatus::Working);
    }
    if lower.contains(GEMINI_IDLE) {
        return Some(AgentTerminalStatus::Idle);
    }

    // Pi/OMP synthetic labels: "[permission]" / "idle" markers
    if lower.contains("action required") || lower.contains("permission") || lower.contains("waiting") {
        // Only meaningful on agent-attributed titles; checked again below, but
        // the explicit marker wins when any agent fragment is present.
        if contains_any(&lower, AGENT_NAME_FRAGMENTS) || lower.starts_with('[') {
            return Some(AgentTerminalStatus::Permission);
        }
    }

    // Claude idle prefix ("✳ claude …" idle suffix form)
    if lower.starts_with(CLAUDE_IDLE) {
        return Some(AgentTerminalStatus::Idle);
    }

    if contains_spinner_glyph(&lower) {
        return Some(AgentTerminalStatus::Working);
    }

    let has_agent_name = contains_any(&lower, AGENT_NAME_FRAGMENTS);
    if !has_agent_name {
        return None;
    }

    if lower.starts_with(". ") {
        return Some(AgentTerminalStatus::Working);
    }
    if lower.starts_with("* ") {
        return Some(AgentTerminalStatus::Idle);
    }
    if has_strong_idle_keyword(&lower) {
        return Some(AgentTerminalStatus::Idle);
    }
    if has_strong_working_keyword(&lower) {
        return Some(AgentTerminalStatus::Working);
    }

    // Agent-attributed title with no other signal: assume idle (at prompt).
    Some(AgentTerminalStatus::Idle)
}

// ---------------------------------------------------------------------------
// Wait-blocked detection (interactive prompts in the tail)
// ---------------------------------------------------------------------------

/// Why a terminal is blocked waiting for interactive input.
/// Ported from Orca `findTerminalWaitBlockedSignal` reason ids.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaitBlockedReason {
    CodexUpdatePrompt,
    CodexCwdPrompt,
    CodexModelMigrationPrompt,
    CodexHooksReviewPrompt,
    CodexTrustWorkspace,
    CodexInteractivePrompt,
    PermissionPrompt,
}

impl WaitBlockedReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::CodexUpdatePrompt => "codex-update-prompt",
            Self::CodexCwdPrompt => "codex-cwd-prompt",
            Self::CodexModelMigrationPrompt => "codex-model-migration-prompt",
            Self::CodexHooksReviewPrompt => "codex-hooks-review-prompt",
            Self::CodexTrustWorkspace => "codex-trust-workspace",
            Self::CodexInteractivePrompt => "codex-interactive-prompt",
            Self::PermissionPrompt => "permission-prompt",
        }
    }
}

/// Scan a terminal tail (lowercased by the caller is NOT required; matching is
/// case-insensitive) for known wait-blocked signals.
pub fn find_wait_blocked_signal(tail: &str) -> Option<WaitBlockedReason> {
    let lower = tail.to_lowercase();

    let has = |needle: &str| lower.contains(needle);

    if has("update available") && has("press enter to continue") {
        return Some(WaitBlockedReason::CodexUpdatePrompt);
    }
    if has("choose working directory to") && has("press enter to continue") {
        return Some(WaitBlockedReason::CodexCwdPrompt);
    }
    if has("codex just got an upgrade") && has("press enter to continue") {
        return Some(WaitBlockedReason::CodexModelMigrationPrompt);
    }
    if has("hooks need review") && has("press enter to confirm") {
        return Some(WaitBlockedReason::CodexHooksReviewPrompt);
    }
    if has("trusted workspace") && has_any(&lower, &["workspace", "folder", "directory", "repo"]) {
        return Some(WaitBlockedReason::CodexTrustWorkspace);
    }
    if (has("press enter to confirm")
        || has("press enter to continue")
        || has("press enter to view")
        || has("press enter to insert")
        || has("press t to trust"))
        && has_any(&lower, &["codex", "permission", "sandbox", "trust", "hook"])
    {
        return Some(WaitBlockedReason::CodexInteractivePrompt);
    }
    if (has("permission required") || has("requires permission"))
        && has_any(&lower, &["allow once", "allow always", "reject", "deny"])
    {
        return Some(WaitBlockedReason::PermissionPrompt);
    }
    None
}

fn has_any(lower: &str, needles: &[&str]) -> bool {
    needles.iter().any(|n| lower.contains(n))
}

// ---------------------------------------------------------------------------
// Dispatch preamble
// ---------------------------------------------------------------------------

/// Preamble construction parameters.
/// Ported from Orca `PreambleParams` (drift / capability omitted — zap has no
/// dispatch capability tokens or base-drift preflight yet).
pub struct PreambleParams<'a> {
    pub task_id: &'a str,
    pub dispatch_id: &'a str,
    pub task_spec: &'a str,
    pub coordinator_handle: &'a str,
    pub worker_handle: &'a str,
    /// CLI binary the worker should invoke (`dais` in zap).
    pub cli_command: &'a str,
    /// Prompt-returning agents idle after worker_done; bare shells exit.
    pub worker_kind: WorkerKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkerKind {
    PromptReturningAgent,
    BareShell,
}

/// Heartbeat cadence taught to workers (minutes).
pub const HEARTBEAT_INTERVAL_MIN: u32 = 5;

/// Build the dispatch preamble: CLI instructions + behavioral rules + TASK block.
/// Ported from Orca `buildDispatchPreamble`; the CLI examples use zap's actual
/// command surface and snake_case JSON payloads (matching zap's
/// `WorkerDonePayload` / `HeartbeatPayload` serde shapes).
pub fn build_dispatch_preamble(params: &PreambleParams) -> String {
    let cli = params.cli_command;
    let wh = params.worker_handle;

    let post_done = match params.worker_kind {
        WorkerKind::BareShell => format!(
            "=== AFTER YOU SEND worker_done ===\n\n\
worker_done ends your turn for this task. Your dispatched work is complete:\n\
stop and take no further actions — do NOT start new or unrelated work,\n\
do NOT run a sleep/poll loop, and do NOT keep calling\n\
`{cli} orchestration check-status`. The coordinator has already recorded your\n\
completion and expects no further output.\n\n\
Exit the shell after completion. Bare-shell workers have no idle agent\n\
prompt for the coordinator to reuse; if there is more work it will dispatch\n\
another worker with a fresh TASK block."
        ),
        WorkerKind::PromptReturningAgent => format!(
            "=== AFTER YOU SEND worker_done ===\n\n\
worker_done ends your turn for this task. Your dispatched work is complete:\n\
stop, return to an idle prompt, and take no further actions — do NOT start\n\
new or unrelated work, do NOT run a sleep/poll loop, and do NOT keep calling\n\
`{cli} orchestration check-status`. The coordinator has already recorded your\n\
completion and expects no further output.\n\n\
Do not exit the shell. Your terminal stays available, and if the\n\
coordinator has more for you it will re-engage this terminal with a fresh\n\
preamble + TASK block, which arrives as new input. When that happens,\n\
reset and start the new task; ignore the previous task's follow-ups."
        ),
    };

    format!(
        "You are working inside zap, a multi-agent terminal. You are a dispatched worker.\n\
Your coordinator's terminal handle is: {coordinator}\n\
Your task ID is: {task_id}\n\n\
You talk to the coordinator only through the CLI commands below. Do not use\n\
any other channel to reach a human during the run.\n\n\
=== CLI COMMANDS ===\n\n\
  # Report the terminal task outcome (REQUIRED exactly once).\n\
  #\n\
  # RULE: --body must be a 3-sentence executive summary (what you did,\n\
  # what you found, what's left). Never send an empty body.\n\
  #\n\
  # RULE: send worker_done exactly once. --outcome succeeded when the\n\
  # requested work is done, --outcome failed when it is not. Never encode\n\
  # failure only in prose and never silently exit. Include BOTH task_id and\n\
  # dispatch_id so a late completion from a failed retry cannot complete the\n\
  # current dispatch.\n\
  {cli} orchestration send-message <run_id> --from {wh} --to {coordinator} \\\n\
    --message-type worker_done --subject \"<short status>\" \\\n\
    --body '{{\"task_id\":\"{task_id}\",\"dispatch_id\":\"{dispatch_id}\",\"outcome\":\"succeeded\",\"result\":\"<3-sentence summary>\"}}'\n\n\
  # BEHAVIOR RULE: send a heartbeat every {hb} minutes while actively working.\n\
  # The coordinator uses this to distinguish \"still thinking\" from \"hung\".\n\
  {cli} orchestration send-message <run_id> --from {wh} --to {coordinator} \\\n\
    --message-type heartbeat --subject \"alive\" \\\n\
    --body '{{\"dispatch_id\":\"{dispatch_id}\"}}'\n\n\
  # NEVER use AskUserQuestion — it opens a local TUI prompt the coordinator\n\
  # cannot see or answer; your session will hang forever. Every interactive\n\
  # question goes through --message-type question or decision_gate instead.\n\n\
  # Escalate a blocker (pre-completion, when you need the coordinator to act):\n\
  {cli} orchestration send-message <run_id> --from {wh} --to {coordinator} \\\n\
    --message-type escalation --subject \"Blocked: <reason>\" \\\n\
    --body \"<details>\"\n\n\
  # Check for messages from the coordinator:\n\
  {cli} orchestration check-status\n\n\
{post_done}\n\n\
=== TASK ===\n\
{spec}",
        coordinator = params.coordinator_handle,
        task_id = params.task_id,
        dispatch_id = params.dispatch_id,
        wh = wh,
        cli = cli,
        hb = HEARTBEAT_INTERVAL_MIN,
        spec = params.task_spec,
    )
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::orchestration::executor::MockPtyExecutor;

    #[test]
    fn test_sanitize_replaces_esc() {
        assert_eq!(sanitize_agent_prompt_text("a\x1b[2Jb"), "a<ESC>[2Jb");
        assert_eq!(sanitize_agent_prompt_text("clean"), "clean");
    }

    #[test]
    fn test_paste_frame_bytes() {
        let bytes = build_agent_prompt_paste_bytes("hi\nthere");
        assert_eq!(&bytes[..6], b"\x1b[200~");
        assert_eq!(&bytes[bytes.len() - 6..], b"\x1b[201~");
        let inner = &bytes[6..bytes.len() - 6];
        assert_eq!(inner, b"hi\nthere");
    }

    #[test]
    fn test_send_agent_prompt_two_writes_with_delay() {
        let exec = MockPtyExecutor::default();
        send_agent_prompt(&exec, "ctx_1", "do work").unwrap();
        let writes = exec.writes.lock();
        assert_eq!(writes.len(), 2);
        assert_eq!(writes[0].0, "ctx_1");
        assert_eq!(&writes[0].1[..6], b"\x1b[200~");
        assert_eq!(writes[1].1, b"\r");
    }

    #[test]
    fn test_split_terminal_action() {
        let a = split_terminal_action(Some("y"), true, false);
        assert_eq!(a.payload, b"y".to_vec());
        assert_eq!(a.suffix, Some(b"\r".to_vec()));

        let b = split_terminal_action(None, false, true);
        assert!(b.payload.is_empty());
        assert_eq!(b.suffix, Some(b"\x03".to_vec()));

        let c = split_terminal_action(Some("text"), false, false);
        assert_eq!(c.payload, b"text".to_vec());
        assert_eq!(c.suffix, None);
    }

    #[test]
    fn test_detect_title_statuses() {
        use AgentTerminalStatus::*;
        assert_eq!(detect_agent_status_from_title("✳ claude idle"), Some(Idle));
        assert_eq!(detect_agent_status_from_title("claude working"), Some(Working));
        assert_eq!(detect_agent_status_from_title("gemini permission needed"), Some(Permission));
        assert_eq!(detect_agent_status_from_title("gemini thinking"), Some(Working));
        assert_eq!(detect_agent_status_from_title("omp ⠙ spinning"), Some(Working));
        assert_eq!(detect_agent_status_from_title("codex — action required"), Some(Permission));
        assert_eq!(detect_agent_status_from_title("* claude code"), Some(Idle));
        assert_eq!(detect_agent_status_from_title(". claude code"), Some(Working));
        assert_eq!(detect_agent_status_from_title("random shell title"), None);
        assert_eq!(detect_agent_status_from_title(""), None);
        assert_eq!(detect_agent_status_from_title("cursor"), None);
    }

    #[test]
    fn test_wait_blocked_signals() {
        assert_eq!(
            find_wait_blocked_signal("Update available — press enter to continue"),
            Some(WaitBlockedReason::CodexUpdatePrompt)
        );
        assert_eq!(
            find_wait_blocked_signal("Choose working directory to proceed. Press Enter to continue"),
            Some(WaitBlockedReason::CodexCwdPrompt)
        );
        assert_eq!(
            find_wait_blocked_signal("Permission required: allow once / deny"),
            Some(WaitBlockedReason::PermissionPrompt)
        );
        assert_eq!(find_wait_blocked_signal("normal output"), None);
    }

    #[test]
    fn test_preamble_contains_core_blocks() {
        let p = build_dispatch_preamble(&PreambleParams {
            task_id: "task_1",
            dispatch_id: "ctx_1",
            task_spec: "Fix the flux capacitor.",
            coordinator_handle: "term_coord",
            worker_handle: "term_worker",
            cli_command: "dais",
            worker_kind: WorkerKind::PromptReturningAgent,
        });
        assert!(p.contains("=== CLI COMMANDS ==="));
        assert!(p.contains("=== TASK ==="));
        assert!(p.contains("Fix the flux capacitor."));
        assert!(p.contains("\"task_id\":\"task_1\""));
        assert!(p.contains("\"dispatch_id\":\"ctx_1\""));
        assert!(p.contains("worker_done"));
        assert!(p.contains("NEVER use AskUserQuestion"));
        assert!(p.contains("=== AFTER YOU SEND worker_done ==="));
        // The worker_done example must parse as the payload zap reconciles.
        let start = p.find("--body '{\"task_id\"").unwrap() + "--body '".len();
        let end = p[start..].find("}'").unwrap() + start + 1;
        let payload: crate::agent::orchestration::types::WorkerDonePayload =
            serde_json::from_str(&p[start..end]).unwrap();
        assert_eq!(payload.task_id, "task_1");
        assert_eq!(payload.dispatch_id, "ctx_1");
        assert_eq!(payload.outcome, "succeeded");
    }
}
