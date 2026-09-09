//! # objects — 本地领域对象数据层(L1 data)
//!
//! P-ARC R1 自 `app/src/objects` 同刀切出。承载与"本地对象存储"解耦的纯
//! 类型与 trait:
//!
//! - `ids`:ClientId / ServerId / SyncId / HashableId / ToServerId /
//!   `server_id_traits!` 宏(原 `app::server::ids`,ids↔objects 环由此打断)。
//! - `server_types`:ObjectIdType / ObjectType / GenericStringObjectFormat /
//!   JsonObjectType / Revision / Owner / StoredObjectPermissions /
//!   StoredObjectMetadata / StoredObjectEventEntrypoint / TEST_USER_UID。
//! - `server_time`:ServerTimestamp / ServerTime(原 `app::server_time`)。
//! - `user_uid`:UserUid(原 `app::auth::user_uid`,auth 侧 re-export 保持兼容)。
//! - `ObjectTypeAndId`(原 `app::drive` 内联定义,drive 侧 re-export)。
//! - `StoredObject` / `StoredObjectModel` / `GenericStoredObject` trait 核心、
//!   `model::{actions, generic_string_model, json_model}`。
//!
//! ## 留守 app 的部分(物理依赖约束)
//!
//! - `ObjectStoreModel` / `ObjectStoreEvent` / `ObjectStore` /
//!   `ObjectStoreViewModel` 仍在 `app::objects`:`StoredObject::upsert_event`
//!   返回 `app::persistence::ModelEvent`,后者携带 AppState / WorkflowObject /
//!   api::Task 等 app 类型,随 store 进 crate 会成反向依赖。
//! - `ContainingObject` / `StoredObjectMetadataExt` 等 UI 展示语义在
//!   `app::objects_display`。
//! - `StoredObject` 的 `to_warp_drive_item` / `renders_in_warp_drive` /
//!   `object_link` / `space` 等依赖 app 单例的方法已从 trait 裁剪,由
//!   app 侧自由函数按类型分发。
//!
//! ## 宿主契约(供 L4 域 crate 参照,如 nav)
//!
//! 域 crate 只 import 本 crate 的类型面;对 ObjectStoreModel/ViewModel 的访问
//! 通过各自的 Data trait 由宿主(app)注入,不直接依赖 app。

pub mod ids;
pub mod model;
pub mod object_type_and_id;
pub mod server_time;
mod server_types;
pub mod user_uid;

pub use object_type_and_id::ObjectTypeAndId;
pub use server_types::*;

pub use model::{
    actions::{
        ObjectAction, ObjectActionSubtype, ObjectActionType, ObjectActions, ObjectActionsEvent,
    },
    generic_string_model::{
        GenericStringModel, GenericStringObjectId, Serializer, StoredStringObject, StringModel,
    },
    json_model::{JsonModel, JsonSerializer},
};

use derivative::Derivative;

use std::{any::Any, fmt::Debug, sync::Arc};
pub use ids::{ClientId, HashableId, HashedSqliteId, ObjectUid, ServerId, SyncId, ToServerId};
pub use server_time::ServerTimestamp;
use ids::{HashableId as _, ToServerId as _};

/// sqlite 写入事件:objects crate 内的唯一 upsert 载体。
///
/// 原 `ModelEvent` 携带大量 app 类型,无法进 crate;本枚举只承载
/// generic string object 的 upsert 形态,由 app 侧 ObjectStore 转换为
/// `ModelEvent` 后发给 SQLite writer。
#[derive(Debug)]
pub enum UpsertEvent {
    UpsertGenericStringObject {
        object: Box<dyn StoredStringObject>,
    },
    UpsertGenericStringObjects(Vec<Box<dyn StoredStringObject>>),
    /// P-ARC R1 逃生门: 宿主(app)专属 upsert 事件(Folder/Notebook/Workflow 等
    /// 携带 app 域类型的 ModelEvent),由宿主侧 sqlite 分发 downcast 处理。
    App(Box<dyn std::any::Any + Send + Sync>),
}

