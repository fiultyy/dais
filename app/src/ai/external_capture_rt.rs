//! External capture runtime (T5) — 单端口入口 + 别名武装, dais 进程级。
//!
//! 锁定口径 (用户拍板): 别名是唯一入口。裸命令(`omp`/`claude`/`pi`)行为
//! 完全不变; 只有 `cc-dais`/`omp-dais`/`pi-dais` 进通道:
//! - **入口**: [`EntryGateway`](proxy_interceptor 单端口入口, 明文 HTTP,
//!   默认 8787 持久化 intercept_config.json), 路径前缀分流 `/cc` `/omp`
//!   `/pi` → 各自出口。auth 透明管道(客户端凭据原样转发, dais 不注不剥)。
//! - **观测**: 每前缀按实例归并 session(T8: `external-cc/omp/pi[-<实例
//!   标记>]`, 标记由别名铸造, 无标记回落默认 session), Spawn 懒发
//!   (首个真实请求才落 block)。
//! - **武装**: 本地 pane 首个 shell 的 bootstrap 脚本不可见区插入三个
//!   同名 shell 函数(`cc-dais`/`omp-dais`/`pi-dais`, heredoc 感知插入零可见
//!   污染)。`cc-dais` 走 `--settings` 深覆盖(静态文件
//!   `~/.config/zap/cc-entry-settings.json`, env 块从用户 `~/.claude/
//!   settings.json` 透传合并 + `ANTHROPIC_BASE_URL` 覆盖为入口 `/cc`);
//!   `omp-dais`/`pi-dais` 传 `--model dais/<动态默认模型>`(启动时从各 CLI
//!   独立配置读取默认模型 ID; `dais/` 前缀保证走入口 `/omp`、`/pi`,
//!   models.yml 由编排侧写)。
//!
//! 生命周期: 外部捕获开关开 → [`ensure_gateway`](Self) 起 8787; 关 →
//! [`shutdown`] 落 Exit + 端口释放。旧 T3 路径(pane 级登记/劫持裸命令/
//! 60s tick)已按票面移除。

use std::sync::LazyLock;

use harness_integration::{EntryGateway, EntrySessionInfo};
use parking_lot::Mutex;
use warpui::EntityId;

/// 入口默认端口(T5 票面)。
pub const ENTRY_PORT: u16 = 8787;

static RT: LazyLock<tokio::runtime::Runtime> = LazyLock::new(|| {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .expect("external capture runtime")
});

/// None = 开关关或启动失败(端口被占)。
static GATEWAY: LazyLock<Mutex<Option<EntryGateway>>> = LazyLock::new(|| Mutex::new(None));

/// 起入口网关(幂等)。开关开时由 InterceptSessionsModel 调用; 端口被占
/// → Err(降级: 开关开着但入口不可用, 快照空)。
pub fn ensure_gateway(port: u16) -> Result<(), String> {
    let mut guard = GATEWAY.lock();
    if guard.is_some() {
        return Ok(());
    }
    let dir = warp_core::paths::state_dir();
    let (blocks, raw) = if dir.as_os_str().is_empty() {
        (None, None)
    } else {
        // 对齐观测台读取路径(harness_blocks.db/harness_raw_cache.db)。
        (
            Some(dir.join("harness_blocks.db")),
            Some(dir.join("harness_raw_cache.db")),
        )
    };
    let gw = RT
        .block_on(EntryGateway::start(
            port,
            blocks.as_deref(),
            raw.as_deref(),
        ))
        .map_err(|e| format!("entry gateway bind failed: {e}"))?;
    log::info!("external-capture: entry gateway up on 127.0.0.1:{port}");
    *guard = Some(gw);
    Ok(())
}

/// 停入口网关(落 Exit + 端口释放)。幂等。stop 同步等 graceful 收尾
/// (上限 2s, 超时 abort 兜底) — 本函数返回时端口确定不可连。
pub fn shutdown() {
    if let Some(gw) = GATEWAY.lock().take() {
        RT.block_on(gw.stop());
        log::info!("external-capture: entry gateway stopped");
    }
}

/// 入口是否在跑 + 端口(None = 未运行)。
pub fn entry_port() -> Option<u16> {
    GATEWAY.lock().as_ref().map(|g| g.port())
}

