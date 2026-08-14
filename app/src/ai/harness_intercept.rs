//! Harness 拦截接线 — 把 `harness_integration::Integration` 挂进
//! AgentDriver 的 third-party harness spawn 链路。
//!
//! 职责（对应 intercept 四层架构的第 3.5 层：生命周期接线）：
//! - 进程级 tokio runtime（`zap-intercept`）：承载 HookServer / TLS proxy /
//!   raw-processor 三个长驻 tokio 任务；GUI 主线程只做一次短暂的
//!   `block_on`（bind 监听器），之后零占用。
//! - [`InterceptSession`]：一次 harness 运行的拦截会话。构造时启动
//!   hooks（+ Full 模式下 proxy），产出注入子进程的环境变量；`finish`
//!   记录 Exit block 后 drop，自动关停 proxy / hook server
//!
//! 数据流：harness CLI 的 LLM 流量经 proxy TLS 旁路捕获 → raw_processor →
//! 持久化 `harness_blocks.db`（intercept badge 计数的数据源）。
use std::path::PathBuf;
use std::sync::LazyLock;

use harness_integration::{Integration, InterceptMode};
use proxy_interceptor::{HarnessType, ProxyManager};

/// 进程级拦截 runtime。两个 worker 线程足够：proxy / hooks 都是低频
/// localhost IO；真正吞吐（SSE chunk 转发）由 reqwest 的连接驱动。
static INTERCEPT_RT: LazyLock<tokio::runtime::Runtime> = LazyLock::new(|| {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .thread_name("zap-intercept")
        .enable_all()
        .build()
        .expect("failed to build zap-intercept runtime")
});
/// Map a `warp_cli` harness selection onto the interceptor's harness type.
pub fn intercept_harness_type(harness: warp_cli::agent::Harness) -> HarnessType {
    match harness {
        warp_cli::agent::Harness::Claude => HarnessType::ClaudeCode,
        // Codex/OpenCode speak the OpenAI-flavoured env override; Gemini and
        // anything else fall through to the generic HTTPS_PROXY injection.
        warp_cli::agent::Harness::OpenCode => HarnessType::Codex,
        warp_cli::agent::Harness::Oz
        | warp_cli::agent::Harness::Gemini
        | warp_cli::agent::Harness::Unknown => HarnessType::Generic,
    }
}

/// One intercepted harness run: owns the [`Integration`] from before the
/// harness CLI starts until its exit code is known.
pub struct InterceptSession {
    integ: Integration,
    session_id: String,
    harness: HarnessType,
    mode: InterceptMode,
    proxy_port: Option<u16>,
    /// Exit 已记录（finish 或 Drop 二选一，避免重复 Exit block）。
    exit_recorded: bool,
}

impl InterceptSession {
    /// Start the intercept session for a harness run.
    ///
    /// `mode == Bypass` → `None`（不拦截）。任何启动失败都降级为 `None`
    /// 并记日志：拦截是旁路能力，不能阻塞 harness 本身的启动。
    #[allow(clippy::new_ret_no_self)]
    pub fn new(
        cli_harness: warp_cli::agent::Harness,
        mode: InterceptMode,
        upstream: Option<proxy_interceptor::UpstreamConfig>,
    ) -> Option<Self> {
        Self::new_for_type(intercept_harness_type(cli_harness), mode, upstream)
    }

