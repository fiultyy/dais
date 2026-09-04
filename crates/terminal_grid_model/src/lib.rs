//! P-ARC R10: terminal grid 模型层 crate。
//! 原 app/src/terminal/model/grid + secrets/find/image_map/SizeInfo 等叶类型下沉;
//! app 经 `terminal::model::grid::*` 等 re-export shim 保持旧路径全兼容。
//! 事件发送(GridEventSink)与富内容 secret 检测(DetermineSecretLevelFn)为缝,
//! 由 app 装配注入,本 crate 零 app 依赖。

pub mod agent_view;
pub mod ansi;
pub mod banner_types;
pub mod block_events;
pub mod block_host;
pub mod block_filter;
pub mod block_serialized;
pub mod block_heights;
pub mod bootstrap;
pub mod find;
pub mod grid;
pub mod image_map;
pub mod secrets;
pub mod prompt_snapshot;
pub mod selected_blocks;
mod size_info;
pub mod text_location;


pub use size_info::{ClipboardType, SizeInfo, SizeUpdate, SizeUpdateReason};
pub use text_location::TextLocation;
pub use grid::{BlockPadding, BlockSize, GridType};
pub use grid::block_types::WithinBlock;

