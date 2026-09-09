//! # P-ARC R2-b 后的 app 侧 workflows 模块(留守层)
//!
//! 纯模型/类型层已下沉 `crates/workflows_ui`(Workflow/Argument/ArgumentType、
//! WorkflowEnum 体系、command_parser、WorkflowAlias、export_workflow POD 版、
//! WorkflowId/WorkflowSelectionSource/AIWorkflowOrigin)。
//! 本模块保留:
//! - `WorkflowObjectModel` / `WorkflowObject`:`StoredObjectModel::upsert_event`
//!   构造 `app::persistence::ModelEvent`(sqlite 事件流保持 ModelEvent 直传,
//!   与 Folder/Notebook 的 `UpsertEvent::App` 用法一致),随 store 进 crate 会成
//!   反向依赖;
//! - `WorkflowType` / `WorkflowSource`:引用 WorkflowObject 与 notebooks 域类型
//!   (NotebookId/NotebookLocation);
//! - `WorkflowViewMode`:`supported_edit_mode/supported_view_mode` 依赖
//!   ObjectStoreViewModel + FeatureFlag,且被 workflows 域外以关联函数语法调用
//!   (pane_group/pane/workflow_pane.rs),下沉将强制散改域外调用点;
//! - 各视图与宿主单例(categories、workflow_view、info_box、manager、
//!   local_workflows、aliases 设置组);
//! - 路径兼容 shim(`workflow`/`workflow_enum`/`command_parser` 模块)与
//!   `export_workflow::export_serialize` 同签名包装,保证 `crate::workflows::*`
//!   全部既有路径不变。

use objects::UpsertEvent;
use crate::objects_display::WarpDriveItemOverrides;
use serde::{Deserialize, Serialize};
use warp_core::context_flag::ContextFlag;
use warp_core::features::FeatureFlag;
use warpui::{AppContext, SingletonEntity};

pub mod categories;
use workflow::Workflow;

pub mod aliases;
pub mod export_workflow;
pub mod info_box;
pub mod local_workflows;
pub mod manager;
pub mod workflow_view;

// R2-b 路径兼容 shim:主体已下沉 crates/workflows_ui,保持
// `crate::workflows::{workflow,workflow_enum,command_parser}::*` 旧路径不变。
pub mod workflow {
    pub use workflows_ui::workflow::*;
}
pub mod workflow_enum {
    pub use workflows_ui::workflow_enum::*;
}
pub mod command_parser {
    pub use workflows_ui::command_parser::*;
}

// R2-b 宿主桥:ObjectStoreModel → workflows_ui::WorkflowData POD 契约。
mod data;

use crate::appearance::Appearance;
use crate::objects::model::view::ObjectStoreViewModel;
use objects::{GenericStoredObject, ObjectType, StoredObjectModel};

use objects::SerializedModel;
use crate::drive::items::workflow::WarpDriveWorkflow;
use crate::drive::items::WarpDriveItem;
use crate::drive::ObjectTypeAndId;
use crate::notebooks::{NotebookId, NotebookLocation};
use crate::persistence::ModelEvent;
use objects::ids::{ServerId, SyncId};
pub use categories::{CategoriesView, CategoriesViewEvent, WorkflowsViewAction};

// 下沉类型的根路径再导出(原定义于本模块,现移 crates/workflows_ui)。
pub use workflows_ui::{AIWorkflowOrigin, WorkflowId, WorkflowSelectionSource};

pub fn init(app: &mut AppContext) {
    categories::init(app);
    self::workflow_view::init(app);
}

#[derive(Copy, Clone, Debug, Deserialize, Serialize, Eq, PartialEq, Hash)]
pub enum WorkflowSource {
    Global,
    Local,
    Project,
    Team {
        team_uid: ServerId,
    },
    PersonalCloud,
    WarpAI,
    Notebook {
        notebook_id: Option<NotebookId>,
        team_uid: Option<ServerId>,
        location: NotebookLocation,
    },

    /// A hardcoded workflow type that allows Dais to surface features as Workflows (e.g.
    /// a command to see our network log)
    App,
}

#[derive(Debug, Clone, Copy)]
pub enum WorkflowViewMode {
    View,
    Edit,
    Create,
}

impl WorkflowViewMode {
    /// The editing mode supported for a workflow.
    ///
    /// Editing is disabled if the user does not have edit permissions.
    pub fn supported_edit_mode(workflow_id: Option<SyncId>, app: &AppContext) -> Self {
        let can_edit = workflow_id
            .map(|id| {
                ObjectStoreViewModel::as_ref(app)
                    .object_editability(&id.uid(), app)
                    .can_edit()
            })
            .unwrap_or(true);

        if !FeatureFlag::SharedWithMe.is_enabled() || can_edit {
            Self::Edit
        } else {
            Self::View
        }
    }

    /// The viewing mode supported for this workflow.
    ///
    /// Viewing is disabled if the user is allowed to edit the workflow and in a context where
    /// running workflows is supported.
    pub fn supported_view_mode(workflow_id: Option<SyncId>, app: &AppContext) -> Self {
        let can_edit = workflow_id
            .map(|id| {
                ObjectStoreViewModel::as_ref(app)
                    .object_editability(&id.uid(), app)
                    .can_edit()
            })
            .unwrap_or(true);

        if FeatureFlag::SharedWithMe.is_enabled() && !can_edit {
            Self::View
        } else if ContextFlag::RunWorkflow.is_enabled() {
            Self::Edit
        } else {
            Self::View
        }
    }

