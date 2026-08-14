//! Harness Blocks — pure (non-async) SQLite data layer for capturing agent
//! harness sessions as an ordered tree of typed blocks, plus a raw
//! request/response cache.

mod migration;
mod raw_cache;
mod query;
mod schema;
mod store;

pub use raw_cache::{RawCache, RawEntry};
pub use query::{
    get_session_summary, get_system_prompt, list_blocks_by_session, list_blocks_by_type,
    list_child_blocks, SessionSummary,
};
pub use schema::{
    BlockType, HarnessBlock, InterceptMode, UnknownBlockType, UnknownInterceptMode,
};
pub use store::BlockStore;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn block(session: &str, block_type: BlockType, sequence: u32) -> HarnessBlock {
        let mut b = HarnessBlock::new(session, "claude", block_type, sequence, b"hello".to_vec(), 1_700_000_000 + i64::from(sequence));
        b.metadata = json!({"seq": sequence});
        b
    }

    #[test]
    fn block_type_roundtrip() {
        for t in BlockType::ALL {
            assert_eq!(t.to_string().parse::<BlockType>().unwrap(), t);
        }
        assert_eq!("system_prompt".parse::<BlockType>().unwrap(), BlockType::SystemPrompt);
        assert!("nope".parse::<BlockType>().is_err());
        assert_eq!("hooks_only".parse::<InterceptMode>().unwrap(), InterceptMode::HooksOnly);
        assert_eq!(InterceptMode::Bypass.to_string(), "bypass");
    }

    #[test]
    fn block_crud() {
        let store = BlockStore::open_in_memory().unwrap();
        let b = block("s1", BlockType::UserPrompt, 0);
        store.insert_block(&b).unwrap();

        let got = store.get_block(&b.id).unwrap().unwrap();
        assert_eq!(got, b); // exercises content, metadata, block_type roundtrip

        assert!(store.get_block("missing").unwrap().is_none());

        store.insert_block(&block("s1", BlockType::Response, 1)).unwrap();
        store.insert_block(&block("s2", BlockType::Exit, 0)).unwrap();
        assert_eq!(store.list_blocks("s1", None, None).unwrap().len(), 2);
        assert_eq!(
            store.list_blocks("s1", Some(BlockType::Response), None)
                .unwrap()
                .iter()
                .map(|b| b.sequence)
                .collect::<Vec<_>>(),
            vec![1]
        );

        assert_eq!(store.delete_session("s1").unwrap(), 2);
        assert!(store.list_blocks("s1", None, None).unwrap().is_empty());
        assert_eq!(store.list_blocks("s2", None, None).unwrap().len(), 1);
    }

    #[test]
    fn parent_child() {
        let store = BlockStore::open_in_memory().unwrap();
        let mut parent = block("s1", BlockType::Response, 0);
        parent.id = "parent-1".to_string();
        store.insert_block(&parent).unwrap();

        let mut child = block("s1", BlockType::ResponseChunk, 1);
        child.parent_id = Some(parent.id.clone());
        store.insert_block(&child).unwrap();
        let mut other = block("s1", BlockType::ResponseChunk, 2);
        other.parent_id = Some("orphan".to_string());
        store.insert_block(&other).unwrap();

        let children = list_child_blocks(&store, &parent.id).unwrap();
        assert_eq!(children.len(), 1);
        assert_eq!(children[0].id, child.id);

        let roots = store.list_root_blocks("s1").unwrap();
        assert_eq!(roots.len(), 1);
        assert_eq!(roots[0].id, parent.id);
    }

    #[test]
    fn query_api() {
        let store = BlockStore::open_in_memory().unwrap();
        let mut prompt = block("s1", BlockType::SystemPrompt, 0);
        prompt.content = b"you are a harness".to_vec();
        store.insert_block(&prompt).unwrap();
        let mut older_prompt = block("s1", BlockType::SystemPrompt, 1);
        older_prompt.content = b"older".to_vec();
        store.insert_block(&older_prompt).unwrap();
        store.insert_block(&block("s1", BlockType::UserPrompt, 2)).unwrap();

        // get_system_prompt returns the latest by sequence
        let sp = get_system_prompt(&store, "s1").unwrap().unwrap();
        assert_eq!(sp.content, b"older".to_vec());

        assert_eq!(list_blocks_by_session(&store, "s1").unwrap().len(), 3);
        assert_eq!(
            list_blocks_by_type(&store, "s1", BlockType::UserPrompt)
                .unwrap()
                .len(),
            1
        );

        let summary = get_session_summary(&store, "s1").unwrap();
        assert_eq!(summary.block_count, 3);
        assert_eq!(summary.first_timestamp, Some(1_700_000_000));
        assert_eq!(summary.last_timestamp, Some(1_700_000_002));
        assert_eq!(summary.block_type_counts.get("system_prompt"), Some(&2));
        assert_eq!(summary.block_type_counts.get("user_prompt"), Some(&1));
    }

    #[test]
    fn raw_cache_insert_drain_peek() {
        let cache = RawCache::open_in_memory().unwrap();
        cache.insert_raw("s1", "request", b"req-1", 100).unwrap();
        cache.insert_raw("s1", "response", b"resp-1", 200).unwrap();
        cache.insert_raw("s2", "request", b"req-2", 300).unwrap();

        let peeked = cache.peek("s1").unwrap();
        assert_eq!(peeked.len(), 2);
        assert_eq!(peeked[0].content, b"req-1".to_vec());
        assert_eq!(peeked[0].direction, "request");

        let drained = cache.drain("s1").unwrap();
        assert_eq!(drained.len(), 2);
        assert!(cache.peek("s1").unwrap().is_empty());
        assert_eq!(cache.drain("s1").unwrap().len(), 0);
        assert_eq!(cache.peek("s2").unwrap().len(), 1); // other session untouched
    }

    #[test]
    fn file_backed_store_roundtrip() {
        let dir = std::env::temp_dir()
            .join(format!("harness_blocks_test_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join("blocks.db");
        {
            let store = BlockStore::open(db.to_str().unwrap()).unwrap();
            store.insert_block(&block("s1", BlockType::Spawn, 0)).unwrap();
        }
        let store = BlockStore::open(db.to_str().unwrap()).unwrap();
        assert_eq!(store.list_blocks("s1", None, None).unwrap().len(), 1);
        std::fs::remove_dir_all(&dir).ok();
    }
}
