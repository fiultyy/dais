use crate::ai::agent::conversation::{AIConversationId, ConversationStatus};
use crate::ai::artifacts::Artifact;
use crate::ai::blocklist::{format_credits, BlocklistAIHistoryEvent, BlocklistAIHistoryModel};
use crate::ai::conversation_navigation::ConversationNavigationData;
use crate::auth::AuthStateProvider;
use crate::ui_components::icons::Icon;
use crate::workspace::{RestoreConversationLayout, WorkspaceAction};
use chrono::{DateTime, Utc};
use itertools::Itertools;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use warp_core::features::FeatureFlag;
use warp_core::ui::theme::{color::internal_colors, WarpTheme};
use warpui::color::ColorU;
use warpui::{AppContext, Entity, EntityId, ModelContext, SingletonEntity, WindowId};

#[derive(Copy, Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub enum StatusFilter {
    #[default]
    All,
    Working,
    Done,
    Failed,
}

#[derive(Copy, Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub enum ArtifactFilter {
    #[default]
    All,
    PullRequest,
    Plan,
    Screenshot,
    File,
}

#[derive(Copy, Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub enum CreatedOnFilter {
    #[default]
    All,
    Last24Hours,
    Past3Days,
    LastWeek,
}

#[derive(Default, Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum OwnerFilter {
    All,
    #[default]
    PersonalOnly,
}

#[derive(Default, PartialEq, Eq, Clone, Debug, Serialize, Deserialize)]
pub struct AgentManagementFilters {
    pub owners: OwnerFilter,
    pub status: StatusFilter,
    pub created_on: CreatedOnFilter,
    pub artifact: ArtifactFilter,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AgentRunDisplayStatus {
    ConversationInProgress,
    ConversationSucceeded,
    ConversationError,
    ConversationBlocked { blocked_action: String },
    ConversationCancelled,
}

impl AgentRunDisplayStatus {
    pub fn from_conversation_status(status: &ConversationStatus) -> Self {
        match status {
            ConversationStatus::InProgress => Self::ConversationInProgress,
            ConversationStatus::Success => Self::ConversationSucceeded,
            ConversationStatus::Error => Self::ConversationError,
            ConversationStatus::Cancelled => Self::ConversationCancelled,
            ConversationStatus::Blocked { blocked_action } => Self::ConversationBlocked {
                blocked_action: blocked_action.clone(),
            },
        }
    }

    pub fn status_filter(&self) -> StatusFilter {
        match self {
            AgentRunDisplayStatus::ConversationInProgress => StatusFilter::Working,
            AgentRunDisplayStatus::ConversationSucceeded => StatusFilter::Done,
            AgentRunDisplayStatus::ConversationError
            | AgentRunDisplayStatus::ConversationBlocked { .. }
            | AgentRunDisplayStatus::ConversationCancelled => StatusFilter::Failed,
        }
    }

    pub fn is_cancellable(&self) -> bool {
        self.is_working()
    }

    pub fn is_working(&self) -> bool {
        matches!(self, AgentRunDisplayStatus::ConversationInProgress)
    }

    pub fn status_icon_and_color(&self, theme: &WarpTheme) -> (Icon, ColorU) {
        match self {
            AgentRunDisplayStatus::ConversationInProgress => {
                (Icon::ClockLoader, theme.ansi_fg_magenta())
            }
            AgentRunDisplayStatus::ConversationSucceeded => (Icon::Check, theme.ansi_fg_green()),
            AgentRunDisplayStatus::ConversationError => (Icon::Triangle, theme.ansi_fg_red()),
            AgentRunDisplayStatus::ConversationBlocked { .. } => {
                (Icon::StopFilled, theme.ansi_fg_yellow())
            }
            AgentRunDisplayStatus::ConversationCancelled => {
                (Icon::StopFilled, internal_colors::neutral_5(theme))
            }
        }
    }
}

impl std::fmt::Display for AgentRunDisplayStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AgentRunDisplayStatus::ConversationInProgress => write!(f, "In progress"),
            AgentRunDisplayStatus::ConversationSucceeded => write!(f, "Done"),
            AgentRunDisplayStatus::ConversationError => write!(f, "Error"),
            AgentRunDisplayStatus::ConversationBlocked { .. } => write!(f, "Blocked"),
            AgentRunDisplayStatus::ConversationCancelled => write!(f, "Cancelled"),
        }
    }
}