/// 包装一个 model 序列化后字符串的 newtype。
///
/// Dais(Wave 4):原定义在 `crate::server::sync_queue`,SyncQueue 整删后
/// 迁到 objects。多个 model 的 `serialized()` 仍然返回它(本地写 sqlite 时使用)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SerializedModel(String);

impl SerializedModel {
    pub fn new(s: String) -> Self {
        Self(s)
    }

    pub fn model_as_str(&self) -> &str {
        &self.0
    }

    pub fn take(self) -> String {
        self.0
    }
}

impl From<String> for SerializedModel {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&str> for SerializedModel {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

/// 冲突状态。
///
/// Dais 本地化后服务端冲突不再产生,结构保留以稳定 sqlite/调用点;
/// 运行时始终 `NoConflicts`。
#[derive(Clone, Debug, Default, PartialEq)]
pub enum ConflictStatus {
    #[default]
    NoConflicts,
    ConflictingChanges {
        revision: Revision,
    },
}

impl ConflictStatus {
    /// Utility function that allows for a more ergonomic way of figuring out whether there is a
    /// conflict (for cases where we don't care about the conflict details).
    pub fn has_conflicts(&self) -> bool {
        matches!(self, ConflictStatus::ConflictingChanges { .. })
    }
}

/// Represents a unique key for a generic string object. The server enforces that
/// no two generic string objects have the same key.
#[derive(PartialEq, Eq, Debug, Clone)]
pub struct GenericStringObjectUniqueKey {
    /// The unique key.  E.g. for cloud prefs this is the storage_key of the pref
    pub key: String,

    /// Whether this key is unique for all generic string objects, or unique per user.
    pub unique_per: UniquePer,
}

#[derive(PartialEq, Eq, Debug, Clone)]
pub enum UniquePer {
    User,
}

/// 本地对象空间。
///
/// `name()` 原依赖 app 的 UserWorkspaces 单例,已裁剪到 app 侧
/// `objects_display`;crate 内仅保数据形态。
#[derive(Default, Clone, Copy, Debug, Eq, Derivative)]
#[derivative(PartialEq, Hash)]
pub enum Space {
    /// The current user's personal drive.
    #[default]
    Personal,
    /// A team that the current user belongs to.
    Team { team_uid: ServerId },
    /// An object shared from a drive the user is not a member of.
    Shared,
}

/// Enum for specifying the location of a dais drive object.
/// Objects can live in top level spaces, or a specific folder.
#[derive(Eq, PartialEq, Copy, Clone, Debug, Hash)]
pub enum StoredObjectLocation {
    Space(Space),
    Folder(SyncId),
    Trash,
}

/// A StoredObject represents
/// therefore shareable and editable (i.e. Notebooks and Workflows). In order
/// to support collaborative editing of these objects, they must each store local
/// revision numbers to ensure a stable way of accepting and rejecting edits.
///
/// Note that this trait must be object-safe and non-generic.  The reason for this
/// is that (a) we need to be able to store instances of it as trait objects in
/// ObjectStoreModel and (b) we need to be able to support mixed collections of different
/// instances of it (e.g. in the map of id -> StoredObject in ObjectStoreModel).
///
/// There are two closely related types to this:
/// 1) GenericStoredObject: This is the concrete generic implementation of StoredObject that
///    holds onto a model of type StoredObjectModel and an id of type SyncId.
/// 2) StoredObjectModel: This is a trait that defines the model type for a StoredObject -
///    this is what implementors of new cloud types typically have to implement.
///
/// These types are tightly coupled.  In an ideal world, rust would allow a mechanism
/// for us having a single interface that new model types could implement that could
/// be generic on id and model types, but as far as I (zach) can tell, that's not currently
/// possible.
///
/// The typical usage pattern for these types is to use dyn StoredObject whenever you
/// don't need access to a model or id, and to downcast to a GenericStoredObject whenever you do.
///
/// 由于历史 cloud-object 命名仍保留,当前所有本地 StoredObject 都需要实现
/// GenericStoredObject。
///
/// # P-ARC R1 裁剪说明
///
/// 以下方法因依赖 app 侧单例/类型,已从 trait 移除,由 app 侧
/// `objects_display` 自由函数按 object_type 分发:
/// - `upsert_event() -> ModelEvent`(ModelEvent 在 app::persistence)
/// - `to_warp_drive_item` / `renders_in_warp_drive`(drive::items)
/// - `object_link`(ChannelState)
/// - `space` / `can_leave` / `containing_objects_path` / `breadcrumbs` /
///   `location` / `is_trashed`(ObjectStoreModel / UserWorkspaces)
pub trait StoredObject: Debug {
    /// Returns the name of this model type (e.g. Workflow, Folder, Notebook)
    fn model_type_name(&self) -> &'static str;

