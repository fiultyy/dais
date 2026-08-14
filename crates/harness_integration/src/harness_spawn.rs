//! Harness spawn configuration: intercept-mode selection, environment
//! injection, and session lifecycle helpers.
//!
//! ## Intercept modes
//! - **Full** — TLS proxy intercepts LLM traffic *and* hooks fire. Maximum
//!   capture fidelity.
//! - **HooksOnly** — no proxy; capture relies solely on hook callbacks.
//!   Useful when the harness supports hooks but TLS interception is unwanted.
//! - **Bypass** — neither proxy nor hooks. Sessions still get `Spawn`/`Exit`
//!   blocks for bookkeeping.
//!
//! ## Environment injection
//! Combines proxy env vars (from `ProxyManager::env_injection`) with the
//! hook-server URL so the spawned harness process knows where to send
//! callbacks.

use std::collections::HashMap;

use harness_blocks::{BlockStore, BlockType, HarnessBlock, InterceptMode};
use parking_lot::Mutex;
use std::sync::Arc;

use proxy_interceptor::{HarnessType, ProxyHandle, ProxyManager};
use proxy_interceptor::UpstreamConfig;

use crate::session::SessionContext;

/// Env var read to override the intercept mode at spawn time.
pub const INTERCEPT_MODE_ENV: &str = "ZAP_INTERCEPT_MODE";

/// Resolve the effective intercept mode.
///
/// Explicit argument wins; otherwise the `ZAP_INTERCEPT_MODE` env var is
/// consulted; defaults to `Full`.
pub fn resolve_intercept_mode(explicit: Option<InterceptMode>) -> InterceptMode {
    if let Some(m) = explicit {
        return m;
    }
    if let Ok(raw) = std::env::var(INTERCEPT_MODE_ENV) {
        if let Ok(m) = raw.parse() {
            return m;
        }
    }
    InterceptMode::Full
}

/// Build the environment variables to inject into the spawned harness process.
///
/// - **Full**: proxy env vars (base URL + CA cert) + `ZAP_HOOK_SERVER_URL`.
/// - **HooksOnly**: only `ZAP_HOOK_SERVER_URL`.
/// - **Bypass**: empty.
pub fn build_spawn_env(
    mode: InterceptMode,
    proxy: Option<&ProxyHandle>,
    hook_url: Option<&str>,
    harness: HarnessType,
) -> Vec<(String, String)> {
    let mut env: Vec<(String, String)> = Vec::new();

    match mode {
        InterceptMode::Full => {
            if let Some(handle) = proxy {
                for (k, v) in ProxyManager::env_injection(handle, harness) {
                    env.push((k.to_string(), v));
                }
            }
            if let Some(url) = hook_url {
                env.push(("ZAP_HOOK_SERVER_URL".to_string(), url.to_string()));
            }
        }
        InterceptMode::HooksOnly => {
            if let Some(url) = hook_url {
                env.push(("ZAP_HOOK_SERVER_URL".to_string(), url.to_string()));
            }
        }
        InterceptMode::Bypass => {}
    }

    env
}

/// Convert the env vec into a `HashMap` for easy lookup / assertion.
pub fn env_to_map(env: &[(String, String)]) -> HashMap<String, String> {
    env.iter().cloned().collect()
}

// ── session lifecycle ────────────────────────────────────────────────────

/// Record the initial `Spawn` block for a session.
pub fn record_spawn(
    store: &Arc<Mutex<BlockStore>>,
    ctx: &SessionContext,
    mode: InterceptMode,
) {
    let metadata = serde_json::json!({
        "mode": mode.as_str(),
        "harness_type": ctx.harness_type,
    });
    let block = {
        let mut b = HarnessBlock::new(
            &ctx.session_id,
            &ctx.harness_type,
            BlockType::Spawn,
            ctx.next_seq(),
            Vec::new(),
            ctx.now_ms(),
        );
        b.metadata = metadata;
        b
    };
    let s = store.lock();
    let _ = s.insert_block(&block);
}

/// Record the terminal `Exit` block for a session.
pub fn record_exit(
    store: &Arc<Mutex<BlockStore>>,
    ctx: &SessionContext,
    exit_code: i32,
) {
    let metadata = serde_json::json!({ "exit_code": exit_code });
    let block = {
        let mut b = HarnessBlock::new(
            &ctx.session_id,
            &ctx.harness_type,
            BlockType::Exit,
            ctx.next_seq(),
            Vec::new(),
            ctx.now_ms(),
        );
        b.metadata = metadata;
        b
    };
    let s = store.lock();
    let _ = s.insert_block(&block);
}

/// Configuration for spawning a captured harness session.
#[derive(Debug, Clone)]
pub struct SpawnConfig {
    pub mode: InterceptMode,
    pub harness_type: HarnessType,
    pub session_id: String,
    pub upstream: UpstreamConfig,
}

/// A session that has been wired up with proxy + hooks (if applicable).
///
/// Owns the `ProxyHandle` (dropping it shuts down the proxy) and exposes the
/// environment to inject into the actual harness process.
pub struct SpawnedSession {
    pub config: SpawnConfig,
    pub proxy: Option<ProxyHandle>,
    pub hook_base_url: Option<String>,
    pub env: Vec<(String, String)>,
}

impl SpawnedSession {
    /// Convenience: env as a map.
    pub fn env_map(&self) -> HashMap<String, String> {
        env_to_map(&self.env)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_defaults_to_full() {
        // Remove env to ensure deterministic default
        std::env::remove_var(INTERCEPT_MODE_ENV);
        assert_eq!(resolve_intercept_mode(None), InterceptMode::Full);
        assert_eq!(
            resolve_intercept_mode(Some(InterceptMode::Bypass)),
            InterceptMode::Bypass
        );
    }

    #[test]
    fn resolve_from_env() {
        std::env::set_var(INTERCEPT_MODE_ENV, "hooks_only");
        assert_eq!(resolve_intercept_mode(None), InterceptMode::HooksOnly);
        std::env::remove_var(INTERCEPT_MODE_ENV);
    }

    #[test]
    fn bypass_env_is_empty() {
        let env = build_spawn_env(InterceptMode::Bypass, None, None, HarnessType::ClaudeCode);
        assert!(env.is_empty());
    }

    #[test]
    fn hooks_only_has_hook_url() {
        let env = build_spawn_env(
            InterceptMode::HooksOnly,
            None,
            Some("http://127.0.0.1:9999"),
            HarnessType::ClaudeCode,
        );
        let map = env_to_map(&env);
        assert_eq!(map.get("ZAP_HOOK_SERVER_URL").unwrap(), "http://127.0.0.1:9999");
        assert!(!map.contains_key("ANTHROPIC_BASE_URL"));
    }

    #[test]
    fn full_without_proxy_has_hook_url() {
        let env = build_spawn_env(
            InterceptMode::Full,
            None,
            Some("http://127.0.0.1:9999"),
            HarnessType::ClaudeCode,
        );
        let map = env_to_map(&env);
        assert!(map.contains_key("ZAP_HOOK_SERVER_URL"));
        // No proxy → no ANTHROPIC_BASE_URL
        assert!(!map.contains_key("ANTHROPIC_BASE_URL"));
    }
}