    /// 按 interceptor harness 类型直接建立会话（GUI CLI agent 映射用：
    /// `codex` CLI 无 warp_cli Harness 对应，直接给 HarnessType::Codex）。
    #[allow(clippy::new_ret_no_self)]
    pub fn new_for_type(
        harness: HarnessType,
        mode: InterceptMode,
        upstream: Option<proxy_interceptor::UpstreamConfig>,
    ) -> Option<Self> {
        if mode == InterceptMode::Bypass {
            return None;
        }
        let session_id = format!("harness-{}", uuid::Uuid::new_v4());

        let Some((blocks_db, raw_db)) = intercept_db_paths() else {
            log::warn!("intercept: state dir unavailable; harness run not intercepted");
            return None;
        };
        let mut integ = match Integration::open_persistent(
            &session_id,
            harness_type_str(harness),
            &blocks_db,
            &raw_db,
        ) {
            Ok(i) => i,
            Err(e) => {
                log::warn!("intercept: open persistent store failed: {e}");
                return None;
            }
        };

        // hooks 是拦截的最低要求（proxy 只在 Full 且成功时叠加）。
        // hooks 起不来 → 整个会话没有捕获通道 → 按契约降级为 None。
        let (proxy_port, hooks_ok) = INTERCEPT_RT.block_on(async {
            if let Err(e) = integ.start_hooks().await {
                log::warn!("intercept: hook server start failed; run not intercepted: {e}");
                return (None, false);
            }
            if mode == InterceptMode::Full {
                match upstream {
                    Some(upstream) => match start_proxy(&mut integ, upstream).await {
                        Ok(port) => (Some(port), true),
                        Err(e) => {
                            // proxy 失败不致命：hooks 仍可用，LLM 流量直连。
                            log::warn!("intercept: proxy start failed (hooks-only fallback): {e}");
                            (None, true)
                        }
                    },
                    None => {
                        log::warn!("intercept: upstream unresolved; hooks-only fallback");
                        (None, true)
                    }
                }
            } else {
                (None, true)
            }
        });
        if !hooks_ok {
            return None; // Integration drop 自动关停半启动的 hook server
        }

        integ.record_spawn(mode);
        log::info!(
            "intercept: session {session_id} started (mode={}, proxy={:?}, hooks={})",
            mode.as_str(),
            proxy_port,
            integ.hook_url().is_some()
        );

        Some(Self {
            integ,
            session_id,
            harness,
            mode,
            proxy_port,
            exit_recorded: false,
        })
    }

    /// Claude Code settings.json 覆盖片段（`--settings` 临时文件内容）。
    ///
    /// CC 的 settings.json `env` 优先级高于进程环境变量，PTY env 注入的
    /// `ANTHROPIC_BASE_URL` 会被用户配置静默覆盖 → LLM 流量绕过 proxy。
    /// 所以对 CC 拦截必须走 `--settings`：env 深覆盖（BASE_URL→本地 proxy、
    /// NODE_EXTRA_CA_CERTS→本地 CA）+ hooks 四端点（带 token 头）。
    /// 仅 ClaudeCode harness 有效；其他 harness 返回 None（回退 PTY env）。
    pub fn claude_settings_overrides(&self) -> Option<serde_json::Value> {
        if self.harness != HarnessType::ClaudeCode {
            return None;
        }
        let mut env = serde_json::Map::new();
        let mut hooks: Vec<serde_json::Value> = Vec::new();

        if let (Some(handle), Some(base)) = (
            self.integ.proxy_handle(),
            self.integ.hook_url(),
        ) {
            let proxy_base = format!("https://127.0.0.1:{}", handle.port);
            let ca = handle.ca_cert_path.display().to_string();
            env.insert("ANTHROPIC_BASE_URL".into(), proxy_base.into());
            env.insert("NODE_EXTRA_CA_CERTS".into(), ca.into());
            let _ = base; // hooks 端点在下面统一加
        }

        if let (Some(url), Some(token)) =
            (self.integ.hook_url(), self.integ.hook_token())
        {
            for event in ["UserPromptSubmit", "PreToolUse", "PostToolUse", "Stop"] {
                hooks.push(serde_json::json!({
                    "hooks": [{
                        "type": "command",
                        "command": format!(
                            "curl -s -o /dev/null -X POST -H 'content-type: application/json' \
                             -H 'x-zap-hook-token: {token}' \
                             --data-binary @- '{url}/hooks/{}'",
                            event.to_lowercase()
                        ),
                        "timeout": 5,
                    }]
                }));
            }
        }

        let mut root = serde_json::Map::new();
        if !env.is_empty() {
            root.insert("env".into(), serde_json::Value::Object(env));
        }
        if !hooks.is_empty() {
            root.insert("hooks".into(), serde_json::json!({
                "UserPromptSubmit": [hooks[0]],
                "PreToolUse": [hooks[1]],
                "PostToolUse": [hooks[2]],
                "Stop": [hooks[3]],
            }));
        }
        if root.is_empty() {
            None
        } else {
            Some(serde_json::Value::Object(root))
        }
    }
    /// Record the terminal Exit block and release the intercept resources
    /// (proxy listener + hook server + sqlite handles) immediately, instead of
    /// `idle_on_complete` follow-ups long after the harness exited.
    ///
    /// Idempotent; `Drop` covers paths that never observe an exit code.
    pub fn finish(&mut self, exit_code: i32) {
        if !self.exit_recorded {
            self.exit_recorded = true;
            self.integ.record_exit(exit_code);
            self.integ = Integration::in_memory("", "").expect("in-memory stores cannot fail");
            log::info!(
                "intercept: session {} finished (exit={exit_code})",
                self.session_id
            );
        }
    }

