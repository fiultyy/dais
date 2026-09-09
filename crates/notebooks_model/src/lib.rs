//! notebooks_model — notebooks 域纯模型 crate(依 workflows_ui/objects 迁移模式)。
//!
//! 自 `app/src/notebooks` 物理迁出的纯模型层:
//! - [`telemetry`]:NotebookTelemetryAction 等 serde telemetry 枚举;
//! - [`interaction_state_model`]:InteractionStateModel(warpui Entity 纯模型,
//!   视图层留在 app);
//! - [`InteractionState`]:编辑器交互态纯枚举(原 app/src/editor/view/model,
//!   经 app 侧 `pub use` 保持 `crate::editor::InteractionState` 兼容);
//! - [`BlockInsertionSource`]:块插入菜单触发来源纯枚举
//!   (原 app/src/notebooks/editor/block_insertion_menu,app 侧 re-export 兼容)。
//!
//! 视图/宿主单例(menu 渲染、EditorView)因物理依赖留在 app。

use serde::{Deserialize, Serialize};
pub mod interaction_state_model;
pub mod telemetry;

pub use interaction_state_model::{InteractionStateModel, InteractionStateModelEvent};

/// Enum representing editor responses to edit and selection actions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InteractionState {
    /// Editor will respond to both edits and selection actions; cursor shown.
    Editable,
    /// Editor was set in an editable state, but has selections that cannot be edited.
    EditableWithInvalidSelection,
    /// Editor will respond to selection actions, and ignore edit actions. No cursor is shown.
    Selectable,
    /// Editor will ignore selection and edit actions. Editor is rendered with dimmed text and no cursor.
    Disabled,
}

/// Where the block insertion menu was triggered from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum BlockInsertionSource {
    AtCursor,
    BlockInsertionButton,
}