/// 前缀观测 session 快照(观测台 UI 数据源)。
pub fn snapshot() -> Vec<EntrySessionInfo> {
    GATEWAY
        .lock()
        .as_ref()
        .map(|g| g.snapshot())
        .unwrap_or_default()
}

// ── 别名武装(bootstrap 静默注入) ──────────────────────────────────────────

/// Bootstrap 武装方言(由调用方从 `ShellType` 映射)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArmingDialect {
    /// bash/zsh: `name(){ <body>; }`
    Posix,
    /// fish: `function name; <body>; end`
    Fish,
}

/// T8 实例标记: **每次别名调用** = 一次 CLI 实例启动, 标记必须在调用时
/// 生成(定义时铸死会让同一 shell 的多次调用共享标记 → 违反一实例一
/// session), 故函数体内嵌运行时表达式 `$(date +%s%N)-$$`(ns 时间戳保证
/// 同 shell 两次调用不同, pid 保证跨 shell 不同; fish 用
/// `(date +%s%N)-$fish_pid`)。
///
/// 标记的落地信道(三类 CLI 均已本机实证), 每别名只带自己 CLI 的信道:
/// - cc-dais: `ANTHROPIC_CUSTOM_HEADERS="x-dais-instance: <tag>"` 进程 env
///   赋值前缀(CC 只从进程 env 读; settings env 块优先级压过进程 env —
///   T3 实证, 不能写进 cc-entry-settings.json)。
/// - omp/pi: `DAIS_INSTANCE_TAG` 进程 env(模型配置 provider `headers` 的
///   env 引用: omp models.yml `DAIS_INSTANCE_TAG`, pi models.json
///   `${DAIS_INSTANCE_TAG}`)。
/// 网关按标记键控 session(`external-<p>-<tag>`), 转发前剥头(管道字节
/// 不变)。
///
/// T9: 解析 CLI 独立默认模型 ID — 动态跟随, 替代 `alias_defs` 原铸死 glm-5.2。
///
/// - **omp**: 读 `$HOME/.omp/agent/config.yml` → `modelRoles.default`
///   (如 `zhipu-coding-plan/glm-5.2` 或 `zhipu-coding-plan/glm-5-turbo`)。
/// - **pi**: 读 `$HOME/.pi/agent/settings.json` → `defaultModel` (如 `glm-5.2`)。
/// 提取裸模型 ID(去 provider 前缀 + `:effort` 后缀)。任意环节缺省/解析失败
/// → 兜底 `"glm-5.2"`(原铸死值)。
fn resolve_default_model_id(cli: &str) -> String {
    const FALLBACK: &str = "glm-5.2";
    let home = std::env::var("HOME").unwrap_or_default();
    let raw = match cli {
        "omp" => {
            let path = std::path::Path::new(&home).join(".omp/agent/config.yml");
            std::fs::read_to_string(&path)
                .ok()
                .and_then(|s| serde_yaml::from_str::<serde_yaml::Value>(&s).ok())
                .and_then(|v| {
                    v.get("modelRoles")?
                        .get("default")?
                        .as_str()
                        .map(String::from)
                })
        }
        "pi" => {
            let path = std::path::Path::new(&home).join(".pi/agent/settings.json");
            std::fs::read_to_string(&path)
                .ok()
                .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
                .and_then(|v| v.get("defaultModel")?.as_str().map(String::from))
        }
        _ => None,
    };
    let Some(raw) = raw else {
        return FALLBACK.to_string();
    };
    // 去 provider 前缀 + `:effort` 后缀。
    let bare = raw.split(':').next().unwrap_or(&raw);
    bare.rsplit('/')
        .next()
        .unwrap_or(bare)
        .to_string()
}

