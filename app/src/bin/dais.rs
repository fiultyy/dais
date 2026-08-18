// On Windows, we don't want to display a console window when the application is running in release
// builds. See https://doc.rust-lang.org/reference/runtime.html#the-windows_subsystem-attribute.
#![cfg_attr(feature = "release_bundle", windows_subsystem = "windows")]

use anyhow::Result;
use warp_core::{
    channel::{Channel, ChannelConfig, ChannelState},
    features::{FeatureFlag, DEBUG_FLAGS},
    AppId,
};

#[cfg(all(target_os = "windows", feature = "windows_high_performance_gpu_default"))]
#[allow(non_upper_case_globals)]
#[no_mangle]
#[used]
pub static NvOptimusEnablement: u32 = 1;

#[cfg(all(target_os = "windows", feature = "windows_high_performance_gpu_default"))]
#[allow(non_upper_case_globals)]
#[no_mangle]
#[used]
pub static AmdPowerXpressRequestHighPerformance: u32 = 1;

// Zap OSS 构建的入口,简单包一层 warp::run()。
fn main() -> Result<()> {
    // ── "serve" 快路径 ──
    // `dais serve` 启动一个轻量级无头 RPC 服务器，处理
    // send-message / check-messages / status 通过 Unix 域套接字。
    // 无需 GPUI 应用基础设施。metadata 文件由服务器写入；
    // 在退出时（通过 ctrlc 或信号），会进行清理。
    let args: Vec<String> = std::env::args().collect();
    if args.len() >= 2 && args[1] == "serve" {
        return run_serve();
    }

    let mut state = ChannelState::new(
        Channel::Oss,
        ChannelConfig {
            // D5 改名身份收口: app_id 决定 Wayland app_id / X11 WM_CLASS /
            // D-Bus well-known name。Linux 数据目录在 paths.rs 里映射为 `dais`。
            app_id: AppId::new("dev", "dais", "Dais"),
            logfile_name: "dais.log".into(),
            autoupdate_config: None,
            mcp_static_config: None,
        },
    );
    if cfg!(debug_assertions) {
        state = state.with_additional_features(DEBUG_FLAGS);
    }
    // 始终启用 IME marked-text 渲染:winit 的 IME 路径在 macOS / Windows 都支持,
    // 但若不在此处显式开启,Zap 会把 preedit / 输入合成更新整体丢弃,只剩 OS 的候选窗
    // 可见 —— 在 Windows 上对日文 / 中文 / 韩文输入都属于实质性损坏。
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    {
        state = state.with_additional_features(&[FeatureFlag::ImeMarkedText]);
    }
    // OSS 版本没有服务端实验系统下发 feature flag,这里显式启用
    // Warp 正式版通过服务端动态开启的几个核心 UI / agent 功能。
    state = state.with_additional_features(&[
        // 外部 harness 接入(claude-code / codex 等),否则 --harness 参数被拒。
        FeatureFlag::AgentHarness,
        // 新 Agent UI:全屏对话视图、上下文块自动挂载等。
        FeatureFlag::AgentView,
        FeatureFlag::AgentViewBlockContext,
        // 左侧面板 Conversation List:实时列出每个终端会话。
        FeatureFlag::AgentViewConversationListView,
        // Agent prompt chip / toolbar 可编辑。
        FeatureFlag::AgentViewPromptChip,
        FeatureFlag::AgentToolbarEditor,
        // 本地编排平面:P1 CLI + DB store 已接线。
        FeatureFlag::Orchestration,
    ]);
    ChannelState::set(state);

    warp::run()
}


