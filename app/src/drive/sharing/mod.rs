use crate::{auth::UserUid, server::ids::ServerId};

// Dais Phase 3(R3-2/S4): 本模块原是云端分享(ACL/邀请/链接分享)的完整类型面。
// `dialog/` 与全部触发入口已在 Phase 2a 删除;本轮按消费方盘点裁掉零引用符号
// (SubjectExt/UserKindExt/TeamKindExt/ShareableObject/头像助手/style.rs,以及
// 仅服务 persistence 编解码的 FromStr/to_serializable_value/serde derive、
// LinkSharingSubjectType 与 Subject::AnyoneWithLink — persistence/objectss.rs
// 与其测试已由 S2 删,零引用)。
// 仍保留的符号均有存活消费方:
// - `SharingAccessLevel`/`ContentEditability` → notebooks / env_vars / workflows
//   的只读判定与 trash 权限(objects/model/view.rs 的
//   access_level/editability 链)。
// - `Subject`/`UserKind`/`TeamKind` → objects/server_types.rs 的
//   StoredObjectPermissions 结构(keep 骨架)及 view.rs 的 access_level 判定。

#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
pub enum SharingAccessLevel {
    View,
    Edit,
    Full,
}

impl SharingAccessLevel {
    pub fn can_trash(self) -> bool {
        self >= SharingAccessLevel::Edit
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Subject {
    User(UserKind),
    #[allow(dead_code)]
    PendingUser {
        email: Option<String>,
    },
    Team(TeamKind),
}

#[derive(Debug, Clone)]
pub enum UserKind {
    Account(UserUid),
}

#[derive(Debug, Clone, PartialEq)]
pub enum TeamKind {
    Team { team_uid: ServerId },
}

impl PartialEq for UserKind {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Account(self_uid), Self::Account(other_uid)) => self_uid == other_uid,
        }
    }
}

impl Subject {
    pub fn is_user(&self, other_uid: UserUid) -> bool {
        match self {
            Subject::User(UserKind::Account(user_uid)) => *user_uid == other_uid,
            Subject::PendingUser { .. } | Subject::Team(_) => false,
        }
    }
}

/// Whether not a shared object's contents are editable by the current user.
///
/// This is not purely a function of their access level since anonymous users are not allowed to
/// edit (due to the lack of attribution).
#[derive(Debug, Clone, Copy)]
pub enum ContentEditability {
    ReadOnly,
    RequiresLogin,
    Editable,
}

impl ContentEditability {
    pub fn can_edit(self) -> bool {
        matches!(self, ContentEditability::Editable)
    }
}
