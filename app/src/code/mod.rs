//! Code pane 域(宿主留守层)。
//!
//! # P-ARC R6 后的本模块
//!
//! 纯模型/纯函数层已下沉 `crates/code_model`(buffer_location /
//! global_buffer_model / file_tree::snapshot / icon / active_file /
//! opened_files / editor::{line,line_iterator,scroll} 及根级胶水类型
//! DiffResult / ImmediateSaveError / SaveStatus / SaveOutcome /
//! ShowCommentEditorProvider / EditorTabBarDropTargetData)。
//!
//! 本模块保留(物理依赖约束,依 R2-a/R2-b/R4 留守层裁决):
//! - `view.rs` / `wasm.rs`:`crate::pane_group`(R3 主线互斥区)、
//!   `crate::terminal`、`crate::workspace` 等宿主视图依赖;
//! - `local_code_editor.rs` / `local_code_editor_wasm.rs`:持有
//!   `crate::terminal::TerminalView` / `crate::editor`,随 terminal 域
//!   crate 化再迁;
//! - `diff_viewer.rs` / `inline_diff.rs`:`crate::editor::InteractionState`、
//!   `crate::ai::blocklist` 宿主锚;
//! - `editor/` 视图主体(model / view / element / diff / comments /
//!   comment_editor 等):`crate::t!` i18n、`crate::appearance`、
//!   `crate::code_review` 反向锚(comments.rs 的 EditorReviewComment 被
//!   code_review::comments 消费,双侧留守);
//! - `file_tree/view.rs` 全家、`footer.rs`、`editor_management.rs`:
//!   dispatch 宿主 Action / `crate::settings` / `crate::ui_components`;
//! - 域 `init()`:宿主 view/file_tree 装配;
//! - 路径兼容 shim(`buffer_location` / `global_buffer_model` / `icon` /
//!   `active_file` / `opened_files` 模块 + 根级类型 re-export),保证
//!   `crate::code::*` 全部既有路径不变(workspace/view.rs、
//!   vertical_tabs.rs、global_search、search/files、remote_server、
//!   ai/blocklist、lib.rs 等域外消费点零改动)。

use warpui::AppContext;

pub mod diff_viewer;
pub mod editor;
pub mod editor_management;
pub mod inline_diff;
#[cfg_attr(not(target_family = "wasm"), path = "local_code_editor.rs")]
#[cfg_attr(target_family = "wasm", path = "local_code_editor_wasm.rs")]
pub mod local_code_editor;

pub mod file_tree;
pub mod footer;



#[cfg_attr(not(target_family = "wasm"), path = "view.rs")]
#[cfg_attr(target_family = "wasm", path = "wasm.rs")]
pub mod view;

pub fn init(app: &mut AppContext) {
    self::view::init(app);
    self::file_tree::init(app);
}

// R6 路径兼容 shim:主体已下沉 crates/code_model,保持
// `crate::code::{buffer_location,global_buffer_model,icon,active_file,
// opened_files}::*` 旧路径不变。
pub mod buffer_location {
    pub use code_model::buffer_location::*;
}
pub mod global_buffer_model {
    pub use code_model::global_buffer_model::*;
}
mod icon {
    pub use code_model::icon::*;
}
pub mod active_file {
    pub use code_model::active_file::*;
}
pub mod opened_files {
    pub use code_model::opened_files::*;
}

// R6 根级类型 shim:原 mod.rs 顶层的纯胶水类型已 verbatim 迁
// crates/code_model/src/lib.rs(唯二可见性放宽见彼处注释)。
pub use code_model::{
    DiffResult, EditorTabBarDropTargetData, ImmediateSaveError, NoopCommentEditorProvider,
    SaveOutcome, SaveStatus, ShowCommentEditorProvider,
};
pub use icon::icon_from_file_path;