    /// Returns the  uid for this object.
    fn uid(&self) -> ObjectUid;

    /// Returns the [`SyncId`] that currently identifies this object.
    fn sync_id(&self) -> SyncId;

    /// Returns the id used to index into sqlite, this is the object's UID with its type
    /// prefixed, such as "Workflow-{UID}"
    fn hashed_sqlite_id(&self) -> HashedSqliteId;

    /// Returns the StoredObjectMetadata struct associated with this object.
    fn metadata(&self) -> &StoredObjectMetadata;

    /// Returns a mutable reference to the StoredObjectMetadata struct associated with this object.
    fn metadata_mut(&mut self) -> &mut StoredObjectMetadata;

    /// Returns the StoredObjectPermissions struct associated with this object.
    fn permissions(&self) -> &StoredObjectPermissions;

    /// Returnsa mutable reference to the StoredObjectPermissions struct associated with this object.
    fn permissions_mut(&mut self) -> &mut StoredObjectPermissions;

    /// Returns the ObjectType i.e. 'Workflow' or 'Notebook'
    fn object_type(&self) -> ObjectType;

    /// Returns the ObjectTypeAndId for this object.
    fn object_type_and_id(&self) -> ObjectTypeAndId;

    /// Sets the server id on this object.
    fn set_server_id(&mut self, server_id: ServerId);

    // Whether to clear this object from the local SQLite DB on a unique key conflict.
    fn should_clear_on_unique_key_conflict(&self) -> bool {
        false
    }

    /// Whether to show a warning if this object is unsaved at quit time
    /// (which typically blocks the user from quitting)
    fn warn_if_unsaved_at_quit(&self) -> bool {
        true
    }

    /// Returns whether this model type should show update toasts in the UI.
    fn should_show_activity_toasts(&self) -> bool {
        true
    }

    // Returns the name of the object.
    fn display_name(&self) -> String;

    /// Returns whether this object has conflicting changes with the server.
    fn has_conflicting_changes(&self) -> bool;

    /// Returns the revision of the conflicting object, if any.
    /// This is used for object-safe access to conflict information.
    fn conflicting_object_revision(&self) -> Option<Revision>;

    /// Clears the conflict status back to NoConflicts.
    fn clear_conflict_status(&mut self);

    /// Updates the object to deal with any conflict status.
    fn replace_object_with_conflict(&mut self);

    /// Whether or not this object can be exported.
    fn can_export(&self) -> bool;

    /// Returns this object as a ref to the Any type.  Needed for typecasts.
    fn as_any(&self) -> &dyn Any;

    /// Returns this object as a mut ref to Any type.  Needed for typecasts.
    fn as_any_mut(&mut self) -> &mut dyn Any;

    /// Returns the trait object as a concrete type reference by downcasting it.
    /// Returns None if the downcast fails.
    fn as_model_type<K, M>(objects: &dyn StoredObject) -> Option<&GenericStoredObject<K, M>>
    where
        Self: Sized,
        K: HashableId + ToServerId + Debug + Into<String> + Clone + 'static,
        M: StoredObjectModel<IdType = K, StoredObjectType = GenericStoredObject<K, M>> + 'static,
    {
        objects
            .as_any()
            .downcast_ref::<GenericStoredObject<K, M>>()
    }