/// 别名函数体按方言包装(单行, 无换行 — 投递安全)。
fn alias_defs(entry_port: u16, dialect: ArmingDialect) -> String {
    let cc_settings = cc_entry_settings_path().display().to_string();
    // Posix: 赋值前缀只对该 command 进程生效, 不污染 shell。
    let (cc_env, cli_env) = (
        r#"ANTHROPIC_CUSTOM_HEADERS="x-dais-instance: $(date +%s%N)-$$""#,
        r#"DAIS_INSTANCE_TAG="$(date +%s%N)-$$""#,
    );
    // fish: 引号外做命令拼接(set -lx 函数作用域导出, 不污染交互 shell)。
    let (cc_env_f, cli_env_f) = (
        r#"set -lx ANTHROPIC_CUSTOM_HEADERS "x-dais-instance: "(date +%s%N)-$fish_pid"#,
        r#"set -lx DAIS_INSTANCE_TAG (date +%s%N)-$fish_pid"#,
    );
    let omp_model = resolve_default_model_id("omp");
    let pi_model = resolve_default_model_id("pi");
    let bodies: [(&str, String, String); 3] = [
        ("cc-dais", format!("command claude --settings"), cc_settings),
        ("omp-dais", format!("command omp --model dais/{omp_model}"), String::new()),
        ("pi-dais", format!("command pi --model dais/{pi_model}"), String::new()),
    ];
    let _ = entry_port; // settings 文件内固化端口; 函数体不重复携带
    bodies
        .iter()
        .map(|(name, body, extra)| {
            let is_cc = *name == "cc-dais";
            match dialect {
                ArmingDialect::Posix => {
                    let env = if is_cc { cc_env } else { cli_env };
                    if is_cc {
                        format!(r#"{name}(){{ {env} {body} '{extra}' "$@"; }}"#)
                    } else {
                        format!(r#"{name}(){{ {env} {body} "$@"; }}"#)
                    }
                }
                ArmingDialect::Fish => {
                    let env = if is_cc { cc_env_f } else { cli_env_f };
                    if is_cc {
                        format!("function {name}; {env}; {body} '{extra}' $argv; end")
                    } else {
                        format!("function {name}; {env}; {body} $argv; end")
                    }
                }
            }
        })
        .collect::<Vec<_>>()
        .join(";")
}

/// `~/.config/zap/cc-entry-settings.json` 路径(zap 配置目录, 非 state)。
fn cc_entry_settings_path() -> std::path::PathBuf {
    let home = std::env::var("HOME").unwrap_or_default();
    std::path::Path::new(&home)
        .join(".config")
        .join("zap")
        .join("cc-entry-settings.json")
}

/// 生成 `cc-entry-settings.json`: env 块 = 用户 `~/.claude/settings.json`
/// env 透传合并 + `ANTHROPIC_BASE_URL` 覆盖为入口 `/cc`(明文 HTTP, 无 CA)。
/// 其余顶层键(模型映射/超时等)同样透传合并, `env` 键覆盖之。
fn cc_entry_settings_content(port: u16) -> String {
    let user_settings: serde_json::Value = std::env::var("HOME")
        .ok()
        .and_then(|h| {
            std::fs::read_to_string(
                std::path::Path::new(&h)
                    .join(".claude")
                    .join("settings.json"),
            )
            .ok()
        })
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or(serde_json::Value::Object(Default::default()));

    let mut merged = match user_settings {
        serde_json::Value::Object(map) => serde_json::Value::Object(map),
        _ => serde_json::Value::Object(Default::default()),
    };
    let obj = merged.as_object_mut().expect("just built object");
    let mut env = obj
        .get("env")
        .and_then(|v| v.as_object())
        .cloned()
        .unwrap_or_default();
    env.insert(
        "ANTHROPIC_BASE_URL".to_string(),
        serde_json::Value::String(format!("http://127.0.0.1:{port}/cc")),
    );
    obj.insert("env".to_string(), serde_json::Value::Object(env));
    serde_json::to_string_pretty(&merged).unwrap_or_default()
}

/// (重)生成 cc-entry-settings.json(每次武装前刷新, 端口/用户配置变化即
/// 生效)。写失败仅记日志 — cc-dais 用旧文件仍可用。
fn refresh_cc_entry_settings(port: u16) -> std::io::Result<()> {
    let path = cc_entry_settings_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, cc_entry_settings_content(port))
}

/// Bootstrap 武装后缀: 入口在跑 + 开关开时返回三别名函数定义串(单行),
/// 并刷新 cc-entry-settings.json; 否则 `None`(bootstrap 原样, 裸命令
/// 零劫持)。
pub fn bootstrap_arming_suffix(dialect: ArmingDialect, enabled: bool) -> Option<String> {
    if !enabled {
        return None;
    }
    let port = entry_port()?;
    if let Err(e) = refresh_cc_entry_settings(port) {
        log::warn!("external-capture: cc-entry-settings refresh failed: {e}");
    }
    log::info!("external-capture: armed aliases cc-dais/omp-dais/pi-dais (entry :{port})");
    Some(alias_defs(port, dialect))
}

/// 将武装函数定义插入 bootstrap 脚本的不可见执行区(T3a 静默机制)。
///
/// zsh/bash 的 bootstrap 是 heredoc 结构(`read ... << 'EOM'` … `EOM`):
/// `EOM` 标记**之后**的字节是独立输入行, 会被 ZLE 当作用户命令回显执行。
/// 插入点必须在 heredoc 结束标记之前(函数定义随 `WARP_BOOTSTRAP_VAR`
/// 一起 eval, 零回显); 脚本无 heredoc 标记(fish, 走临时文件 source,
/// 本就不回显)则尾部追加。
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

// view 身份参数保留给武装调用方签名兼容(pty_controller 传 view id)。
#[allow(dead_code)]
type _ViewId = EntityId;

#[cfg(test)]
mod tests {
    use super::*;

    /// T4: HOME/XDG/GATEWAY 都是进程级全局 — 本模块内改动它们的测试必须
    /// 串行(test 线程并行跑, 否则互相踩)。
    static T4_LOCK: parking_lot::Mutex<()> = parking_lot::Mutex::new(());

    #[test]
    fn alias_defs_shapes() {
        let _env = T4_LOCK.lock(); // cc-dais 段含 HOME 派生路径, 与改 HOME 的测试串行
        let posix = alias_defs(8787, ArmingDialect::Posix);
        // T8: 三别名各带调用时铸标记的 env 前缀(cc 走 ANTHROPIC_CUSTOM_
        // HEADERS, omp/pi 走 DAIS_INSTANCE_TAG)。
        assert!(posix.contains(
            r#"cc-dais(){ ANTHROPIC_CUSTOM_HEADERS="x-dais-instance: $(date +%s%N)-$$" command claude --settings"#
        ));
        assert!(posix.contains("/cc-entry-settings.json' \"$@\"; }"));
        assert!(posix.contains(
            r#"omp-dais(){ DAIS_INSTANCE_TAG="$(date +%s%N)-$$" command omp --model dais/glm-5.2 "$@"; }"#
        ));
        assert!(posix.contains(
            r#"pi-dais(){ DAIS_INSTANCE_TAG="$(date +%s%N)-$$" command pi --model dais/glm-5.2 "$@"; }"#
        ));
        assert!(!posix.contains('\n'), "单行投递安全");

        let fish = alias_defs(8787, ArmingDialect::Fish);
        assert!(fish.contains(
            r#"set -lx ANTHROPIC_CUSTOM_HEADERS "x-dais-instance: "(date +%s%N)-$fish_pid"#
        ));
        assert!(fish.contains(
            "set -lx DAIS_INSTANCE_TAG (date +%s%N)-$fish_pid; command omp --model dais/glm-5.2 $argv; end"
        ));
    }

    /// T8: 标记必须**调用时**铸(函数体内运行时表达式), 不是定义时铸死 —
    /// 否则同一 shell 的多次 omp-dais 调用共享标记, 违反一实例一 session。
    /// 断言: 每个别名体都含 ns 时间戳+pid 表达式; defs 是纯模板(两次
    /// 生成全等, 不携带任何铸造期状态)。
    #[test]
    fn instance_tags_minted_at_call_time_not_def_time() {
        let _env = T4_LOCK.lock(); // 同上: a/b 全等断言跨 HOME 派生路径
        let a = alias_defs(8787, ArmingDialect::Posix);
        let b = alias_defs(8787, ArmingDialect::Posix);
        assert_eq!(a, b, "defs 是纯模板, 不携带调用期铸造状态");
        // 每个别名各含一次调用时铸造表达式。
        assert_eq!(a.matches("$(date +%s%N)-$$").count(), 3, "三别名各铸: {a}");
        // fish 同口径。
        let f = alias_defs(8787, ArmingDialect::Fish);
        assert_eq!(f.matches("(date +%s%N)-$fish_pid").count(), 3, "{f}");
    }

    #[test]
    fn cc_entry_settings_merges_user_env_and_overrides_base_url() {
        let _env = T4_LOCK.lock();
        let dir = tempfile::tempdir().unwrap();
        let orig_home = std::env::var("HOME").ok();
        std::env::set_var("HOME", dir.path());
        let result = (|| {
            std::fs::create_dir_all(dir.path().join(".claude")).unwrap();
            std::fs::write(
                dir.path().join(".claude/settings.json"),
                r#"{"env":{"ANTHROPIC_BASE_URL":"https://user-relay.example.com","ANTHROPIC_MODEL":"claude-x"},"permissions":{"allow":["Bash"]}}"#,
            )
            .unwrap();

            let v: serde_json::Value =
                serde_json::from_str(&cc_entry_settings_content(8787)).unwrap();
            // 覆盖: base URL → 入口 /cc(明文)。
            assert_eq!(
                v["env"]["ANTHROPIC_BASE_URL"],
                "http://127.0.0.1:8787/cc"
            );
            // 透传: 用户 env 其余键 + 顶层其余键。
            assert_eq!(v["env"]["ANTHROPIC_MODEL"], "claude-x");
            assert_eq!(v["permissions"]["allow"][0], "Bash");

            // 用户 settings 缺失 → 仅覆盖键, 不 panic。
            std::fs::remove_file(dir.path().join(".claude/settings.json")).unwrap();
            let v: serde_json::Value =
                serde_json::from_str(&cc_entry_settings_content(8787)).unwrap();
            assert_eq!(
                v["env"]["ANTHROPIC_BASE_URL"],
                "http://127.0.0.1:8787/cc"
            );
        })();
        match orig_home {
            Some(h) => std::env::set_var("HOME", h),
            None => std::env::remove_var("HOME"),
        }
        result
    }

    #[test]
    fn arming_inserts_before_heredoc_end_marker() {
        let script =
            b" read -r -d '' V << 'EOM'; eval \"$V\"\nbody\nEOM\n";
        let out = insert_arming_into_script(script, "cc-dais(){ x; }");
        let s = String::from_utf8(out).unwrap();
        let eom = s.rfind("\nEOM\n").expect("EOM preserved");
        let defs = s.find("cc-dais(){ x; }").expect("defs present");
        assert!(defs < eom, "defs must land BEFORE the EOM marker");

        // fish 形状(无 heredoc): 尾部追加。
        let script = b"warp_bootstrapped\nend\n";
        let out = insert_arming_into_script(script, "function cc-dais; x; end");
        assert!(String::from_utf8_lossy(&out).ends_with("function cc-dais; x; end"));
    }

    // ── T4-E2E 回归钉 ──────────────────────────────────────────────────

    /// 别名函数体: 三别名均携带一次性实例标记前缀(T8), cc-dais 另携
    /// --settings 全路径(入 HOME), omp/pi 携 --model dais/glm-5.2; 裸命令
    /// (claude/omp/pi)零函数定义 — bootstrap 注入不劫持裸调用。
    #[test]
    fn t4_alias_bodies_pin_settings_path_model_and_zero_bare_hijack() {
        let _env = T4_LOCK.lock();
        let dir = tempfile::tempdir().unwrap();
        let orig_home = std::env::var("HOME").ok();
        std::env::set_var("HOME", dir.path());
        let result = (|| {
            let settings = dir
                .path()
                .join(".config/zap/cc-entry-settings.json")
                .display()
                .to_string();

            let posix = alias_defs(8787, ArmingDialect::Posix);
            // 取别名段: 从本别名定义起点到下一别名起点(defs 以 ';' 直连,
            // def 体内也含 ';', 不能简单 split)。
            let seg = |hay: &str, name: &str, next: &str| -> String {
                let s = hay.find(name).unwrap_or_else(|| panic!("{name} 缺失: {hay}"));
                let rest = &hay[s..];
                let e = rest.find(next).unwrap_or(rest.len());
                rest[..e].trim_end_matches(';').to_string()
            };
            // cc-dais: --settings 全路径; 不带 --model。T8: 全等钉完整函数体
            // (含 ANTHROPIC_CUSTOM_HEADERS 调用时铸标记前缀 — 引号值内含
            // 空格, 不能切割解析)。
            let cc = seg(&posix, "cc-dais()", "omp-dais()");
            assert_eq!(
                cc,
                format!(
                    r#"cc-dais(){{ ANTHROPIC_CUSTOM_HEADERS="x-dais-instance: $(date +%s%N)-$$" command claude --settings '{settings}' "$@"; }}"#
                )
            );
            assert!(!cc.contains("--model"), "cc-dais 不携带 --model");
            // omp-dais / pi-dais: --model dais/glm-5.2; 不带 --settings;
            // DAIS_INSTANCE_TAG 调用时铸。
            for (name, bin, next) in [
                ("omp-dais", "omp", "pi-dais()"),
                ("pi-dais", "pi", "\u{0}none"),
            ] {
                let d = seg(&posix, &format!("{name}()"), next);
                assert_eq!(
                    d,
                    format!(
                        r#"{name}(){{ DAIS_INSTANCE_TAG="$(date +%s%N)-$$" command {bin} --model dais/glm-5.2 "$@"; }}"#
                    )
                );
                assert!(!d.contains("--settings"), "{name} 不携带 --settings");
            }
            // fish 方言: 三别名齐全(pi 也钉, 全等)。
            let fish = alias_defs(8787, ArmingDialect::Fish);
            assert_eq!(
                seg(&fish, "function pi-dais", "\u{0}none"),
                "function pi-dais; set -lx DAIS_INSTANCE_TAG (date +%s%N)-$fish_pid; command pi --model dais/glm-5.2 $argv; end"
            );

            // 裸命令零劫持: 不存在裸名(claude/omp/pi)函数定义。
            for defs in [&posix, &fish] {
                for bare in ["claude()", "omp()", "pi()"] {
                    assert!(
                        !defs.contains(bare),
                        "bootstrap 后缀不得定义裸命令 {bare}: {defs}"
                    );
                }
            }
        })();
        match orig_home {
            Some(h) => std::env::set_var("HOME", h),
            None => std::env::remove_var("HOME"),
        }
        result
    }
    /// T9: 别名武装动态跟随 CLI 独立默认模型 — omp/pi 各读独立配置
    /// (config.yml modelRoles.default / settings.json defaultModel), 嵌入
    /// `--model dais/<id>`。无配置兜底原铸死值 glm-5.2。
    #[test]
    fn t9_alias_model_follows_cli_default() {
        let _env = T4_LOCK.lock();
        let dir = tempfile::tempdir().unwrap();
        let orig_home = std::env::var("HOME").ok();
        std::env::set_var("HOME", dir.path());
        let result = (|| {
            // ── 装备 omp config.yml (modelRoles.default 含 :effort 后缀) ──
            std::fs::create_dir_all(dir.path().join(".omp/agent")).unwrap();
            std::fs::write(
                dir.path().join(".omp/agent/config.yml"),
                "modelRoles:\n  default: zhipu-coding-plan/glm-5-turbo\n",
            )
            .unwrap();
            // ── 装备 pi settings.json (defaultModel 裸 id) ──
            std::fs::create_dir_all(dir.path().join(".pi/agent")).unwrap();
            std::fs::write(
                dir.path().join(".pi/agent/settings.json"),
                r#"{"defaultModel":"glm-5-turbo"}"#,
            )
            .unwrap();

            let posix = alias_defs(8787, ArmingDialect::Posix);
            // omp-dais: 动态读 config.yml → glm-5-turbo。
            assert!(
                posix.contains("--model dais/glm-5-turbo"),
                "omp-dais 应跟随 omp config 默认模型: {posix}"
            );
            // pi-dais: 动态读 settings.json → glm-5-turbo。
            assert!(
                posix.contains("pi --model dais/glm-5-turbo"),
                "pi-dais 应跟随 pi settings 默认模型: {posix}"
            );
            // fish 同口径。
            let fish = alias_defs(8787, ArmingDialect::Fish);
            assert!(
                fish.contains("--model dais/glm-5-turbo"),
                "fish omp-dais 应跟随: {fish}"
            );
            assert!(
                fish.contains("pi --model dais/glm-5-turbo"),
                "fish pi-dais 应跟随: {fish}"
            );

            // ── 无配置 → 兜底 glm-5.2 (原铸死值不变) ──
            std::fs::remove_file(dir.path().join(".omp/agent/config.yml")).unwrap();
            std::fs::remove_file(dir.path().join(".pi/agent/settings.json")).unwrap();
            let posix_fb = alias_defs(8787, ArmingDialect::Posix);
            assert!(
                posix_fb.contains("omp --model dais/glm-5.2"),
                "omp 配置缺失 → 兜底 glm-5.2: {posix_fb}"
            );
            assert!(
                posix_fb.contains("pi --model dais/glm-5.2"),
                "pi 配置缺失 → 兜底 glm-5.2: {posix_fb}"
            );
        })();
        match orig_home {
            Some(h) => std::env::set_var("HOME", h),
            None => std::env::remove_var("HOME"),
        }
        result
    }

    /// cc-entry-settings.json: 端口参数贯通(BASE_URL 随入口端口变化)。
    /// 只读断言(不写盘), 覆盖优先级由上方 merge 测试钉。
    #[test]
    fn t4_cc_entry_settings_port_wires_into_base_url() {
        for port in [8787u16, 39021] {
            let v: serde_json::Value =
                serde_json::from_str(&cc_entry_settings_content(port)).unwrap();
            assert_eq!(
                v["env"]["ANTHROPIC_BASE_URL"],
                format!("http://127.0.0.1:{port}/cc"),
                "端口 {port} 必须贯通到 BASE_URL 覆盖"
            );
        }
    }

    /// 生命周期回归钉: 开 → 入口在跑 + bootstrap 武装(别名 + settings 落盘);
    /// 关 → 端口关闭 + 不再武装(开关开也不武装) — 裸命令回到零劫持。
    #[test]
    fn t4_gateway_lifecycle_switch_off_closes_entry_port() {
        let _env = T4_LOCK.lock();
        let dir = tempfile::tempdir().unwrap();
        let orig_home = std::env::var("HOME").ok();
        let orig_state = std::env::var("XDG_STATE_HOME").ok();
        std::env::set_var("HOME", dir.path());
        // 观测 DB 落临时 state 目录, 不碰真实用户 state。
        std::env::set_var("XDG_STATE_HOME", dir.path());
        assert!(
            warp_core::paths::state_dir().starts_with(dir.path()),
            "XDG_STATE_HOME 重定向未生效, 中止以免污染真实 state"
        );
        // state 子目录(zap/)不存在时 BlockStore 打不开 — 应用启动路径会
        // 先建目录, 测试里同样预建。
        std::fs::create_dir_all(warp_core::paths::state_dir()).unwrap();

        let result = (|| {
            ensure_gateway(0).expect("随机端口绑定必成");
            let port = entry_port().expect("开关开 → 入口在跑");
            assert_ne!(port, 0);

            // 开关开 + 入口在跑 → bootstrap 武装: 三别名 + settings 刷新落盘。
            let suffix = bootstrap_arming_suffix(ArmingDialect::Posix, true).unwrap();
            assert!(suffix.contains("cc-dais()"), "armed suffix: {suffix}");
            assert!(suffix.contains("omp-dais()"));
            assert!(suffix.contains("pi-dais()"));
            let settings_path = dir
                .path()
                .join(".config/zap/cc-entry-settings.json");
            let text = std::fs::read_to_string(&settings_path)
                .expect("cc-entry-settings.json 必须随武装落盘");
            let v: serde_json::Value = serde_json::from_str(&text).unwrap();
            assert_eq!(
                v["env"]["ANTHROPIC_BASE_URL"],
                format!("http://127.0.0.1:{port}/cc")
            );

            // 开关关(入口在跑也不武装) — 裸命令零劫持。
            assert_eq!(bootstrap_arming_suffix(ArmingDialect::Posix, false), None);

            // 开关关 → shutdown: 端口关闭 + 不再武装(开也不)。
            shutdown();
            assert_eq!(entry_port(), None, "关 → entry_port None");
            assert_eq!(
                bootstrap_arming_suffix(ArmingDialect::Posix, true),
                None,
                "入口已关, 开关开也不武装"
            );
            // T7: stop 同步等端口释放(graceful 优先, 超时 abort 兜底) —
            // shutdown 返回即确定不可连, 旧轮询已删。
            assert!(
                std::net::TcpStream::connect(("127.0.0.1", port)).is_err(),
                "shutdown 返回后 entry 端口必须立即不可连"
            );
        })();

        shutdown(); // 幂等兜底(断言失败也勿泄漏网关到其他测试)
        match orig_home {
            Some(h) => std::env::set_var("HOME", h),
            None => std::env::remove_var("HOME"),
        }
        match orig_state {
            Some(v) => std::env::set_var("XDG_STATE_HOME", v),
            None => std::env::remove_var("XDG_STATE_HOME"),
        }
        result
    }

    #[test]
    fn arming_suffix_disabled_is_none() {
        let _env = T4_LOCK.lock();
        assert_eq!(bootstrap_arming_suffix(ArmingDialect::Posix, false), None);
        assert_eq!(bootstrap_arming_suffix(ArmingDialect::Posix, true), None);
    }
}
