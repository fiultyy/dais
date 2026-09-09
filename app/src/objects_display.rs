//! objects 展示语义层(原 app/src/objects/mod.rs 内联区块)。
//!
//! P-ARC R1:依赖 AppContext / Breadcrumb / WarpDriveItemId / UserProfiles
//! 等展示与宿主类型,故留 app;纯数据类型已下沉 crates/objects。

use crate::{
    drive::{folders::FolderObject, items::WarpDriveItemId},
    objects::{
        ids::SyncId, user_uid::UserUid, ObjectTypeAndId,
    },
    ui_components::breadcrumb::Breadcrumb,
    util::time_format::format_approx_duration_from_now_utc,
    workflows::WorkflowSource,
    workspaces::{user_profiles::UserProfiles, user_workspaces::UserWorkspaces},
};
use chrono::{Duration, Utc};
use objects::{Owner, Revision, Space, StoredObject, StoredObjectMetadata};
use warpui::{AppContext, SingletonEntity};

use crate::objects::model::persistence::ObjectStoreModel;
use objects::{ServerTimestamp, StringModel};

/// Encapsulates an object that can contain other objects, and keeps
/// information necessary to show breadcrumbs.
///
/// W3-d8: 原 `crate::objects::breadcrumbs` 模块已裁撤, 类型本地化到此。
#[derive(Clone, Debug)]
pub struct ContainingObject {
    pub name: String,
    pub kind: ContainingObjectKind,
}

impl Breadcrumb for ContainingObject {
    fn label(&self) -> String {
        self.name.clone()
    }

    fn enabled(&self) -> bool {
        true
    }
}

impl From<&FolderObject> for ContainingObject {
    fn from(folder: &FolderObject) -> Self {
        Self {
            name: folder.display_name(),
            kind: ContainingObjectKind::Object(ObjectTypeAndId::Folder(folder.id)),
        }
    }
}

pub trait SpaceExt { fn into_containing_object(&self, app: &AppContext) -> ContainingObject; fn name(&self, app: &AppContext) -> String; }

impl SpaceExt for Space {
    fn into_containing_object(&self, app: &AppContext) -> ContainingObject {
        ContainingObject {
            name: self.name(app).clone(),
            kind: ContainingObjectKind::Space(*self),
        }
    }

