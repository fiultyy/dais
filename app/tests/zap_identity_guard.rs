//! zap 身份残留护栏(zap-purge,2026-08-22)。
//!
//! 断言 app/src 与 crates/ 源码无 zap 身份字面。有意保留的例外逐条列在
//! [`WHITELIST`],每条带理由;新增例外必须同时新增理由注释。
//!
//! 两层机制:
//! 1. **标记行跳过**: 含 `zap-purge` 的行是清理时留下的显式标注(契约说明/
//!    数据兼容注释),直接放行——标注本身即"已经审过"的信号。
//! 2. **白名单**: 逐条列出文件级/行级豁免,覆盖五类:
//!    - 外部世界字面(Zapfino 字体、zerx-lab 上游仓库)
//!    - 历史计划标记(`TODO(zap-cloud-removal …)` 指向既有计划文档)
//!    - 过渡契约(DAIS_* 优先、ZAP_* 回退的 env;hook 子进程双写)
//!    - 数据兼容 key(wire/shell API/持久化 marker/CI 工件名)
//!    - 测试夹具历史快照(旧 cwd、providers:zap: 配置)

use std::fs;
use std::path::{Path, PathBuf};

/// 一条白名单: 文件路径后缀匹配 + 行内子串(大小写敏感),命中即豁免。
struct Exemption {
    /// 文件路径以后缀匹配(如 `runtime_rpc.rs`、`autoupdate/linux.rs`)。
    path_suffix: &'static str,
    /// 该行必须包含的子串;空串 = 该文件任意含 zap 行均豁免(仅限
    /// 整文件皆契约/夹具的场合,慎用)。
    needle: &'static str,
    reason: &'static str,
}

const WHITELIST: &[Exemption] = &[
    // ── 外部世界字面 ──
    Exemption { path_suffix: ".rs", needle: "Zapfino", reason: "macOS 真实字体名" },
    Exemption { path_suffix: ".rs", needle: "zerx-lab", reason: "上游仓库历史事实(fork 血统)" },
    // ── 历史计划标记 ──
    Exemption { path_suffix: ".rs", needle: "zap-cloud-removal", reason: "TODO 指向既有 docs/zap-cloud-removal-plan.md 计划" },
    // ── 遗留 FeatureFlag 变体名 ──
    Exemption { path_suffix: ".rs", needle: "ZapLaunchModal", reason: "FeatureFlag 变体旧名,防序列化/配置兼容保留" },
    Exemption { path_suffix: ".rs", needle: "ZapNewSettingsModes", reason: "FeatureFlag 变体旧名,防序列化/配置兼容保留" },
    // ── 过渡契约 env(zap-purge 一次性,收口时移除)──
    Exemption { path_suffix: ".rs", needle: "ZAP_UNSTABLE_FEATURES", reason: "DAIS_UNSTABLE_FEATURES 优先、旧名回退" },
    Exemption { path_suffix: ".rs", needle: "ZAP_HOOK", reason: "hook 子进程 env 双写: 外部 hook 脚本仍读旧名" },
    Exemption { path_suffix: ".rs", needle: "ZAP_UPSTREAM_BASE", reason: "DAIS_UPSTREAM_BASE 优先、旧名回退" },
    Exemption { path_suffix: ".rs", needle: "ZAP_API_KEY", reason: "DAIS_API_KEY 优先、旧名回退" },
    Exemption { path_suffix: ".rs", needle: "ZAP_OMP_KEY", reason: "DAIS_OMP_KEY 优先、旧名回退" },
    Exemption { path_suffix: ".rs", needle: "ZAP_CONFIG", reason: "DAIS_CONFIG 优先、旧名回退(gist wire 数据)" },
    Exemption { path_suffix: ".rs", needle: "ZAP_WORKLOAD_AUDIENCE", reason: "OIDC audience 旧名回退" },
    // ── wire / shell API ──
    Exemption { path_suffix: ".rs", needle: "Zap-Run-GeneratorCommand", reason: "PowerShell in-band 命令名,已装脚本依赖" },
    Exemption { path_suffix: ".rs", needle: "Zap(", reason: "DCS 版本应答 \\x1bP>|Zap({version}) wire 序列" },
    Exemption { path_suffix: ".rs", needle: "X-Zap-", reason: "HTTP wire 头(X-Zap-Client-Version 等)" },
    Exemption { path_suffix: ".rs", needle: "x-zap-", reason: "HTTP wire 头/过滤表(x-zap-hook-token、x-zap-instance)" },
    // ── CI 打包契约(资产名/包名与 script/* 与 zap_release.yml 产物对齐)──
    Exemption { path_suffix: "autoupdate/linux.rs", needle: "", reason: "AppImage 资产名+deb/rpm/arch/AUR 包名 CI 契约" },
    Exemption { path_suffix: "autoupdate/windows.rs", needle: "", reason: "ZapSetup.exe 资产名 CI 契约" },
    Exemption { path_suffix: "autoupdate/mac.rs", needle: "", reason: "dmg 资产名/app_name_prefix CI 契约" },
    Exemption { path_suffix: "autoupdate/github.rs", needle: "", reason: "GitHub Release 资产探测契约" },
    Exemption { path_suffix: ".rs", needle: "ZapDockTilePlugin", reason: "Objective-C dock 插件文件名,随 bundle 产物" },
    // ── 数据兼容 key / legacy 路径 ──
    Exemption { path_suffix: "bin/dais.rs", needle: "", reason: "legacy bundle ID dev.zap.Dais + zap:// URL scheme(已注册用户)" },
    Exemption { path_suffix: "persistence/sqlite.rs", needle: "", reason: "zap 时代 app-group 迁移 marker/函数(磁盘数据)" },
    Exemption { path_suffix: "sqlite_tests.rs", needle: "", reason: "迁移行为测试(迁移的正是 zap 旧数据)" },
    Exemption { path_suffix: ".rs", needle: "zap-minidump-", reason: "legacy crash dump 文件名(既有 crash reporter 配置)" },
    Exemption { path_suffix: ".rs", needle: "zap.prompt_chips", reason: "legacy 日志文件名排除项" },
    Exemption { path_suffix: ".rs", needle: "zap-app-group-sqlite-migrated", reason: "磁盘迁移 marker" },
    Exemption { path_suffix: ".rs", needle: "zap-local-secure-storage-fallback-key", reason: "加密 key 材料" },
    Exemption { path_suffix: ".rs", needle: "zap.ssh", reason: "macOS Keychain service 名(既有条目)" },
    Exemption { path_suffix: ".rs", needle: "Software\\\\Zap", reason: "Windows 注册表 base 路径(既有键)" },
    Exemption { path_suffix: ".rs", needle: "~/.zap/", reason: "remote-server 旧数据目录(已部署远端)" },
    Exemption { path_suffix: ".rs", needle: "ZapDev", reason: "dev.zap.ZapDev 等旧 app ID 兼容断言" },
    Exemption { path_suffix: ".rs", needle: "dev.warp.Zap", reason: "旧 bundle ID 兼容映射/断言" },
    Exemption { path_suffix: ".rs", needle: "dev.zap.", reason: "旧 bundle ID 兼容映射/断言" },
    // ── 测试夹具历史快照(解析目标为任意数据,zap 只是数据)──
    Exemption { path_suffix: "ai/observatory/context_usage.rs", needle: "", reason: "providers:zap: 配置夹具 + zap/* 旧模型别名兼容注释" },
    Exemption { path_suffix: "ai/observatory/system_prompt_segments.rs", needle: "", reason: "旧 cwd /home/yy/warpdotdev/zap 夹具" },
    Exemption { path_suffix: "ai/observatory/view.rs", needle: "", reason: "旧 cwd 夹具" },
    Exemption { path_suffix: "ai/external_capture_rt.rs", needle: "zap/", reason: "旧 state 子目录(zap/)回退语义" },
    Exemption { path_suffix: "external_editor/mac.rs", needle: "", reason: "外部编辑器 bundle ID org 匹配(第三方 app)" },
    Exemption { path_suffix: "external_editor/mac_test.rs", needle: "", reason: "外部 bundle ID 断言" },
    Exemption { path_suffix: "completer/engine/path_test.rs", needle: "", reason: "Zap.app 路径夹具(路径解析测试)" },
    Exemption { path_suffix: "parsers/simple/lexer_test.rs", needle: "", reason: "Windows 安装器路径夹具" },
    // ── runtime RPC 过渡(zap-purge 一次性,收口时移除)──
    Exemption { path_suffix: "runtime_rpc.rs", needle: "zap", reason: "dais-runtime.json 读侧旧名回退 + 残留清理注释" },
    // ── CI 工件名(setup/install 脚本与测试)──
    Exemption { path_suffix: "setup_tests.rs", needle: "", reason: "legacy 路径/工件名兼容断言" },
];

