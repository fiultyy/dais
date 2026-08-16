//! External capture manager (T1c skeleton + T2b live registration, Plan A).
//!
//! ## Why this lives in `harness_integration` (not the app crate)
//!
//! `harness_integration` is the designated integration layer between
//! [`proxy_interceptor`] (TLS proxy + CA) and the hook/block data layer.
//! External capture — terminals the user launches themselves (`claude`,
//! `codex`, …) — needs exactly those same seams, so the resident manager
//! belongs beside the per-session [`crate::Integration`]: the crate keeps
//! both lifecycle shapes (session-scoped vs app-scoped) discoverable in one
//! place, stays headless-testable (`cargo test -p harness_integration`),
//! and remains reusable outside the GUI app. The app layer (T3) wraps this
//! in a singleton and owns the app lifetime; this type owns the mechanics.
//!
//! ## Plan A shape (T2b)
//!
//! - Hook servers are **per registration**: [`HookServer::start`] binds the
//!   session's own [`SessionContext`], so hook→session attribution is solved
//!   by construction (each registration has a distinct token and a distinct
//!   server).
//! - Proxies are **per registration** via the shared [`ProxyManager`], which
//!   the manager holds for the app lifetime so the local CA is generated
//!   exactly once.
//! - Registration wiring: [`ExternalCaptureManager::register_external_session`]
//!   mints the session id, starts the hook server, allocates the proxy,
//!   spawns the raw processor, records the `Spawn` block (metadata
//!   `mode: "external"` — deliberately *not* a new [`harness_blocks::InterceptMode`]
//!   variant, which would break exhaustive UI matches in the app), and
//!   returns the per-registration env-var set built by [`env_lines_for`].
//! - Reclaim: [`ExternalCaptureManager::reap_idle`] unregisters registrations
//!   idle beyond [`IDLE_TIMEOUT_MS`] (drops proxy + hook server, records an
//!   `Exit` block; blocks stay in the DB for observatory history), and
//!   [`ExternalCaptureManager::stop_registration`] does the same on demand.
//!   Idle is defined as *no `RawEvent` for 30 min*; the clock is injectable
//!   via [`ExternalCaptureManager::with_clock`] so tests never sleep.
//! - No hard cap on registration count: resource pressure is bounded by the
//!   idle reaper (T3 wires a periodic tick that calls `reap_idle`).
//!
//! The per-session spawn path ([`crate::harness_spawn`]) is intentionally
//! untouched — external capture composes env *values* for processes the app
//! does not spawn, via [`env_lines_for`].

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use harness_blocks::{BlockStore, BlockType, HarnessBlock, RawCache};
use parking_lot::Mutex;
use proxy_interceptor::{HarnessType, ProxyHandle, ProxyManager, UpstreamConfig};
use tokio::task::JoinHandle;
use uuid::Uuid;

use crate::hook_server::HookServer;
use crate::raw_processor::run_raw_processor;
use crate::session::SessionContext;

/// Hook server URL env var (same contract as `harness_spawn::build_spawn_env`).
pub const HOOK_SERVER_URL_ENV: &str = "ZAP_HOOK_SERVER_URL";
/// Hook server token env var (same contract as `harness_spawn::build_spawn_env`).
pub const HOOK_TOKEN_ENV: &str = "ZAP_HOOK_TOKEN";

/// Idle threshold for [`ExternalCaptureManager::reap_idle`]: a registration
/// with no `RawEvent` for this long is reclaimed (30 min, milliseconds).
pub const IDLE_TIMEOUT_MS: i64 = 30 * 60 * 1000;

/// Stable id of one external-capture registration. Opaque, copyable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RegistrationId(u64);

impl RegistrationId {
    /// Numeric form (for logging / display).
    pub fn as_u64(self) -> u64 {
        self.0
    }
}

/// Registry data for one external capture. Pure data — snapshots cheaply and
/// carries everything T3's UI needs to render a registration row.
#[derive(Debug, Clone)]
pub struct Slot {
    /// Captured session id (uuid v4, minted at registration).
    pub session_id: String,
    /// Harness this registration captures.
    pub harness: HarnessType,
    /// Assigned TLS-proxy port.
    pub proxy_port: u16,
    /// Shared CA cert path (app-lifetime [`ProxyManager`]).
    pub ca_path: PathBuf,
    /// Per-registration hook server base URL (`http://127.0.0.1:<port>`).
    pub hook_base_url: String,
    /// Per-registration hook auth token.
    pub hook_token: String,
    /// Last `RawEvent` time (ms, injected clock); refreshed by `reap_idle`
    /// and `registrations` reads.
    pub last_activity_ms: i64,
    /// Registration time (ms, injected clock).
    pub born_at: i64,
}

