//! Mailbox push-delivery — injects a message pointer into a worker's PTY when
//! the agent is idle, so the worker pulls its mail with the check command.
//!
//! Ported from Orca's `deliverPendingMessagesForLeaf` chain (orca-runtime.ts
//! 31906/32598/32721-32755). This is the "A chain" (mailbox pointer push);
//! the "B chain" (full prompt injection for dispatches) lives in
//! `prompt_injection.rs`. Both share the split-submit discipline: payload
//! write, 500 ms, then a lone `\r` — agent TUIs swallow a `\r` that arrives
//! in the same PTY write as the text.
//!
//! Correctness model (1:1 with Orca):
//! - Message bodies always live in SQLite; pending = `read = 0 AND
//!   delivered_at IS NULL`. No failure path mutates the DB except a
//!   successful pointer write (which sets `delivered_at`).
//! - An in-memory per-mailbox watermark (`last_pointed_sequence`) prevents
//!   re-injecting a pointer for sequences already announced.
//! - An in-memory flight set serializes deliveries per mailbox; a delivery
//!   in progress parks new triggers (they retry on the next poll).
//! - The agent's pull path (`orchestration check-messages <handle>`) is the
//!   authoritative consumer; this push is only an accelerator.

use std::collections::HashMap;
use std::sync::OnceLock;
use std::time::Duration;

use parking_lot::Mutex;

use super::db::{Message, OrchestrationResult};
use super::executor::PtyExecutor;
use super::OrchestrationStore;
use super::idle_detector::{classify_idle, IdleSignal, IdleVerdict};
use super::prompt_injection::{detect_agent_status_from_title, AgentTerminalStatus};

/// Delay between the pointer text and the submit CR (agent TUIs swallow a
/// same-write `\r`). Mirrors Orca `AGENT_PROMPT_SUBMIT_DELAY_MS`.
pub const POINTER_SUBMIT_DELAY_MS: u64 = 500;

// ---------------------------------------------------------------------------
// Pointer format
// ---------------------------------------------------------------------------

/// The literal text injected into the worker's terminal.
/// Ported from Orca `formatMessagePointer` (formatter.ts:111-114):
/// a pure pointer — the body is pulled by the check command, never pushed.
pub fn format_message_pointer(n: usize, handle: &str) -> String {
    format!(
        "\nYou have {n} orchestration message(s). Run `dais orchestration check-messages {handle}`.\n"
    )
}

// ---------------------------------------------------------------------------
// Process-wide dispatch registry (populated by pane assignment)
// ---------------------------------------------------------------------------

/// Per-dispatch push state: last observed agent status (for idle-edge
/// detection) + delivery bookkeeping. Registered when a dispatch is assigned
/// to a terminal pane; the router polls every entry each tick.
#[derive(Default)]
pub struct DispatchPushState {
    pub last_status: Option<AgentTerminalStatus>,
    /// Last idle verdict from multi-signal classification.
    pub last_idle_verdict: Option<IdleVerdict>,
    last_pointed_sequence: i32,
    in_flight: bool,
}

static REGISTRY: OnceLock<Mutex<HashMap<String, DispatchPushState>>> = OnceLock::new();

fn registry() -> &'static Mutex<HashMap<String, DispatchPushState>> {
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Register a dispatch for push delivery (called on pane assignment).
pub fn register_dispatch(dispatch_id: &str) {
    registry().lock().entry(dispatch_id.to_string()).or_default();
}

/// Unregister a dispatch (called on unassignment / pane teardown).
/// Drops the watermark so a future re-assignment starts clean (Orca's
/// `retirePendingMessageDeliveryForPty` cold-recovery semantics: a reborn
/// PTY must never receive a stale Enter).
pub fn unregister_dispatch(dispatch_id: &str) {
    registry().lock().remove(dispatch_id);
}

/// Snapshot of registered dispatch ids.
pub fn registered_dispatches() -> Vec<String> {
    registry().lock().keys().cloned().collect()
}

// ---------------------------------------------------------------------------
// Idle-edge detection
// ---------------------------------------------------------------------------

