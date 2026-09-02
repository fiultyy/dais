//! code_model — code 域纯模型层 crate(P-ARC R6,依 R1-R4 同刀切模式)。
//!
//! 自 `app/src/code` 同刀切出的零 app 依赖部分:
//! - [`buffer_location`]:`RemotePath` / `BufferLocation` / `SyncClock`
//!   (warp_core HostId + warp_util StandardizedPath 的远端路径语义);
//! - [`global_buffer_model`]:`GlobalBufferModel`(1541 行 buffer 生命周期中枢,
//!   FileModel/SaveBuffer 双路径,local_fs/local_tty feature 门控随迁);
//! - [`file_tree::snapshot`]:`FileEntry` / `FileTreeSnapshot` sum_tree 快照模型
//!   (+ iterator / snapshot_tests 经 `#[path]` 随迁);
//! - [`icon`]:`icon_from_file_path` 纯函数(扩展名→图标);
//! - [`active_file`] / [`opened_files`]:`ActiveFileModel` / `OpenedFilesModel`;
//! - [`editor::line`] / [`editor::line_iterator`] / [`editor::scroll`]:
//!   `EditorLineLocation` / `LineIterator` / `ScrollWheelBehavior` /
//!   `ScrollPosition` / `ScrollTrigger` 纯类型(视图主体留守 app);
//! - 根级胶水类型(原 `app/src/code/mod.rs` 顶层):`DiffResult`、
//!   `ImmediateSaveError`、`SaveStatus`、`SaveOutcome`、
//!   `ShowCommentEditorProvider`(+ `NoopCommentEditorProvider`)、
//!   `EditorTabBarDropTargetData`。
//!
//! 只依赖既有 crate 面(warpui / warp_editor / warp_core / warp_util /
//! warp_files / sum_tree / ai 系以外的纯库),禁止依赖任何 app 类型
//! (无 `dais::` / `crate::` 反向引用),零 `ObjectStoreModel` 引用。
//!
//! ## 留守 app 的部分(物理依赖约束,依 R2-a/R2-b/R4 留守层裁决)
//!
//! - `app/src/code/mod.rs`:域 `init()`(宿主 view/file_tree 装配)与全部
//!   模块声明;纯类型经 re-export shim 保持 `crate::code::*` 旧路径不变;
//! - `editor/{view,element,model,comments,comment_editor,embedded_comment,
//!   nav_bar,find,goto_line,diff}.rs`:`crate::t!` i18n、`crate::appearance`、
//!   `crate::view_components`、`crate::code_review`(R6-b 域,telemetry/
//!   comments 反向锚)等宿主视图依赖(model.rs 另被
//!   `crate::util::link_detection` 锚定,diff.rs 被 `crate::appearance`
//!   `Appearance` 锚定);
//! - `file_tree/view.rs` 全家:dispatch 宿主 `FileTreeAction` /
//!   `crate::ui_components`;
//! - `view.rs` / `wasm.rs` / `local_code_editor*.rs` / `inline_diff.rs` /
//!   `diff_viewer.rs` / `footer.rs` / `editor_management.rs`:warpui 视图、
//!   `crate::pane_group`(R3 主线互斥区)、`crate::terminal`、
//!   `crate::settings`、`crate::ai` 宿主依赖。
//!
//! app 侧 `code::{buffer_location,global_buffer_model,icon,active_file,
//! opened_files}`、`code::editor::{line,line_iterator,scroll}`、
//! `code::file_tree::snapshot` 均为到本 crate 的 re-export shim,
//! 旧路径继续可解析。

pub mod active_file;
pub mod buffer_location;
pub mod editor;
pub mod file_tree;
pub mod global_buffer_model;
pub mod icon;
pub mod opened_files;

pub use active_file::{ActiveFileEvent, ActiveFileModel};
pub use buffer_location::{BufferLocation, RemotePath, SyncClock};
pub use global_buffer_model::GlobalBufferModel;
pub use icon::icon_from_file_path;
pub use opened_files::{OpenedFilesInRepo, OpenedFilesModel};

use pathfinder_geometry::rect::RectF;
use std::any::Any;
use std::fmt::Debug;
use std::ops::AddAssign;
use warp_util::file::FileSaveError;
use warpui::elements::DropTargetData;
use warpui::AppContext;

// ↓↓↓ 以下根级类型自 app/src/code/mod.rs verbatim 迁入(R6)。
// 唯二改动(可见性放宽,跨 crate 消费必要,对齐 R4 sftp_ops 先例):
// - NoopCommentEditorProvider:私有 → pub(app code/editor/view.rs 构造);
// - EditorTabBarDropTargetData.index:私有字段 → pub(同上 :1924 构造)。

#[derive(Debug, thiserror::Error)]
#[cfg_attr(target_family = "wasm", allow(dead_code))]
pub enum ImmediateSaveError {
    #[error("No FileId")]
    NoFileId,
    #[error("failed to save file: {0:#}")]
    FailedToSave(#[from] FileSaveError),
    #[error("There is no file tab currently selected")]
    NoActiveFileTab,
}

/// Trait to determine whether we should show the comment editor based on state held
/// by the parent of the [`CodeEditorView`].
pub trait ShowCommentEditorProvider: Debug + 'static {
    /// Returns whether the comment editor should be shown given the location of the line where
    /// the editor would be shown.
    #[cfg_attr(not(feature = "local_fs"), allow(dead_code))]
    fn should_show_comment_editor(&self, editor_line_location: RectF, app: &AppContext) -> bool;
}

#[derive(Debug)]
pub struct NoopCommentEditorProvider;

impl ShowCommentEditorProvider for NoopCommentEditorProvider {
    fn should_show_comment_editor(&self, _editor_line_location: RectF, _app: &AppContext) -> bool {
        false
    }
}

#[cfg_attr(target_family = "wasm", expect(dead_code))]
#[derive(Debug)]
pub enum SaveStatus {
    /// Save completed immediately and successfully.
    SavedImmediately,
    /// Save operation is in progress asynchronously (e.g., save-as dialog).
    AsyncSaveInProgress,
    /// Save failed with an error.
    Failed(#[allow(unused)] ImmediateSaveError),
}

#[derive(Debug, Eq, PartialEq)]
#[cfg_attr(target_family = "wasm", expect(dead_code))]
pub enum SaveOutcome {
    Canceled,
    Failed,
    Succeeded,
}

/// The diff that results from editing a file.
#[derive(Debug, Default, Clone)]
pub struct DiffResult {
    /// The changes in unified diff format.
    pub unified_diff: String,
    /// Number of lines added.
    pub lines_added: usize,
    /// Number of lines removed.
    pub lines_removed: usize,
}

impl AddAssign<&DiffResult> for DiffResult {
    fn add_assign(&mut self, other: &DiffResult) {
        self.lines_added += other.lines_added;
        self.lines_removed += other.lines_removed;

        // There's no standardized multi-file diff format, but concatenating the diffs is enough
        // for our needs: https://en.wikipedia.org/wiki/Diff#Extensions
        if !self.unified_diff.is_empty() && !other.unified_diff.is_empty() {
            self.unified_diff.push('\n');
        }
        self.unified_diff.push_str(&other.unified_diff);
    }
}

#[derive(Debug)]
#[cfg_attr(target_family = "wasm", expect(dead_code))]
pub struct EditorTabBarDropTargetData {
    pub index: usize,
}

impl DropTargetData for EditorTabBarDropTargetData {
    fn as_any(&self) -> &dyn Any {
        self
    }
}
