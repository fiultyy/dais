use warpui::{
    async_assert, async_assert_eq,
    integration::{AssertionCallback, AssertionWithDataCallback},
    AppContext, SingletonEntity,
};

use crate::{
    ai::facts::{view::AIFactPage, AIFactObjectModel},
    integration_testing::view_getters::workspace_view,
};
use crate::objects::model::persistence::ObjectStoreModel;
use objects::{ids::SyncId, GenericStringObjectId};

/// Assert that a specific AI fact exists with the given content
pub fn assert_rule_exists(
    expected_id_key: impl Into<String>,
    expected_content: impl Into<String>,
) -> AssertionWithDataCallback {
    let expected_id_key = expected_id_key.into();
    let expected_content = expected_content.into();
    Box::new(move |app, _window_id, data| {
        let sync_id: &SyncId = data.get(&expected_id_key).expect("No saved AI fact ID");
        ObjectStoreModel::handle(app).read(app, |object_store_model, _| {
            if let Some(ai_fact) = object_store_model
                .get_object_of_type::<GenericStringObjectId, AIFactObjectModel>(sync_id)
            {
                let content = match &ai_fact.model().string_model {
                    crate::ai::facts::AIFact::Memory(memory) => &memory.content,
                };
                async_assert_eq!(content, &expected_content, "AI fact content should match")
            } else {
                async_assert!(false, "AI fact should exist")
            }
        })
    })
}

/// Assert that the total number of AI facts matches the expected count
pub fn assert_rule_count(expected_count: usize) -> AssertionCallback {
    Box::new(move |app, _| {
        ObjectStoreModel::handle(app).read(app, |object_store_model, ctx| {
            let count = rule_count(object_store_model, ctx);
            async_assert_eq!(count, expected_count, "Rule count should match")
        })
    })
}

/// Helper function to count AI facts in the object store
pub fn rule_count(object_store_model: &ObjectStoreModel, _ctx: &AppContext) -> usize {
    object_store_model
        .get_all_objects_of_type::<GenericStringObjectId, AIFactObjectModel>()
        .count()
}

