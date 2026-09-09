//! 留守 re-export 层 (P-ARC R2-a): `WarpDriveItem` trait 契约与纯展示 item
//! 已下沉 crates/drive_ui, 本模块保旧路径继续可解析 (复刻 R1 auth::user_uid 模式)。
//!
//! 留守文件 (持有 app 侧 `*Object`, 物理依赖约束, 详见 drive_ui crate 文档):
//! folder / notebook / ai_fact / mcp_server — 在 app 侧 impl
//! drive_ui::WarpDriveItem。

pub use drive_ui::items::{WarpDriveItem, WarpDriveItemId};

pub mod ai_fact;
pub mod ai_fact_collection {
    pub use drive_ui::items::ai_fact_collection::*;
}
pub mod env_var_collection {
    pub use drive_ui::items::env_var_collection::*;
}
pub mod folder;
pub mod mcp_server;
pub mod mcp_server_collection {
    pub use drive_ui::items::mcp_server_collection::*;
}
pub mod notebook;
pub mod space {
    pub use drive_ui::items::space::*;
}
