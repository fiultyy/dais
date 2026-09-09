//! P-ARC R7: workspace 域纯模型层(tab 设置/分裂区域/侧栏 telemetry)。
//!
//! 宿主(app)经 `crate::workspace::{tab_settings, regions}` re-export shim
//! 消费,旧路径全兼容;telemetry 经 vertical_tabs re-export。
//! 留守: view.rs(22.8k)/vertical_tabs.rs/server_file_browser 等视图主体,
//! 深耦 terminal/ai/pane_group,随宿主模块波次下沉。

pub mod regions;
pub mod tab_settings;
pub mod telemetry;
