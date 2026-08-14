//! Raw-event processor — drains the proxy's [`RawEvent`] channel and converts
//! captured traffic into blocks.
//!
//! Data flow per request/response pair:
//! ```text
//! RawEvent::Request       → RawCache("request") + parse_anthropic_request → BlockStore
//! RawEvent::ResponseChunk → accumulate per request-id
//! RawEvent::ResponseDone  → RawCache("response") + parse_anthropic_response → BlockStore
//! ```

use std::collections::HashMap;
use std::sync::Arc;

use bytes::Bytes;
use harness_blocks::{BlockStore, RawCache};
use parking_lot::Mutex;
use proxy_interceptor::RawEvent;
use uuid::Uuid;

use crate::block_builder::{parse_anthropic_request, parse_anthropic_response};
use crate::session::SessionContext;

type Store = Arc<Mutex<BlockStore>>;
type Cache = Arc<Mutex<RawCache>>;

/// Run the raw-event processor until the proxy's sender is dropped (channel
/// closed). Designed to be `tokio::spawn`-ed as a background task.
pub async fn run_raw_processor(
    mut rx: tokio::sync::mpsc::Receiver<RawEvent>,
    store: Store,
    raw_cache: Cache,
    ctx: Arc<SessionContext>,
) {
    // Accumulated response chunks keyed by the proxy's request id.
    let mut pending: HashMap<Uuid, Vec<(u64, Bytes)>> = HashMap::new();

    while let Some(event) = rx.recv().await {
        match event {
            RawEvent::Request { id, body, .. } => {
                let ts = ctx.now_ms();
                let session = ctx.session_id.clone();

                // 1. Raw cache
                {
                    let cache = raw_cache.lock();
                    if let Err(e) = cache.insert_raw(&session, "request", &body, ts) {
                        tracing::warn!("raw_cache insert request failed: {e}");
                    }
                }

                // 2. Parse → blocks
                let blocks = parse_anthropic_request(&body, &ctx);
                {
                    let s = store.lock();
                    for b in &blocks {
                        if let Err(e) = s.insert_block(b) {
                            tracing::warn!("block insert (request) failed: {e}");
                        }
                    }
                }

                // Track id so we can correlate the response.
                pending.entry(id).or_default();
            }

            RawEvent::ResponseChunk { id, seq, chunk } => {
                pending.entry(id).or_default().push((seq, chunk));
            }

            RawEvent::ResponseDone { id, .. } => {
                let Some(mut chunks) = pending.remove(&id) else {
                    continue;
                };

                // Sort by proxy sequence to guarantee byte order.
                chunks.sort_by_key(|(seq, _)| *seq);
                let mut body: Vec<u8> = Vec::with_capacity(chunks.len() * 128);
                for (_, c) in chunks {
                    body.extend_from_slice(&c);
                }

                let ts = ctx.now_ms();
                let session = ctx.session_id.clone();

                {
                    let cache = raw_cache.lock();
                    if let Err(e) = cache.insert_raw(&session, "response", &body, ts) {
                        tracing::warn!("raw_cache insert response failed: {e}");
                    }
                }

                let blocks = parse_anthropic_response(&body, &ctx);
                {
                    let s = store.lock();
                    for b in &blocks {
                        if let Err(e) = s.insert_block(b) {
                            tracing::warn!("block insert (response) failed: {e}");
                        }
                    }
                }
            }
        }
    }
}
