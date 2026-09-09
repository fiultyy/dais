//! workflows_ui — workflows 域数据层 crate(P-ARC R2-b,依 R1 objects/env_vars_ui 模式)。
//!
//! 可独立下沉的纯模型/类型自 `app/src/workflows` 同刀切出:
//! - [`workflow`]:Workflow / Argument / ArgumentType(serde 模型 + 纯查询方法);
//! - [`workflow_enum`]:WorkflowEnum / EnumVariants / WorkflowEnumObject(Model)
//!   及其 StringModel / JsonModel impl(objects crate 类型面);
//! - [`command_parser`]:WorkflowDisplayData / WorkflowCommandDisplayData 等
//!   纯计算层(输入编辑器与 info box 的渲染数据);
//! - [`alias`]:WorkflowAlias(设置持久化的 POD 数据形态);
//! - [`export_workflow`]:YAML 导入/导出的 serde 骨架,对象存储访问经
//!   [`data::WorkflowData`] trait 注入(POD 数据桥)。
//!
//! ## 留守 app 的部分(物理依赖约束,对应 R1 objects 留守层)
//!
//! - `WorkflowObjectModel` / `WorkflowObject`:`StoredObjectModel::upsert_event`
//!   构造 `app::persistence::ModelEvent`(经 `UpsertEvent::App` 逃生门),
//!   与 Folder/Notebook 一致留在 app;
//! - `WorkflowType` / `WorkflowSource`:引用 WorkflowObject 与 notebooks 域类型;
//! - `WorkflowViewMode`:`supported_edit_mode/supported_view_mode` 依赖
//!   ObjectStoreViewModel + FeatureFlag,且被 workflows 域外以关联函数语法调用
//!   (pane_group/pane/workflow_pane.rs),下沉会强制散改调用点;
//! - `WorkflowAliases` 设置组、categories/workflow_view/info_box/manager/
//!   local_workflows 等视图与宿主单例。
//!
//! ## 宿主契约(与 objects crate 的 "宿主契约" 同款)
//!
//! 本 crate 只 import objects 等下层 crate 的类型面;对 ObjectStoreModel 的访问
//! 通过 [`data::WorkflowData`] 由宿主(app,见 `app/src/workflows/data.rs` 的
//! HostWorkflowData)注入,不直接依赖 app;sandbox `--ui-preview` 可用 `()` 空实现。

pub mod alias;
pub mod command_parser;
pub mod data;
pub mod export_workflow;
pub mod workflow;
pub mod workflow_enum;

pub use alias::WorkflowAlias;
pub use data::WorkflowData;
pub use workflow_enum::{EnumVariants, WorkflowEnum, WorkflowEnumObject, WorkflowEnumObjectModel};

use objects::ids::ServerId;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct WorkflowId(ServerId);
objects::server_id_traits! { WorkflowId, "Workflow" }

#[derive(Copy, Clone, Debug, Deserialize, Serialize, Eq, PartialEq, Hash, PartialOrd)]
pub enum WorkflowSelectionSource {
    DaisDrive,
    CommandPalette,
    UniversalSearch,
    Voltron,
    WarpAI,
    Notebook,
    SlashMenu,
    UpArrowHistory,
    WorkflowView,
    AgentMode,
    Undefined,
    Alias,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AIWorkflowOrigin {
    CommandSearch,
    AgentMode,
    LegacyWarpAI,
}