/// Stores conversation metadata needed for display in conversation views.
pub struct ConversationMetadata {
    pub nav_data: ConversationNavigationData,
}

/// Wrapper around conversation data stored in the `AgentConversationsModel`.
///
/// The name is kept for API stability; the task half was removed along with the
/// ambient agent subsystem.
pub enum ConversationOrTask<'a> {
    Conversation(&'a ConversationMetadata),
}

impl ConversationOrTask<'_> {
    pub fn title(&self, app: &AppContext) -> String {
        match self {
            ConversationOrTask::Conversation(metadata) => {
                // We try to read the title from the history model first (that's the most up-to-date),
                // but fall back to the one stored in the navigation data.
                let history_model = BlocklistAIHistoryModel::as_ref(app);
                history_model
                    .conversation(&metadata.nav_data.id)
                    .and_then(|conv| conv.title().clone())
                    .unwrap_or(metadata.nav_data.title.clone())
            }
        }
    }

    /// Map to conversation status for the UI status display
    pub fn status(&self, app: &AppContext) -> ConversationStatus {
        match self {
            ConversationOrTask::Conversation(metadata) => {
                let history_model = BlocklistAIHistoryModel::as_ref(app);
                history_model
                    .conversation(&metadata.nav_data.id)
                    .map(|conv| conv.status().clone())
                    .unwrap_or(ConversationStatus::Success)
            }
        }
    }

    pub fn display_status(&self, app: &AppContext) -> AgentRunDisplayStatus {
        match self {
            ConversationOrTask::Conversation(metadata) => {
                let history_model = BlocklistAIHistoryModel::as_ref(app);
                history_model
                    .conversation(&metadata.nav_data.id)
                    .map(|conv| AgentRunDisplayStatus::from_conversation_status(conv.status()))
                    .unwrap_or(AgentRunDisplayStatus::ConversationSucceeded)
            }
        }
    }

    /// Grab the creator name from the auth state (conversations are created by the local user).
    pub fn creator_name(&self, _app: &AppContext) -> Option<String> {
        AuthStateProvider::as_ref(_app).get().username_for_display()
    }

    /// Grab the creator UID from the auth state (conversations are created by the local user).
    pub fn creator_uid(&self, _app: &AppContext) -> Option<String> {
        AuthStateProvider::as_ref(_app)
            .get()
            .user_id()
            .map(|uid| uid.to_string())
    }

    /// Returns the request usage for the conversation
    pub(super) fn request_usage(&self, app: &AppContext) -> Option<f32> {
        match self {
            ConversationOrTask::Conversation(metadata) => {
                let history_model = BlocklistAIHistoryModel::as_ref(app);
                history_model
                    .conversation(&metadata.nav_data.id)
                    .map(|conv| conv.credits_spent())
                    .or_else(|| {
                        history_model
                            .get_conversation_metadata(&metadata.nav_data.id)
                            .and_then(|m| m.credits_spent)
                    })
            }
        }
    }

    /// Formats the request usage for display.
    pub fn display_request_usage(&self, app: &AppContext) -> Option<String> {
        self.request_usage(app).map(format_credits)
    }

    pub fn last_updated(&self) -> DateTime<Utc> {
        match self {
            ConversationOrTask::Conversation(metadata) => metadata.nav_data.last_updated.into(),
        }
    }

    pub fn created_at(&self) -> DateTime<Utc> {
        match self {
            ConversationOrTask::Conversation(metadata) => metadata.nav_data.last_updated.into(),
        }
    }

    /// Returns the navigation data for the conversation, used for emitting the Navigate event.
    pub fn navigation_data(&self) -> Option<&ConversationNavigationData> {
        match self {
            ConversationOrTask::Conversation(metadata) => Some(&metadata.nav_data),
        }
    }

    /// Returns artifacts for the conversation.
    pub fn artifacts(&self, app: &AppContext) -> Vec<Artifact> {
        match self {
            ConversationOrTask::Conversation(metadata) => {
                let history_model = BlocklistAIHistoryModel::as_ref(app);
                history_model
                    .conversation(&metadata.nav_data.id)
                    .map(|conv| conv.artifacts().to_vec())
                    .or_else(|| {
                        history_model
                            .get_conversation_metadata(&metadata.nav_data.id)
                            .map(|m| m.artifacts.clone())
                    })
                    .unwrap_or_default()
            }
        }
    }

    /// Check if this item matches the current status filter.
    fn matches_status(&self, status_filter: &StatusFilter, app: &AppContext) -> bool {
        match status_filter {
            StatusFilter::All => true,
            StatusFilter::Working | StatusFilter::Done | StatusFilter::Failed => {
                self.display_status(app).status_filter() == *status_filter
            }
        }
    }

    /// Check if this item matches the artifact filter.
    fn matches_artifact(&self, artifact_filter: &ArtifactFilter, app: &AppContext) -> bool {
        artifacts_match_filter(&self.artifacts(app), artifact_filter)
    }

    /// Returns the `WorkspaceAction` to dispatch when opening this conversation.
    pub fn get_open_action(
        &self,
        restore_layout: Option<RestoreConversationLayout>,
    ) -> Option<WorkspaceAction> {
        match self {
            ConversationOrTask::Conversation(metadata) => {
                let nav_data = &metadata.nav_data;
                Some(WorkspaceAction::RestoreOrNavigateToConversation {
                    conversation_id: nav_data.id,
                    window_id: nav_data.window_id,
                    pane_view_locator: nav_data.pane_view_locator,
                    terminal_view_id: nav_data.terminal_view_id,
                    restore_layout,
                })
            }
        }
    }
}

