//! Orchestration PTY bridge — connects the orchestration plane's `PtyExecutor`
//! trait to Warp's GPUI terminal infrastructure.
//!
//! ## Architecture
//!
//! The orchestration `PtyExecutor` trait is synchronous (`&self`), but GPUI
//! terminal writes require `AppContext`. We bridge this with a channel:
//!
//! 1. `OrchestrationPtySender` implements `PtyExecutor` — it pushes
//!    `(handle, bytes)` into an `async_channel::Sender`. Safe from any thread.
//! 2. `PtyBridgeConsumer` is a GPUI model that drains the channel via
//!    `spawn_stream_local` and calls `TerminalView::write_agent_bytes_to_pty`.
//!
//! ## Handle format
//!
//! Handles are orchestration dispatch IDs (e.g. `ctx_abcd1234`). The
//! `ViewRegistry` maps dispatch_id → `WeakViewHandle<TerminalView>` when a
//! worker is assigned to a pane.

use std::sync::Arc;

use async_channel::{bounded, Receiver, Sender};

use ::ai::agent::orchestration::db::{OrchestrationError, OrchestrationResult};
use ::ai::agent::orchestration::executor::PtyExecutor;

use std::sync::OnceLock;

static GLOBAL_PTY_SENDER: OnceLock<OrchestrationPtySender> = OnceLock::new();

/// Store the process-wide PTY sender. Called once at app launch.
pub fn set_global_pty_sender(sender: OrchestrationPtySender) {
    let _ = GLOBAL_PTY_SENDER.set(sender);
}

/// Get the process-wide PTY sender, if initialized.
pub fn global_pty_sender() -> Option<&'static OrchestrationPtySender> {
    GLOBAL_PTY_SENDER.get()
}

use crate::terminal::view::TerminalView;

// ---------------------------------------------------------------------------
// Channel types
// ---------------------------------------------------------------------------

/// A write request pushed by `OrchestrationPtySender`.
#[derive(Debug)]
pub struct PtyWriteRequest {
    /// Orchestration handle (dispatch_id) identifying the target terminal.
    pub handle: String,
    /// Raw bytes to write to the PTY.
    pub bytes: Vec<u8>,
}

// ---------------------------------------------------------------------------
// OrchestrationPtySender — PtyExecutor impl backed by a channel
// ---------------------------------------------------------------------------

/// Implements `PtyExecutor` by pushing writes into an async channel.
/// Cloneable + Send + Sync.
#[derive(Clone)]
pub struct OrchestrationPtySender {
    tx: Sender<PtyWriteRequest>,
    /// Optional liveness checker — when set, `write_to_pty` rejects
    /// handles not registered in ViewRegistry before enqueueing,
    /// preventing false-positive Ok on dead/unassigned terminals.
    handle_alive: Option<Arc<dyn Fn(&str) -> bool + Send + Sync>>,
}

impl OrchestrationPtySender {
    /// Create a sender/receiver pair with the given buffer capacity.
    pub fn channel(buffer: usize) -> (Self, Receiver<PtyWriteRequest>) {
        let (tx, rx) = bounded(buffer);
        (Self { tx, handle_alive: None }, rx)
    }

    /// Attach a handle-liveness checker (cloned ViewRegistry keys).
    /// Must be called before the sender is stored globally.
    pub fn with_handle_checker(
        mut self,
        checker: Arc<dyn Fn(&str) -> bool + Send + Sync>,
    ) -> Self {
        self.handle_alive = Some(checker);
        self
    }
}


impl PtyExecutor for OrchestrationPtySender {
    fn write_to_pty(&self, handle: &str, bytes: &[u8]) -> OrchestrationResult<()> {
        if let Some(check) = &self.handle_alive {
            if !check(handle) {
                return Err(OrchestrationError::Task(format!(
                    "terminal handle not registered (dead or unassigned): {}",
                    handle
                )));
            }
        }
        self.tx
            .try_send(PtyWriteRequest {
                handle: handle.to_string(),
                bytes: bytes.to_vec(),
            })
            .map_err(|e| OrchestrationError::Task(format!("pty bridge send failed: {e}")))
    }
}

