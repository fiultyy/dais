//! InterceptSessionsModel — 全局拦截会话配置单例 (issue #13)。
//!
//! 持有三份 UI 需要的状态:
//! 1. [`InterceptMode`] — 新 harness session 的拦截模式 (Full / HooksOnly / Bypass),
//!    spawn 时经 `harness_integration::resolve_intercept_mode` 注入环境变量。
//! 2. Upstream 显式覆盖 — API Base (留空 = 三级优先解析里的自动探测) 与
//!    Auth 环境变量名覆盖,经 `UpstreamConfig::resolve` 得到探测结果。
//! 3. block 计数 — 从持久化的 [`BlockStore`] (`<state_dir>/harness_blocks.db`)
//!    查询已捕获 blocks 总数,供 tab badge / 配置栏显示。
//!
//! 该 model 只做配置与读查询,不启动 proxy / hook server;那些生命周期由
//! `harness_integration::Integration` 在 session spawn 时管理。

use std::sync::Arc;

use harness_integration::{BlockStore, HarnessType, InterceptMode, UpstreamConfig};
use parking_lot::Mutex;
use warpui::{Entity, ModelContext, SingletonEntity};

/// Events emitted when the intercept configuration or captured-block count changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterceptSessionsModelEvent {
    /// The intercept mode changed (Full / HooksOnly / Bypass).
    ModeChanged,
    /// The explicit upstream overrides (api base / auth env) changed.
    UpstreamChanged,
    /// The captured block count was refreshed.
    BlocksChanged,
}

pub struct InterceptSessionsModel {
    mode: InterceptMode,
    /// Explicit upstream API base override. Empty string = auto-detect
    /// (env `ZAP_UPSTREAM_BASE`, then harness default).
    upstream_base: String,
    /// Explicit auth env-var name override (e.g. `ANTHROPIC_API_KEY`).
    /// Empty = keep the resolved default from [`UpstreamConfig`].
    upstream_auth_env: String,
    /// Cached captured-block count from the persistent BlockStore.
    block_count: u64,
    /// Persistent block store at `<state_dir>/harness_blocks.db`.
    /// `None` when the store cannot be opened (read-only queries then report 0).
    store: Option<Arc<Mutex<BlockStore>>>,
}

/// 持久化的拦截配置（`<state_dir>/intercept_config.json`）。
/// block 计数等运行态不持久化。
#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
struct PersistedConfig {
    mode: Option<InterceptMode>,
    upstream_base: Option<String>,
    upstream_auth_env: Option<String>,
}

fn config_path() -> Option<std::path::PathBuf> {
    let dir = warp_core::paths::state_dir();
    if dir.as_os_str().is_empty() {
        return None;
    }
    Some(dir.join("intercept_config.json"))
}