pub(crate) fn artifacts_match_filter(
    artifacts: &[Artifact],
    artifact_filter: &ArtifactFilter,
) -> bool {
    match artifact_filter {
        ArtifactFilter::All => true,
        ArtifactFilter::PullRequest => artifacts
            .iter()
            .any(|artifact| matches!(artifact, Artifact::PullRequest { .. })),
        ArtifactFilter::Plan => artifacts
            .iter()
            .any(|artifact| matches!(artifact, Artifact::Plan { .. })),
        ArtifactFilter::Screenshot => artifacts
            .iter()
            .any(|artifact| matches!(artifact, Artifact::Screenshot { .. })),
        ArtifactFilter::File => artifacts
            .iter()
            .any(|artifact| matches!(artifact, Artifact::File { .. })),
    }
}

/// This model serves as a unified interface for reading local agent conversations.
/// It syncs conversation navigation data from the history model and exposes filtered,
/// sorted views over it.
///
/// This model backs the conversation list view.
pub struct AgentConversationsModel {
    /// A map of conversation IDs to local conversations.
    conversations: HashMap<AIConversationId, ConversationMetadata>,
    /// Set of view IDs actively consuming this model's data per window.
    /// Dais:本地化后无轮询,仅作为 register_view_open/closed 的占位记录使用。
    active_data_consumers_per_window: HashMap<WindowId, HashSet<EntityId>>,
    /// Whether we have finished the initial load
    has_finished_initial_load: bool,
}

pub enum AgentConversationsModelEvent {
    /// Initial sync of conversations completed.
    ConversationsLoaded,
    /// Conversation status data was updated
    ConversationUpdated,
    /// Conversation artifacts were updated (plans, PRs, etc.)
    ConversationArtifactsUpdated { conversation_id: AIConversationId },
}

