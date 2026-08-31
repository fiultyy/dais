use crate::{
    cloud_object::{
        model::{persistence::ObjectStoreEvent, view::ObjectStoreViewModel},
        Owner, Revision, Space, StoredObject,
    },
    drive::sharing::{ContentEditability, SharingAccessLevel},
    env_vars::EnvVarCollectionObject,
    server::ids::{ClientId, SyncId},
    AppContext, ObjectStoreModel,
};

use warpui::{Entity, ModelContext, SingletonEntity};

use super::EnvVarCollectionObjectModel;

#[derive(Default, Clone)]
pub enum ActiveEnvVarCollection {
    #[default]
    None,
    // An EnvVarCollection already stored in ObjectStoreModel, all relevant data should be queried
    // from ObjectStoreModel directly
    CommittedEnvVarCollection(SyncId),
    // An EnvVarCollection that has been created and displayed in the view, but is not yet
    // committed to ObjectStoreModel
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
    pub fn new(ctx: &mut ModelContext<Self>) -> Self {
        // Dais:原 `UpdateManager` 订阅用于接收云端同步完成事件(Create/Update/Trash
        // /Untrash::Success),无云 = 永不触发;`trash_object`/`untrash_object` 已本地化
        // 不 emit `ObjectOperationComplete`。Phase 2c‑1 删除订阅 + handler。
        // `ObjectStoreModel` 订阅保留(本地对象变更仍需 breadcrumbs 刷新)。
        let object_store_model = ObjectStoreModel::handle(ctx);

        ctx.subscribe_to_model(&object_store_model, |me, event, ctx| {
            me.handle_object_store_event(event, ctx);
        });

        Self {
            ..Default::default()
        }
    }

    fn handle_object_store_event(
        &mut self,
        event: &ObjectStoreEvent,
        _ctx: &mut ModelContext<Self>,
    ) {
        if let ObjectStoreEvent::ObjectMoved { type_and_id, .. } = event {
            // Dais(W3-d8): 面包屑刷新随 cloud_object::breadcrumbs 裁撤,
            // ObjectMoved 不再触发 BreadcrumbsChanged。
            let _ = type_and_id;
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
    pub fn access_level(&self, app: &AppContext) -> SharingAccessLevel {
        match &self.active_env_var_collection {
            ActiveEnvVarCollection::CommittedEnvVarCollection(sync_id) => {
                ObjectStoreViewModel::as_ref(app).access_level(&sync_id.uid(), app)
            }
            ActiveEnvVarCollection::None | ActiveEnvVarCollection::NewEnvVarCollection(_) => {
                SharingAccessLevel::Full
            }
        }
    }

    pub fn editability(&self, app: &AppContext) -> ContentEditability {
        match &self.active_env_var_collection {
            ActiveEnvVarCollection::CommittedEnvVarCollection(sync_id) => {
                ObjectStoreViewModel::as_ref(app).object_editability(&sync_id.uid(), app)
            }
            ActiveEnvVarCollection::None | ActiveEnvVarCollection::NewEnvVarCollection(_) => {
                ContentEditability::Editable
            }
        }
    }

    /// The space that this env var collection is in.
    pub fn space(&self, app: &AppContext) -> Option<Space> {
        match &self.active_env_var_collection {
            ActiveEnvVarCollection::None => None,
            ActiveEnvVarCollection::CommittedEnvVarCollection(sync_id) => {
                ObjectStoreViewModel::as_ref(app).object_space(&sync_id.uid(), app)
            }
            ActiveEnvVarCollection::NewEnvVarCollection(env_var_collection) => {
                Some(env_var_collection.space(app))
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

    // Dais(W3-d8): `breadcrumbs()` 已随 cloud_object::breadcrumbs 裁撤;
    // containing_objects_path 仍由 StoredObject trait 提供。

    pub fn trash_status(&self, ctx: &AppContext) -> TrashStatus {
        match &self.active_env_var_collection {
            ActiveEnvVarCollection::None | ActiveEnvVarCollection::NewEnvVarCollection(_) => {
                TrashStatus::Active
            }
            ActiveEnvVarCollection::CommittedEnvVarCollection(id) => {
                let object_store_model = ObjectStoreModel::as_ref(ctx);
                match object_store_model.get_env_var_collection(id) {
                    Some(env_var_collection) => {
                        if env_var_collection.is_trashed(object_store_model) {
                            TrashStatus::Trashed
                        } else {
                            TrashStatus::Active
                        }
                    }
                    None => TrashStatus::Deleted,
                }
            }
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
