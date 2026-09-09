//! # drive_ui — Dais Drive 索引展示类型 (L4 domain-ui, P-ARC R2-a)
//!
//! 自 `app/src/drive/items` 同刀切出 (复刻 R1/objects 模式), 承载与宿主 app
//! 解耦的纯展示面:
//!
//! - `WarpDriveItem` trait 契约 + `WarpDriveItemId`(索引行唯一标识)。
//! - 纯展示 item: `space` / `ai_fact_collection` / `mcp_server_collection` /
//!   `env_var_collection`。
//! - `DriveObjectType`(原 `app::drive` 内联定义, drive 侧 re-export) +
//!   `objects_styling::warp_drive_icon_color`(图标配色)。
//!
//! 只依赖既有 crate 面 (warp_core / warpui / objects / env_vars_ui / i18n),
//! 禁止依赖任何 app 类型 (无 `dais::` / `crate::` 反向引用), 零
//! `ObjectStoreModel` 引用 — store 访问一律由宿主注入。
//!
//! ## 留守 app 的 item (物理依赖约束)
//!
//! - `folder` / `notebook` / `workflow` / `ai_fact` / `mcp_server` 五类 item
//!   持有 app 侧 `*Object`:
//!   `FolderObjectModel::upsert_event` 依 app `persistence::ModelEvent`;
//!   `NotebookObject` / `WorkflowObject`(G2 互斥区) / `AIFactObject` /
//!   `MCPServerObject` 分别在 notebooks / workflows / ai 域, 随其域 crate 化
//!   再迁, 届时在 app 侧 impl 本 crate trait 即可 (trait 对宿主实现开放)。
//!
//! app 侧 `drive::items` / `drive::objects_styling` / `drive::DriveObjectType`
//! 均为到本 crate 的 re-export, 旧路径继续可解析。

pub mod drive_object_type;
pub mod items;
pub mod objects_styling;

pub use drive_object_type::DriveObjectType;
pub use objects_styling::warp_drive_icon_color;

pub use items::{WarpDriveItem, WarpDriveItemId};
