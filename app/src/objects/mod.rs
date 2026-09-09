//! # Dais 本地化说明(Phase 2d-4b,2026-05-11 修订)
//!
//! 本模块在上游 Dais 中承担 "云对象" 抽象,统一描述 Notebook / Workflow / EnvVar /
//! Fact / MCP / ExecutionProfile / AIDocument 等需要在多设备间同步的对象类型。
//!
//! 在 Dais 中云端同步链路(RTC / ObjectStore / SyncQueue / ServerApiProvider)
//! 已被剥离(详见 `docs/zap-cloud-removal-plan.md`),本模块**变为纯本地对象抽象**:
//!
//! - `StoredObject` trait → 实际语义是 "本地领域对象 trait",承载 metadata / permissions /
//!   versions / display_name / upsert_event / as_any / clone_box;命名上的 `Cloud` 前缀
//!   仅为减少跨上游 cherry-pick 的 diff 面而保留,不再具有任何云端含义。
//! - `GenericStoredObject<K, M>` → 本地领域对象的泛型承载结构。
//! - `StoredObjectModel` trait → 本地对象类型描述。
//! - `ObjectStoreModel`(`model/persistence.rs`)→ 进程内本地对象全局存储 + SQLite 背存。
//! - `ObjectStoreMutationEvent` → 变更结果事件(ObjectStore 发出),被 workspace 等订阅。
//! - `ObjectTypeAndId` → 本地 ID 判别式,被 Drive UI / search 等 60+ 处使用。
//!
//! 之所以采用 "保留原名 + 文档注释" 而非物理重命名(`StoredObject` → `LocalObject`),
//! 是为了把重命名的 200+ 处级联改动留到上游同步策略稳定后再统一做,本阶段**只
//! 标注语义已本地化**,不动符号名。
//!
//! 真正的 "服务端往返" 类型正在分批物理删除；服务端对象 enum、字段转换、
//! 初始加载 fan-in 与旧服务端泛型对象承载结构已删除。
//! # P-ARC R1 后的 app 侧 objects 模块
//!
//! 纯数据/类型层已下沉 `crates/objects`(ids、server_types、user_uid、
//! server_time、StoredObject/StoredObjectModel trait、GenericStoredObject、
//! model::{actions,generic_string_model,json_model,object_type_and_id})。
//! 本模块保留:
//! - 被定义为 `app::persistence::ModelEvent` 封印的存储层:
//!   `model/persistence.rs`(ObjectStoreModel/ObjectStoreEvent)、
//!   `object_store.rs`(ObjectStore)、`model/view.rs`(ObjectStoreViewModel);
//! - 展示语义(原 mod.rs 内 ContainingObject/StoredObjectMetadataExt,
//!   已剥离至 `crate::objects_display`);
//! - 各域对象的 getter/操作 helper(依赖 app 侧域类型)。

use self::model::persistence::ObjectStoreModel;
use crate::{
    appearance::Appearance,
    drive::{
        folders::FolderObject,
        items::{WarpDriveItem, WarpDriveItemId},
    },
    ui_components::breadcrumb::Breadcrumb,
    util::time_format::format_approx_duration_from_now_utc,
    workflows::{WorkflowObject, WorkflowSource},
    workspaces::{user_profiles::UserProfiles, user_workspaces::UserWorkspaces},
};
use chrono::{Duration, Utc};
use derivative::Derivative;
use std::{
    any::Any,
    collections::HashSet,
    fmt::Debug,
    sync::Arc,
};
use warp_core::features::FeatureFlag;
use warpui::{AppContext, SingletonEntity};

pub mod model;
pub mod object_store;

pub use objects::*;
pub use crate::objects_display::{
    ContainingObject, ContainingObjectKind, StoredObjectMetadataExt,
};