/// Read-only view of a registration (snapshot copy, safe to hand to UI).
#[derive(Debug, Clone)]
pub struct RegistrationSnapshot {
    pub id: RegistrationId,
    pub session_id: String,
    pub harness: HarnessType,
    pub proxy_port: u16,
    pub hook_base_url: String,
    pub hook_token: String,
    pub last_activity_ms: i64,
    pub born_at: i64,
}

/// Handoff returned by [`ExternalCaptureManager::register_external_session`]:
/// everything the caller (T3 UI / terminal launcher) needs to point an
/// externally spawned harness process at this registration.
#[derive(Debug, Clone)]
pub struct Registration {
    pub id: RegistrationId,
    pub session_id: String,
    pub harness: HarnessType,
    /// TLS proxy port (harness env points its API base / HTTPS_PROXY here).
    pub proxy_port: u16,
    /// Per-registration hook callback base URL.
    pub hook_base_url: String,
    /// Per-registration hook auth token.
    pub hook_token: String,
    /// Complete per-registration env-var set ([`env_lines_for`]).
    pub env: Vec<(String, String)>,
}

/// Injectable clock (ms since epoch). Tests pass a controllable closure;
/// production uses the system clock.
type Clock = Arc<dyn Fn() -> i64 + Send + Sync>;

fn system_now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Live machinery behind one registration. Dropping it tears the
/// registration down: proxy listener closes, hook server aborts, background
/// tasks stop. Blocks already written stay in the store.
#[allow(dead_code)] // `proxy`/`hook` are read only via Drop side effects (kept alive).
struct LiveRegistration {
    ctx: Arc<SessionContext>,
    /// Store handle kept for the spawn/exit-block writes at reclaim time.
    store: Arc<Mutex<BlockStore>>,
    /// Kept alive for the registration's lifetime; `Drop` releases the port.
    proxy: ProxyHandle,
    /// Kept alive for the registration's lifetime; `Drop` aborts the server.
    hook: HookServer,
    processor: JoinHandle<()>,
    forwarder: JoinHandle<()>,
    /// `RawEvent` arrival time (ms, injected clock); the idle signal.
    activity: Arc<AtomicI64>,
}


impl Drop for LiveRegistration {
    fn drop(&mut self) {
        // Aborting the forwarder drops its sender, which ends the processor's
        // recv loop; aborting the processor covers the in-flight case.
        self.forwarder.abort();
        self.processor.abort();
        // `proxy` / `hook` field drops shut down the TLS listener / axum task.
    }
}

struct Inner {
    proxy: ProxyManager,
}

struct Entry {
    slot: Slot,
    live: LiveRegistration,
}

/// Resident (app-lifetime) manager for external captures.
///
/// Owns the shared [`ProxyManager`] purely for CA lifecycle: initialization
/// is deferred and idempotent, so app startup does not pay CA generation
/// until the first external capture actually needs it, and a failed
/// initialization stays retryable.
pub struct ExternalCaptureManager {
    /// `None` until [`ExternalCaptureManager::ensure_initialized`] succeeds.
    inner: Option<Inner>,
    entries: HashMap<RegistrationId, Entry>,
    next_id: u64,
    /// `(blocks_db, raw_cache_db)`; `None` → in-memory stores (unit tests /
    /// non-persistent use). The app passes the persistent observatory paths
    /// so captured blocks are visible to the UI.
    db_paths: Option<(PathBuf, PathBuf)>,
    clock: Clock,
}

impl Default for ExternalCaptureManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Map harness enum to the harness_type string recorded in blocks (same
/// mapping the app's intercept wiring uses).
fn harness_type_str(harness: HarnessType) -> &'static str {
    match harness {
        HarnessType::ClaudeCode => "claude-code",
        HarnessType::Codex => "codex",
        HarnessType::Omp => "omp",
        HarnessType::Generic => "generic",
    }
}

