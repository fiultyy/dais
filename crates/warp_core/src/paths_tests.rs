use dirs::home_dir;

use super::*;

#[test]
fn test_data_dir_path() {
    let home_dir = home_dir().expect("Should be able to compute home directory");
    // ChannelState, by default, is configured for Channel::Oss.
    cfg_if::cfg_if! {
        if #[cfg(target_os = "macos")] {
            assert_eq!(data_dir(), home_dir.join(".dais"));
        } else if #[cfg(any(target_os = "linux", target_os = "freebsd"))] {
            assert_eq!(data_dir(), home_dir.join(".local/share/dais"));
        } else if #[cfg(windows)] {
            assert_eq!(data_dir(), home_dir.join("AppData\\Roaming\\zap\\Zap\\data")); // zap-purge: legacy Windows path, kept for compat
        } else {
            unimplemented!("Need to update tests for current platform!");
        }
    }
}

#[test]
fn test_config_local_dir_path() {
    let home_dir = home_dir().expect("Should be able to compute home directory");
    // ChannelState, by default, is configured for Channel::Oss.
    cfg_if::cfg_if! {
        if #[cfg(target_os = "macos")] {
            assert_eq!(config_local_dir(), home_dir.join(".dais"));
        } else if #[cfg(any(target_os = "linux", target_os = "freebsd"))] {
            assert_eq!(config_local_dir(), home_dir.join(".config/dais"));
        } else if #[cfg(windows)] {
            assert_eq!(config_local_dir(), home_dir.join("AppData\\Local\\zap\\Zap\\config")); // zap-purge: legacy Windows path, kept for compat
        } else {
            unimplemented!("Need to update tests for current platform!");
        }
    }
}

#[test]
fn test_warp_home_config_dir_path() {
    let home_dir = home_dir().expect("Should be able to compute home directory");
    let expected_dir_name = match ChannelState::data_profile() {
        Some(data_profile) => format!(".dais-{data_profile}"),
        None => ".dais".to_string(),
    };

    assert_eq!(
        warp_home_config_dir(),
        Some(home_dir.join(expected_dir_name))
    );
}

#[test]
fn test_warp_home_skills_and_mcp_paths() {
    let Some(config_dir) = warp_home_config_dir() else {
        panic!("Should be able to compute Dais home config directory");
    };

    assert_eq!(warp_home_skills_dir(), Some(config_dir.join("skills")));
    assert_eq!(
        warp_home_mcp_config_file_path(),
        Some(config_dir.join(".mcp.json"))
    );
}
#[test]
fn test_cache_dir_path() {
    let home_dir = home_dir().expect("Should be able to compute home directory");
    // ChannelState, by default, is configured for Channel::Oss.
    cfg_if::cfg_if! {
        if #[cfg(target_os = "macos")] {
            assert_eq!(cache_dir(), home_dir.join("Library/Application Support/dev.dais.Dais"));
        } else if #[cfg(any(target_os = "linux", target_os = "freebsd"))] {
            assert_eq!(cache_dir(), home_dir.join(".cache/dais"));
        } else if #[cfg(windows)] {
            assert_eq!(cache_dir(), home_dir.join("AppData\\Local\\zap\\Zap\\cache")); // zap-purge: legacy Windows path, kept for compat
        } else {
            unimplemented!("Need to update tests for current platform!");
        }
    }
}

#[test]
fn test_state_dir_path() {
    let home_dir = home_dir().expect("Should be able to compute home directory");
    cfg_if::cfg_if! {
        // ChannelState, by default, is configured for Channel::Oss.
        if #[cfg(target_os = "macos")] {
            assert_eq!(state_dir(), home_dir.join("Library/Application Support/dev.dais.Dais"));
        } else if #[cfg(any(target_os = "linux", target_os = "freebsd"))] {
            assert_eq!(state_dir(), home_dir.join(".local/state/dais"));
        } else if #[cfg(windows)] {
            assert_eq!(state_dir(), home_dir.join("AppData\\Local\\zap\\Zap\\data")); // zap-purge: legacy Windows path, kept for compat
        } else {
            unimplemented!("Need to update tests for current platform!");
        }
    }
}

#[test]
fn test_oss_secure_state_dir_is_disabled() {
    // ChannelState 默认是 Channel::Oss。Dais 不应该探测旧 Zap 官方 App Group,
    // 否则 macOS 会把它识别成访问其他 App 数据并在每次启动时弹权限窗。
    assert_eq!(secure_state_dir(), None);
}

#[test]
fn test_project_path_for_dais_dev_app_id() {
    // Covers the `starts_with("Zap")` compat branch in `project_dirs_for_app_id` on Linux,
    // which maps suffixed legacy application names like `ZapDev` to a dashed lowercase
    // directory matching the Linux package name (e.g. `dais-dev`). // zap-purge: legacy app ID
    let project_dirs = project_dirs_for_app_id(AppId::new("dev", "zap", "ZapDev"), None) // zap-purge: legacy app ID, kept for compat
        .expect("should be able to compute project dirs");
    cfg_if::cfg_if! {
        if #[cfg(target_os = "macos")] {
            assert_eq!(project_dirs.project_path(), "dev.zap.ZapDev"); // zap-purge: legacy app ID, kept for compat
        } else if #[cfg(any(target_os = "linux", target_os = "freebsd"))] {
            assert_eq!(project_dirs.project_path(), "dais-dev");
        } else if #[cfg(windows)] {
            assert_eq!(project_dirs.project_path(), "zap\\ZapDev"); // zap-purge: legacy app ID, kept for compat
        } else {
            unimplemented!("Need to update tests for current platform!");
        }
    }
}

#[test]
fn test_project_path_for_oss_app_id() {
    let project_dirs = project_dirs_for_app_id(AppId::new("dev", "zap", "Zap"), None)  // zap-purge: legacy app ID, kept for compat
        .expect("should be able to compute project dirs");
    cfg_if::cfg_if! {
        if #[cfg(target_os = "macos")] {
            assert_eq!(project_dirs.project_path(), "dev.zap.Zap");  // zap-purge: legacy app ID, kept for compat
        } else if #[cfg(any(target_os = "linux", target_os = "freebsd"))] {
            assert_eq!(project_dirs.project_path(), "dais");
        } else if #[cfg(windows)] {
            assert_eq!(project_dirs.project_path(), "zap\\Zap");  // zap-purge: legacy app ID, kept for compat
        } else {
            unimplemented!("Need to update tests for current platform!");
        }
    }
}

#[test]
fn test_project_path_for_dais_default_app_id() {
    // D5 改名后的 OSS 默认 app_id (`dev.dais.Dais`): Linux 数据根仍归一为
    // `dais`,与旧 `dev.zap.Zap` 落在同一目录,无数据迁移。
    let project_dirs = project_dirs_for_app_id(AppId::new("dev", "dais", "Dais"), None)
        .expect("should be able to compute project dirs");
    cfg_if::cfg_if! {
        if #[cfg(target_os = "macos")] {
            assert_eq!(project_dirs.project_path(), "dev.dais.Dais");
        } else if #[cfg(any(target_os = "linux", target_os = "freebsd"))] {
            assert_eq!(project_dirs.project_path(), "dais");
        } else if #[cfg(windows)] {
            assert_eq!(project_dirs.project_path(), "dais\\Dais");
        } else {
            unimplemented!("Need to update tests for current platform!");
        }
    }
}
