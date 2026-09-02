//! R2-b 宿主桥:把 app 的 `ObjectStoreModel` 适配为
//! `workflows_ui::WorkflowData` POD 契约(对应 env_vars 的 HostEnvVarData 模式)。
//!
//! crate 侧(`workflows_ui::export_workflow`)只看到 `&dyn WorkflowData`;
//! app 侧留守 shim(`export_workflow::export_serialize`)构造本桥注入。

use warpui::{AppContext, SingletonEntity};

use crate::objects::model::persistence::ObjectStoreModel;
use objects::ids::SyncId;
use workflows_ui::{WorkflowData, WorkflowEnumObject};

pub(crate) struct HostWorkflowData<'a>(pub &'a AppContext);

impl WorkflowData for HostWorkflowData<'_> {
    fn workflow_enum(&self, enum_id: &SyncId) -> Option<WorkflowEnumObject> {
        ObjectStoreModel::as_ref(self.0)
            .get_workflow_enum(enum_id)
            .cloned()
    }
}
