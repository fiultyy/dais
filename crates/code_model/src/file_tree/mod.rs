//! file_tree 快照模型层。
//!
//! 视图主体(view.rs 全家)因 dispatch 宿主 `FileTreeAction` /
//! `crate::ui_components` 依赖留守 `app/src/code/file_tree`
//! (P-ARC R6 留守边界,详见 crate lib.rs 头注释)。

pub mod snapshot;