/// Whether this poll tick should attempt delivery for the dispatch.
///
/// Uses multi-signal idle classification: title (primary), alt_screen,
/// output silence, and precmd recency. Falls back to Unknown when no
/// signal is available — Unknown never fires (conservative).
pub fn should_push(
    state: &mut DispatchPushState,
    sig: &IdleSignal,
    has_pending: bool,
) -> bool {
    // Backward-compatible: still extract title-based status for last_status tracking.
    state.last_status = sig.title.as_deref().and_then(detect_agent_status_from_title);
    let verdict = classify_idle(sig);
    state.last_idle_verdict = Some(verdict);
    verdict == IdleVerdict::Idle && has_pending
}

/// Outcome of one delivery attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PushOutcome {
    /// Nothing pending (or nothing newer than the watermark).
    NothingNew,
    /// A delivery is already in progress for this mailbox — parked.
    InFlight,
    /// Agent not idle / no title — left in DB for a later tick.
    NotIdle,
    /// Pointer written, messages marked delivered. Enter is sent after
    /// [`POINTER_SUBMIT_DELAY_MS`] (blocking — callers run off the hot path).
    Delivered { count: usize },
    /// PTY write failed; DB untouched, watermark untouched.
    WriteFailed(String),
}

/// Attempt push delivery for one dispatch (synchronous; contains the 500 ms
/// submit delay). Ported from Orca `deliverPendingMessages` (32598-32755).
pub fn deliver_pending<S: OrchestrationStore>(
    store: &S,
    executor: &dyn PtyExecutor,
    dispatch_id: &str,
    sig: &IdleSignal,
) -> OrchestrationResult<PushOutcome> {
    let mut reg = registry().lock();
    let Some(state) = reg.get_mut(dispatch_id) else {
        return Ok(PushOutcome::NothingNew);
    };

    // 1. Pending set (DB is the source of truth), minus messages claimed by a
    //    live `check --wait` waiter — the waiter/push mutual exclusion that
    //    Orca enforces via messageTypeHasLiveWaiter (32636-32643): without
    //    it, a blocking check and a pointer push double-consume one row.
    let pending: Vec<Message> = store
        .get_undelivered_unread(dispatch_id)?
        .into_iter()
        .filter(|m| m.sequence > state.last_pointed_sequence)
        .filter(|m| {
            !store
                .has_live_waiter(dispatch_id, &m.message_type)
                .unwrap_or(false)
        })
        .collect();
    if pending.is_empty() {
        return Ok(PushOutcome::NothingNew);
    }

    // 2. Idle gate (multi-signal classification; busy/unknown agents keep mail in the DB).
    if !should_push(state, sig, true) {
        return Ok(PushOutcome::NotIdle);
    }

    // 3. Flight gate — one delivery per mailbox at a time.
    if state.in_flight {
        return Ok(PushOutcome::InFlight);
    }
    state.in_flight = true;
    drop(reg); // don't hold the registry lock across PTY writes + sleep

    // Everything below must release the flight and settle the watermark.
    let sequences: Vec<i32> = pending.iter().map(|m| m.sequence).collect();
    let max_seq = *sequences.iter().max().unwrap();
    let pointer = format_message_pointer(pending.len(), dispatch_id);

    let result = (|| -> OrchestrationResult<PushOutcome> {
        // 4. Write the pointer (plain write, like Orca 32721 — the pointer
        //    is a shell-prompt line, not a pasted prompt).
        executor.write_to_pty(dispatch_id, pointer.as_bytes())?;

        // 5. Split submit: lone `\r` after the delay.
        std::thread::sleep(Duration::from_millis(POINTER_SUBMIT_DELAY_MS));
        executor.write_to_pty(dispatch_id, b"\r")?;

        // 6. Both writes succeeded → mark messages delivered. Doing this
        //    AFTER the CR (not between the two writes) closes the crash
        //    window where delivered_at was set but the pointer was never
        //    submitted — the agent would never see the pointer, yet the
        //    DB shows it as delivered.
        store.mark_delivered(&sequences)?;

        Ok(PushOutcome::Delivered { count: pending.len() })
    })();

    // 7. Settle (Orca `settlePendingMessageDelivery`): watermark advances
    //    only on success; the flight always clears.
    let mut reg = registry().lock();
    if let Some(state) = reg.get_mut(dispatch_id) {
        state.in_flight = false;
        if let Ok(PushOutcome::Delivered { .. }) = &result {
            state.last_pointed_sequence = max_seq;
        }
    }

    match result {
        Ok(o) => Ok(o),
        Err(e) => Ok(PushOutcome::WriteFailed(e.to_string())),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::orchestration::executor::MockPtyExecutor;
    use crate::agent::orchestration::idle_detector::idle_signal_from_title;
    use crate::agent::orchestration::store::DieselOrchestrationStore;
    use crate::agent::orchestration::types::MessageType;

    /// Shorthand: build an IdleSignal from a title string slice.
    fn sig(title: &str) -> IdleSignal {
        idle_signal_from_title(Some(title.to_string()))
    }

    /// Shorthand: build an IdleSignal with no title.
    fn no_sig() -> IdleSignal {
        IdleSignal::default()
    }

    fn setup() -> (DieselOrchestrationStore, MockPtyExecutor) {
        (
            DieselOrchestrationStore::in_memory().expect("store"),
            MockPtyExecutor::default(),
        )
    }

    fn seed_message(store: &DieselOrchestrationStore, to: &str) -> i32 {
        store
            .enqueue_message("run_1", "coordinator", to, MessageType::Status, "hi", "body")
            .unwrap()
    }

    #[test]
    fn test_pointer_format() {
        let p = format_message_pointer(3, "ctx_1");
        assert!(p.starts_with('\n'));
        assert!(p.ends_with('\n'));
        assert!(p.contains("You have 3 orchestration message(s)."));
        assert!(p.contains("check-messages ctx_1"));
    }

    #[test]
    fn test_deliver_when_idle() {
        let (store, exec) = setup();
        register_dispatch("ctx_t1");
        seed_message(&store, "ctx_t1");

        let out = deliver_pending(&store, &exec, "ctx_t1", &sig("✳ claude idle")).unwrap();
        assert_eq!(out, PushOutcome::Delivered { count: 1 });

        // Two writes: pointer, then lone CR.
        let writes = exec.writes.lock();
        assert_eq!(writes.len(), 2);
        assert!(String::from_utf8_lossy(&writes[0].1).contains("1 orchestration message"));
        assert_eq!(writes[1].1, b"\r".to_vec());

        // delivered_at set → second attempt has nothing new.
        drop(writes);
        let out2 = deliver_pending(&store, &exec, "ctx_t1", &sig("✳ claude idle")).unwrap();
        assert_eq!(out2, PushOutcome::NothingNew);
        unregister_dispatch("ctx_t1");
    }

    #[test]
    fn test_busy_agent_keeps_mail() {
        let (store, exec) = setup();
        register_dispatch("ctx_t2");
        seed_message(&store, "ctx_t2");

        // Working title → NotIdle, DB untouched.
        let out = deliver_pending(&store, &exec, "ctx_t2", &sig("claude working")).unwrap();
        assert_eq!(out, PushOutcome::NotIdle);
        assert!(store.get_undelivered_unread("ctx_t2").unwrap().len() == 1);
        unregister_dispatch("ctx_t2");
    }

    #[test]
    fn test_no_title_never_fires() {
        let (store, exec) = setup();
        register_dispatch("ctx_t3");
        seed_message(&store, "ctx_t3");
        let out = deliver_pending(&store, &exec, "ctx_t3", &no_sig()).unwrap();
        assert_eq!(out, PushOutcome::NotIdle);
        unregister_dispatch("ctx_t3");
    }

    #[test]
    fn test_write_failure_leaves_db_untouched() {
        // A message already pointed (watermark set), new one arrives, write
        // fails → the new message must stay undelivered.
        let (store, _exec) = setup();
        register_dispatch("ctx_t4");
        seed_message(&store, "ctx_t4");
        // Simulate a previous successful delivery advancing the watermark.
        {
            let mut reg = registry().lock();
            let st = reg.get_mut("ctx_t4").unwrap();
            st.last_status = Some(AgentTerminalStatus::Idle);
            st.last_pointed_sequence = 1;
        }
        // No executor to inject with: use a failing executor.
        struct FailExec;
        impl PtyExecutor for FailExec {
            fn write_to_pty(&self, _h: &str, _b: &[u8]) -> OrchestrationResult<()> {
                Err(crate::agent::orchestration::db::OrchestrationError::Connection(
                    "pty gone".into(),
                ))
            }
        }
        seed_message(&store, "ctx_t4");
        let out = deliver_pending(&store, &FailExec, "ctx_t4", &sig("✳ claude idle")).unwrap();
        assert!(matches!(out, PushOutcome::WriteFailed(_)));
        // Both messages: the first still undelivered at DB level? No — the
        // first was never marked delivered in this test (watermark was
        // simulated), so both remain pending; the failure changed nothing.
        assert_eq!(store.get_undelivered_unread("ctx_t4").unwrap().len(), 2);
        unregister_dispatch("ctx_t4");
    }

    #[test]
    fn test_waiter_claim_blocks_push() {
        let (store, exec) = setup();
        register_dispatch("ctx_t5");
        seed_message(&store, "ctx_t5");

        // Live waiter claiming all types → push skips (mutual exclusion).
        store.upsert_waiter("wtr_1", "ctx_t5", "[]", 60).unwrap();
        let out = deliver_pending(&store, &exec, "ctx_t5", &sig("✳ claude idle")).unwrap();
        assert_eq!(out, PushOutcome::NothingNew);
        assert_eq!(store.get_undelivered_unread("ctx_t5").unwrap().len(), 1);

        // Claim removed → push proceeds.
        store.delete_waiter("wtr_1").unwrap();
        let out = deliver_pending(&store, &exec, "ctx_t5", &sig("✳ claude idle")).unwrap();
        assert_eq!(out, PushOutcome::Delivered { count: 1 });
        unregister_dispatch("ctx_t5");
    }

    #[test]
    fn test_expired_waiter_claim_ignored() {
        let (store, exec) = setup();
        register_dispatch("ctx_t6");
        seed_message(&store, "ctx_t6");

        // TTL in the past → claim is dead, push proceeds.
        store.upsert_waiter("wtr_2", "ctx_t6", "[]", -10).unwrap();
        let out = deliver_pending(&store, &exec, "ctx_t6", &sig("✳ claude idle")).unwrap();
        assert_eq!(out, PushOutcome::Delivered { count: 1 });
        unregister_dispatch("ctx_t6");
    }

    #[test]
    fn test_type_filtered_waiter_only_claims_its_type() {
        let (store, exec) = setup();
        register_dispatch("ctx_t7");
        store
            .enqueue_message("run_1", "coordinator", "ctx_t7", MessageType::Status, "a", "b")
            .unwrap();

        // Waiter claims only 'worker_done' → status message still pushes.
        store
            .upsert_waiter("wtr_3", "ctx_t7", "[\"worker_done\"]", 60)
            .unwrap();
        let out = deliver_pending(&store, &exec, "ctx_t7", &sig("✳ claude idle")).unwrap();
        assert_eq!(out, PushOutcome::Delivered { count: 1 });
        unregister_dispatch("ctx_t7");
    }

    #[test]
    fn test_multi_signal_idle_path_delivers() {
        // No title signal at all: bare shell with recent precmd + settled
        // output is Idle via the multi-signal fallback.
        let (store, exec) = setup();
        register_dispatch("ctx_t8");
        seed_message(&store, "ctx_t8");
        let mut s = IdleSignal::default();
        s.since_last_precmd_ms = Some(3_000);
        s.output_silent_for_ms = Some(2_500); // > bare-shell threshold (2s)
        let out = deliver_pending(&store, &exec, "ctx_t8", &s).unwrap();
        assert_eq!(out, PushOutcome::Delivered { count: 1 });
        unregister_dispatch("ctx_t8");
    }

    #[test]
    fn test_multi_signal_busy_alt_screen() {
        // alt-screen TUI rendering → Busy even with settled output.
        let (store, exec) = setup();
        register_dispatch("ctx_t9");
        seed_message(&store, "ctx_t9");
        let mut s = IdleSignal::default();
        s.alt_screen_active = true;
        s.since_last_precmd_ms = Some(3_000);
        s.output_silent_for_ms = Some(5_000);
        let out = deliver_pending(&store, &exec, "ctx_t9", &s).unwrap();
        assert_eq!(out, PushOutcome::NotIdle);
        unregister_dispatch("ctx_t9");
    }

    #[test]
    fn test_bare_shell_short_silence_stays_unknown() {
        // Bare shell (no title) with only 800ms silence: below the 2s
        // bare-shell threshold → Unknown → no push.
        let (store, exec) = setup();
        register_dispatch("ctx_t10");
        seed_message(&store, "ctx_t10");
        let mut s = IdleSignal::default();
        s.since_last_precmd_ms = Some(3_000);
        s.output_silent_for_ms = Some(800);
        let out = deliver_pending(&store, &exec, "ctx_t10", &s).unwrap();
        assert_eq!(out, PushOutcome::NotIdle);
        unregister_dispatch("ctx_t10");
    }
}
