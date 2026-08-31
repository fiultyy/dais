use std::sync::Arc;

use futures_lite::future::yield_now;
use warpui::{AppContext, SingletonEntity};

use crate::cloud_object::{model::persistence::ObjectStoreModel, Space};
use crate::search::async_snapshot_data_source::AsyncSnapshotDataSource;
use crate::search::command_search::searcher::CommandSearchItemAction;
use crate::search::data_source::{Query, QueryResult};
use crate::search::mixer::{BoxFuture, DataSourceRunErrorWrapper};
use crate::search::workflows::fuzzy_match::FuzzyMatchWorkflowResult;
use crate::search::QueryFilter;
use crate::server::ids::SyncId;
use crate::settings::AISettings;
use crate::workflows::{WorkflowObjectModel, WorkflowSource};

use super::WorkflowSearchItem;

pub(crate) struct WorkflowMatchCandidate {
    pub id: SyncId,
    pub model: Arc<WorkflowObjectModel>,
    pub source: WorkflowSource,
}

pub(crate) struct StoredWorkflowsSnapshot {
    candidates: Vec<WorkflowMatchCandidate>,
    query_text: String,
    filter_to_agent_mode: bool,
    filter_to_command_workflows: bool,
}

/// Creates an async data source for object-backed workflows (i.e. those that live in the
/// local object store; 本地化后 ObjectStoreModel 即 SQLite 本地对象库,无云端拉取).
pub fn stored_workflows_data_source(
) -> AsyncSnapshotDataSource<StoredWorkflowsSnapshot, CommandSearchItemAction> {
    AsyncSnapshotDataSource::new(
        |query: &Query, app: &AppContext| {
            let is_ai_enabled = AISettings::as_ref(app).is_any_ai_enabled(app);
            let filter_to_agent_mode = query.filters.contains(&QueryFilter::AgentModeWorkflows);
            let filter_to_command_workflows =
                query.filters.contains(&QueryFilter::Workflows) || !is_ai_enabled;

            // 本地化:所有对象都在本地 Personal space,不再遍历 team/shared 空间。
            let candidates: Vec<WorkflowMatchCandidate> = ObjectStoreModel::as_ref(app)
                .active_workflows_in_space(Space::Personal, app)
                .map(|workflow| WorkflowMatchCandidate {
                    id: workflow.id,
                    model: workflow.shared_model(),
                    source: WorkflowSource::PersonalCloud,
                })
                .collect();

            StoredWorkflowsSnapshot {
                candidates,
                query_text: query.text.clone(),
                filter_to_agent_mode,
                filter_to_command_workflows,
            }
        },
        fuzzy_match_stored_workflows,
    )
}

pub(crate) fn fuzzy_match_stored_workflows(
    snapshot: StoredWorkflowsSnapshot,
) -> BoxFuture<'static, Result<Vec<QueryResult<CommandSearchItemAction>>, DataSourceRunErrorWrapper>>
{
    Box::pin(async move {
        let mut results = Vec::new();

        // Workflows are a small dataset with moderate per-item cost (4 fuzzy matches against
        // short strings), so we use a medium chunk size.
        for chunk in snapshot.candidates.chunks(256) {
            for candidate in chunk {
                let is_agent_mode = candidate.model.data.is_agent_mode_workflow();

                if (snapshot.filter_to_command_workflows && is_agent_mode)
                    || (snapshot.filter_to_agent_mode && !is_agent_mode)
                {
                    continue;
                }

                if let Some(match_result) = FuzzyMatchWorkflowResult::try_match(
                    &snapshot.query_text,
                    &candidate.model.data,
                    "",
                ) {
                    results.push(
                        WorkflowSearchItem {
                            identity: super::WorkflowIdentity::Cloud {
                                id: candidate.id,
                                model: candidate.model.clone(),
                            },
                            source: candidate.source,
                            fuzzy_matched_workflow: match_result,
                        }
                        .into(),
                    );
                }
            }
            yield_now().await;
        }

        Ok(results)
    })
}
