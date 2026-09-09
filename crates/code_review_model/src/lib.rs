//! code_review_model — code_review 域纯模型层 crate(P-ARC R6,依 R1-R4
//! 同刀切模式)。
//!
//! 自 `app/src/code_review` 同刀切出的零 app 依赖部分,目前仅:
//! - [`hidden_lines`]:`calculate_hidden_lines`(diff 上下文折叠的隐藏行
//!   区间纯计算,RangeSet 求补集,内联单测随迁)。
//!
//! 只依赖既有 crate 面(ai / rangemap / warp_editor),禁止依赖任何 app
//! 类型(无 `dais::` / `crate::` 反向引用),零 `ObjectStoreModel` 引用,
//! 无 i18n 宏。
//!
//! ## 留守 app 的部分(物理依赖约束,依 R2-a/R2-b/R4 留守层裁决)
//!
//! code_review 域主体留守 `app/src/code_review`,锚点清单:
//! - `diff_state.rs`:`crate::features::FeatureFlag` /
//!   `crate::terminal::local_shell::LocalShellState` / `crate::throttle` /
//!   `crate::util::git` / `super::telemetry_event`;
//! - `comments/` 全家:`crate::ai::agent::{CurrentHead, DiffBase}`、
//!   `crate::code::editor::EditorReviewComment`(comments.rs 留守)、
//!   `super::diff_state::{DiffMode, DiffLineType, DiffStateModel}` 三重锚;
//! - `diff_size_limits.rs` / `file_invalidation_queue.rs`:引用
//!   `super::diff_state` 留守类型;
//! - `telemetry_event.rs`:`crate::server::telemetry::CLIAgentType` /
//!   `crate::view_components::find::FindDirection` / `super::diff_state`;
//! - `code_review_view.rs` / `comment_list_view.rs` / `comment_rendering.rs` /
//!   `git_dialog/*` / `find_model.rs` / `context.rs` /
//!   `git_status_update.rs` / `scroll_preservation.rs` / `editor_state.rs` /
//!   `diff_menu.rs` / `diff_selector.rs`:`crate::t!` i18n、
//!   `crate::code::local_code_editor::LocalCodeEditorView`(留守)、
//!   `crate::appearance`、`crate::ai` 等宿主视图依赖。
//!
//! app 侧 `code_review::hidden_lines` 为到本 crate 的 re-export shim,
//! 旧路径继续可解析。

pub mod hidden_lines;

pub use hidden_lines::calculate_hidden_lines;
