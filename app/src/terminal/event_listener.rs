use std::sync::Arc;

use crate::terminal::event::Event as TerminalEvent;
use async_channel::Sender;

/// A wrapper struct that emits events which originate from the PTY event loop.
/// Instead of passing individual senders, we can pass through this struct
/// so that users have access to all of the senders in one nicely wrapped struct.
#[derive(Clone)]
pub struct ChannelEventListener {
    /// We have a dedicated channel for "wakeup"s because we throttle the receiver
    /// so that we can coalesce successive wakeup events during situations of high
    /// throughput (e.g. running `yes`).
    wakeups_tx: Sender<()>,
    terminal_events_tx: Sender<TerminalEvent>,
    pty_reads_tx: async_broadcast::Sender<Arc<Vec<u8>>>,
}

impl ChannelEventListener {
    pub fn new(
        wakeups_tx: Sender<()>,
        terminal_events_tx: Sender<TerminalEvent>,
        pty_reads_tx: async_broadcast::Sender<Arc<Vec<u8>>>,
    ) -> Self {
        ChannelEventListener {
            wakeups_tx,
            terminal_events_tx,
            pty_reads_tx,
        }
    }

    #[cfg(any(test, feature = "integration_tests"))]
    pub fn are_any_events_pending(&self) -> bool {
        !self.wakeups_tx.is_empty()
            || !self.terminal_events_tx.is_empty()
            || !self.pty_reads_tx.is_empty()
    }

    pub fn send_wakeup_event(&self) {
        if let Err(e) = self.wakeups_tx.try_send(()) {
            log::warn!("Failed to send Wakeup event: {e:?}");
        }
    }

    pub fn send_terminal_event(&self, event: TerminalEvent) {
        if let Err(e) = self.terminal_events_tx.try_send(event) {
            let try_send_error_dbg = format!("{e:?}");
            log::warn!(
                "Failed to send Terminal event {:?}: {:?}",
                e.into_inner(),
                try_send_error_dbg
            );
        }
    }

    pub fn send_handler_event(&self, event: HandlerEvent) {
        if let Err(e) = self
            .terminal_events_tx
            .try_send(TerminalEvent::Handler(event))
        {
            log::warn!("Failed to send Terminal Handler event {e:?}");
        }
    }

    pub fn send_pty_read_event(&self, bytes: &[u8]) {
        // Don't bother sending the event if there aren't any
        // active receivers. This avoids an unnecessary allocation of the bytes vector.
        // Note that we don't simply close the sending side since receivers
        // might come alive at some point in the future.
        if self.pty_reads_tx.receiver_count() > 0 {
            if let Err(e) = self.pty_reads_tx.try_broadcast(Arc::new(bytes.to_vec())) {
                log::warn!("Failed to send pty read event: {e:?}");
            }
        }
    }
}

#[cfg(test)]
mod testing;

use crate::terminal::model::terminal_model::HandlerEvent;

#[cfg(test)]
pub use testing::*;

/// R10 缝装配: 把 terminal_grid_model::GridEventSink 事件映射回 app Event 循环。
#[derive(Clone)]
pub struct GridEventSinkAdapter(pub ChannelEventListener);

impl terminal_grid_model::grid::GridEventSink for GridEventSinkAdapter {
    fn send_cursor_blinking_change(&self, blinking: bool) {
        self.0
            .send_terminal_event(TerminalEvent::CursorBlinkingChange(blinking));
    }

    fn send_bell(&self) {
        self.0.send_terminal_event(TerminalEvent::Bell);
    }

    fn send_mouse_cursor_dirty(&self) {
        self.0.send_terminal_event(TerminalEvent::MouseCursorDirty);
    }

    fn send_clipboard_store(
        &self,
        kind: crate::terminal::ClipboardType,
        text: std::sync::Arc<str>,
    ) {
        self.0.send_terminal_event(TerminalEvent::ClipboardStore(
            kind,
            text.to_string(),
        ));
    }

    fn send_clipboard_load(
        &self,
        kind: crate::terminal::ClipboardType,
        respond: terminal_grid_model::grid::ClipboardRespondFn,
    ) {
        self.0.send_terminal_event(TerminalEvent::ClipboardLoad(
            kind,
            Arc::new(move |text: &str| respond(text.to_string())),
        ));
    }

    fn send_honor_ps1_out_of_sync(&self) {
        self.0.send_terminal_event(TerminalEvent::HonorPS1OutOfSync);
    }

    fn send_text_selection_changed(&self) {
        self.0
            .send_terminal_event(TerminalEvent::TextSelectionChanged);
    }

    fn send_image_received(
        &self,
        image_id: u32,
        image_data: Vec<u8>,
        protocol: terminal_grid_model::grid::ImageProtocol,
    ) {
        let protocol = match protocol {
            terminal_grid_model::grid::ImageProtocol::Kitty => {
                crate::server::telemetry::ImageProtocol::Kitty
            }
            terminal_grid_model::grid::ImageProtocol::ITerm => {
                crate::server::telemetry::ImageProtocol::ITerm
            }
        };
        self.0.send_terminal_event(TerminalEvent::ImageReceived {
            image_id,
            image_data,
            image_protocol: protocol,
        });
    }
}

/// R10 缝装配: 富内容 secret 检测(ai::blocklist 留守 app)。
pub fn host_determine_secret_level(plaintext: &str) -> crate::terminal::model::secrets::SecretLevel {
    let secrets_with_levels =
        crate::ai::blocklist::block::secret_redaction::find_secrets_in_text_with_levels(plaintext);
    secrets_with_levels
        .into_iter()
        .map(|(_, level)| level)
        .max_by_key(|level| level.priority())
        .unwrap_or(crate::terminal::model::secrets::SecretLevel::User)
}
