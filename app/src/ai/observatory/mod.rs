//! Observatory 面板 — 拦截/编排/观测 GUI 数据与视图模块。
//!
//! 数据源:
//! - blocks/sessions: `harness_blocks.db`（rusqlite 只读直查）
//! - orchestration: orchestration store（cfg(feature="orchestration")）
//!
//! 挂载: 独立 tab pane（工具条眼睛按钮 → `Workspace::toggle_observatory`）。

pub mod context_usage;
pub mod format;
pub mod model;
pub mod row;
pub mod view;
