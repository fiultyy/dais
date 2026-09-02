//! ActiveEnvVarCollectionData — 活跃 EVC 状态机(G2 store-glue)。
//!
//! P-ARC R1 crate 化改写:原直连 `ObjectStoreModel` / `ObjectStoreViewModel`
//! (app 侧留守层,不能进 crate 依赖),改经 [`EnvVarData`] trait 注入——
//! nav 模板的 Data-trait 契约。宿主 (app) 包装 ObjectStoreModel/ViewModel
//! 实现本 trait;sandbox `--ui-preview` 喂假数据。
//!
//! POD 契约:trait 方法签名零 app 类型;权限枚举镜像
//! [`AccessLevel`] / [`Editability`](`app::drive::sharing` 同形状),
//! 宿主适配层做转换。

use objects::{
    Owner, Revision, Space,
    ids::{ClientId, SyncId},
};

use warpui::{Entity, ModelContext};

use crate::model::{EnvVarCollectionObject, EnvVarCollectionObjectModel};

/// [`EnvVarData::access_level`] 的 POD 镜像
/// (app 侧 `drive::sharing::SharingAccessLevel`)。
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
pub enum AccessLevel {
    View,
    Edit,
    Full,
}

impl AccessLevel {
    pub fn can_trash(self) -> bool {
        self >= AccessLevel::Edit
    }
}

/// [`EnvVarData::editability`] 的 POD 镜像
/// (app 侧 `drive::sharing::ContentEditability`)。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Editability {
    ReadOnly,
    RequiresLogin,
    Editable,
}

impl Editability {
    pub fn can_edit(self) -> bool {
        matches!(self, Editability::Editable)
    }
}

/// 宿主数据契约:env_vars 域对本地对象存储的只读投影。
///
/// 实现方 = app(包装 `ObjectStoreModel` / `ObjectStoreViewModel`)。
/// 所有方法要求只读、无副作用,可在渲染帧内调用。
pub trait EnvVarData {
    /// 按 id 取已提交的 EVC 对象克隆;不存在(含已物理删除)返回 None。
    fn get_collection(&self, app: &warpui::AppContext, id: &SyncId)
        -> Option<EnvVarCollectionObject>;

    /// 对象存储初始加载(本地 sqlite)是否已完成。
    fn initial_load_completed(&self, app: &warpui::AppContext) -> bool;

    /// 该 uid 对象在当前用户视角下的访问级别。
    fn access_level(&self, app: &warpui::AppContext, uid: &str) -> AccessLevel;

    /// 该 uid 对象的编辑性(含匿名用户不可编辑语义)。
    fn editability(&self, app: &warpui::AppContext, uid: &str) -> Editability;

    /// 该 uid 对象所在空间;不存在返回 None。
    fn object_space(&self, app: &warpui::AppContext, uid: &str) -> Option<Space>;

    /// 该 id 对象是否在回收站中;对象不存在返回 None。
    fn is_trashed(&self, app: &warpui::AppContext, id: &SyncId) -> Option<bool>;
}

/// [`EnvVarData`] 的空实现(nav::NavDataSource for () 同款):
/// sandbox/测试占位,全部返回"无数据"。
impl EnvVarData for () {
    fn get_collection(
        &self,
        _app: &warpui::AppContext,
        _id: &SyncId,
    ) -> Option<EnvVarCollectionObject> {
        None
    }

    fn initial_load_completed(&self, _app: &warpui::AppContext) -> bool {
        true
    }

    fn access_level(&self, _app: &warpui::AppContext, _uid: &str) -> AccessLevel {
        AccessLevel::Full
    }

    fn editability(&self, _app: &warpui::AppContext, _uid: &str) -> Editability {
        Editability::Editable
    }

    fn object_space(&self, _app: &warpui::AppContext, _uid: &str) -> Option<Space> {
        None
    }

    fn is_trashed(&self, _app: &warpui::AppContext, _id: &SyncId) -> Option<bool> {
        Some(false)
    }
}

#[derive(Default, Clone)]
pub enum ActiveEnvVarCollection {
    #[default]
    None,
    // An EnvVarCollection already stored in the local object store, all relevant
    // data should be queried via [`EnvVarData`]
    CommittedEnvVarCollection(SyncId),
    // An EnvVarCollection that has been created and displayed in the view, but is not yet
    // committed to the object store
    NewEnvVarCollection(Box<EnvVarCollectionObject>),
}

#[derive(Default, PartialEq, Debug)]
pub enum SavingStatus {
    #[default]
    Saved,
    Unsaved,
    New,
}

#[derive(Default)]
pub struct ActiveEnvVarCollectionData {
    pub saving_status: SavingStatus,
    pub active_env_var_collection: ActiveEnvVarCollection,
    pub revision_ts: Option<Revision>,
}

impl ActiveEnvVarCollectionData {
    pub fn new(_ctx: &mut ModelContext<Self>) -> Self {
        // Dais:原 `ObjectStore` 订阅用于接收云端同步完成事件(Create/Update/Trash
        // /Untrash::Success),无云 = 永不触发;Phase 2c‑1 已删。
        // 原 `ObjectStoreModel` 订阅随 crate 化移交宿主:app 侧在装配时订阅
        // `ObjectStoreEvent` 并转发本 model 的刷新(crate 不再直连 store 单例)。
        Self {
            ..Default::default()
        }
    }

