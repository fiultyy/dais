//! External capture runtime (T5) — 单端口入口 + 别名武装, zap 进程级。
//!
//! 锁定口径 (用户拍板): 别名是唯一入口。裸命令(`omp`/`claude`/`pi`)行为
//! 完全不变; 只有 `cc-zap`/`omp-zap`/`pi-zap` 进通道:
//! - **入口**: [`EntryGateway`](proxy_interceptor 单端口入口, 明文 HTTP,
//!   默认 8787 持久化 intercept_config.json), 路径前缀分流 `/cc` `/omp`
//!   `/pi` → 各自出口。auth 透明管道(客户端凭据原样转发, zap 不注不剥)。
//! - **观测**: 每前缀归并一个常驻 session(`external-cc/omp/pi`), Spawn
//!   懒发(首个真实请求才落 block)。
//! - **武装**: 本地 pane 首个 shell 的 bootstrap 脚本不可见区插入三个
//!   同名 shell 函数(`cc-zap`/`omp-zap`/`pi-zap`, heredoc 感知插入零可见
//!   污染)。`cc-zap` 走 `--settings` 深覆盖(静态文件
//!   `~/.config/zap/cc-entry-settings.json`, env 块从用户 `~/.claude/
//!   settings.json` 透传合并 + `ANTHROPIC_BASE_URL` 覆盖为入口 `/cc`);
//!   `omp-zap`/`pi-zap` 传 `--model zap/glm-5.2`(models.yml 由编排侧写,
//!   baseUrl 指向入口 `/omp`、`/pi`)。
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

/// 停入口网关(落 Exit + 端口释放)。幂等。
pub fn shutdown() {
    if let Some(gw) = GATEWAY.lock().take() {
        gw.stop();
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

/// 别名函数体按方言包装(单行, 无换行 — 投递安全)。
fn alias_defs(entry_port: u16, dialect: ArmingDialect) -> String {
    let cc_settings = cc_entry_settings_path().display().to_string();
    let bodies = [
        (
            "cc-zap",
            format!("command claude --settings '{cc_settings}'"),
        ),
        ("omp-zap", "command omp --model zap/glm-5.2".to_string()),
        ("pi-zap", "command pi --model zap/glm-5.2".to_string()),
    ];
    let _ = entry_port; // settings 文件内固化端口; 函数体不重复携带
    bodies
        .iter()
        .map(|(name, body)| match dialect {
            ArmingDialect::Posix => format!(r#"{name}(){{ {body} "$@"; }}"#),
            ArmingDialect::Fish => format!("function {name}; {body} $argv; end"),
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
/// 生效)。写失败仅记日志 — cc-zap 用旧文件仍可用。
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
    log::info!("external-capture: armed aliases cc-zap/omp-zap/pi-zap (entry :{port})");
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

    #[test]
    fn alias_defs_shapes() {
        let posix = alias_defs(8787, ArmingDialect::Posix);
        assert!(posix.contains(
            "cc-zap(){ command claude --settings '"
        ));
        assert!(posix.contains("/cc-entry-settings.json' \"$@\"; }"));
        assert!(posix.contains("omp-zap(){ command omp --model zap/glm-5.2 \"$@\"; }"));
        assert!(posix.contains("pi-zap(){ command pi --model zap/glm-5.2 \"$@\"; }"));
        assert!(!posix.contains('\n'), "单行投递安全");

        let fish = alias_defs(8787, ArmingDialect::Fish);
        assert!(fish.contains("function cc-zap; command claude --settings '"));
        assert!(fish.contains("function omp-zap; command omp --model zap/glm-5.2 $argv; end"));
    }

    #[test]
    fn cc_entry_settings_merges_user_env_and_overrides_base_url() {
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
        let out = insert_arming_into_script(script, "cc-zap(){ x; }");
        let s = String::from_utf8(out).unwrap();
        let eom = s.rfind("\nEOM\n").expect("EOM preserved");
        let defs = s.find("cc-zap(){ x; }").expect("defs present");
        assert!(defs < eom, "defs must land BEFORE the EOM marker");

        // fish 形状(无 heredoc): 尾部追加。
        let script = b"warp_bootstrapped\nend\n";
        let out = insert_arming_into_script(script, "function cc-zap; x; end");
        assert!(String::from_utf8_lossy(&out).ends_with("function cc-zap; x; end"));
    }

    #[test]
    fn arming_suffix_disabled_is_none() {
        assert_eq!(bootstrap_arming_suffix(ArmingDialect::Posix, false), None);
        // 开关开但入口未跑(测试进程无网关) → 同样 None(裸命令零劫持)。
        assert_eq!(bootstrap_arming_suffix(ArmingDialect::Posix, true), None);
    }
}