fn is_exempt(rel: &str, line: &str) -> Option<&'static str> {
    // 标记行: 清理时显式标注过的说明/契约注释,直接放行。
    if line.contains("zap-purge") {
        return Some("zap-purge 标注行");
    }
    WHITELIST
        .iter()
        .find(|e| rel.ends_with(e.path_suffix) && (e.needle.is_empty() || line.contains(e.needle)))
        .map(|e| e.reason)
}

fn collect_rs_files(root: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(root) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if name == "target" || name == ".git" || name == "node_modules" {
                continue;
            }
            collect_rs_files(&path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
            out.push(path);
        }
    }
}

#[test]
fn no_zap_identity_literals_in_sources() {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    // app crate 的 manifest 目录即 app/,其 src/ 是主域;
    // 仓库根是 manifest 的上两级(app/ 的父目录)。
    let repo_root = manifest
        .parent()
        .and_then(|p| p.parent())
        .expect("repo root")
        .to_path_buf();
    let roots = [manifest.join("src"), repo_root.join("crates")];

    let mut files = Vec::new();
    for root in &roots {
        collect_rs_files(root, &mut files);
    }
    files.sort();

    let mut violations = Vec::new();
    for file in &files {
        let rel = file.strip_prefix(&repo_root).unwrap_or(file).to_string_lossy().to_string();
        let Ok(content) = fs::read_to_string(file) else {
            violations.push(format!("{rel}: <unreadable>"));
            continue;
        };
        for (idx, line) in content.lines().enumerate() {
            if line.to_lowercase().contains("zap") {
                let reason = is_exempt(&rel, line);
                if reason.is_none() {
                    violations.push(format!("{rel}:{}: {}", idx + 1, line.trim()));
                }
            }
        }
    }

    assert!(
        violations.is_empty(),
        "发现未豁免的 zap 身份字面(加入 WHITELIST 前先确认不是该改成 dais 的残留):\n{}",
        violations.join("\n")
    );
}
