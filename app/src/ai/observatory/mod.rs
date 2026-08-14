//! Observatory 面板 — 拦截/编排/观测 GUI 数据与视图模块。
//!
//! 数据源:
//! - blocks/sessions: `harness_blocks.db`（rusqlite 只读直查）
//! - orchestration: orchestration store（cfg(feature="orchestration")）
//!
//! 挂载: 右侧面板第三种内容（`CurrentWorkspaceState.is_observatory_open`）。

pub mod model;
pub mod view;