    // ── 观测台读取接口 ──

    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    pub fn proxy_port(&self) -> Option<u16> {
        self.proxy_port
    }

    /// 注入 harness 进程的 env（proxy env + hook url/token），
    /// 供 GUI 命令前缀改写（`env K=V ... <cli>`）。
    pub fn spawn_env(&self) -> Vec<(String, String)> {
        harness_integration::build_spawn_env(
            self.mode,
            self.integ.proxy_handle(),
            self.integ.hook_url().as_deref(),
            self.integ.hook_token().as_deref(),
            self.harness,
        )
    }

    pub fn hook_url(&self) -> Option<String> {
        self.integ.hook_url()
    }
}

impl Drop for InterceptSession {
    fn drop(&mut self) {
        self.finish(1);
    }
}

/// Bind the TLS proxy onto the integration's raw-processor pipeline.
async fn start_proxy(
    integ: &mut Integration,
    upstream: proxy_interceptor::UpstreamConfig,
) -> anyhow::Result<u16> {
    // ProxyManager 持有 reqwest client + 本地 CA；每次 run 现建（CA 复用
    // 磁盘缓存，成本可忽略）。
    let manager = ProxyManager::new().map_err(anyhow::Error::msg)?;
    integ.start_proxy(&manager, upstream).await
}

fn harness_type_str(harness: HarnessType) -> &'static str {
    match harness {
        HarnessType::ClaudeCode => "claude-code",
        HarnessType::Codex => "codex",
        HarnessType::Omp => "omp",
        HarnessType::Generic => "generic",
    }
}

/// Persistent store paths: `<state_dir>/harness_blocks.db` (blocks, shared
/// with the intercept badge counter) and `<state_dir>/harness_raw_cache.db`.
fn intercept_db_paths() -> Option<(PathBuf, PathBuf)> {
    let dir = warp_core::paths::state_dir();
    if dir.as_os_str().is_empty() {
        return None;
    }
    std::fs::create_dir_all(&dir).ok()?;
    Some((
        dir.join("harness_blocks.db"),
        dir.join("harness_raw_cache.db"),
    ))
}

/// AgentDriver 接线入口：按全局拦截配置为 third-party harness run 建立
/// 拦截会话。Oz harness / flag 关 / Bypass / 启动失败 → `None`。
pub fn maybe_start_intercept(
    selected_harness: warp_cli::agent::Harness,
    ctx: &mut warpui::ModelContext<crate::ai::agent_sdk::AgentDriver>,
) -> Option<InterceptSession> {
    use crate::features::FeatureFlag;
    use crate::terminal::intercept_sessions::InterceptSessionsModel;
    use warpui::SingletonEntity;

    // Oz 走 Zap 内建 agent 基础设施，无 LLM 流量可拦截。
    if selected_harness == warp_cli::agent::Harness::Oz {
        return None;
    }
    if !FeatureFlag::AgentHarness.is_enabled() {
        return None;
    }
    let model = InterceptSessionsModel::as_ref(ctx);
    let mode = model.mode();
    if mode == InterceptMode::Bypass {
        return None;
    }
    // CC 默认上游形状是 x-api-key/ANTHROPIC_API_KEY（官方 Anthropic）；
    // 用户走 AUTH_TOKEN/Bearer 中转（BigModel 等）时该形状会被上游 401。
    // 未显式覆盖 base 时，读用户 ~/.claude/settings.json 的 env 归一上游。
    let harness_type = intercept_harness_type(selected_harness);
    let mut upstream = model.resolve_upstream(harness_type);
    if harness_type == HarnessType::ClaudeCode && model.upstream_base().is_empty() {
        upstream = claude_upstream_from_user_settings().or(upstream);
    }
    InterceptSession::new(selected_harness, mode, upstream)
}

