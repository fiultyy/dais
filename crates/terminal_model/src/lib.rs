//! P-ARC R8: terminal 域模型层(零宿主锚子集)。
//!
//! 迁入: kitty 图像协议 / index 坐标 / completions / iterm_image / rich_content / tmux 控制模式 /
//! ExecutedExecutorCommandEvent(纯数据事件)。
//! 宿主 app/src/terminal/model/mod.rs 与 event.rs re-export shim,
//! 旧路径 crate::terminal::model::{kitty,...} 全兼容。
//!
//! 留守(锚点): grid(18k, ai::blocklist secret_redaction + terminal::event/
//! event_listener/SizeInfo/ClipboardType + block_filter + util)/ansi(锚
//! terminal_model::TmuxInstallationState/completions 兄弟)/selection/
//! header_grid/session(block 系被 crate::ai 强锚)。详见 R8 上报。

use serde::Serialize;
use std::str::FromStr;

pub mod completions;
pub mod executor_event;
pub mod index;
pub mod iterm_image;
pub mod kitty;
pub mod rich_content;
pub mod tmux;

pub use executor_event::{ExecutedExecutorCommandEvent, ParseGeneratorOutputError};

/// R10: 自 app terminal_model.rs 提升的范围 trait(grid/terminal_grid_model 共用)。
pub trait RangeInModel {
    fn range(&self) -> std::ops::RangeInclusive<crate::index::Point>;
}

/// R11: 自 app terminal_model.rs 提升(app 旧路径 re-export 兼容)。
#[derive(Debug, Default, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TmuxInstallationState {
    /// This means tmux was installed by Dais in this session, successfully or unsuccessfully.
    /// It also means we had root access and used a package manager to install tmux and all
    /// dependencies.
    InstalledByWarpRootInThisSession,
    /// This means tmux was installed by Dais in this session, successfully or unsuccessfully.
    InstalledByWarpInThisSession,
    InstalledByWarpInPriorSession,
    /// This means that warp did not install it locally. It was either installed by the user
    /// or it was installed by warp in a prior session using the package manager.
    InstalledByUser,
    /// This means we never tried to install tmux in this session.
    #[default]
    NotInstalled,
}

impl FromStr for TmuxInstallationState {
    type Err = anyhow::Error;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        match input {
            "installed_by_warp_root_in_this_session" => {
                Ok(TmuxInstallationState::InstalledByWarpRootInThisSession)
            }
            "installed_by_warp_in_this_session" => {
                Ok(TmuxInstallationState::InstalledByWarpInThisSession)
            }
            "warp" | "installed_by_warp_in_prior_session" => {
                Ok(TmuxInstallationState::InstalledByWarpInPriorSession)
            }
            "user" | "installed_by_user" => Ok(TmuxInstallationState::InstalledByUser),
            "not_installed" => Ok(TmuxInstallationState::NotInstalled),
            _ => Err(anyhow::anyhow!("Invalid TmuxInstallationState")),
        }
    }
}