/// Record the session-opening `Spawn` block with `mode: "external"`.
///
/// Local (not `harness_spawn::record_spawn`) on purpose: `InterceptMode` has
/// no `External` variant — adding one would break exhaustive UI matches in
/// the app — and external metadata carries the registration reason.
fn record_external_spawn(store: &Arc<Mutex<BlockStore>>, ctx: &SessionContext) {
    let block = {
        let mut b = HarnessBlock::new(
            &ctx.session_id,
            &ctx.harness_type,
            BlockType::Spawn,
            ctx.next_seq(),
            Vec::new(),
            ctx.now_ms(),
        );
        b.metadata = serde_json::json!({
            "mode": "external",
            "harness_type": ctx.harness_type,
        });
        b
    };
    let s = store.lock();
    let _ = s.insert_block(&block);
}

/// Record the terminal `Exit` block (`reason`: "idle_timeout" | "stopped").
fn record_external_exit(store: &Arc<Mutex<BlockStore>>, ctx: &SessionContext, reason: &str) {
    let block = {
        let mut b = HarnessBlock::new(
            &ctx.session_id,
            &ctx.harness_type,
            BlockType::Exit,
            ctx.next_seq(),
            Vec::new(),
            ctx.now_ms(),
        );
        b.metadata = serde_json::json!({ "exit_code": 0, "reason": reason });
        b
    };
    let s = store.lock();
    let _ = s.insert_block(&block);
}

impl ExternalCaptureManager {
    /// Uninitialized, in-memory, system-clock shell. Call
    /// [`Self::ensure_initialized`] (or the first `register_external_session`,
    /// which does it for you) before CA matters.
    pub fn new() -> Self {
        Self {
            inner: None,
            entries: HashMap::new(),
            next_id: 0,
            db_paths: None,
            clock: Arc::new(system_now_ms),
        }
    }

    /// Persistent-store variant: every registration's blocks/raw-cache land
    /// in these files (the app passes the observatory's `harness_blocks.db`
    /// so sessions appear in the UI automatically).
    pub fn with_db_paths(
        mut self,
        blocks_db: impl Into<PathBuf>,
        raw_cache_db: impl Into<PathBuf>,
    ) -> Self {
        self.db_paths = Some((blocks_db.into(), raw_cache_db.into()));
        self
    }

    /// Override the clock (ms since epoch). Primarily for tests — the idle
    /// reaper becomes deterministic without sleeping.
    pub fn with_clock(mut self, clock: impl Fn() -> i64 + Send + Sync + 'static) -> Self {
        self.clock = Arc::new(clock);
        self
    }

    /// Idempotent initialization: creates the shared [`ProxyManager`]
    /// (which generates/loads the local CA via `ensure_local_ca` — reused,
    /// not reimplemented) on the first call only. Subsequent calls are
    /// no-ops that preserve the CA and the registry.
    pub fn ensure_initialized(&mut self) -> proxy_interceptor::Result<()> {
        if self.inner.is_none() {
            self.inner = Some(Inner {
                proxy: ProxyManager::new()?,
            });
        }
        Ok(())
    }

    /// Whether [`Self::ensure_initialized`] has succeeded.
    pub fn is_initialized(&self) -> bool {
        self.inner.is_some()
    }

    /// Persisted CA cert path (`None` before initialization).
    pub fn ca_cert_path(&self) -> Option<&Path> {
        self.inner.as_ref().map(|inner| inner.proxy.ca_cert_path())
    }

    /// CA cert path as owned `PathBuf`, or `None` before initialization.
    pub fn ca_cert_path_buf(&self) -> Option<PathBuf> {
        self.ca_cert_path().map(Path::to_path_buf)
    }