/// Read the user's `~/.claude/settings.json` env block and, when it configures
/// an `ANTHROPIC_AUTH_TOKEN`-style relay, rebuild the upstream in Bearer shape
/// pointing at the relay base (mirrors how CC itself authenticates).
fn claude_upstream_from_user_settings() -> Option<proxy_interceptor::UpstreamConfig> {
    let home = std::env::var("HOME").ok()?;
    let raw = std::fs::read_to_string(format!("{home}/.claude/settings.json")).ok()?;
    let v: serde_json::Value = serde_json::from_str(&raw).ok()?;
    let env = v.get("env")?;
    let token = env.get("ANTHROPIC_AUTH_TOKEN")?.as_str()?.to_string();
    let base = env.get("ANTHROPIC_BASE_URL")?.as_str()?.to_string();
    if token.is_empty() || base.is_empty() {
        return None;
    }
    // proxy handler 从本进程 env 读 token（api_key_env 指向）注入上游
    // Bearer header —— 与 CC 自身的 AUTH_TOKEN 认证方式一致。
    std::env::set_var("ZAP_CC_AUTH_TOKEN", token);
    Some(proxy_interceptor::UpstreamConfig {
        api_base: base.trim_end_matches('/').to_string(),
        auth_header: "authorization".into(),
        auth_prefix: "Bearer ".into(),
        api_key_env: "ZAP_CC_AUTH_TOKEN".into(),
        request_path: "/v1/messages".into(),
        response_format: proxy_interceptor::ResponseFormat::AnthropicSSE,
    })
}

// ─── GUI 交互式 CLI tab 拦截 ──────────────────────────────────────────────

/// GUI 交互 CC tab 的拦截注册表：terminal view id → (session, settings 临时文件)。
///
/// 生命周期粗粒度：CC 进程退出不主动 `finish` —— Drop（app 退出 / 同一
/// view 重新启动 agent）兜底释放 proxy 与 hook server；blocks/raw 已实时
/// 落库，仅缺 Exit block。
static GUI_INTERCEPT: LazyLock<parking_lot::Mutex<std::collections::HashMap<String, GuiIntercept>>> =
    LazyLock::new(|| parking_lot::Mutex::new(std::collections::HashMap::new()));

struct GuiIntercept {
    session: InterceptSession,
    /// 持有以保 CC settings 临时文件存活；drop 时自动清理。非 CC 无此文件。
    _settings_file: Option<tempfile::NamedTempFile>,
}

/// 为 GUI 交互式 CLI agent tab 构造带拦截的启动命令。
///
/// - **Claude**：`claude --settings '<path>'`（CC settings.json env 优先级
   ///   高于进程 env，PTY 注入无效，必须走 settings 深覆盖）。
/// - **Codex**：`env OPENAI_BASE_URL='...' codex`（OpenAI 形状 env 前缀）。
/// - **Gemini**：`env HTTPS_PROXY='...' SSL_CERT_FILE='...' gemini`（generic）。
/// - 其余（DeepSeek 等）/ HooksOnly（无 proxy env 可注）/ 启动失败 → `None`，
///   调用方回退原命令。
///
/// `terminal_view_id`（`EntityId::to_string()`）用于会话注册：同一 view
/// 重复启动 agent 时先释放旧会话，避免 proxy 端口累积泄漏。
pub fn intercept_cli_agent_command(
    agent: crate::terminal::cli_agent::CLIAgent,
    terminal_view_id: String,
    ctx: &warpui::AppContext,
) -> Option<String> {
    intercept_command_line(agent.command_prefix(), agent, terminal_view_id, ctx)
}