fn load_persisted_config() -> PersistedConfig {
    let Some(path) = config_path() else {
        return PersistedConfig::default();
    };
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

fn save_persisted_config(cfg: &PersistedConfig) {
    let Some(path) = config_path() else { return };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    match serde_json::to_string_pretty(cfg) {
        Ok(raw) => {
            if let Err(e) = std::fs::write(&path, raw) {
                log::warn!("intercept: cannot persist config {}: {e}", path.display());
            }
        }
        Err(e) => log::warn!("intercept: cannot serialize config: {e}"),
    }
}
impl InterceptSessionsModel {
    pub fn new(_ctx: &mut ModelContext<Self>) -> Self {
        // flag 未开启时不打开/创建 DB,避免未启用用户产生启动期文件 IO
        // (create_dir_all + SQLite open + COUNT(*))。store 保持 None,
        // refresh_block_count 在 flag 开启后按需惰性打开。
        let store = if crate::features::FeatureFlag::AgentHarness.is_enabled() {
            open_persistent_store()
        } else {
            None
        };
        let block_count = store
            .as_ref()
            .and_then(|s| s.lock().block_count().ok())
            .unwrap_or(0);
        // 持久化配置只在 flag 开启时加载（未启用用户不读文件）
        let persisted = if crate::features::FeatureFlag::AgentHarness.is_enabled() {
            load_persisted_config()
        } else {
            PersistedConfig::default()
        };
        Self {
            mode: persisted.mode.unwrap_or(InterceptMode::Full),
            upstream_base: persisted.upstream_base.unwrap_or_default(),
            upstream_auth_env: persisted.upstream_auth_env.unwrap_or_default(),
            block_count,
            store,
        }
    }

    /// 当前配置写盘（mode/upstream 覆盖变更时调用）。
    fn persist(&self) {
        save_persisted_config(&PersistedConfig {
            mode: Some(self.mode),
            upstream_base: Some(self.upstream_base.clone()),
            upstream_auth_env: Some(self.upstream_auth_env.clone()),
        });
    }

    // ── intercept mode ────────────────────────────────────────────────────

    pub fn mode(&self) -> InterceptMode {
        self.mode
    }

    pub fn set_mode(&mut self, mode: InterceptMode, ctx: &mut ModelContext<Self>) {
        if self.mode == mode {
            return;
        }
        self.mode = mode;
        self.persist();
        ctx.emit(InterceptSessionsModelEvent::ModeChanged);
    }

    // ── upstream overrides ────────────────────────────────────────────────

    /// Explicit API base override; empty means auto-detect.
    pub fn upstream_base(&self) -> &str {
        &self.upstream_base
    }

    /// Set the explicit API base override (empty = auto-detect).
    pub fn set_upstream_base(&mut self, base: String, ctx: &mut ModelContext<Self>) {
        let base = base.trim().to_string();
        if self.upstream_base == base {
            return;
        }
        self.upstream_base = base;
        self.persist();
        ctx.emit(InterceptSessionsModelEvent::UpstreamChanged);
    }

    /// Explicit auth env-var override; empty keeps the resolved default.
    pub fn upstream_auth_env(&self) -> &str {
        &self.upstream_auth_env
    }

    /// Set the explicit auth env-var override (empty = resolved default).
    pub fn set_upstream_auth_env(&mut self, env: String, ctx: &mut ModelContext<Self>) {
        let env = env.trim().to_string();
        if self.upstream_auth_env == env {
            return;
        }
        self.upstream_auth_env = env;
        self.persist();
        ctx.emit(InterceptSessionsModelEvent::UpstreamChanged);
    }

    /// Resolve the effective upstream config for `harness`, applying the
    /// explicit overrides on top of `UpstreamConfig::resolve`'s three-tier
    /// precedence (explicit base > `ZAP_UPSTREAM_BASE` env > harness default).
    /// Returns `None` only if resolution fails for the given harness.
    pub fn resolve_upstream(&self, harness: HarnessType) -> Option<UpstreamConfig> {
        let explicit = if self.upstream_base.is_empty() {
            None
        } else {
            Some(self.upstream_base.as_str())
        };
        let mut config = UpstreamConfig::resolve(harness, explicit).ok()?;
        if !self.upstream_auth_env.is_empty() {
            config.api_key_env = self.upstream_auth_env.clone();
        }
        Some(config)
    }

    // ── block counter ─────────────────────────────────────────────────────

    /// Captured block count (cached from the persistent store).
    pub fn block_count(&self) -> u64 {
        self.block_count
    }

    /// Re-query the persistent BlockStore and update the cached count.
    /// Emits [`InterceptSessionsModelEvent::BlocksChanged`] when it changed.
    pub fn refresh_block_count(&mut self, ctx: &mut ModelContext<Self>) {
        // 惰性打开:构造时 flag 关闭的实例,首次 refresh 时按需补开。
        if self.store.is_none() && crate::features::FeatureFlag::AgentHarness.is_enabled() {
            self.store = open_persistent_store();
        }
        let new_count = self
            .store
            .as_ref()
            .and_then(|s| s.lock().block_count().ok())
            .unwrap_or(self.block_count);
        if new_count != self.block_count {
            self.block_count = new_count;
            ctx.emit(InterceptSessionsModelEvent::BlocksChanged);
        }
    }
}

impl Entity for InterceptSessionsModel {
    type Event = InterceptSessionsModelEvent;
}

impl SingletonEntity for InterceptSessionsModel {}

/// Open the persistent block store used by the intercept UI.
///
/// Path: `<state_dir>/harness_blocks.db` — same base directory convention as
/// the app sqlite database (see `persistence::database_file_path`).
fn open_persistent_store() -> Option<Arc<Mutex<BlockStore>>> {
    let dir = warp_core::paths::state_dir();
    if dir.as_os_str().is_empty() {
        return None;
    }
    if let Err(e) = std::fs::create_dir_all(&dir) {
        log::warn!("intercept: cannot create state dir {}: {e}", dir.display());
        return None;
    }
    let path = dir.join("harness_blocks.db");
    match BlockStore::open(path.to_string_lossy().to_string()) {
        Ok(store) => Some(Arc::new(Mutex::new(store))),
        Err(e) => {
            log::warn!("intercept: cannot open block store {}: {e}", path.display());
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model_with_overrides(base: &str, auth_env: &str) -> InterceptSessionsModel {
        InterceptSessionsModel {
            mode: InterceptMode::Full,
            upstream_base: base.to_string(),
            upstream_auth_env: auth_env.to_string(),
            block_count: 0,
            store: None,
        }
    }

    /// PersistedConfig serde 往返：字段缺失（旧版本文件）→ default 安全回退。
    #[test]
    fn persisted_config_roundtrip_and_defaults() {
        let cfg = PersistedConfig {
            mode: Some(InterceptMode::HooksOnly),
            upstream_base: Some("http://localhost:9999".to_string()),
            upstream_auth_env: Some("MY_KEY".to_string()),
        };
        let raw = serde_json::to_string(&cfg).unwrap();
        let back: PersistedConfig = serde_json::from_str(&raw).unwrap();
        assert_eq!(back.mode, Some(InterceptMode::HooksOnly));
        assert_eq!(back.upstream_base.as_deref(), Some("http://localhost:9999"));
        assert_eq!(back.upstream_auth_env.as_deref(), Some("MY_KEY"));

        // 空对象（旧文件/损坏后 default）→ 全 None → 构造时安全回退默认
        let empty: PersistedConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(empty.mode, None);
        assert_eq!(empty.upstream_base, None);

        // 非法 JSON → load 侧 unwrap_or_default 吞掉
        let bad: Option<PersistedConfig> = serde_json::from_str("not json").ok();
        assert!(bad.is_none());
    }

    #[test]
    fn resolve_upstream_defaults_without_overrides() {
        std::env::remove_var("ZAP_UPSTREAM_BASE");
        let model = model_with_overrides("", "");
        let config = model.resolve_upstream(HarnessType::ClaudeCode).unwrap();
        assert_eq!(config.api_base, "https://api.anthropic.com");
        assert_eq!(config.auth_header, "x-api-key");
        assert_eq!(config.api_key_env, "ANTHROPIC_API_KEY");
    }

    #[test]
    fn resolve_upstream_applies_explicit_overrides() {
        std::env::remove_var("ZAP_UPSTREAM_BASE");
        let model = model_with_overrides("http://localhost:9999", "MY_TEST_KEY");
        let config = model.resolve_upstream(HarnessType::ClaudeCode).unwrap();
        assert_eq!(config.api_base, "http://localhost:9999");
        // Explicit auth-env override wins over the resolved default.
        assert_eq!(config.api_key_env, "MY_TEST_KEY");
        // The auth header scheme still follows the harness default.
        assert_eq!(config.auth_header, "x-api-key");
    }
}