impl Entity for AgentConversationsModel {
    type Event = AgentConversationsModelEvent;
}

impl SingletonEntity for AgentConversationsModel {}

impl AgentConversationsModel {
    pub fn new(ctx: &mut ModelContext<Self>) -> Self {
        // Dais(本地化):本模型不再轮询远端 ambient agent tasks(该子系统已删除),
        // 只同步本地 conversation 导航数据;has_finished_initial_load 直接为 true。
        //
        // Issue #93 修复:必须订阅 BlocklistAIHistoryModel 的事件,否则用户在历史对话
        // 列表中删除对话后,本模型缓存的 conversations 不会刷新,UI 将持续展示已删除的项。
        let history_model = BlocklistAIHistoryModel::handle(ctx);
        ctx.subscribe_to_model(&history_model, |me, event, ctx| {
            me.handle_history_event(event, ctx);
        });

        Self {
            conversations: HashMap::new(),
            active_data_consumers_per_window: HashMap::new(),
            has_finished_initial_load: true,
        }
    }

    pub fn is_loading(&self) -> bool {
        !self.has_finished_initial_load
    }

    /// Sync all conversations to the AgentConversationsModel.
    ///
    /// This function will loop through all active panes, recently closed panes, and historical
    /// conversations to construct a complete snapshot of conversations.
    pub fn sync_conversations(&mut self, ctx: &mut ModelContext<Self>) {
        if !FeatureFlag::InteractiveConversationManagementView.is_enabled() {
            return;
        }

        let nav_data_list = ConversationNavigationData::all_conversations(ctx);

        self.conversations.clear();
        for nav_data in nav_data_list {
            let conversation_id = nav_data.id;
            let metadata = ConversationMetadata { nav_data };
            self.conversations.insert(conversation_id, metadata);
        }

        ctx.emit(AgentConversationsModelEvent::ConversationsLoaded);
    }

    /// Called when a view that consumes this model's data becomes visible.
    /// Uses view_id to make registration idempotent.
    pub fn register_view_open(
        &mut self,
        window_id: WindowId,
        view_id: EntityId,
        ctx: &mut ModelContext<Self>,
    ) {
        self.active_data_consumers_per_window
            .entry(window_id)
            .or_default()
            .insert(view_id);
        self.sync_conversations(ctx);
    }

    /// Called when a view that consumes this model's data becomes hidden.
    /// Uses view_id to make unregistration idempotent.
    pub fn register_view_closed(
        &mut self,
        window_id: WindowId,
        view_id: EntityId,
        _ctx: &mut ModelContext<Self>,
    ) {
        if let Some(views) = self.active_data_consumers_per_window.get_mut(&window_id) {
            views.remove(&view_id);
            if views.is_empty() {
                self.active_data_consumers_per_window.remove(&window_id);
            }
        }
    }

    /// Returns true if we have local conversations in this view
    pub fn has_items(&self) -> bool {
        !self.conversations.is_empty()
    }

