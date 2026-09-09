//! `SyncId` 序列化/uid 稳定性测试。
//!
//! 原文件在 `app/src/server/ids_test.rs`,引用 notebooks 的 id 类型;
//! crate 化后改用 crate 内 `server_id_traits!` 本地造 id 类型,
//! 保证宏派生的 From<String>/Into<SyncId>/Display 行为与 app 侧一致。

use crate::server_id_traits;
use super::{ClientId, ServerId, SyncId};

// crate 内本地 id 类型,走与 app 侧完全相同的宏路径。
pub struct TestNotebookId(ServerId);
server_id_traits! { TestNotebookId, "Notebook" }

impl From<TestNotebookId> for SyncId {
    fn from(id: TestNotebookId) -> Self {
        Self::ServerId(id.0)
    }
}

#[test]
pub fn test_client_sync_id_serialization() {
    let id: SyncId = SyncId::ClientId(ClientId::new());
    let serialized = serde_json::to_string(&id).expect("failed to serialize");
    assert_eq!(serialized, format!("\"{}\"", id.uid()));
    let deserialized: SyncId =
        serde_json::from_str(serialized.as_str()).expect("failed to deserialize");
    assert_eq!(id, deserialized);
}

#[test]
pub fn test_server_sync_id_serialization() {
    let id = SyncId::ServerId(TestNotebookId::from(ServerId::from(123)).into());
    let serialized = serde_json::to_string(&id).expect("failed to serialize");
    assert_eq!(serialized, format!("\"{}\"", ServerId::from(123)));
    let deserialized: SyncId =
        serde_json::from_str(serialized.as_str()).expect("failed to deserialize");
    assert_eq!(id, deserialized);
}

#[test]
pub fn test_server_sync_id_uid_serialization() {
    let id = SyncId::ServerId(TestNotebookId::from(String::from("Ymgrzu0nh2HwDNeYEtXF1x")).into());
    let serialized = serde_json::to_string(&id).expect("failed to serialize");
    assert_eq!(
        serialized,
        format!("\"{}\"", String::from("Ymgrzu0nh2HwDNeYEtXF1x"))
    );
    let deserialized: SyncId =
        serde_json::from_str(serialized.as_str()).expect("failed to deserialize");
    assert_eq!(id, deserialized);
}
