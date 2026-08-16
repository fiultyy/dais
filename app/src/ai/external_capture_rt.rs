//! External capture runtime (T3) — zap 进程内外部捕获接线, pane(view)级。
//!
//! 锁定口径 (用户拍板): zap=高级 TTY, 只管自家 pane 拉起的 agent; zap 外
//! 终端不接。1 pane=1 通道, 登记一律经 [`harness_integration`] 的
//! `register_external_session`, 两条注入路径互补:
//! 1. **bootstrap 武装**(主路径, 覆盖手敲): 本地 pane 首个 shell 的
//!    bootstrap 脚本尾部追加同名 shell 函数([`bootstrap_arming_suffix`],
//!    `omp/ompi/claude/claude-code/codex`), 函数体内 `env K=V ... name`
//!    前缀注入 — 手敲/输入编辑器/粘贴任何输入方式命中函数即注入,
//!    与键盘路由无关;
//! 2. **export 前缀**(补充路径, 输入编辑器/agent 流): ExecuteCommand 事件
//!    嗅探([`env_prefix_for_command`]), 已武装 pane 走 by_view 幂等守卫
//!    直接跳过。
//! 武装登记受 [`tick`] 保护式回收豁免("pane 存活=通道存活": 函数体烧死了
//! 端口, 活 pane 下回收会让函数指向死端口); view drop 即
//! `stop_registration`(reason=stopped)。存量 pane(接线前创建)不回溯武装 —
//! 设计内。
//!
//! 归属: harness 枚举判定 + env 组装在 harness_integration(集成层),
//! zap 侧本模块只做 pane 映射/时序/开关; 开关状态归
//! [`crate::terminal::intercept_sessions::InterceptSessionsModel`] 持久化。

use std::collections::HashMap;
use std::io::Write as _;
use std::sync::LazyLock;

use harness_integration::{
    ExternalCaptureManager, HarnessType, Registration, RegistrationId, RegistrationSnapshot,
};
use parking_lot::Mutex;
use warpui::EntityId;

/// Reap tick 间隔。远小于 30min idle 阈值(orch1 审计点 c)。
pub const TICK_INTERVAL_MS: u64 = 60_000;

struct State {
    manager: ExternalCaptureManager,
    /// pane(view)级映射: terminal view id → registration。
    /// 懒登记(首 harness 命令), view drop 反查销毁。
    by_view: HashMap<EntityId, RegistrationId>,
    /// claude wrapper 的 `--settings` 临时文件保活(案三轮): 句柄随 view
    /// 存活, remove 即 drop 自动删除(同 GUI `GuiIntercept::_settings_file`)。
    settings_files: HashMap<EntityId, tempfile::NamedTempFile>,
}

static RT: LazyLock<tokio::runtime::Runtime> = LazyLock::new(|| {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .expect("external capture runtime")
});

static STATE: LazyLock<Mutex<State>> = LazyLock::new(|| {
    let dir = warp_core::paths::state_dir();
    let mut manager = ExternalCaptureManager::new();
    if !dir.as_os_str().is_empty() {
        // 对齐观测台读取路径(model.rs blocks_db_path/open_raw_db),
        // 否则 session 永不出现 (orch1 预研情报)。
        manager = manager.with_db_paths(
            dir.join("harness_blocks.db"),
            dir.join("harness_raw_cache.db"),
        );
    }
    Mutex::new(State {
        manager,
        by_view: HashMap::new(),
        settings_files: HashMap::new(),
    })
});

/// 首命令嗅探: omp(含 `ompi` 别名容错)→Omp / claude→ClaudeCode / codex→Codex。
///
/// 词表匹配命令首 token(兼容路径前缀 `/usr/bin/omp`)。未命中已知
/// harness → `None`(不登记): 普通 shell 命令 pane 不该建 proxy。
/// `Generic` 兜底(只保 hook 事件)不在此触发 — 无可靠信号区分
/// "未知 harness"与"普通命令", 泛登记会让每个 pane 敲 ls 都建
/// proxy+hook server; Generic 通道留给显式登记入口(env_lines_for
/// 已支持), 见回报"遗留注意点"。
pub fn sniff_harness(command: &str) -> Option<HarnessType> {
    let first = command.split_whitespace().next()?;
    let name = first.rsplit('/').next()?.to_lowercase();
    match name.as_str() {
        "omp" | "ompi" => Some(HarnessType::Omp),
        "claude" | "claude-code" => Some(HarnessType::ClaudeCode),
        "codex" => Some(HarnessType::Codex),
        _ => None,
    }
}

