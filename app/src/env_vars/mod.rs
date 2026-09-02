//! env_vars 域 app 残根(P-ARC R1 低危子集)。
//!
//! 数据模型与状态机已下沉 `env_vars_ui` crate:
//! - 模型符号自 `crate::env_vars::` re-export(`env_vars_ui::model`),维持
//!   terminal/search/ai/drive/workspace 等既有调用点路径不变;
//! - 视图主体(G3)本轮留 app:`EnvVarCollectionView` 硬依赖 app::editor
//!   (EditorView 41.9k,scout 判定为环依赖死结,待 editor 视图层先行下沉)、
//!   pane_group(BackingView/PaneConfiguration/PaneEvent)、menu、
//!   search::external_secrets、view_components 等 app 模块。
//!
//! command_dialog 的 `EnvVarSecretCommand` 定义已上移 crate,本模块
//! re-export 保持 `crate::env_vars::view::command_dialog::EnvVarSecretCommand`
//! 路径(requested_command_attribution_test 消费)。

pub mod env_var_collection_block;
pub mod manager;
pub mod view;

/// 历史路径兼容(原 app::env_vars::command_dialog 等,现居 view:: 下)。
pub mod command_dialog {
    pub use crate::env_vars::view::command_dialog::*;
}
pub use view::{
    editors, env_var_collection, fixed_view_components, menus, secrets, unsaved_changes_dialog,
};

pub use env_vars_ui::data::{
    ActiveEnvVarCollection, ActiveEnvVarCollectionData, ActiveEnvVarCollectionDataEvent,
    SavingStatus,
};
pub mod active_env_var_collection_data {
    pub use env_vars_ui::data::{
        ActiveEnvVarCollection, ActiveEnvVarCollectionData, ActiveEnvVarCollectionDataEvent,
        SavingStatus, TrashStatus,
    };
}

pub use env_vars_ui::{
    EnvVar, EnvVarCollection, EnvVarCollectionObject, EnvVarCollectionObjectModel,
    EnvVarCollectionType, EnvVarValue, serialize_variables_for_shell,
};
