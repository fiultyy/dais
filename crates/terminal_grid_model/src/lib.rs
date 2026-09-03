//! P-ARC R10: terminal grid 模型层 crate。
//! 原 app/src/terminal/model/grid + secrets/find/image_map/SizeInfo 等叶类型下沉;
//! app 经 `terminal::model::grid::*` 等 re-export shim 保持旧路径全兼容。
//! 事件发送(GridEventSink)与富内容 secret 检测(DetermineSecretLevelFn)为缝,
//! 由 app 装配注入,本 crate 零 app 依赖。

pub mod ansi;
pub mod bootstrap;
pub mod find;
pub mod grid;
pub mod image_map;
pub mod secrets;
mod size_info;

pub use size_info::{ClipboardType, SizeInfo, SizeUpdate, SizeUpdateReason};

