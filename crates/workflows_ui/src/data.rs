//! WorkflowData — workflows 域对宿主对象存储的只读投影(POD 数据桥)。
//!
//! P-ARC R2-b:原直连 `ObjectStoreModel`(app 侧留守层,不能进 crate 依赖),
//! 改经 [`WorkflowData`] trait 注入 — nav 模板 / env_vars_ui::EnvVarData 同款契约。
//! 宿主(app)见 `app/src/workflows/data.rs` 的 HostWorkflowData;sandbox
//! `--ui-preview` 喂 `()` 空实现。
//!
//! POD 契约:trait 方法签名零 app 类型;返回值只承载 objects crate 的
//! [`WorkflowEnumObject`](crate::workflow_enum::WorkflowEnumObject)。

use objects::ids::SyncId;

use crate::workflow_enum::WorkflowEnumObject;

/// 宿主数据契约:workflows 域对本地对象存储的只读投影。
///
/// 实现方 = app(包装 `ObjectStoreModel`)。所有方法要求只读、无副作用,
/// 可在渲染帧内调用。
pub trait WorkflowData {
    /// 按参数引用的 enum id 查询已存储的 workflow enum 对象。
    /// 导出 YAML 时解析 `ArgumentType::Enum` 参数需要(`export_workflow`)。
    fn workflow_enum(&self, enum_id: &SyncId) -> Option<WorkflowEnumObject>;
}

/// [`WorkflowData`] 的空实现(nav::NavDataSource for () / EnvVarData for () 同款):
/// sandbox/测试占位,全部返回"无数据"。
impl WorkflowData for () {
    fn workflow_enum(&self, _enum_id: &SyncId) -> Option<WorkflowEnumObject> {
        None
    }
}

/// test-support feature 暴露的内存版数据桥,供宿主测试直接构造枚举对象
/// (对应 objects crate test-support 暴露 mock helpers 的惯例)。
#[cfg(feature = "test-support")]
pub struct InMemoryWorkflowData {
    enums: Vec<(SyncId, WorkflowEnumObject)>,
}

#[cfg(feature = "test-support")]
impl InMemoryWorkflowData {
    pub fn new() -> Self {
        Self { enums: Vec::new() }
    }

    pub fn with_enum(mut self, enum_id: SyncId, object: WorkflowEnumObject) -> Self {
        self.enums.push((enum_id, object));
        self
    }
}

#[cfg(feature = "test-support")]
impl Default for InMemoryWorkflowData {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(feature = "test-support")]
impl WorkflowData for InMemoryWorkflowData {
    fn workflow_enum(&self, enum_id: &SyncId) -> Option<WorkflowEnumObject> {
        self.enums
            .iter()
            .find(|(id, _)| id == enum_id)
            .map(|(_, object)| object.clone())
    }
}