    pub fn reset(&mut self) {
        self.active_env_var_collection = ActiveEnvVarCollection::None;
    }

    pub fn open_new(
        &mut self,
        owner: Owner,
        initial_folder_id: Option<SyncId>,
        ctx: &mut ModelContext<Self>,
    ) {
        self.reset();

        let new_id = ClientId::default();

        // Set the active env var collection to be an uncommitted collection
        self.active_env_var_collection = ActiveEnvVarCollection::NewEnvVarCollection(Box::new(
            EnvVarCollectionObject::new_local(
                EnvVarCollectionObjectModel::default(),
                owner,
                initial_folder_id,
                new_id,
            ),
        ));

        ctx.notify();
    }

    pub fn open_existing(&mut self, env_var_collection_id: SyncId, ctx: &mut ModelContext<Self>) {
        self.reset();
        self.saving_status = SavingStatus::Saved;
        self.active_env_var_collection =
            ActiveEnvVarCollection::CommittedEnvVarCollection(env_var_collection_id);

        ctx.notify();
    }

    pub fn id(&self) -> Option<SyncId> {
        match &self.active_env_var_collection {
            ActiveEnvVarCollection::None => None,
            ActiveEnvVarCollection::CommittedEnvVarCollection(id) => Some(*id),
            ActiveEnvVarCollection::NewEnvVarCollection(env_var_collection) => {
                Some(env_var_collection.id)
            }
        }
    }

    /// The current user's access level on this env var collection.
    pub fn access_level(&self, app: &warpui::AppContext, data: &dyn EnvVarData) -> AccessLevel {
        match &self.active_env_var_collection {
            ActiveEnvVarCollection::CommittedEnvVarCollection(sync_id) => {
                data.access_level(app, &sync_id.uid())
            }
            ActiveEnvVarCollection::None | ActiveEnvVarCollection::NewEnvVarCollection(_) => {
                AccessLevel::Full
            }
        }
    }

    pub fn editability(&self, app: &warpui::AppContext, data: &dyn EnvVarData) -> Editability {
        match &self.active_env_var_collection {
            ActiveEnvVarCollection::CommittedEnvVarCollection(sync_id) => {
                data.editability(app, &sync_id.uid())
            }
            ActiveEnvVarCollection::None | ActiveEnvVarCollection::NewEnvVarCollection(_) => {
                Editability::Editable
            }
        }
    }

    /// The space that this env var collection is in.
    pub fn space(&self, app: &warpui::AppContext, data: &dyn EnvVarData) -> Option<Space> {
        match &self.active_env_var_collection {
            ActiveEnvVarCollection::None => None,
            ActiveEnvVarCollection::CommittedEnvVarCollection(sync_id) => {
                data.object_space(app, &sync_id.uid())
            }
            ActiveEnvVarCollection::NewEnvVarCollection(env_var_collection) => {
                // 未提交对象:按 owner 推导空间由宿主裁定;crate 侧保守返回
                // owner 所指 Team/Personal 判别的原始权限数据。
                Some(match env_var_collection.permissions.owner {
                    Owner::Team { team_uid } => Space::Team { team_uid },
                    Owner::User { .. } => Space::Personal,
                })
            }
        }
    }

    pub fn active_env_var_collection(&self) -> ActiveEnvVarCollection {
        self.active_env_var_collection.clone()
    }

    /// Whether or not the EVC has been synced to the server.
    pub fn is_on_server(&self) -> bool {
        matches!(
            &self.active_env_var_collection,
            ActiveEnvVarCollection::CommittedEnvVarCollection(SyncId::ServerId(_))
        )
    }

    pub fn is_active_env_var_collection(&self, env_var_collection_id: SyncId) -> bool {
        self.id() == Some(env_var_collection_id)
    }

    // Dais(W3-d8): `breadcrumbs()` 已随 objects::breadcrumbs 裁撤。

    pub fn trash_status(&self, app: &warpui::AppContext, data: &dyn EnvVarData) -> TrashStatus {
        match &self.active_env_var_collection {
            ActiveEnvVarCollection::None | ActiveEnvVarCollection::NewEnvVarCollection(_) => {
                TrashStatus::Active
            }
            ActiveEnvVarCollection::CommittedEnvVarCollection(id) => match data.is_trashed(app, id)
            {
                Some(true) => TrashStatus::Trashed,
                Some(false) => TrashStatus::Active,
                None => TrashStatus::Deleted,
            },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TrashStatus {
    Active,
    Trashed,
    Deleted,
}

pub enum ActiveEnvVarCollectionDataEvent {
    /// The EVC was trashed or untrashed
    /// (used for refreshing the pane overflow items)
    TrashStatusChanged,
}

impl Entity for ActiveEnvVarCollectionData {
    type Event = ActiveEnvVarCollectionDataEvent;
}
