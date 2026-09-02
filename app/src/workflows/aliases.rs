use std::collections::{HashMap, HashSet};

use anyhow::Error;
use warp_core::{
    define_settings_group,
    settings::{RespectUserSyncSetting, Setting, SupportedPlatforms, SyncToCloud},
};
use warpui::{AppContext, ModelContext, SingletonEntity};

use crate::drive::ObjectTypeAndId;
use objects::{StoredObject as _, ids::SyncId};
use crate::objects::model::persistence::ObjectStoreModel;
use crate::objects::model::persistence::{ObjectStoreEvent};

// R2-b:WorkflowAlias POD 数据形态已下沉 workflows_ui,此处保持
// `crate::workflows::aliases::WorkflowAlias` 旧路径可用。
pub use workflows_ui::WorkflowAlias;

define_settings_group!(WorkflowAliases, settings: [
    aliases: Aliases {
        type: Vec<WorkflowAlias>,
        default: vec![],
        supported_platforms: SupportedPlatforms::ALL,
        sync_to_cloud: SyncToCloud::Globally(RespectUserSyncSetting::Yes),
        private: true,
        storage_key: "WorkflowAliases",
    }
]);

impl WorkflowAliases {
    /// Call once to subscribe to ObjectStore notifications that a workflow has been deleted.
    pub fn connect(&self, ctx: &mut ModelContext<Self>) {
        ctx.subscribe_to_model(&ObjectStoreModel::handle(ctx), |me, event, ctx| {
            let result = match event {
                ObjectStoreEvent::ObjectTrashed {
                    type_and_id: ObjectTypeAndId::Workflow(server_id),
                    ..
                } => me.remove_aliases_for_workflow(*server_id, ctx),
                _ => Result::Ok(()),
            };

            if let Err(e) = result {
                log::error!("Error removing aliases for workflow: {e:?}");
            }
        });
    }

    pub fn get_all_aliases(&self) -> &[WorkflowAlias] {
        &self.aliases
    }

    /// A mapping of all aliases, for autocomplete.
    pub fn autocomplete_data(&self, ctx: &AppContext) -> HashMap<String, String> {
        let object_store_model = ObjectStoreModel::as_ref(ctx);
        let mut alias_data = HashMap::with_capacity(self.aliases.len());
        for alias in self.aliases.iter() {
            if let Some(backing_workflow) = object_store_model.get_workflow(&alias.workflow_id) {
                alias_data.insert(alias.alias.clone(), backing_workflow.display_name());}
        }
        alias_data
    }

    // potentially support autocomplete
    pub fn match_alias(&self, input_text: &str) -> Option<WorkflowAlias> {
        self.aliases
            .iter()
            .find(|alias| alias.alias == input_text)
            .cloned()
    }

    pub fn get_aliases_for_workflow(&self, workflow_id: SyncId) -> Vec<&WorkflowAlias> {
        self.aliases
            .iter()
            .filter(|alias| alias.workflow_id == workflow_id)
            .collect()
    }

    pub fn set_aliases(
        &mut self,
        aliases_to_add: Vec<WorkflowAlias>,
        ctx: &mut ModelContext<WorkflowAliases>,
    ) -> Result<(), Error> {
        let mut aliases = self.aliases.clone();
        let to_exclude = aliases_to_add
            .iter()
            .map(|a| a.alias.clone())
            .collect::<HashSet<_>>();
        aliases.retain(|a| !to_exclude.contains(&a.alias));
        aliases.extend(aliases_to_add);

        self.aliases.set_value(aliases, ctx)
    }

    pub fn remove_aliases(
        &mut self,
        aliases_to_exclude: Vec<String>,
        ctx: &mut ModelContext<WorkflowAliases>,
    ) -> Result<(), Error> {
        let mut aliases = self.aliases.clone();
        let to_exclude = aliases_to_exclude.into_iter().collect::<HashSet<_>>();
        aliases.retain(|a| !to_exclude.contains(&a.alias));
        self.aliases.set_value(aliases, ctx)
    }

    /// Migrate all aliases from one workflow id to another.
    /// Useful when a workflow id changes, like on initial save.
    pub fn update_workflow_id(
        &mut self,
        old_workflow_id: SyncId,
        new_workflow_id: SyncId,
        ctx: &mut ModelContext<Self>,
    ) -> Result<(), Error> {
        let mut aliases = self.aliases.clone();
        for alias in aliases.iter_mut() {
            if alias.workflow_id == old_workflow_id {
                alias.workflow_id = new_workflow_id}
        }
        self.aliases.set_value(aliases, ctx)
    }

    pub fn remove_aliases_for_workflow(
        &mut self,
        workflow_id: SyncId,
        ctx: &mut ModelContext<WorkflowAliases>,
    ) -> Result<(), Error> {
        let aliases = self
            .aliases
            .iter()
            .filter(|a| a.workflow_id != workflow_id)
            .cloned()
            .collect();
        self.aliases.set_value(aliases, ctx)
    }
}

#[cfg(test)]
#[path = "aliases_test.rs"]
mod tests;
