#![cfg_attr(target_family = "wasm", allow(dead_code, unused_imports))]

//! editor 域(宿主留守层,P-ARC R6)。
//!
//! 纯类型层(line / line_iterator / scroll)已下沉 `crates/code_model`;
//! 本模块保留视图主体(model / view / element / diff / comments /
//! comment_editor / embedded_comment / nav_bar / find / goto_line),
//! 锚点:`crate::t!` i18n、`crate::appearance`(diff.rs add_color)、
//! `crate::util::link_detection`(model.rs)、`crate::code_review`
//! (comments.rs / embedded_comment.rs / model.rs 反向锚)。
//! 下方 inline shim 保证 `crate::code::editor::{line,line_iterator,scroll}`
//! 旧路径不变(diff.rs / model.rs / code_review::comments 等消费点零改动)。

pub(crate) mod comment_editor;
mod comments;
pub(super) mod diff;
mod element;
pub mod embedded_comment;
pub mod find;
pub mod goto_line;
pub mod model;
mod nav_bar;
pub mod view;

// R6 路径兼容 shim(隐私与原声明一致:line_iterator 原为私有 mod)。
pub mod line {
    pub use code_model::editor::line::*;
}
mod line_iterator {
    pub use code_model::editor::line_iterator::*;
}
pub mod scroll {
    pub use code_model::editor::scroll::*;
}

pub use comment_editor::{CommentEditor, CommentEditorEvent};
pub use comments::EditorCommentsModel;
pub use comments::EditorReviewComment;
pub(crate) use diff::{add_color, remove_color};
pub use element::GutterHoverTarget;
pub use nav_bar::NavBarBehavior;