    /// Returns the trait object as a concrete mutable type reference by downcasting it.
    /// Returns None if the downcast fails.
    fn as_model_type_mut<K, M>(
        objects: &mut dyn StoredObject,
    ) -> Option<&mut GenericStoredObject<K, M>>
    where
        Self: Sized,
        K: HashableId + ToServerId + Debug + Into<String> + Clone + 'static,
        M: StoredObjectModel<IdType = K, StoredObjectType = GenericStoredObject<K, M>> + 'static,
    {
        objects
            .as_any_mut()
            .downcast_mut::<GenericStoredObject<K, M>>()
    }

    /// 返回这个 stored object 的 boxed clone。
    /// 不能直接要求 StoredObject trait derive Clone,否则 trait 不再 object safe。
    fn clone_box(&self) -> Box<dyn StoredObject>;
}

/// Defines a common trait for object store models to implement.
/// "model" 是 stored object 的领域数据,例如 notebook/workflow/folder 的具体内容;
/// metadata、permissions、sync status 逻辑不放在这里。
///
/// See the comments for StoredObject to understand the relationship between
/// this trait, StoredObject and GenericStoredObject.  They are tightly coupled.
///
/// When building new model types (e.g. for settings or launch configs) we should just
/// have to implement this trait, and not the entire StoredObject trait.
///
/// # P-ARC R1 裁剪说明
///
/// `upsert_event` / `bulk_upsert_event` 返回类型由 `ModelEvent` 改为
/// [`UpsertEvent`];`to_warp_drive_item` / `renders_in_warp_drive` 已移除。
pub trait StoredObjectModel: Debug + Clone + Send + Sync {
    /// The associated StoredObject type for this model (e.g. NotebookObject, WorkflowObject, etc)
    type StoredObjectType: StoredObject + 'static;
    // TODO: @ianhodge - remove for sync ID refactor.
    type IdType: HashableId + ToServerId + Debug + Into<String> + Clone + 'static;

    /// Returns the name of this model type (e.g. Workflow, Folder, Notebook)
    fn model_type_name(&self) -> &'static str;

    /// Returns the ObjectTypeAndId for this object.
    fn object_type_and_id(&self, id: SyncId) -> ObjectTypeAndId;

    /// Returns the ObjectType for this model.
    fn object_type(&self) -> ObjectType;

    /// Returns whether this model type should show update toasts in the UI.
    fn should_show_activity_toasts(&self) -> bool {
        true
    }

    /// Whether to show a warning if this model is unsaved at quit time
    /// (which typically blocks the user from quitting)
    fn warn_if_unsaved_at_quit(&self) -> bool {
        true
    }

    /// Returns the display name for this model (e.g. to show in the Dais Drive index)
    fn display_name(&self) -> String;

    /// Sets the display name to show in the Dais Drive Index.  Setting the name
    /// is not currently supported by all object types, hence the default empty
    /// implementation.
    fn set_display_name(&mut self, _name: &str) {}

    /// Returns the upsert event for putting this model into the SQLite database.
    fn upsert_event(&self, object: &Self::StoredObjectType) -> UpsertEvent;

    /// Returns a bulk upsert event for putting a list of this model into the SQLite database.
    fn bulk_upsert_event(objects: &[Self::StoredObjectType]) -> UpsertEvent;

    /// Returns a serialized model.
    fn serialized(&self) -> SerializedModel;

    /// Returns whether this model type supports being moved to the given space.
    fn can_move_to_space(&self, _current_space: Space, _new_space: Space) -> bool {
        true
    }

    /// Returns whether this model type should clear on a unique key conflict.
    fn should_clear_on_unique_key_conflict(&self) -> bool {
        false
    }

    /// Returns whether this model type supports web links
    fn supports_linking(&self) -> bool {
        true
    }

