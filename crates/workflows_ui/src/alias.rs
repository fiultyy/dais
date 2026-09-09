//! WorkflowAlias — 别名的 POD 数据形态(原 `app/src/workflows/aliases.rs` 内联定义)。
//!
//! 设置组 `WorkflowAliases`(define_settings_group! 单例 + ObjectStore 订阅)
//! 依赖 app 侧宿主类型,留在 `app/src/workflows/aliases.rs` 留守层;
//! 本模块只承载可独立下沉的数据结构。

use std::collections::HashMap;

use objects::ids::SyncId;
use settings_value::SettingsValue;
use serde::{Deserialize, Serialize};

#[derive(Debug, PartialEq, Serialize, Deserialize, Clone, schemars::JsonSchema, SettingsValue)]
#[schemars(description = "A shortcut alias for a Dais Drive workflow.")]
pub struct WorkflowAlias {
    #[schemars(description = "The alias text that triggers this workflow.")]
    pub alias: String,
    #[schemars(description = "The identifier of the linked workflow.")]
    pub workflow_id: SyncId,
    #[schemars(description = "Pre-filled argument values for the workflow.")]
    pub arguments: Option<HashMap<String, String>>,
    #[schemars(description = "The identifier of the environment variable collection to use.")]
    pub env_vars: Option<SyncId>,
}