    fn is_editable(&self) -> bool {
        match self {
            Self::View => false,
            Self::Edit | Self::Create => true,
        }
    }
}

/// Wrapper type for a workflow that may be saved locally or in the object store.
#[derive(Clone, Debug, PartialEq)]
pub enum WorkflowType {
    /// Saved workflows sourced from local, global, project, app collections, saved locally.
    Local(Workflow),
    /// Saved workflows from the local object store.
    Cloud(Box<WorkflowObject>),
    /// Ephemeral/transient workflows created from Dais AI output
    AIGenerated {
        workflow: Workflow,
        origin: AIWorkflowOrigin,
    },
    /// A workflow that's part of a cloud notebook.
    Notebook(Workflow),
}

impl WorkflowType {
    pub fn as_workflow(&self) -> &Workflow {
        match self {
            WorkflowType::Local(workflow) => workflow,
            WorkflowType::AIGenerated { workflow, .. } => workflow,
            WorkflowType::Cloud(workflow) => &workflow.model().data,
            WorkflowType::Notebook(workflow) => workflow,
        }
    }

    /// Returns the contained [`Workflow`], consuming `self`.
    pub fn take_workflow(self) -> Workflow {
        match self {
            WorkflowType::Local(workflow) => workflow,
            WorkflowType::AIGenerated { workflow, .. } => workflow,
            WorkflowType::Cloud(workflow) => workflow.model().data.clone(),
            WorkflowType::Notebook(workflow) => workflow,
        }
    }

    /// The object type and ID for the object containing this workflow, if there is
    /// one. This is currently only supported for object-backed workflows, not workflows within notebooks.
    pub fn object_id(&self) -> Option<ObjectTypeAndId> {
        match self {
            WorkflowType::Cloud(workflow) => Some(ObjectTypeAndId::Workflow(workflow.id)),
            _ => None,
        }
    }

    pub fn sync_id(&self) -> Option<SyncId> {
        match self {
            WorkflowType::Cloud(workflow) => Some(workflow.id),
            _ => None,
        }
    }

    pub fn server_id(&self) -> Option<WorkflowId> {
        match self.object_id() {
            Some(ObjectTypeAndId::Workflow(id)) => id.into_server().map(Into::into),
            _ => None,
        }
    }

    /// We don't show env var selection for Agent Mode suggested commands.
    pub(super) fn should_show_env_var_selection(&self) -> bool {
        !matches!(self, WorkflowType::AIGenerated { .. },)
    }
}

/// The model for a `WorkflowObject`.
#[derive(Clone, Debug, PartialEq)]
pub struct WorkflowObjectModel {
    pub data: Workflow,
}

impl WorkflowObjectModel {
    pub fn new(workflow: Workflow) -> Self {
        Self { data: workflow }
    }
}

/// `WorkflowObject` is an object-store backed workflow.
pub type WorkflowObject = GenericStoredObject<WorkflowId, WorkflowObjectModel>;

impl StoredObjectModel for WorkflowObjectModel {
    type StoredObjectType = WorkflowObject;
    type IdType = WorkflowId;

    fn model_type_name(&self) -> &'static str {
        if self.data.is_agent_mode_workflow() {
            "Prompt"
        } else {
            "Workflow"
        }
    }

    fn object_type(&self) -> ObjectType {
        ObjectType::Workflow
    }

    fn object_type_and_id(&self, id: SyncId) -> ObjectTypeAndId {
        ObjectTypeAndId::Workflow(id)
    }

    fn display_name(&self) -> String {
        self.data.name().to_string()
    }

    fn set_display_name(&mut self, name: &str) {
        self.data.set_name(name);
    }

    fn upsert_event(&self, workflow: &WorkflowObject) -> UpsertEvent {
        UpsertEvent::App(Box::new(ModelEvent::UpsertWorkflow {
            workflow: workflow.clone(),
        }))
    }

    fn bulk_upsert_event(objects: &[WorkflowObject]) -> UpsertEvent {
        UpsertEvent::App(Box::new(ModelEvent::UpsertWorkflows(objects.to_vec())))
    }



    fn serialized(&self) -> SerializedModel {
        SerializedModel::new(
            serde_json::to_string(&self.data).expect("failed to serialize workflow"),
        )
    }



    fn can_export(&self) -> bool {
        true
    }
}


#[cfg(test)]
#[path = "mod_test.rs"]
mod tests;

impl WarpDriveItemOverrides for WorkflowObjectModel {

    fn renders_in_warp_drive(&self) -> bool {
        true
    }

}

impl WorkflowObjectModel {
    fn to_warp_drive_item(
        &self,
        id: objects::SyncId,
        _appearance: &Appearance,
        object: &WorkflowObject,
    ) -> Option<Box<dyn WarpDriveItem>> {
        Some(Box::new(WarpDriveWorkflow::new(
            objects::ObjectTypeAndId::Workflow(id),
            object.clone(),
        )))
    }
}