/// 带原命令行的拦截改写（保留用户参数）：
/// - Claude：`<cmd> --settings '<path>'`（追加，不丢 `--resume` 等参数）
/// - env 形状：`env K='V' … <cmd>`
/// `None` = 不拦截，调用方原样执行。
pub fn intercept_command_line(
    command: &str,
    agent: crate::terminal::cli_agent::CLIAgent,
    terminal_view_id: String,
    ctx: &warpui::AppContext,
) -> Option<String> {
    use crate::features::FeatureFlag;
    use crate::terminal::cli_agent::CLIAgent;
    use crate::terminal::intercept_sessions::InterceptSessionsModel;
    use std::io::Write as _;
    use warpui::SingletonEntity;

    if !FeatureFlag::AgentHarness.is_enabled() {
        return None;
    }
    let model = InterceptSessionsModel::as_ref(ctx);
    let mode = model.mode();
    if mode != InterceptMode::Full && agent != CLIAgent::Claude {
        // HooksOnly 对非 CC harness 无捕获通道（hooks 是 CC settings 机制）。
        return None;
    }

    match agent {
        CLIAgent::Claude => {
            // 与 AgentDriver 路线（maybe_start_intercept）同基准：
            // 显式覆盖优先，否则读用户 ~/.claude/settings.json 归一 Bearer 上游。
            let mut upstream = model.resolve_upstream(HarnessType::ClaudeCode);
            if model.upstream_base().is_empty() {
                upstream = claude_upstream_from_user_settings().or(upstream);
            }
            let session = InterceptSession::new(warp_cli::agent::Harness::Claude, mode, upstream)?;
            let settings = session.claude_settings_overrides()?;
            let mut file = tempfile::NamedTempFile::new().ok()?;
            file.write_all(settings.to_string().as_bytes()).ok()?;
            let path = file.path().display().to_string();
            GUI_INTERCEPT.lock().insert(
                terminal_view_id,
                GuiIntercept { session, _settings_file: Some(file) },
            );
            Some(format!("{command} --settings '{path}'"))
        }
        CLIAgent::Codex | CLIAgent::OpenCode => {
            intercept_env_command(command, HarnessType::Codex, &model, mode, terminal_view_id)
        }
        CLIAgent::Omp => {
            intercept_env_command(command, HarnessType::Omp, &model, mode, terminal_view_id)
        }
        // Gemini/Amp/Droid/Copilot/Pi/Auggie/CursorCli/Goose/DeepSeek/Antigravity/
        // Unknown：generic HTTPS_PROXY 形状，需显式上游（Proxy tab base 覆盖
        // 或 ZAP_UPSTREAM_BASE），否则 resolve 失败回退原命令。
        _ => intercept_env_command(command, HarnessType::Generic, &model, mode, terminal_view_id),
    }
}

/// 非 CC harness 的通用 env 前缀改写：起 session → `env K='V' … <cli>`。
/// proxy 未起（env 仅剩 hook 变量）→ `None`（拦截无意义）。
fn intercept_env_command(
    command: &str,
    harness_type: HarnessType,
    model: &crate::terminal::intercept_sessions::InterceptSessionsModel,
    mode: InterceptMode,
    terminal_view_id: String,
) -> Option<String> {
    let upstream = model.resolve_upstream(harness_type)?;
    let session = InterceptSession::new_for_type(harness_type, mode, Some(upstream))?;
    let env = session.spawn_env();
    if !env
        .iter()
        .any(|(k, _)| k != "ZAP_HOOK_SERVER_URL" && k != "ZAP_HOOK_TOKEN")
    {
        return None;
    }
    let prefix = env
        .iter()
        .map(|(k, v)| format!("{k}='{v}'"))
        .collect::<Vec<_>>()
        .join(" ");
    GUI_INTERCEPT.lock().insert(
        terminal_view_id,
        GuiIntercept { session, _settings_file: None },
    );
    Some(format!("env {prefix} {command}"))
}

/// 释放指定 terminal view 的 GUI 拦截会话（tab 关闭时调用）。
/// Drop 即 finish(1) + 关 proxy/hook server；无注册则 no-op。
///
/// 注意：undo-close 恢复的 tab 若 CC 进程仍存活将失去拦截
/// （settings 临时文件随 drop 删除，旧进程上游请求失败）——
/// 相比 proxy 端口随开关 tab 无限泄漏，这是正确的取舍。
pub fn release_gui_intercept(terminal_view_id: &str) {
    if let Some(g) = GUI_INTERCEPT.lock().remove(terminal_view_id) {
        log::info!(
            "intercept: released GUI intercept session {}",
            g.session.session_id()
        );
        drop(g);
    }
}

/// 当前活跃的 GUI 交互拦截会话快照（观测台 Proxy tab 数据源）。
/// 仅含交互式 CC tab 注册表；`agent run` CLI 会话由 AgentDriver
/// 持有且生命周期短暂，不在此列。
pub fn active_gui_intercepts() -> Vec<ActiveInterceptGui> {
    GUI_INTERCEPT
        .lock()
        .values()
        .map(|g| ActiveInterceptGui {
            session_id: g.session.session_id().to_string(),
            proxy_port: g.session.proxy_port(),
            hook_url: g.session.hook_url(),
        })
        .collect()
}

/// 活跃拦截会话的观测台行。
#[derive(Clone, Debug)]
pub struct ActiveInterceptGui {
    pub session_id: String,
    pub proxy_port: Option<u16>,
    pub hook_url: Option<String>,
}