// ---------------------------------------------------------------------------
// ViewRegistry — maps orchestration handles to TerminalView weak handles
// ---------------------------------------------------------------------------

/// Maps orchestration dispatch IDs to terminal views.
/// Populated when a worker dispatch is assigned to a pane.
#[derive(Default, Clone)]
pub struct ViewRegistry {
    inner: std::sync::Arc<parking_lot::Mutex<std::collections::HashMap<String, warpui::WeakViewHandle<TerminalView>>>>,
}

impl ViewRegistry {
    pub fn register(&self, handle: &str, view: warpui::WeakViewHandle<TerminalView>) {
        self.inner.lock().insert(handle.to_string(), view);
    }

    pub fn unregister(&self, handle: &str) {
        self.inner.lock().remove(handle);
    }

    /// Check if a handle is registered (non-GPUI safe — no view upgrade).
    /// Used by the PTY sender's liveness checker to reject dead handles.
    pub fn has_handle(&self, handle: &str) -> bool {
        self.inner.lock().contains_key(handle)
    }
    pub fn get(
        &self,
        handle: &str,
        app: &warpui::AppContext,
    ) -> Option<warpui::ViewHandle<TerminalView>> {
        let map = self.inner.lock();
        let weak = map.get(handle)?;
        weak.upgrade(app)
    }
}

impl warpui::Entity for ViewRegistry {
    type Event = ();
}

impl warpui::SingletonEntity for ViewRegistry {}

// ---------------------------------------------------------------------------
// PtyBridgeConsumer — GPUI model that drains the channel
// ---------------------------------------------------------------------------

/// GPUI model that drains the PTY write channel and dispatches to
/// `TerminalView`s via the `ViewRegistry`.
pub struct PtyBridgeConsumer {
    rx: Receiver<PtyWriteRequest>,
    registry: ViewRegistry,
}

impl PtyBridgeConsumer {
    pub fn new(rx: Receiver<PtyWriteRequest>, registry: ViewRegistry) -> Self {
        Self { rx, registry }
    }

    /// Set up the drain loop. Call once at model creation.
    pub fn start(&mut self, ctx: &mut warpui::ModelContext<Self>) {
        let rx = self.rx.clone();
        let registry = self.registry.clone();
        ctx.spawn_stream_local(
            rx,
            move |me, req, ctx| {
                let _ = &me; // consumer is stateless; registry does the work
                let Some(view) = registry.get(&req.handle, ctx) else {
                    log::warn!(
                        "orchestration pty bridge: no view registered for handle '{}'",
                        req.handle
                    );
                    return;
                };
                view.update(ctx, |view, ctx| {
                    view.write_agent_bytes_to_pty(
                        req.bytes.clone(),
                        &::ai::agent::action::AIAgentPtyWriteMode::Raw,
                        ctx,
                    );
                });
            },
            |_, _| {},
        );
    }
}

impl warpui::Entity for PtyBridgeConsumer {
    type Event = ();
}

impl warpui::SingletonEntity for PtyBridgeConsumer {}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sender_pushes_to_channel() {
        let (sender, rx) = OrchestrationPtySender::channel(16);
        sender.write_to_pty("ctx_1", b"echo hello\n").unwrap();

        let req = rx.try_recv().unwrap();
        assert_eq!(req.handle, "ctx_1");
        assert_eq!(req.bytes, b"echo hello\n");
    }

    #[test]
    fn test_sender_is_clone() {
        let (sender, rx) = OrchestrationPtySender::channel(16);
        let sender2 = sender.clone();
        sender.write_to_pty("ctx_1", b"a\n").unwrap();
        sender2.write_to_pty("ctx_2", b"b\n").unwrap();

        let r1 = rx.try_recv().unwrap();
        let r2 = rx.try_recv().unwrap();
        assert_eq!(r1.handle, "ctx_1");
        assert_eq!(r2.handle, "ctx_2");
    }
}

pub mod block_settle;
pub mod shell_event_bridge;
pub mod terminal_tail;
pub mod session_activity;

pub mod interactive;
pub mod dispatch_assign;
pub mod dispatch_send;
#[cfg(unix)]
pub mod runtime_rpc;
