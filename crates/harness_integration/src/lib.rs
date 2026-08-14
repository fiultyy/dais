//! harness_integration — the integration layer that ties the TLS proxy
//! ([`proxy_interceptor`]) and the hook callbacks together with the block
//! data layer ([`harness_blocks`]).
//!
//! ## Architecture
//!
//! ```text
//!                  ┌──────────────────────────────────┐
//!   harness  ──HTTPS──▶  proxy_interceptor (TLS MITM)  │
//!   process          │   raw_tx ──▶ RawEvent channel   │
//!     │              └──────────────┬───────────────────┘
//!     │ hooks                       │
//!     │ (HTTP POST)                 ▼
//!     │              ┌──── raw_processor ────┐
//!     ▼              │  RawEvent → RawCache   │
//!   hook_server ──blocks──▶ BlockStore ◀──────┘
//! ```
//!
//! The [`Integration`] struct owns the shared [`BlockStore`] / [`RawCache`] /
//! [`SessionContext`] and orchestrates starting the proxy + hook server.

mod block_builder;
mod harness_spawn;
mod hook_server;
mod raw_processor;
mod session;

pub use block_builder::{parse_anthropic_request, parse_anthropic_response};
pub use harness_spawn::{
    build_spawn_env, env_to_map, record_exit, record_spawn,
    resolve_intercept_mode, SpawnConfig, SpawnedSession, INTERCEPT_MODE_ENV,
};
pub use hook_server::HookServer;
pub use raw_processor::run_raw_processor;
pub use session::SessionContext;

// Re-export key types from upstream crates for ergonomic single-crate access.
pub use harness_blocks::{
    get_session_summary, get_system_prompt, list_blocks_by_session, list_blocks_by_type,
    BlockStore, BlockType, HarnessBlock, InterceptMode, RawCache,
};
pub use proxy_interceptor::{
    HarnessType, ProxyHandle, ProxyManager, ProxyServer, RawEvent, ResponseFormat, UpstreamConfig,
};

use std::sync::Arc;

use parking_lot::Mutex;

/// Orchestrator for a single captured session.
///
/// Holds the shared stores and context; optionally owns a running hook server
/// and a proxy handle. Cheap to move; drop shuts everything down.
pub struct Integration {
    store: Arc<Mutex<BlockStore>>,
    raw_cache: Arc<Mutex<RawCache>>,
    ctx: Arc<SessionContext>,
    hook_server: Option<HookServer>,
    /// The proxy handle + background processor task.
    proxy: Option<ProxyBundle>,
}

struct ProxyBundle {
    handle: ProxyHandle,
    /// Background task draining RawEvents. Aborted on drop.
    task: tokio::task::JoinHandle<()>,
}

impl Drop for ProxyBundle {
    fn drop(&mut self) {
        // Aborting the processor task also closes the channel; the ProxyHandle
        // shuts down the TLS listener on drop.
        self.task.abort();
    }
}

impl Integration {
    /// Create a new integration backed by in-memory stores.
    pub fn in_memory(session_id: &str, harness_type: &str) -> anyhow::Result<Self> {
        let store = BlockStore::open_in_memory()?;
        let raw_cache = RawCache::open_in_memory()?;
        Ok(Self {
            store: Arc::new(Mutex::new(store)),
            raw_cache: Arc::new(Mutex::new(raw_cache)),
            ctx: Arc::new(SessionContext::new(session_id, harness_type)),
            hook_server: None,
            proxy: None,
        })
    }

    /// Access the shared session context.
    pub fn ctx(&self) -> &SessionContext {
        &self.ctx
    }

    /// Borrow the store under the internal lock. The closure receives a
    /// `&BlockStore` for read queries.
    pub fn with_store<R>(&self, f: impl FnOnce(&BlockStore) -> R) -> R {
        let guard = self.store.lock();
        f(&guard)
    }

    // ── hooks ────────────────────────────────────────────────────────────

    /// Start the hook server (idempotent — returns the existing one).
    pub async fn start_hooks(&mut self) -> anyhow::Result<&HookServer> {
        if self.hook_server.is_none() {
            let server =
                HookServer::start(self.store.clone(), self.ctx.clone()).await?;
            self.hook_server = Some(server);
        }
        Ok(self.hook_server.as_ref().unwrap())
    }

    /// Hook callback base URL, if the server is running.
    pub fn hook_url(&self) -> Option<String> {
        self.hook_server.as_ref().map(|s| s.base_url())
    }

    // ── proxy ────────────────────────────────────────────────────────────

    /// Allocate a TLS proxy pointing at `upstream` and spawn the background
    /// raw-event processor. Requires `ProxyManager` to have been created.
    pub async fn start_proxy(
        &mut self,
        manager: &ProxyManager,
        upstream: UpstreamConfig,
    ) -> anyhow::Result<u16> {
        if self.proxy.is_some() {
            return Ok(self.proxy.as_ref().unwrap().handle.port);
        }
        let mut handle = manager.allocate(upstream).await.map_err(anyhow::Error::msg)?;
        let raw_rx = std::mem::replace(&mut handle.raw_rx, {
            // Replace with a closed receiver so the moved-out field is valid.
            let (_, rx) = tokio::sync::mpsc::channel(1);
            rx
        });

        let task = tokio::spawn(run_raw_processor(
            raw_rx,
            self.store.clone(),
            self.raw_cache.clone(),
            self.ctx.clone(),
        ));

        let port = handle.port;
        self.proxy = Some(ProxyBundle { handle, task });
        Ok(port)
    }

    /// Proxy port, if running.
    pub fn proxy_port(&self) -> Option<u16> {
        self.proxy.as_ref().map(|p| p.handle.port)
    }

    /// Path to the proxy's CA certificate, if running.
    pub fn ca_cert_path(&self) -> Option<&std::path::Path> {
        self.proxy.as_ref().map(|p| p.handle.ca_cert_path.as_path())
    }

    /// Borrow the proxy handle (for env injection).
    pub fn proxy_handle(&self) -> Option<&ProxyHandle> {
        self.proxy.as_ref().map(|p| &p.handle)
    }

    // ── lifecycle ────────────────────────────────────────────────────────

    /// Record the initial Spawn block.
    pub fn record_spawn(&self, mode: InterceptMode) {
        harness_spawn::record_spawn(&self.store, &self.ctx, mode);
    }

    /// Record the terminal Exit block.
    pub fn record_exit(&self, exit_code: i32) {
        harness_spawn::record_exit(&self.store, &self.ctx, exit_code);
    }
}