    /// Register one external capture session (T2b core).
    ///
    /// Wires, in order: uuid v4 session id → per-session [`SessionContext`]
    /// → dedicated [`HookServer`] bound to that ctx (attribution by
    /// construction) → dedicated proxy via the shared [`ProxyManager`]
    /// (upstream resolved three-tier: explicit `ZAP_UPSTREAM_BASE` env >
    /// harness default) → raw-event forwarder that timestamps activity on
    /// the injected clock → [`run_raw_processor`] → `Spawn` block
    /// (`mode: "external"`). Returns the handoff ([`Registration`]) with the
    /// complete env-var set for the harness process.
    ///
    /// No hard cap on registration count; idle reaping bounds resources.
    pub async fn register_external_session(
        &mut self,
        harness: HarnessType,
    ) -> anyhow::Result<Registration> {
        self.ensure_initialized().map_err(anyhow::Error::msg)?;
        let inner = self.inner.as_ref().expect("ensure_initialized just ran");

        let session_id = Uuid::new_v4().to_string();
        let ctx = Arc::new(SessionContext::new(
            session_id.clone(),
            harness_type_str(harness),
        ));

        let store: Arc<Mutex<BlockStore>> = match &self.db_paths {
            Some((blocks_db, _)) => Arc::new(Mutex::new(BlockStore::open(
                blocks_db.to_string_lossy().to_string(),
            )?)),
            None => Arc::new(Mutex::new(BlockStore::open_in_memory()?)),
        };
        let raw_cache: Arc<Mutex<RawCache>> = match &self.db_paths {
            Some((_, raw_db)) => Arc::new(Mutex::new(RawCache::open(
                raw_db.to_string_lossy().to_string(),
            )?)),
            None => Arc::new(Mutex::new(RawCache::open_in_memory()?)),
        };

        let hook = HookServer::start(store.clone(), ctx.clone()).await?;

        let upstream = UpstreamConfig::resolve(harness, None).map_err(anyhow::Error::msg)?;
        let mut proxy = inner.proxy.allocate(upstream).await.map_err(anyhow::Error::msg)?;

        // Detach the raw-event channel (same trick as `Integration::start_proxy`):
        // the forwarder counts activity, then feeds the processor.
        let raw_rx = std::mem::replace(&mut proxy.raw_rx, {
            let (_, rx) = tokio::sync::mpsc::channel(1);
            rx
        });
        let activity = Arc::new(AtomicI64::new((self.clock)()));
        let born_at = activity.load(Ordering::Relaxed);

        let (fwd_tx, proc_rx) = tokio::sync::mpsc::channel(256);
        let forwarder = {
            let activity = activity.clone();
            let clock = self.clock.clone();
            tokio::spawn(async move {
                let mut raw_rx = raw_rx;
                while let Some(event) = raw_rx.recv().await {
                    activity.store(clock(), Ordering::Relaxed);
                    if fwd_tx.send(event).await.is_err() {
                        break;
                    }
                }
            })
        };
        let processor = tokio::spawn(run_raw_processor(
            proc_rx,
            store.clone(),
            raw_cache,
            ctx.clone(),
        ));

        record_external_spawn(&store, &ctx);

        let registration = Registration {
            id: RegistrationId(self.next_id),
            session_id: session_id.clone(),
            harness,
            proxy_port: proxy.port,
            hook_base_url: hook.base_url(),
            hook_token: hook.token().to_string(),
            env: env_lines_for(
                proxy.port,
                &proxy.ca_cert_path,
                &hook.base_url(),
                hook.token(),
                harness,
            ),
        };

        let slot = Slot {
            session_id,
            harness,
            proxy_port: proxy.port,
            ca_path: proxy.ca_cert_path.clone(),
            hook_base_url: hook.base_url(),
            hook_token: hook.token().to_string(),
            last_activity_ms: born_at,
            born_at,
        };
        let live = LiveRegistration {
            ctx,
            store,
            proxy,
            hook,
            processor,
            forwarder,
            activity,
        };

        self.next_id += 1;
        self.entries.insert(registration.id, Entry { slot, live });
        Ok(registration)
    }

    /// Graceful stop: records the `Exit` block (`reason: "stopped"`) and
    /// tears the registration down (drops proxy + hook server). Blocks stay
    /// in the store. Returns `false` if the id was unknown.
    pub fn stop_registration(&mut self, id: RegistrationId) -> bool {
        match self.entries.remove(&id) {
            Some(entry) => {
                record_external_exit(&entry.live.store, &entry.live.ctx, "stopped");
                true
            }
            None => false,
        }
    }

    /// Reclaim registrations idle beyond [`IDLE_TIMEOUT_MS`] (no `RawEvent`,
    /// injected clock): each is unregistered, its proxy + hook server
    /// dropped, and an `Exit` block (`reason: "idle_timeout"`) recorded.
    /// Returns the reaped ids. The app (T3) calls this on a periodic tick;
    /// blocks stay in the DB (observatory history).
    pub fn reap_idle(&mut self) -> Vec<RegistrationId> {
        let now = (self.clock)();
        let reaped: Vec<RegistrationId> = self
            .entries
            .iter()
            .filter(|(_, entry)| {
                now.saturating_sub(entry.live.activity.load(Ordering::Relaxed)) > IDLE_TIMEOUT_MS
            })
            .map(|(id, _)| *id)
            .collect();
        for id in &reaped {
            if let Some(entry) = self.entries.remove(id) {
                record_external_exit(&entry.live.store, &entry.live.ctx, "idle_timeout");
            }
        }
        reaped
    }

