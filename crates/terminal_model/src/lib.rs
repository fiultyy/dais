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

pub mod completions;
pub mod executor_event;
pub mod index;
pub mod iterm_image;
pub mod kitty;
pub mod rich_content;
pub mod tmux;

pub use executor_event::{ExecutedExecutorCommandEvent, ParseGeneratorOutputError};