/// Shell-quote a value for `export K=V` injection (single-quote style,
/// escape embedded quotes).
fn shell_quote(value: &str) -> String {
    if value.is_empty() || value.chars().all(|c| c.is_ascii_alphanumeric() || "-_./:=@%+".contains(c)) {
        value.to_string()
    } else {
        format!("'{}'", value.replace('\'', r"'\''"))
    }
}

/// 每 view 登记核心(两注入路径共用): 首次为该 view 登记一个外部捕获
/// 通道(proxy + hook + 独立 session), 已登记 view 幂等返回 `None`。
///
/// CA 预热 + 登记都在 RT 上; 短暂 block_on 可接受(一次性质开销, 且
/// hook/proxy bind 均为 localhost)。锁只在进出两瞬持握, register 的
/// await 期间锁随 guard 持有(block_on 为同步驱动栈, 无跨线程 await
/// 逃逸; localhost bind ~几十 ms, tick/快照等待上限可控)。失败 → None
/// 降级为不捕获, 绝不阻塞调用路径。
fn register_for_view(view_id: EntityId, harness: HarnessType) -> Option<Registration> {
    // 已登记 → 不重复注入。
    if STATE.lock().by_view.contains_key(&view_id) {
        return None;
    }
    RT.block_on(async {
        let mut state = STATE.lock();
        state.manager.ensure_initialized().ok()?;
        state
            .manager
            .register_external_session(harness)
            .await
            .ok()
            .map(|reg| {
                state.by_view.insert(view_id, reg.id);
                reg
            })
    })
}

/// 首命令执行钩子(补充路径, 输入编辑器/agent 流的 ExecuteCommand 事件):
/// 需要注入时返回 `export ...;` 前缀(拼在原命令前写入 PTY), 否则
/// `None`(原样执行)。
///
/// 幂等: 每 view 只登记/注入一次(bootstrap 武装过的 pane 直接跳过);
/// 开关关(`enabled=false`)→ `None` 零注入。
pub fn env_prefix_for_command(view_id: EntityId, command: &str, enabled: bool) -> Option<String> {
    if !enabled {
        return None;
    }
    let harness = sniff_harness(command)?;
    let registration = register_for_view(view_id, harness)?;

    let prefix = registration
        .env
        .iter()
        .map(|(k, v)| format!("export {}={}", k, shell_quote(v)))
        .collect::<Vec<_>>()
        .join(" ");
    log::info!(
        "external-capture: view {view_id:?} harness {harness:?} session {} proxy {} hook {}",
        registration.session_id,
        registration.proxy_port,
        registration.hook_base_url
    );
    Some(format!("{}; ", prefix))
}

/// Bootstrap 武装方言(由调用方从 `ShellType` 映射)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArmingDialect {
    /// bash/zsh: `name(){ <body>; }`
    Posix,
    /// fish: `function name; <body>; end`
    Fish,
}

/// 单个 wrapper 函数体的注入形状。
#[derive(Debug, Clone)]
enum WrapperBody {
    /// `env K=V ... name` — 前缀 env 注入(omp/codex)。`env` 经 PATH 找真
    /// 二进制, 天然无函数递归; 赋值只作用于该 harness 进程。
    Env(String),
    /// `command name --settings '<path>'` — CC settings 深覆盖(claude)。
    /// CC 的 settings.json `env` 块优先级压过进程 env, 裸 env 注入会被
    /// 用户 `~/.claude/settings.json` 静默覆盖(T3 三轮实证) → 对齐 GUI
    /// 拦截路径, 用临时 settings 文件经 `--settings` 深覆盖。`command`
    /// 经 PATH 找真二进制(无递归)。
    ClaudeSettings(String),
}

