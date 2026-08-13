//! Terminal tail reader — extracts rendered text from a `TerminalView` identified
//! by orchestration dispatch ID.
//!
//! ## Architecture
//!
//! The contract functions `terminal_tail` / `terminal_title` come in two flavours:
//!
//! 1. **Direct (GPUI thread)** — when `AppContext` is available, the caller passes it
//!    through `terminal_tail_with_cx` / `terminal_title_with_cx` and we look up the
//!    `ViewRegistry` singleton directly. Zero allocation, no blocking.
//!
//! 2. **Channel (off-GPUI thread)** — when `AppContext` is unavailable, the caller uses
//!    `terminal_tail` / `terminal_title` which send a one-shot request through a global
//!    channel. A `TerminalTailBridge` GPUI singleton drains the channel on the main
//!    thread and writes the response back.
//!
//!    **Warning:** calling the channel flavour from the GPUI main thread will deadlock,
//!    because the consumer also runs on that thread. Only use when the caller is on a
//!    worker thread.
//!
//! ## Registration
//!
//! The bridge singleton must be registered at app startup (typically in `lib.rs`):
//!
//! ```ignore
//! ctx.add_singleton_model(|ctx| {
//!     crate::ai::orchestration::terminal_tail::TerminalTailBridge::new(ctx)
//! });
//! ```

use crate::ai::orchestration::ViewRegistry;
use crate::terminal::view::TerminalView;
use std::sync::OnceLock;
use warpui::AppContext;
use warpui::SingletonEntity;

// ---------------------------------------------------------------------------
// Global bridge handle (for the channel flavour)
// ---------------------------------------------------------------------------

static GLOBAL_TAIL_BRIDGE: OnceLock<async_channel::Sender<TailRequest>> = OnceLock::new();

/// Register the process-wide tail bridge sender. Called once at app launch
/// (runtime input, so `OnceLock` is appropriate here).
pub fn set_global_tail_sender(sender: async_channel::Sender<TailRequest>) {
    let _ = GLOBAL_TAIL_BRIDGE.set(sender);
}

// ---------------------------------------------------------------------------
// Request / response types
// ---------------------------------------------------------------------------

/// A request sent to the GPUI main thread to extract terminal content.
pub enum TailRequest {
    /// Extract tail text from the terminal identified by `dispatch_id`.
    Tail {
        dispatch_id: String,
        max_lines: usize,
        max_bytes: usize,
        respond: async_channel::Sender<Option<String>>,
    },
    /// Extract the terminal title.
    Title {
        dispatch_id: String,
        respond: async_channel::Sender<Option<String>>,
    },
}

// ---------------------------------------------------------------------------
// TerminalTailBridge — GPUI singleton that services tail requests
// ---------------------------------------------------------------------------

/// GPUI model that listens for tail requests and executes them on the main
/// thread where `ViewRegistry` singleton access is safe.
pub struct TerminalTailBridge {
    #[allow(dead_code)]
    rx: async_channel::Receiver<TailRequest>,
}

impl TerminalTailBridge {
    /// Create a new bridge and start the consumer loop.
    /// Call from a `ModelContext<Self>` (i.e. during GPUI model registration).
    pub fn new(ctx: &mut warpui::ModelContext<Self>) -> Self {
        let (tx, rx) = async_channel::bounded::<TailRequest>(16);
        set_global_tail_sender(tx);

        // Consumer: drain requests on the GPUI main thread via spawn_stream_local.
        let rx_stream = rx.clone();
        ctx.spawn_stream_local(
            rx_stream,
            move |_me, req, ctx| {
                match req {
                    TailRequest::Tail {
                        dispatch_id,
                        max_lines,
                        max_bytes,
                        respond,
                    } => {
                        let result =
                            extract_tail_from_registry(&dispatch_id, max_lines, max_bytes, ctx);
                        let _ = respond.try_send(result);
                    }
                    TailRequest::Title {
                        dispatch_id,
                        respond,
                    } => {
                        let result = extract_title_from_registry(&dispatch_id, ctx);
                        let _ = respond.try_send(result);
                    }
                }
            },
            |_, _| {},
        );

        Self { rx }
    }
}

impl warpui::Entity for TerminalTailBridge {
    type Event = ();
}

impl warpui::SingletonEntity for TerminalTailBridge {}

// ---------------------------------------------------------------------------
// Contract functions — channel flavour (may block / deadlock on GPUI thread)
// ---------------------------------------------------------------------------

