//! Orchestration PTY bridge — connects the orchestration plane's `PtyExecutor`
//! trait to Warp's GPUI terminal infrastructure.
//!
//! ## Architecture
//!
//! The orchestration `PtyExecutor` trait is synchronous (`&self`), but GPUI
//! terminal writes require `AppContext`. We bridge this with a channel:
//!
//! 1. `OrchestrationPtySender` implements `PtyExecutor` — it pushes
//!    `(handle, bytes)` into a bounded `SyncSender`. Safe from any thread.
//! 2. A GPUI consumer model (future P3) drains the channel and dispatches
//!    to `PtyController`s via a `HandleRegistry`.
//!
//! ## Current status
//!
//! P2: Channel sender is implemented. The consumer + handle registry are
//! deferred to P3 — they require terminal registration infrastructure
//! (mapping orchestration handles to pane views).

use std::sync::mpsc::{sync_channel, Receiver, SyncSender};

use ::ai::agent::orchestration::db::{OrchestrationError, OrchestrationResult};
use ::ai::agent::orchestration::executor::PtyExecutor;

// ---------------------------------------------------------------------------
// Channel types
// ---------------------------------------------------------------------------

/// A write request pushed by `OrchestrationPtySender`.
#[derive(Debug)]
pub struct PtyWriteRequest {
    /// Orchestration handle identifying the target terminal.
    pub handle: String,
    /// Raw bytes to write to the PTY.
    pub bytes: Vec<u8>,
}

// ---------------------------------------------------------------------------
// OrchestrationPtySender — PtyExecutor impl backed by a channel
// ---------------------------------------------------------------------------

/// Implements `PtyExecutor` by pushing writes into a bounded channel.
/// Cloneable + Send + Sync.
///
/// The consumer side (GPUI model that drains the channel) will be wired
/// in P3 when terminal registration is implemented.
#[derive(Clone)]
pub struct OrchestrationPtySender {
    tx: SyncSender<PtyWriteRequest>,
}

impl OrchestrationPtySender {
    /// Create a sender/receiver pair with the given buffer size.
    /// The receiver will be consumed by the GPUI bridge model.
    pub fn channel(buffer: usize) -> (Self, Receiver<PtyWriteRequest>) {
        let (tx, rx) = sync_channel(buffer);
        (Self { tx }, rx)
    }

    /// Try to receive a pending write request (for testing or the consumer).
    pub fn try_recv(rx: &Receiver<PtyWriteRequest>) -> Option<PtyWriteRequest> {
        rx.try_recv().ok()
    }
}

impl PtyExecutor for OrchestrationPtySender {
    fn write_to_pty(&self, handle: &str, bytes: &[u8]) -> OrchestrationResult<()> {
        self.tx
            .send(PtyWriteRequest {
                handle: handle.to_string(),
                bytes: bytes.to_vec(),
            })
            .map_err(|_| OrchestrationError::Task("pty bridge channel closed".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sender_pushes_to_channel() {
        let (sender, rx) = OrchestrationPtySender::channel(16);
        sender.write_to_pty("pane-1", b"echo hello\n").unwrap();

        let req = rx.try_recv().unwrap();
        assert_eq!(req.handle, "pane-1");
        assert_eq!(req.bytes, b"echo hello\n");
    }

    #[test]
    fn test_sender_channel_full_returns_error() {
        let (sender, _rx) = OrchestrationPtySender::channel(1);
        // First write fills the buffer.
        sender.write_to_pty("pane-1", b"cmd1\n").unwrap();
        // Second write blocks (sync_channel(1) = 1 slot, so this errors).
        let result = sender.write_to_pty("pane-1", b"cmd2\n");
        assert!(result.is_err());
    }

    #[test]
    fn test_sender_is_clone() {
        let (sender, rx) = OrchestrationPtySender::channel(16);
        let sender2 = sender.clone();
        sender.write_to_pty("pane-1", b"a\n").unwrap();
        sender2.write_to_pty("pane-2", b"b\n").unwrap();

        let r1 = rx.try_recv().unwrap();
        let r2 = rx.try_recv().unwrap();
        assert_eq!(r1.handle, "pane-1");
        assert_eq!(r2.handle, "pane-2");
    }
}

pub mod shell_event_bridge;