    fn name(&self, app: &AppContext) -> String {
        match self {
            Space::Personal => "Personal".to_string(),
            Space::Team { team_uid, .. } => {
                let user_workspaces = UserWorkspaces::as_ref(app);
                if let Some(team) = user_workspaces.team_from_uid(*team_uid) {
                    team.name.clone()
                } else {
                    "Team".to_string()
                }
            }
            Space::Shared => "Shared with me".to_string(),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub enum ContainingObjectKind {
    Space(Space),
    Object(ObjectTypeAndId),
}

impl ContainingObjectKind {
    pub fn into_item_id(self) -> WarpDriveItemId {
        match self {
            ContainingObjectKind::Space(space) => WarpDriveItemId::Space(space),
            ContainingObjectKind::Object(object) => WarpDriveItemId::Object(object),
        }
    }
}

impl From<Space> for WorkflowSource {
    fn from(space: Space) -> Self {
        match space {
            Space::Personal => WorkflowSource::PersonalCloud,
            Space::Team { team_uid } => WorkflowSource::Team { team_uid },
            // TODO(ben): Model sharing in workflow telemetry.
            Space::Shared => WorkflowSource::PersonalCloud,
        }
    }
}

impl From<Owner> for WorkflowSource {
    fn from(owner: Owner) -> WorkflowSource {
        match owner {
            // TODO(ben): Represent shared objects in telemetry.
            Owner::User { .. } => Self::PersonalCloud,
            Owner::Team { team_uid } => Self::Team { team_uid },
        }
    }
}

/// Extension trait for StoredObjectMetadata with methods that require AppContext.
pub trait StoredObjectMetadataExt {
    /// Returns a semantic summary of the last edit to the object. For example, "Alice edited 4 weeks ago".
    /// Returns None if the revision and last_editor are None.
    fn semantic_editing_history(&self, app: &AppContext) -> Option<String>;

    /// Returns a semantic summary of the object's creator. For example, "Alice" or "joan@warp.dev".
    #[cfg_attr(target_family = "wasm", expect(dead_code))]
    fn semantic_creator(&self, app: &AppContext) -> Option<String>;

    /// Returns semantic summary of countdown of days until permadeletion.
    /// Ex: "27 days until permanent deletion"
    fn semantic_permadeletion_countdown(&self, app: &AppContext) -> Option<String>;
}

impl StoredObjectMetadataExt for StoredObjectMetadata {
    fn semantic_editing_history(&self, app: &AppContext) -> Option<String> {
        let user_profiles = UserProfiles::as_ref(app);

        // First, the editor. For example, "Joan Didion" or "joan@warp.dev".
        let editor_string = self
            .last_editor_uid
            .as_ref()
            .and_then(|uid| user_profiles.displayable_identifier_for_uid(UserUid::new(uid)));

        // Second, the time elapsed since the edit. For example, "just now" or "3 months ago".
        let time_ago_string = self
            .revision
            .clone()
            .map(|r| format_approx_duration_from_now_utc(r.utc()));

        let full_string = match (editor_string, time_ago_string) {
            (Some(name), Some(time_ago)) if name.is_empty() => format!("Edited {time_ago}"),
            (Some(name), Some(time_ago)) => format!("{name} edited {time_ago}"),
            (None, Some(time_ago)) => format!("Edited {time_ago}"),
            (Some(name), None) => format!("Last edited by {name}"),
            _ => return None,
        };

        Some(full_string)
    }

    fn semantic_creator(&self, app: &AppContext) -> Option<String> {
        // Todo(Jack): add creation ts.
        let user_profiles = UserProfiles::as_ref(app);
        self.creator_uid
            .as_ref()
            .and_then(|uid| user_profiles.displayable_identifier_for_uid(UserUid::new(uid)))
    }

    fn semantic_permadeletion_countdown(&self, app: &AppContext) -> Option<String> {
        // 2 cases:
        // 1) Either the object is a root level object.
        // 2) Or the object is inside folder(s), call recursive function to get trashed_ts of top level folder.
        if let Some(trashed_ts) = self
            .trashed_ts
            .or_else(|| get_top_folder_trashed_ts(self.folder_id, app))
        {
            let deletion_time = trashed_ts.utc() + Duration::days(31);
            let current_time = Utc::now();
            let days_left = deletion_time.signed_duration_since(current_time).num_days();

            let full_string = match days_left {
                0 | 1 => "1 day until permanent deletion".to_string(),
                _ => format!("{days_left} days until permanent deletion"),
            };
            Some(full_string)
        } else {
            None
        }
    }
}

/// Helper function to retrieve trashed_ts of top level folder given a folder_id of an object.
fn get_top_folder_trashed_ts(
    folder_id: Option<SyncId>,
    app: &AppContext,
) -> Option<ServerTimestamp> {
    let mut folder_id = folder_id;
    let object_store_model = ObjectStoreModel::as_ref(app);
    while let Some(current_folder_id) = folder_id {
        // If the parent folder isn't in ObjectStoreModel, short-circuit so we don't loop forever.
        let folder = object_store_model.get_folder_by_uid(&current_folder_id.uid())?;

        if let Some(_parent_folder_id) = folder.metadata.folder_id {
            folder_id = folder.metadata.folder_id
        } else {
            return folder.metadata.trashed_ts;
        }
    }
    None
}



/// P-ARC R1: crate `StringModel` 裁撤 drive 展示语义后的宿主侧扩展 trait。
/// 各域 StringModel impl 若需 drive 徽章/链接行为,另 impl 本 trait。
/// 不要求 StringModel: Folder/Notebook/Workflow 的 ObjectModel 走 StoredObjectModel。
pub trait WarpDriveItemOverrides {
    fn renders_in_warp_drive(&self) -> bool;
}


/// P-ARC R1: 原 StoredObject trait 的宿主侧查询语义(is_trashed/location/space 等,
/// 依赖 ObjectStoreModel 遍历),crate trait 裁撤后由本 trait 承载并给 dyn 实现兜底。
pub trait StoredObjectHostExt {
    fn is_trashed(&self, store: &crate::objects::model::persistence::ObjectStoreModel) -> bool;
    fn is_trashed_internal(
        &self,
        store: &crate::objects::model::persistence::ObjectStoreModel,
        ancestors: &mut std::collections::HashSet<String>,
    ) -> bool;
    fn containing_objects_path(&self, app: &warpui::AppContext) -> Vec<ContainingObject>;
    fn breadcrumbs(&self, app: &warpui::AppContext) -> String;
    fn location(
        &self,
        store: &crate::objects::model::persistence::ObjectStoreModel,
        app: &warpui::AppContext,
    ) -> crate::objects::StoredObjectLocation;
    fn is_in_space(&self, space: crate::objects::Space, app: &warpui::AppContext) -> bool;
    fn is_welcome_object(&self) -> bool;
    fn renders_in_warp_drive_dyn(&self) -> bool;
    fn space(&self, app: &warpui::AppContext) -> crate::objects::Space;
}

impl<T: crate::objects::StoredObject + ?Sized> StoredObjectHostExt for T {
    fn is_trashed(&self, store: &crate::objects::model::persistence::ObjectStoreModel) -> bool {
        self.is_trashed_internal(store, &mut std::collections::HashSet::new())
    }
    fn is_trashed_internal(
        &self,
        store: &crate::objects::model::persistence::ObjectStoreModel,
        ancestors: &mut std::collections::HashSet<String>,
    ) -> bool {
        if self.metadata().trashed_ts.is_some() {
            return true;
        }
        match self.metadata().folder_id.map(|id| id.uid()) {
            Some(parent_uid) => {
                if !ancestors.insert(parent_uid.clone()) {
                    return true; // cycle
                }
                store
                    .get_folder_by_uid(&parent_uid)
                    .map(|f| f.is_trashed_internal(store, ancestors))
                    .unwrap_or(false)
            }
            None => false,
        }
    }
    fn containing_objects_path(&self, app: &warpui::AppContext) -> Vec<ContainingObject> {
        let space = self.space(app);
        match self.metadata().folder_id {
            Some(folder_id) => {
                let store = crate::objects::model::persistence::ObjectStoreModel::as_ref(app);
                if let Some(folder) = store.get_folder_by_uid(&folder_id.uid()) {
                    let mut path = folder.containing_objects_path(app);
                    path.push((&*folder).into());
                    path
                } else {
                    vec![space.into_containing_object(app)]
                }
            }
            None => vec![space.into_containing_object(app)],
        }
    }
    fn breadcrumbs(&self, app: &warpui::AppContext) -> String {
        self.containing_objects_path(app)
            .into_iter()
            .map(|o| o.name)
            .collect::<Vec<String>>()
            .join(" / ")
    }
    fn location(
        &self,
        store: &crate::objects::model::persistence::ObjectStoreModel,
        app: &warpui::AppContext,
    ) -> crate::objects::StoredObjectLocation {
        // 原实现: 有 folder 则 folder 的 location,否则 space
        use crate::objects::StoredObjectLocation;
        if let Some(fid) = self.metadata().folder_id {
            if store.get_folder(&fid).is_some() {
                return StoredObjectLocation::Folder(fid);
            }
        }
        StoredObjectLocation::Space(self.space(app))
    }
    fn is_in_space(&self, space: crate::objects::Space, app: &warpui::AppContext) -> bool {
        self.space(app) == space
    }
    fn is_welcome_object(&self) -> bool {
        self.metadata().is_welcome_object
    }
    fn renders_in_warp_drive_dyn(&self) -> bool {
        true
    }
    fn space(&self, app: &warpui::AppContext) -> crate::objects::Space {
        crate::workspaces::user_workspaces::UserWorkspaces::as_ref(app)
            .owner_to_space(self.permissions().owner.clone(), app)
    }
}


/// P-ARC R1: 原 StoredObject/GenericStoredObject 的展示 helper 自由函数化。
pub fn object_link() -> String {
    // 原 object_link 生成 cloud 形态链接;本地化后保留原样(Dais 语义另议)。
    String::new()
}