/// Lightweight headless serve mode: starts a runtime RPC server + metadata,
/// handles send-message / check-messages / status via direct DB access
/// (no GPUI needed). Blocks until Ctrl-C.
#[cfg(all(unix, feature = "orchestration"))]
fn run_serve() -> Result<()> {
    use warp::runtime_rpc::{
        self, clear_metadata, RuntimeMetadata,
    };

    // Initialize channel state so paths::state_dir() etc. work.
    let mut state = ChannelState::new(
        Channel::Oss,
        ChannelConfig {
            // 与 GUI 入口保持一致(D5 改名: dev.zap.Zap → dev.dais.Dais)。
            app_id: AppId::new("dev", "dais", "Dais"),
            logfile_name: "dais.log".into(),
            autoupdate_config: None,
            mcp_static_config: None,
        },
    );
    if cfg!(debug_assertions) {
        state = state.with_additional_features(DEBUG_FLAGS);
    }
    state = state.with_additional_features(&[FeatureFlag::Orchestration]);
    ChannelState::set(state);

    // Start the RPC socket server.
    let (socket_path, _handle) = runtime_rpc::spawn_rpc_server("serve")
        .map_err(|e| anyhow::anyhow!("failed to start RPC server: {e}"))?;

    let meta = RuntimeMetadata {
        socket_path: socket_path.to_string_lossy().to_string(),
        pid: std::process::id(),
        mode: "serve".into(),
    };
    runtime_rpc::write_metadata(&meta);
    eprintln!("dais serve: RPC server listening on {}", meta.socket_path);
    eprintln!("dais serve: metadata at {}", runtime_rpc::runtime_metadata_path().display());

    // Block until the process is killed (Ctrl-C / SIGTERM).
    // The default Rust runtime installs SIGINT/SIGTERM handlers that
    // terminate the process; we just need to keep main alive.
    eprintln!("dais serve: running. Press Ctrl-C to stop.");
    loop {
        std::thread::sleep(std::time::Duration::from_secs(3600));
    }
    // Ctrl-C/SIGTERM kills the process immediately, so this is unreachable.
    // Metadata is stale-safe: CLI callers use is_pid_alive() to detect
    // dead serve processes and clean up then.
    #[allow(unreachable_code)]
    {
        clear_metadata();
        Ok(())
    }
}

/// Serve mode fallback: not supported on non-Unix or without orchestration feature.
#[cfg(not(all(unix, feature = "orchestration")))]
fn run_serve() -> Result<()> {
    let msg = {
        #[cfg(not(unix))]
        { "dais serve is only supported on Unix platforms" }
        #[cfg(all(unix, not(feature = "orchestration")))]
        { "dais serve requires the 'orchestration' feature" }
    };
    anyhow::bail!("{msg}")
}

// If we're not using an external plist, embed the following as the Info.plist.
#[cfg(all(not(feature = "extern_plist"), target_os = "macos"))]
embed_plist::embed_info_plist_bytes!(r#"
    <?xml version="1.0" encoding="UTF-8"?>
    <!DOCTYPE plist PUBLIC "-//Apple Computer//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
    <plist version="1.0">
    <dict>
    <key>CFBundleDevelopmentRegion</key>
    <string>English</string>
    <key>CFBundleDisplayName</key>
    <string>Zap</string>
    <key>CFBundleExecutable</key>
    <string>dais</string>
    <key>CFBundleIdentifier</key>
    <string>dev.zap.Zap</string>
    <key>CFBundleInfoDictionaryVersion</key>
    <string>6.0</string>
    <key>CFBundleLocalizations</key>
    <array>
    <string>en</string>
    <string>ja</string>
    <string>zh-CN</string>
    </array>
    <key>CFBundleName</key>
    <string>Zap</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleShortVersionString</key>
    <string>0.1.0</string>
    <key>LSApplicationCategoryType</key>
    <string>public.app-category.developer-tools</string>
    <key>NSHighResolutionCapable</key>
    <true/>
    <key>UIDesignRequiresCompatibility</key>
    <true/>
    <key>CFBundleURLTypes</key>
    <array><dict><key>CFBundleURLName</key><string>Custom App</string><key>CFBundleURLSchemes</key><array><string>zap</string></array></dict></array>
    <key>NSHumanReadableCopyright</key>
    <string>© 2026, Zap</string>
    </dict>
    </plist>
"#.as_bytes());
