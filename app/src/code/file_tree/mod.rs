//! File picker component for rendering expandable folder structures.
//!
//! P-ARC R6:快照模型(snapshot / snapshot/iterator / snapshot_tests)已下沉
//! `crates/code_model`;视图主体(view.rs 全家)因 dispatch 宿主
//! `FileTreeAction` / `crate::ui_components` 依赖留守本模块。

pub mod snapshot {
    pub use code_model::file_tree::snapshot::*;
}

#[cfg_attr(not(feature = "local_fs"), allow(dead_code, unused_imports))]
mod view;

pub use view::*;