    fn handle_history_event(
        &mut self,
        event: &BlocklistAIHistoryEvent,
        ctx: &mut ModelContext<Self>,
    ) {
        if !FeatureFlag::InteractiveConversationManagementView.is_enabled() {
            return;
        }
        match event {
            // Events that affect conversation navigation data - need full sync
            BlocklistAIHistoryEvent::StartedNewConversation { .. }
            | BlocklistAIHistoryEvent::SetActiveConversation { .. }
            | BlocklistAIHistoryEvent::AppendedExchange { .. }
            | BlocklistAIHistoryEvent::SplitConversation { .. }
            | BlocklistAIHistoryEvent::RestoredConversations { .. }
            | BlocklistAIHistoryEvent::RemoveConversation { .. }
            | BlocklistAIHistoryEvent::DeletedConversation { .. }
            | BlocklistAIHistoryEvent::ClearedConversationsInTerminalView { .. }
            | BlocklistAIHistoryEvent::ClearedActiveConversation { .. } => {
                self.sync_conversations(ctx);
            }

            // Status changes - just trigger re-render since status is looked up at render time
            BlocklistAIHistoryEvent::UpdatedConversationStatus { .. } => {
                ctx.emit(AgentConversationsModelEvent::ConversationUpdated);
            }

            // Artifact changes - notify consumers so they re-read artifacts at render time.
            BlocklistAIHistoryEvent::UpdatedConversationArtifacts {
                conversation_id, ..
            } => {
                ctx.emit(AgentConversationsModelEvent::ConversationArtifactsUpdated {
                    conversation_id: *conversation_id,
                });
            }

            // Exchange-level changes that don't affect conversation navigation.
            BlocklistAIHistoryEvent::CreatedSubtask { .. }
            | BlocklistAIHistoryEvent::UpgradedTask { .. }
            | BlocklistAIHistoryEvent::ReassignedExchange { .. }
            | BlocklistAIHistoryEvent::UpdatedTodoList { .. }
            | BlocklistAIHistoryEvent::UpdatedAutoexecuteOverride { .. }
            | BlocklistAIHistoryEvent::UpdatedConversationMetadata { .. }
            // UpdatedStreamingExchange covers streaming and other exchange-level updates but
            // doesn't change any ConversationNavigationData fields (title comes from
            // UpdateTaskDescription, last_updated uses exchange.start_time which is set at append time).
            | BlocklistAIHistoryEvent::UpdatedStreamingExchange { .. }
            | BlocklistAIHistoryEvent::ConversationAgentIdAssigned { .. }
            => {}
        }
    }

    /// Returns an iterator with all conversations with filters applied, sorted with the
    /// most recently updated items first.
    pub fn get_tasks_and_conversations(
        &self,
        filters: &AgentManagementFilters,
        app: &AppContext,
    ) -> impl Iterator<Item = ConversationOrTask<'_>> {
        let status_filter = move |t: &ConversationOrTask| t.matches_status(&filters.status, app);

        let now = Utc::now();
        let created_cutoff = match filters.created_on {
            CreatedOnFilter::All => None,
            CreatedOnFilter::Last24Hours => Some(now - chrono::Duration::hours(24)),
            CreatedOnFilter::Past3Days => Some(now - chrono::Duration::days(3)),
            CreatedOnFilter::LastWeek => Some(now - chrono::Duration::days(7)),
        };

        let created_on_filter = move |t: &ConversationOrTask| match created_cutoff {
            Some(cutoff) => t.created_at() >= cutoff,
            None => true,
        };

        let artifact_filter_value = filters.artifact;
        let artifact_filter =
            move |t: &ConversationOrTask| t.matches_artifact(&artifact_filter_value, app);

        self.conversations
            .values()
            .map(ConversationOrTask::Conversation)
            .filter(status_filter)
            .filter(created_on_filter)
            .filter(artifact_filter)
            .sorted_by(|a, b| b.last_updated().cmp(&a.last_updated()))
    }

    /// Get a conversation by its AIConversationId
    pub fn get_conversation(
        &self,
        conversation_id: &AIConversationId,
    ) -> Option<ConversationOrTask<'_>> {
        self.conversations
            .get(conversation_id)
            .map(ConversationOrTask::Conversation)
    }

    /// Clears all stored conversation data in memory.
    /// This is used when logging out to ensure no conversation history persists across users.
    pub(crate) fn reset(&mut self) {
        self.conversations.clear();
        self.active_data_consumers_per_window.clear();
        // Reset the initial load flag so that we can retry the initial sync with the new logged in user
        self.has_finished_initial_load = false;
    }
}

#[cfg(test)]
#[path = "agent_conversations_model_tests.rs"]
mod tests;