    /// Raw removal without an `Exit` block (caller-managed teardown).
    /// Prefer [`Self::stop_registration`] for the normal lifecycle.
    pub fn unregister(&mut self, id: RegistrationId) -> Option<Slot> {
        self.entries.remove(&id).map(|mut entry| {
            entry.slot.last_activity_ms = entry.live.activity.load(Ordering::Relaxed);
            entry.slot
        })
    }

    /// Look up a slot. `last_activity_ms` is as of the last sync; call
    /// [`Self::registrations`] for a fresh read.
    pub fn get(&self, id: RegistrationId) -> Option<&Slot> {
        self.entries.get(&id).map(|e| &e.slot)
    }

    /// Read-only registry snapshot with activity synced from the live
    /// counters.
    pub fn registrations(&self) -> Vec<RegistrationSnapshot> {
        self.entries
            .iter()
            .map(|(id, entry)| RegistrationSnapshot {
                id: *id,
                session_id: entry.slot.session_id.clone(),
                harness: entry.slot.harness,
                proxy_port: entry.slot.proxy_port,
                hook_base_url: entry.slot.hook_base_url.clone(),
                hook_token: entry.slot.hook_token.clone(),
                last_activity_ms: entry.live.activity.load(Ordering::Relaxed),
                born_at: entry.slot.born_at,
            })
            .collect()
    }
}