    /// Whether this model type can be exported.
    fn can_export(&self) -> bool {
        false
    }
}

/// GenericStoredObject 是历史 cloud-object 体系保留下来的本地对象通用实现。
///
/// 新对象可以直接使用 GenericStoredObject<K, M>,其中 K 是 id type,M 是 model type。
///
/// For example, NotebookObject becomes:
///
///   pub type NotebookObject = GenericStoredObject<NotebookId, NotebookObjectModel>
///
/// The advantage of using the generic model is you get common implementations
/// of StoredObject methods like ```versions``` for free.
///
/// See the comments for StoredObject to understand the relationship between
/// this trait, StoredObject and StoredObjectModel.  They are tightly coupled.
#[derive(Clone, Debug, PartialEq)]
pub struct GenericStoredObject<K, M>
where
    K: HashableId + ToServerId + Debug + Into<String> + Clone + 'static,
    M: StoredObjectModel<IdType = K> + 'static,
{
    pub id: SyncId,
    pub metadata: StoredObjectMetadata,
    pub permissions: StoredObjectPermissions,
    /// Tracks whether this object has a conflict with the server version.
    /// This is runtime state (not persisted) - conflicts are always NoConflicts when loaded from SQLite.
    pub conflict_status: ConflictStatus,

    // Intentionally not public to prevent users of this class from holding
    // onto references to the model outside of this struct.
    //
    // This is an Arc in order to support clone-on-write semantics for the model.
    // By wrapping the model in an Arc, clones become cheap, and we can avoid
    // doing deep clones of the model whenever the containing object is cloned.
    //
    // Callers who want to update the model need to call set_model to update the
    // entire model atomically.
    model: Arc<M>,
}

impl<K, M> StoredObject for GenericStoredObject<K, M>
where
    K: HashableId + ToServerId + Debug + Into<String> + Clone + 'static,
    M: StoredObjectModel<IdType = K, StoredObjectType = GenericStoredObject<K, M>> + 'static,
{
    fn model_type_name(&self) -> &'static str {
        self.model.model_type_name()
    }

    fn uid(&self) -> ObjectUid {
        self.id.uid()
    }

    fn hashed_sqlite_id(&self) -> HashedSqliteId {
        self.id.sqlite_uid_hash(self.object_type().into())
    }

    fn sync_id(&self) -> SyncId {
        self.id
    }

    fn should_show_activity_toasts(&self) -> bool {
        self.model.should_show_activity_toasts()
    }

    fn warn_if_unsaved_at_quit(&self) -> bool {
        self.model.warn_if_unsaved_at_quit()
    }

    fn metadata(&self) -> &StoredObjectMetadata {
        &self.metadata
    }

    fn metadata_mut(&mut self) -> &mut StoredObjectMetadata {
        &mut self.metadata
    }

    fn permissions(&self) -> &StoredObjectPermissions {
        &self.permissions
    }

    fn permissions_mut(&mut self) -> &mut StoredObjectPermissions {
        &mut self.permissions
    }

    fn object_type(&self) -> ObjectType {
        self.model.object_type()
    }

    fn object_type_and_id(&self) -> ObjectTypeAndId {
        self.model.object_type_and_id(self.id)
    }

    fn should_clear_on_unique_key_conflict(&self) -> bool {
        self.model.should_clear_on_unique_key_conflict()
    }

    fn has_conflicting_changes(&self) -> bool {
        self.conflict_status.has_conflicts()
    }

    fn conflicting_object_revision(&self) -> Option<Revision> {
        match &self.conflict_status {
            ConflictStatus::ConflictingChanges { revision } => Some(revision.clone()),
            ConflictStatus::NoConflicts => None,
        }
    }

    fn clear_conflict_status(&mut self) {
        self.conflict_status = ConflictStatus::NoConflicts;
    }

    fn replace_object_with_conflict(&mut self) {
        let mut new_conflict = ConflictStatus::NoConflicts;
        std::mem::swap(&mut new_conflict, &mut self.conflict_status);

        let _ = new_conflict;
        self.conflict_status = ConflictStatus::NoConflicts;
    }

    fn set_server_id(&mut self, server_id: ServerId) {
        self.id = SyncId::ServerId(server_id);
    }

    fn display_name(&self) -> String {
        self.model.display_name()
    }