/// Extract the last `max_lines` rendered lines from the terminal view associated
/// with `dispatch_id`, truncating the result to `max_bytes`.
///
/// Returns `None` if:
/// - the bridge is not initialised (app startup missing),
/// - the dispatch_id is not registered in `ViewRegistry`, or
/// - the terminal has no rendered blocks.
pub fn terminal_tail(dispatch_id: &str, max_lines: usize, max_bytes: usize) -> Option<String> {
    let tx = GLOBAL_TAIL_BRIDGE.get()?;
    let (respond, rx) = async_channel::bounded::<Option<String>>(1);
    let _ = tx.try_send(TailRequest::Tail {
        dispatch_id: dispatch_id.to_string(),
        max_lines,
        max_bytes,
        respond,
    });
    warpui::r#async::block_on(rx.recv()).ok()?
}

/// Extract the terminal title string for the view associated with `dispatch_id`.
///
/// Returns `None` if the bridge is not initialised or the view is not found.
pub fn terminal_title(dispatch_id: &str) -> Option<String> {
    let tx = GLOBAL_TAIL_BRIDGE.get()?;
    let (respond, rx) = async_channel::bounded::<Option<String>>(1);
    let _ = tx.try_send(TailRequest::Title {
        dispatch_id: dispatch_id.to_string(),
        respond,
    });
    warpui::r#async::block_on(rx.recv()).ok()?
}

// ---------------------------------------------------------------------------
// Contract functions — direct flavour (GPUI thread only, no blocking)
// ---------------------------------------------------------------------------

/// Same as [`terminal_tail`] but uses `ViewRegistry` directly via the provided
/// `AppContext`. Safe to call from the GPUI main thread. Prefer this when `cx`
/// is available.
pub fn terminal_tail_with_cx(
    dispatch_id: &str,
    max_lines: usize,
    max_bytes: usize,
    cx: &AppContext,
) -> Option<String> {
    extract_tail_from_registry(dispatch_id, max_lines, max_bytes, cx)
}

/// Same as [`terminal_title`] but uses `ViewRegistry` directly via the provided
/// `AppContext`. Safe to call from the GPUI main thread.
pub fn terminal_title_with_cx(dispatch_id: &str, cx: &AppContext) -> Option<String> {
    extract_title_from_registry(dispatch_id, cx)
}

// ---------------------------------------------------------------------------
// Internal extraction helpers (run on GPUI thread)
// ---------------------------------------------------------------------------

/// Look up the `TerminalView` from `ViewRegistry` and extract tail content.
fn extract_tail_from_registry(
    dispatch_id: &str,
    max_lines: usize,
    max_bytes: usize,
    cx: &AppContext,
) -> Option<String> {
    let registry = ViewRegistry::handle(cx).read(cx, |m, _| m.clone());
    let view = registry.get(dispatch_id, cx)?;
    view.read(cx, |tv, _| extract_tail(tv, max_lines, max_bytes))
}

/// Look up the `TerminalView` from `ViewRegistry` and extract its title.
fn extract_title_from_registry(dispatch_id: &str, cx: &AppContext) -> Option<String> {
    let registry = ViewRegistry::handle(cx).read(cx, |m, _| m.clone());
    let view = registry.get(dispatch_id, cx)?;
    view.read(cx, |tv, _| extract_title(tv))
}

/// Extract tail text from a `TerminalView` (no GPUI dependency beyond the
/// `model` lock).
fn extract_tail(tv: &TerminalView, max_lines: usize, max_bytes: usize) -> Option<String> {
    let model = tv.model.lock();
    let block_list = model.block_list();
    let blocks = block_list.blocks();

    if blocks.is_empty() {
        return None;
    }

    // Collect rendered text lines from all blocks' output grids.
    let mut all_lines: Vec<String> = Vec::new();
    for block in blocks.iter() {
        // Skip in-band command blocks — hidden to the user, arbitrarily many.
        if block.is_in_band_command_block() {
            continue;
        }

        // Get the output grid text (the main rendered content of the block).
        let output_text = block.output_grid().contents_to_string(false, None);
        for line in output_text.lines() {
            all_lines.push(line.to_string());
        }
    }

    if all_lines.is_empty() {
        return None;
    }

    // Take the last `max_lines` lines.
    let start = if all_lines.len() > max_lines {
        all_lines.len() - max_lines
    } else {
        0
    };
    let tail_lines = &all_lines[start..];

    // Join and truncate to max_bytes from the end.
    let joined = tail_lines.join("\n");

    if joined.len() > max_bytes {
        let byte_start = joined.len() - max_bytes;
        // Find a safe char boundary to avoid splitting multi-byte chars.
        let safe_start = joined.floor_char_boundary(byte_start);
        Some(format!("...(truncated)\n{}", &joined[safe_start..]))
    } else {
        Some(joined)
    }
}

/// Extract the terminal title from a `TerminalView` (no GPUI dependency).
fn extract_title(tv: &TerminalView) -> Option<String> {
    let title = tv.terminal_title_text();
    if title.trim().is_empty() {
        None
    } else {
        Some(title)
    }
}