/// Assemble the environment for an externally launched harness process.
///
/// Composition mirrors `harness_spawn::build_spawn_env` in **Full** mode
/// (proxy vars + hook vars) but takes recorded *values* instead of a live
/// [`proxy_interceptor::ProxyHandle`]: external captures are processes the
/// app did not spawn, so there is no handle to borrow — the registration
/// captures the port/path at registration time and replays them here.
///
/// Key sets per harness (via [`ProxyManager::env_injection_for`], reused —
/// not duplicated):
/// - `ClaudeCode` → `ANTHROPIC_BASE_URL` + `NODE_EXTRA_CA_CERTS` + hook vars
/// - `Codex` → `OPENAI_BASE_URL` + hook vars
/// - `Omp` | `Generic` → `HTTPS_PROXY` + `SSL_CERT_FILE` + hook vars
pub fn env_lines_for(
    proxy_port: u16,
    ca_path: &Path,
    hook_url: &str,
    hook_token: &str,
    harness: HarnessType,
) -> Vec<(String, String)> {
    let mut env: Vec<(String, String)> =
        ProxyManager::env_injection_for(proxy_port, ca_path, harness)
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect();
    env.push((HOOK_SERVER_URL_ENV.to_string(), hook_url.to_string()));
    env.push((HOOK_TOKEN_ENV.to_string(), hook_token.to_string()));
    env
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env_map(env: &[(String, String)]) -> HashMap<String, String> {
        env.iter().cloned().collect()
    }

    // ── ensure_initialized ────────────────────────────────────────────────

    /// HOME is process-global; only this test touches it in this crate, and
    /// it restores the original value on exit. The tempdir isolates CA
    /// generation from the developer machine's real `~/.config/zap/proxy-ca`.
    #[test]
    fn ensure_initialized_is_idempotent_and_preserves_registry() {
        let tmp = tempfile::tempdir().unwrap();
        let orig_home = std::env::var("HOME").ok();
        std::env::set_var("HOME", tmp.path());
        let result = (|| {
            let mut mgr = ExternalCaptureManager::new();
            assert!(!mgr.is_initialized());
            assert!(mgr.ca_cert_path().is_none());

            // First call: initializes (generates CA under the temp HOME).
            mgr.ensure_initialized().unwrap();
            assert!(mgr.is_initialized());
            let ca_path = mgr.ca_cert_path().expect("initialized").to_path_buf();
            assert_eq!(
                ca_path,
                tmp.path()
                    .join(".config")
                    .join("zap")
                    .join("proxy-ca")
                    .join("ca-cert.pem")
            );
            assert!(ca_path.is_file(), "CA cert must be persisted on first init");
            // CA generation actually ran: the key file exists too.
            assert!(tmp.path().join(".config/zap/proxy-ca/ca-key.pem").is_file());

            let ca_bytes_before = std::fs::read(&ca_path).unwrap();

            mgr.ensure_initialized().unwrap(); // second call: no-op
            assert!(mgr.is_initialized());
            assert_eq!(mgr.ca_cert_path(), Some(ca_path.as_path()));
            // No regeneration: persisted CA bytes unchanged.
            assert_eq!(std::fs::read(&ca_path).unwrap(), ca_bytes_before);
            // Registry survives re-init.
            assert!(mgr.registrations().is_empty());
        })();
        match orig_home {
            Some(home) => std::env::set_var("HOME", home),
            None => std::env::remove_var("HOME"),
        }
        result
    }

    // ── env_lines_for: per-harness key sets ────────────────────────────────

    const PORT: u16 = 8443;
    const CA: &str = "/tmp/zap-test/ca-cert.pem";
    const HOOK_URL: &str = "http://127.0.0.1:9911/hook";
    const HOOK_TOKEN: &str = "tok-abc123";

    fn lines(harness: HarnessType) -> (Vec<(String, String)>, HashMap<String, String>) {
        let env = env_lines_for(PORT, Path::new(CA), HOOK_URL, HOOK_TOKEN, harness);
        let map = env_map(&env);
        // Hook vars are always present and correctly valued.
        assert_eq!(map.get(HOOK_SERVER_URL_ENV).unwrap(), HOOK_URL);
        assert_eq!(map.get(HOOK_TOKEN_ENV).unwrap(), HOOK_TOKEN);
        (env, map)
    }

    #[test]
    fn env_lines_for_claude_code() {
        let (env, map) = lines(HarnessType::ClaudeCode);
        // Exactly: ANTHROPIC_BASE_URL + NODE_EXTRA_CA_CERTS + 2 hook keys.
        assert_eq!(env.len(), 4);
        assert_eq!(
            map.get("ANTHROPIC_BASE_URL").unwrap(),
            "https://127.0.0.1:8443"
        );
        assert_eq!(map.get("NODE_EXTRA_CA_CERTS").unwrap(), CA);
        assert!(!map.contains_key("HTTPS_PROXY"));
        assert!(!map.contains_key("SSL_CERT_FILE"));
        assert!(!map.contains_key("OPENAI_BASE_URL"));
    }

    #[test]
    fn env_lines_for_codex() {
        let (env, map) = lines(HarnessType::Codex);
        // Exactly: OPENAI_BASE_URL + 2 hook keys (no CA var for Codex).
        assert_eq!(env.len(), 3);
        assert_eq!(map.get("OPENAI_BASE_URL").unwrap(), "https://127.0.0.1:8443");
        assert!(!map.contains_key("NODE_EXTRA_CA_CERTS"));
        assert!(!map.contains_key("HTTPS_PROXY"));
        assert!(!map.contains_key("SSL_CERT_FILE"));
    }

    #[test]
    fn env_lines_for_omp_and_generic_share_https_proxy_shape() {
        for harness in [HarnessType::Omp, HarnessType::Generic] {
            let (env, map) = lines(harness);
            // Exactly: HTTPS_PROXY + SSL_CERT_FILE + 2 hook keys.
            assert_eq!(env.len(), 4, "{harness:?}");
            assert_eq!(map.get("HTTPS_PROXY").unwrap(), "https://127.0.0.1:8443");
            assert_eq!(map.get("SSL_CERT_FILE").unwrap(), CA);
            assert!(!map.contains_key("ANTHROPIC_BASE_URL"));
            assert!(!map.contains_key("OPENAI_BASE_URL"));
        }
    }

    // ── manager shell state ────────────────────────────────────────────────

    #[test]
    fn uninitialized_state_is_reported_consistently() {
        let mut mgr = ExternalCaptureManager::new();
        assert!(!mgr.is_initialized());
        assert!(mgr.ca_cert_path().is_none());
        assert!(mgr.ca_cert_path_buf().is_none());
        assert!(mgr.registrations().is_empty());
        assert_eq!(RegistrationId(0).as_u64(), 0);
        // Unknown-id operations are graceful no-ops.
        assert!(!mgr.stop_registration(RegistrationId(42)));
        assert!(mgr.unregister(RegistrationId(42)).is_none());
        assert!(mgr.get(RegistrationId(42)).is_none());
    }

    #[test]
    fn builders_set_persistence_and_clock() {
        // Builder plumbing is observable through behavior elsewhere; here we
        // pin that builders preserve the uninitialized shell state.
        let fake = std::sync::atomic::AtomicI64::new(7_000);
        let mgr = ExternalCaptureManager::new()
            .with_db_paths("/nonexistent/b.db", "/nonexistent/r.db")
            .with_clock(move || fake.load(Ordering::Relaxed));
        assert!(!mgr.is_initialized(), "builders must not initialize");
        assert!(mgr.registrations().is_empty());
    }
}
