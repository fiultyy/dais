//! R2-b 留守 shim:YAML 导入/导出主体已下沉 `workflows_ui::export_workflow`。
//!
//! `export_serialize` 保持原签名 `(workflow, serializer, app)`,内部经
//! `HostWorkflowData` POD 数据桥注入,域外调用点(如 drive/export.rs)不变。

use serde::ser::Serializer;
use warpui::AppContext;

use super::data::HostWorkflowData;
use super::workflow::Workflow;

pub use workflows_ui::export_workflow::{export_deserialize, export_serialize_with_data};

/// Custom serialize function used to export a workflow to YAML.
///
/// 原逻辑已下沉 `workflows_ui::export_workflow::export_serialize_with_data`;
/// 本包装仅为保留 `&AppContext` 签名并注入宿主数据桥。
pub fn export_serialize<S>(
    workflow: &Workflow,
    serializer: S,
    app: &AppContext,
) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    export_serialize_with_data(workflow, serializer, &HostWorkflowData(app))
}