/// 纯函数: 由(命令名, 函数体形状)列表构建单行同名包装函数定义串。
///
/// 单行无换行 → 追加到 bootstrap 脚本/rc 文件/括号粘贴任意投递方式都
/// 安全。
fn arming_defs(targets: &[(String, WrapperBody)], dialect: ArmingDialect) -> String {
    targets
        .iter()
        .map(|(name, body)| {
            let inner = match body {
                WrapperBody::Env(assignments) => format!("env {assignments} {name}"),
                WrapperBody::ClaudeSettings(path) => {
                    format!("command {name} --settings '{path}'")
                }
            };
            match dialect {
                ArmingDialect::Posix => format!(r#"{name}(){{ {inner} "$@"; }}"#),
                ArmingDialect::Fish => format!("function {name}; {inner} $argv; end"),
            }
        })
        .collect::<Vec<_>>()
        .join(";")
}

/// CC `--settings` 临时文件内容(三轮): 与 GUI 拦截
/// `InterceptSession::claude_settings_overrides` 同构 — env 深覆盖
/// (BASE_URL→本地 proxy、NODE_EXTRA_CA_CERTS→本地 CA)+ hooks 四端点
/// (curl 带 token 头)。hook 端点用 hook_server 实际路由的 snake_case
/// (`user_prompt_submit` 等; GUI 侧 `to_lowercase()` 生成无下划线端点
/// 疑 404, 已在回报标注, 不在本票红线内修)。
fn claude_settings_json(
    proxy_port: u16,
    ca_path: &str,
    hook_base_url: &str,
    hook_token: &str,
) -> serde_json::Value {
    let hooks: Vec<serde_json::Value> = ["user_prompt_submit", "pre_tool_use", "post_tool_use", "stop"]
        .into_iter()
        .map(|event| {
            serde_json::json!({
                "hooks": [{
                    "type": "command",
                    "command": format!(
                        "curl -s -o /dev/null -X POST -H 'content-type: application/json' \
                         -H 'x-zap-hook-token: {hook_token}' \
                         --data-binary @- '{hook_base_url}/hooks/{event}'"
                    ),
                    "timeout": 5,
                }]
            })
        })
        .collect();
    serde_json::json!({
        "env": {
            "ANTHROPIC_BASE_URL": format!("https://127.0.0.1:{proxy_port}"),
            "NODE_EXTRA_CA_CERTS": ca_path,
        },
        "hooks": {
            "UserPromptSubmit": [hooks[0]],
            "PreToolUse": [hooks[1]],
            "PostToolUse": [hooks[2]],
            "Stop": [hooks[3]],
        },
    })
}

/// Bootstrap 武装后缀(主路径, 覆盖手敲): 为 pane 首个本地 shell 登记通道,
/// 返回插入 bootstrap 脚本不可见区的同名 harness 包装函数定义(单行)。
/// shell source bootstrap 时函数即定义, 首提示前就位 — 之后无论
/// 手敲/编辑器/粘贴, 命中 `omp/ompi/claude/claude-code/codex` 即完成注入:
/// - omp/ompi: env 前缀(等 oh-my-pi 侧 override env, 见二轮回报)
/// - claude/claude-code: `--settings` 临时文件深覆盖(本轮)
/// - codex: env 前缀(本机 config.toml 零 base_url 配置, `OPENAI_BASE_URL`
///   是有效入口 — 已核)
///
/// 登记标签取 Omp(anthropic 形状与 ClaudeCode 相同, env 组装按各函数
/// 目标 harness 独立进行, 标签仅影响观测台行展示)。幂等/开关关/登记
/// 失败 → `None`(bootstrap 原样, 绝不阻塞 pane 启动)。
pub fn bootstrap_arming_suffix(
    view_id: EntityId,
    dialect: ArmingDialect,
    enabled: bool,
) -> Option<String> {
    if !enabled {
        return None;
    }
    let registration = register_for_view(view_id, HarnessType::Omp)?;

    // CC settings 临时文件: 内容同 GUI 拦截, 句柄存 State 保活
    // (remove 即 drop 自动删除)。写失败 → claude 组降级为 env 前缀
    // (弱注入, 但绝不阻塞武装)。
    let settings_path = (|| {
        let mut file = tempfile::NamedTempFile::new().ok()?;
        let ca = STATE
            .lock()
            .manager
            .ca_cert_path_buf()
            .unwrap_or_default();
        let json = claude_settings_json(
            registration.proxy_port,
            &ca.display().to_string(),
            &registration.hook_base_url,
            &registration.hook_token,
        );
        file.write_all(json.to_string().as_bytes()).ok()?;
        let path = file.path().display().to_string();
        STATE.lock().settings_files.insert(view_id, file);
        Some(path)
    })();

    let defs = {
        let state = STATE.lock();
        // 登记刚初始化过 manager, CA 必在; 取不到只会让 CA 变体丢失,
        // 不致命(env_injection_for 仅 ClaudeCode/Omp 用到 ca)。
        let ca = state
            .manager
            .ca_cert_path_buf()
            .unwrap_or_else(std::path::PathBuf::new);
        let env_for = |harness| {
            harness_integration::env_lines_for(
                registration.proxy_port,
                &ca,
                &registration.hook_base_url,
                &registration.hook_token,
                harness,
            )
        };
        let assignments = |harness| {
            env_for(harness)
                .iter()
                .map(|(k, v)| format!("{}={}", k, shell_quote(v)))
                .collect::<Vec<_>>()
                .join(" ")
        };
        let targets: Vec<(String, WrapperBody)> = [
            ("omp", HarnessType::Omp),
            ("ompi", HarnessType::Omp),
            ("claude", HarnessType::ClaudeCode),
            ("claude-code", HarnessType::ClaudeCode),
            ("codex", HarnessType::Codex),
        ]
        .into_iter()
        .map(|(name, harness)| {
            let body = if harness == HarnessType::ClaudeCode {
                match &settings_path {
                    Some(p) => WrapperBody::ClaudeSettings(p.clone()),
                    None => WrapperBody::Env(assignments(harness)),
                }
            } else {
                WrapperBody::Env(assignments(harness))
            };
            (name.to_string(), body)
        })
        .collect();
        arming_defs(&targets, dialect)
    };
    log::info!(
        "external-capture: view {view_id:?} armed harness functions (claude --settings: {}), session {} proxy {} hook {}",
        settings_path.is_some(),
        registration.session_id,
        registration.proxy_port,
        registration.hook_base_url
    );
    Some(defs)
}

/// 将武装函数定义插入 bootstrap 脚本的不可见执行区(案B)。
///
/// zsh/bash 的 bootstrap 是 heredoc 结构(`read ... << 'EOM'` … `EOM`):
/// `EOM` 标记**之后**的字节是独立输入行, 会被 ZLE 当作用户命令回显执行
/// (实测: 尾部追加=pane 里可见的一大串)。插入点必须在 heredoc 结束
/// 标记之前(函数定义随 `WARP_BOOTSTRAP_VAR` 一起 eval, 零回显); 脚本无
/// heredoc 标记(fish, 走临时文件 source, 本就不回显)则尾部追加。
pub fn insert_arming_into_script(script: &[u8], defs: &str) -> Vec<u8> {
    const HEREDOC_END: &[u8] = b"\nEOM\n";
    let mut out = Vec::with_capacity(script.len() + defs.len() + 1);
    match script
        .windows(HEREDOC_END.len())
        .rposition(|w| w == HEREDOC_END)
    {
        Some(pos) => {
            // pos 指向起始 `\n`; 插在其后 = `EOM` 行之前的独立一行。
            out.extend_from_slice(&script[..=pos]);
            out.extend_from_slice(defs.as_bytes());
            out.push(b'\n');
            out.extend_from_slice(&script[pos + 1..]);
        }
        None => {
            out.extend_from_slice(script);
            if !script.is_empty() && !script.ends_with(b"\n") {
                out.push(b'\n');
            }
            out.extend_from_slice(defs.as_bytes());
        }
    }
    out
}

/// View 销毁钩子: pane 关闭 → 停对应登记(Exit block + proxy/hook 双
/// drop)并释放 claude `--settings` 临时文件(remove 即 drop 自动删除)。
/// 未知 view 是 no-op。undo 隐藏的 pane 不 drop view, 通道
/// 保留 — 与"pane 存活=通道存活"一致。
pub fn stop_by_view(view_id: EntityId) {
    let mut state = STATE.lock();
    // 临时文件句柄 remove → drop 自动删除(先于登记停止, 顺序无耦合)。
    state.settings_files.remove(&view_id);
    if let Some(reg_id) = state.by_view.remove(&view_id) {
        state.manager.stop_registration(reg_id);
        log::info!("external-capture: view {view_id:?} registration stopped");
    }
}

/// 周期维护: 物化懒 Spawn(hook 激活检测)+ 闲置回收(武装登记豁免 —
/// "pane 存活=通道存活", 函数体烧死了端口, 活 pane 下回收会让武装函数
/// 指向死端口; pane 关闭由 `stop_by_view` 收尾)。由
/// [`crate::terminal::intercept_sessions::InterceptSessionsModel`] 的
/// timer 每 [`TICK_INTERVAL_MS`] 调用。
pub fn tick() {
    let mut state = STATE.lock();
    let protected: Vec<RegistrationId> = state.by_view.values().copied().collect();
    let reaped = state.manager.tick_except(&protected);
    if !reaped.is_empty() {
        log::info!("external-capture: reaped {} idle registration(s)", reaped.len());
    }
}

/// 活动登记快照(观测台 UI 数据源)。
pub fn snapshot() -> Vec<RegistrationSnapshot> {
    STATE.lock().manager.registrations()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sniff_matches_known_harnesses() {
        assert_eq!(sniff_harness("omp --slow"), Some(HarnessType::Omp));
        assert_eq!(sniff_harness("  omp"), Some(HarnessType::Omp));
        assert_eq!(sniff_harness("/usr/bin/omp chat"), Some(HarnessType::Omp));
        assert_eq!(sniff_harness("claude --harness x"), Some(HarnessType::ClaudeCode));
        assert_eq!(sniff_harness("claude-code run"), Some(HarnessType::ClaudeCode));
        assert_eq!(sniff_harness("codex exec"), Some(HarnessType::Codex));
        // 普通命令不登记。
        assert_eq!(sniff_harness("ls -la"), None);
        assert_eq!(sniff_harness("cargo build"), None);
        assert_eq!(sniff_harness(""), None);
        // 词表是首 token 匹配, 不误伤子串。
        assert_eq!(sniff_harness("echo omp"), None);
    }

    #[test]
    fn shell_quote_plain_and_special() {
        assert_eq!(shell_quote("https://127.0.0.1:8443"), "https://127.0.0.1:8443");
        assert_eq!(shell_quote("a'b"), "'a'\\''b'");
        assert_eq!(shell_quote(""), "");
    }

    #[test]
    fn env_prefix_disabled_or_plain_command_is_none() {
        let id = EntityId::from_usize(0);
        assert_eq!(env_prefix_for_command(id, "omp", false), None, "开关关→零注入");
        assert_eq!(env_prefix_for_command(id, "ls", true), None, "普通命令不登记");
    }

    #[test]
    fn tick_and_snapshot_are_safe_on_fresh_state() {
        // 未登记任何东西时 tick/snapshot 不 panic(真实 DB 路径走 state_dir,
        // 测试环境下 in-memory/不可写目录都会被吞)。
        tick();
        let _ = snapshot();
    }

    #[test]
    fn arming_defs_posix_and_fish_shapes() {
        let targets = vec![
            (
                "omp".to_string(),
                WrapperBody::Env(
                    "ANTHROPIC_BASE_URL=https://127.0.0.1:8443".to_string(),
                ),
            ),
            (
                "claude".to_string(),
                WrapperBody::ClaudeSettings("/tmp/.tmpXYZ0".to_string()),
            ),
            (
                "codex".to_string(),
                WrapperBody::Env(
                    "OPENAI_BASE_URL=https://127.0.0.1:8443 NO_PROXY='127.0.0.1,localhost'"
                        .to_string(),
                ),
            ),
        ];

        let posix = arming_defs(&targets, ArmingDialect::Posix);
        assert_eq!(
            posix,
            concat!(
                r#"omp(){ env ANTHROPIC_BASE_URL=https://127.0.0.1:8443 omp "$@"; };"#,
                r#"claude(){ command claude --settings '/tmp/.tmpXYZ0' "$@"; };"#,
                r#"codex(){ env OPENAI_BASE_URL=https://127.0.0.1:8443 NO_PROXY='127.0.0.1,localhost' codex "$@"; }"#,
            )
        );
        assert!(!posix.contains('\n'), "单行: 追加投递(rc 文件/括号粘贴)安全");

        let fish = arming_defs(&targets, ArmingDialect::Fish);
        assert_eq!(
            fish,
            concat!(
                "function omp; env ANTHROPIC_BASE_URL=https://127.0.0.1:8443 omp $argv; end;",
                "function claude; command claude --settings '/tmp/.tmpXYZ0' $argv; end;",
                "function codex; env OPENAI_BASE_URL=https://127.0.0.1:8443 NO_PROXY='127.0.0.1,localhost' codex $argv; end",
            )
        );
    }

    #[test]
    fn claude_settings_json_shape() {
        let json = claude_settings_json(
            8443,
            "/tmp/ca.pem",
            "http://127.0.0.1:9911",
            "tok-abc",
        );
        // env 深覆盖块(GUI 拦截同构)。
        assert_eq!(
            json["env"]["ANTHROPIC_BASE_URL"],
            "https://127.0.0.1:8443"
        );
        assert_eq!(json["env"]["NODE_EXTRA_CA_CERTS"], "/tmp/ca.pem");
        // hooks 四端点: CC 事件键 + snake_case 路由(hook_server 实际路由)。
        for (key, route) in [
            ("UserPromptSubmit", "user_prompt_submit"),
            ("PreToolUse", "pre_tool_use"),
            ("PostToolUse", "post_tool_use"),
            ("Stop", "stop"),
        ] {
            let cmd = json["hooks"][key][0]["hooks"][0]["command"]
                .as_str()
                .unwrap();
            assert!(
                cmd.contains("x-zap-hook-token: tok-abc"),
                "{key} hook carries auth token"
            );
            assert!(
                cmd.contains(&format!("http://127.0.0.1:9911/hooks/{route}")),
                "{key} hook targets snake_case route"
            );
        }
    }

    #[test]
    fn arming_suffix_disabled_is_none() {
        assert_eq!(
            bootstrap_arming_suffix(EntityId::from_usize(0), ArmingDialect::Posix, false),
            None,
            "开关关→零武装"
        );
    }

    #[test]
    fn arming_inserts_before_heredoc_end_marker() {
        // zsh.sh 形状(heredoc 尾标记): defs 必须进 heredoc 内部,
        // 否则成为 EOM 之后的独立输入行 → ZLE 回显(案B 可见性根因)。
        let script = b" setopt ... PS2=\"\"\n read -r -d '' WARP_BOOTSTRAP_VAR << 'EOM'; eval \"$WARP_BOOTSTRAP_VAR\"\nbody line\nEOM\n";
        let out = insert_arming_into_script(script, "omp(){ env K=V omp \"$@\"; }");
        let s = String::from_utf8(out).unwrap();
        let eom = s.rfind("\nEOM\n").expect("EOM marker preserved");
        let defs_pos = s.find("omp(){ env K=V").expect("defs present");
        assert!(defs_pos < eom, "defs must land BEFORE the EOM marker");
        assert_eq!(
            &s[defs_pos - 1..defs_pos],
            "\n",
            "defs on its own line inside the heredoc"
        );
        assert_eq!(
            &s[eom + "\nEOM\n".len() - 1..],
            "\n",
            "nothing after EOM: silent by construction"
        );

        // bash.sh 形状(EOM 后还有 stty/eval 行): 插入点仍是 EOM 之前。
        let script = b"read -r -d '' V << 'EOM'\nbody\nEOM\nstty sane\neval \"$V\"\n";
        let out = insert_arming_into_script(script, "DEF");
        let s = String::from_utf8(out).unwrap();
        assert!(s.contains("\nDEF\nEOM\n"), "defs inserted before EOM, bash shape");
        assert!(s.ends_with("eval \"$V\"\n"), "post-EOM lines untouched");

        // fish 形状(无 heredoc 标记, 走临时文件 source): 尾部追加。
        let script = b"warp_bootstrapped\nset -g WARP_BOOTSTRAPPED 1\nend\n";
        let out = insert_arming_into_script(script, "function omp; env K=V omp $argv; end");
        assert_eq!(
            String::from_utf8_lossy(&out),
            "warp_bootstrapped\nset -g WARP_BOOTSTRAPPED 1\nend\nfunction omp; env K=V omp $argv; end",
        );
    }
}
