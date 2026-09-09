//! `ObjectTypeAndId` — 本地对象 ID 判别式。
//!
//! 原定义在 `app/src/drive/mod.rs`,P-ARC R1 随 objects 同刀切进 crate:
//! 它只依赖 objects 自身的 id/类型枚举,drive 侧改为 re-export。

use crate::{
    GenericStringObjectFormat, ObjectType, ObjectIdType,
    ids::{HashedSqliteId, ObjectUid, ServerId, SyncId},
};

/// Enum to use to pass down type and id between actions to avoid multiplying actions whenever we
/// need to pass the object id etc.
#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash)]
pub enum ObjectTypeAndId {
    Notebook(SyncId),
    Workflow(SyncId),
    Folder(SyncId),
    GenericStringObject {
        object_type: GenericStringObjectFormat,
        id: SyncId,
    },
}

impl ObjectTypeAndId {
    pub fn from_id_and_type(id: SyncId, object_type: ObjectType) -> Self {
        match object_type {
            ObjectType::Notebook => Self::Notebook(id),
            ObjectType::Workflow => Self::Workflow(id),
            ObjectType::Folder => Self::Folder(id),
            ObjectType::GenericStringObject(format) => Self::GenericStringObject {
                object_type: format,
                id,
            },
        }
    }

    pub fn uid(self) -> ObjectUid {
        match self {
            Self::Notebook(id) => id.uid(),
            Self::Workflow(id) => id.uid(),
            Self::Folder(id) => id.uid(),
            Self::GenericStringObject { id, .. } => id.uid(),
        }
    }

    pub fn sync_id(self) -> SyncId {
        match self {
            Self::Notebook(id)
            | Self::Workflow(id)
            | Self::Folder(id)
            | Self::GenericStringObject { id, .. } => id,
        }
    }

    pub fn sqlite_uid_hash(self) -> HashedSqliteId {
        match self {
            ObjectTypeAndId::Notebook(id) => id.sqlite_uid_hash(ObjectIdType::Notebook),
            ObjectTypeAndId::Workflow(id) => id.sqlite_uid_hash(ObjectIdType::Workflow),
            ObjectTypeAndId::Folder(id) => id.sqlite_uid_hash(ObjectIdType::Folder),
            ObjectTypeAndId::GenericStringObject { object_type: _, id } => {
                id.sqlite_uid_hash(ObjectIdType::GenericStringObject)
            }
        }
    }

    pub fn object_id_type(&self) -> ObjectIdType {
        match self {
            ObjectTypeAndId::Notebook(_) => ObjectIdType::Notebook,
            ObjectTypeAndId::Workflow(_) => ObjectIdType::Workflow,
            ObjectTypeAndId::GenericStringObject { .. } => ObjectIdType::GenericStringObject,
            ObjectTypeAndId::Folder(_) => ObjectIdType::Folder,
        }
    }

    pub fn object_type(&self) -> ObjectType {
        match self {
            ObjectTypeAndId::Notebook(_) => ObjectType::Notebook,
            ObjectTypeAndId::Workflow(_) => ObjectType::Workflow,
            ObjectTypeAndId::Folder(_) => ObjectType::Folder,
            ObjectTypeAndId::GenericStringObject { object_type, .. } => {
                ObjectType::GenericStringObject(*object_type)
            }
        }
    }

    pub fn as_notebook_id(self) -> Option<SyncId> {
        match self {
            ObjectTypeAndId::Notebook(id) => Some(id),
            _ => None,
        }
    }

    pub fn as_generic_string_object_id(self) -> Option<SyncId> {
        match self {
            ObjectTypeAndId::GenericStringObject { object_type: _, id } => Some(id),
            _ => None,
        }
    }

    pub fn has_server_id(self) -> bool {
        matches!(
            self,
            ObjectTypeAndId::Notebook(SyncId::ServerId(_))
                | ObjectTypeAndId::Workflow(SyncId::ServerId(_))
                | ObjectTypeAndId::Folder(SyncId::ServerId(_))
                | ObjectTypeAndId::GenericStringObject {
                    id: SyncId::ServerId(_),
                    ..
                }
        )
    }

    pub fn server_id(self) -> Option<ServerId> {
        match self {
            ObjectTypeAndId::Notebook(SyncId::ServerId(notebook_id)) => Some(notebook_id),
            ObjectTypeAndId::Workflow(SyncId::ServerId(workflow_id)) => Some(workflow_id),
            ObjectTypeAndId::Folder(SyncId::ServerId(folder_id)) => Some(folder_id),
            ObjectTypeAndId::GenericStringObject {
                id: SyncId::ServerId(json_object_id),
                ..
            } => Some(json_object_id),
            _ => None,
        }
    }

    pub fn drive_row_position_id(self) -> String {
        format!("WarpDriveRow_{}", self.uid())
    }

    pub fn from_generic_string_object(object_type: GenericStringObjectFormat, id: SyncId) -> Self {
        Self::GenericStringObject { object_type, id }
    }
}