    fn can_export(&self) -> bool {
        self.model.can_export()
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn clone_box(&self) -> Box<dyn StoredObject> {
        Box::new(self.clone())
    }
}

impl<K, M> GenericStoredObject<K, M>
where
    K: HashableId + ToServerId + Debug + Into<String> + Clone + 'static,
    M: StoredObjectModel<IdType = K, StoredObjectType = GenericStoredObject<K, M>> + 'static,
{
    /// Gets a reference to the model held by the object.
    pub fn model(&self) -> &M {
        &self.model
    }

    /// Returns a shared handle to the model.
    pub fn shared_model(&self) -> Arc<M> {
        self.model.clone()
    }

    /// Sets a new version of the model on the object, replacing the old version.
    pub fn set_model(&mut self, model: M) {
        self.model = model.into();
    }

    /// Returns a bulk upsert event for putting a list of this object into the SQLite database.
    pub fn bulk_upsert_event(objects: &[Self]) -> UpsertEvent {
        M::bulk_upsert_event(objects)
    }

    /// Constructs a new instance of this model with the given id, model, metadata and permissions.
    pub fn new(
        id: SyncId,
        model: M,
        metadata: StoredObjectMetadata,
        permissions: StoredObjectPermissions,
    ) -> Self {
        Self {
            id,
            model: model.into(),
            metadata,
            permissions,
            conflict_status: ConflictStatus::NoConflicts,
        }
    }

    /// Creates a new GenericStoredObject with the given model, owner, and initial folder id.
    /// This is for the local creation flow, as opposed to creating from a server update.
    pub fn new_local(
        model: M,
        owner: Owner,
        initial_folder_id: Option<SyncId>,
        client_id: ClientId,
    ) -> Self {
        Self {
            id: SyncId::ClientId(client_id),
            model: model.into(),
            metadata: StoredObjectMetadata {
                folder_id: initial_folder_id,
                revision: Default::default(),
                metadata_last_updated_ts: Default::default(),
                current_editor_uid: Default::default(),
                trashed_ts: Default::default(),
                // Objects created from the client are never welcome objects.
                is_welcome_object: false,
                creator_uid: None,
                last_editor_uid: None,
                last_task_run_ts: None,
            },
            permissions: StoredObjectPermissions { owner },
            conflict_status: ConflictStatus::NoConflicts,
        }
    }
}

impl<'a, K, M> From<&'a dyn StoredObject> for Option<&'a GenericStoredObject<K, M>>
where
    K: HashableId + ToServerId + Debug + Into<String> + Clone + 'static,
    M: StoredObjectModel<IdType = K, StoredObjectType = GenericStoredObject<K, M>> + 'static,
{
    fn from(value: &'a dyn StoredObject) -> Self {
        <GenericStoredObject<K, M> as StoredObject>::as_model_type::<K, M>(value)
    }
}

impl<'a, K, M> From<&'a Box<dyn StoredObject>> for Option<&'a GenericStoredObject<K, M>>
where
    K: HashableId + ToServerId + Debug + Into<String> + Clone + 'static,
    M: StoredObjectModel<IdType = K, StoredObjectType = GenericStoredObject<K, M>> + 'static,
{
    fn from(value: &'a Box<dyn StoredObject>) -> Self {
        <GenericStoredObject<K, M> as StoredObject>::as_model_type::<K, M>(value.as_ref())
    }
}

impl<'a, K, M> From<&'a mut Box<dyn StoredObject>> for Option<&'a mut GenericStoredObject<K, M>>
where
    K: HashableId + ToServerId + Debug + Into<String> + Clone + 'static,
    M: StoredObjectModel<IdType = K, StoredObjectType = GenericStoredObject<K, M>> + 'static,
{
    fn from(value: &'a mut Box<dyn StoredObject>) -> Self {
        <GenericStoredObject<K, M> as StoredObject>::as_model_type_mut::<K, M>(value.as_mut())
    }
}

impl Clone for Box<dyn StoredObject> {
    fn clone(&self) -> Self {
        self.clone_box()
    }
}

impl From<&dyn StoredObject> for ObjectType {
    fn from(value: &dyn StoredObject) -> Self {
        value.object_type()
    }
}

impl From<&Box<dyn StoredObject>> for ObjectType {
    fn from(value: &Box<dyn StoredObject>) -> Self {
        <ObjectType as From<&dyn StoredObject>>::from(value.as_ref())
    }
}
