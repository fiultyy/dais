use anyhow::anyhow;
use warp_terminal::model::{grid::Dimensions, Point};

use crate::{
    ai::{ agent::{conversation::AIConversationId, task::TaskId, AIAgentActionId} },
    terminal::{ event::Event, model::{ grid::{grid_handler::GridHandler, RespectDisplayedOutput}, RespectObfuscatedSecrets, }, },
};

use super::{Block, SerializedAIMetadata};

impl Block {
    /// `true` if the command is executing and the user has opened the agent mode input.
    ///
    /// "tagged in" means that the agent mode input should be shown, but control has yet
    /// to be passed to the agent.
    pub fn is_agent_tagged_in(&self) -> bool {
        if !self.is_active_and_long_running() {
            return false;
        }

        match &self.interaction_mode {
            InteractionMode::User(user_mode) => user_mode.did_user_tag_in_agent,
            _ => false,
        }
    }

    /// `true` if the command is eligible for tagging in the agent (e.g. showing the agent mode
    /// input and sending a query to trigger the CLI subagent).
    ///
    /// Notably, this is NOT true if the subagent had already taken control of the command, and
    /// the user took control back from the subagent.
    ///
    /// See doc comment on `InteractionMode` for explanation on the semantics of 'tagged in',
    /// 'agent in control', 'user take control, and 'agent handoff'.
    pub fn is_eligible_to_tag_in_agent(&self) -> bool {
        if !self.is_active_and_long_running()
            || self.is_in_band_command_block()
            || !self.bootstrap_stage.is_bootstrapped()
            || self.env_var_metadata().is_some()
        {
            return false;
        }

        match &self.interaction_mode {
            InteractionMode::User(user_mode) => !user_mode.did_user_tag_in_agent,
            _ => false,
        }
    }

    pub fn set_is_agent_tagged_in(&mut self, value: bool) {
        if let InteractionMode::User(UserMode {
            ref mut did_user_tag_in_agent,
        }) = &mut self.interaction_mode
        {
            if *did_user_tag_in_agent != value {
                *did_user_tag_in_agent = value;
                self.event_sink.send_agent_tagged_in_changed(value);
            }
        }
    }

    /// Returns `true` if an agent is monitoring/interacting with this command.
    pub fn is_agent_monitoring(&self) -> bool {
        self.is_active_and_long_running() && self.long_running_control_state().is_some()
    }

    /// Returns `true` if the agent is either in control or has been tagged in by the user.
    pub fn is_agent_in_control_or_tagged_in(&self) -> bool {
        self.is_agent_in_control() || self.is_agent_tagged_in()
    }

    pub fn cli_subagent_task_id(&self) -> Option<&TaskId> {
        self.agent_interaction_metadata()
            .and_then(|metadata| metadata.subagent_task_id())
    }

    pub fn upgrade_cli_subagent_task_id(&mut self, new_task_id: TaskId) -> anyhow::Result<()> {
        if let InteractionMode::Agent(AgentInteractionMetadata {
            subagent_task_id: Some(ref mut task_id),
            ..
        }) = &mut self.interaction_mode
        {
            *task_id = new_task_id;
            Ok(())
        } else {
            Err(anyhow!("Tried to upgrade CLI subagent task ID for block with no prior CLI subagent task ID."))
        }
    }

    /// Returns `true` if this command is active and the agent is in control.
    pub fn is_agent_in_control(&self) -> bool {
        self.is_active_and_long_running()
            && self
                .long_running_control_state()
                .is_some_and(LongRunningCommandControlState::is_agent_in_control)
    }

    /// Returns `true` if the agent is actively driving this command.
    ///
    /// This is broader than `is_agent_in_control`: it also covers the window between
    /// when the agent writes an agent-requested command to the PTY (synchronous) and
    /// when the CLI subagent is later spawned and `long_running_control_state` is set
    /// (asynchronous, via `BlocklistAIHistoryEvent::CreatedSubtask`). Returns `false`
    /// once the user takes over, even for agent-initiated commands.
    pub fn is_agent_driving_command(&self) -> bool {
        if self.is_agent_in_control() {
            return true;
        }
        // Agent-initiated command where the CLI subagent hasn't formally taken control yet.
        self.interaction_mode
            .agent_interaction_metadata()
            .is_some_and(|metadata| {
                metadata.requested_command_action_id().is_some()
                    && metadata.long_running_control_state().is_none()
            })
    }

    /// Returns `true` if the agent's interaction with this command is currently blocked by user
    /// approval.
    pub fn is_agent_blocked(&self) -> bool {
        self.is_active_and_long_running()
            && self
                .long_running_control_state()
                .is_some_and(LongRunningCommandControlState::is_agent_blocked)
    }

    /// Returns `true` if the command is eligible to be handed off to an agent.
    pub fn is_eligible_for_agent_handoff(&self) -> bool {
        self.is_active_and_long_running()
            && self
                .long_running_control_state()
                .is_some_and(LongRunningCommandControlState::is_user_in_control)
    }

    pub fn update_is_agent_blocked(&mut self, new_value: bool) {
        if let InteractionMode::Agent(AgentInteractionMetadata {
            long_running_control_state:
                Some(LongRunningCommandControlState::Agent {
                    ref mut is_blocked, ..
                }),
            ..
        }) = self.interaction_mode
        {
            *is_blocked = new_value;
        }
    }

    /// Sets control to user with Stop reason if a long-running control state exists.
    pub fn set_user_control_with_stop_reason(&mut self) {
        if let InteractionMode::Agent(AgentInteractionMetadata {
            long_running_control_state: Some(ref mut state),
            ..
        }) = self.interaction_mode
        {
            *state = LongRunningCommandControlState::User {
                reason: UserTakeOverReason::Stop,
            };
        }
    }

    /// Returns `true` if agent responses should be hidden in the UI.
    pub fn should_hide_responses(&self) -> bool {
        self.is_active_and_long_running()
            && self
                .long_running_control_state()
                .is_some_and(LongRunningCommandControlState::should_hide_responses)
    }

    /// Returns the `agent_interaction_metadata` associated with this block, if any.
    pub fn agent_interaction_metadata(&self) -> Option<&AgentInteractionMetadata> {
        self.interaction_mode.agent_interaction_metadata()
    }

    pub fn ai_conversation_id(&self) -> Option<AIConversationId> {
        match &self.interaction_mode {
            InteractionMode::Agent(metadata) => Some(metadata.conversation_id),
            _ => None,
        }
    }

    pub fn requested_command_action_id(&self) -> Option<&AIAgentActionId> {
        match &self.interaction_mode {
            InteractionMode::Agent(metadata) => metadata.requested_command_action_id(),
            _ => None,
        }
    }

    /// Returns the `long_running_control_state` associated with this block, if any.
    pub fn long_running_control_state(&self) -> Option<&LongRunningCommandControlState> {
        self.interaction_mode.long_running_control_state()
    }

    pub fn has_agent_written_to_block(&self) -> bool {
        self.interaction_mode
            .agent_interaction_metadata()
            .is_some_and(|metadata| metadata.has_agent_written_to_block())
    }

    pub fn mark_agent_written_to_block(&mut self) {
        if let InteractionMode::Agent(metadata) = &mut self.interaction_mode {
            metadata.has_agent_written_to_block = true;
        }
    }

    pub fn set_should_hide(&mut self, value: bool) {
        self.interaction_mode.set_should_hide_block(value);
    }

    pub fn set_agent_interaction_mode_for_requested_command(
        &mut self,
        requested_command_action_id: AIAgentActionId,
        subagent_task_id: Option<TaskId>,
        conversation_id: AIConversationId,
    ) {
        self.interaction_mode = InteractionMode::Agent(AgentInteractionMetadata {
            requested_command_action_id: Some(requested_command_action_id),
            conversation_id,
            subagent_task_id,
            long_running_control_state: None,
            has_agent_written_to_block: false,
            should_hide_block: true,
        })
    }

    pub fn set_agent_interaction_mode_for_agent_monitored_command(
        &mut self,
        task_id: &TaskId,
        conversation_id: AIConversationId,
    ) -> Result<(), UpdateInteractionModeError> {
        let new_mode = self
            .interaction_mode
            .to_agent_monitored(task_id, conversation_id)?;
        self.interaction_mode = new_mode;
        Ok(())
    }

    pub fn set_agent_interaction_mode(
        &mut self,
        agent_interaction_metadata: AgentInteractionMetadata,
    ) {
        self.interaction_mode = InteractionMode::new_agent(agent_interaction_metadata);
    }

    pub fn set_interaction_mode_from_serialized_ai_metadata(
        &mut self,
        serialized_metadata: SerializedAIMetadata,
    ) {
        self.interaction_mode = InteractionMode::Agent(serialized_metadata.into());
    }

    pub fn take_over_control_for_user(
        &mut self,
        reason: UserTakeOverReason,
    ) -> Result<(), UpdateInteractionModeError> {
        self.interaction_mode.take_over_for_user(reason)
    }

    pub fn handoff_control_to_agent(&mut self) -> Result<(), UpdateInteractionModeError> {
        self.interaction_mode.handoff_to_agent()
    }

    /// Returns true if the interaction mode is agent-monitored and subagent response visibility was actually toggled.
    pub fn toggle_subagent_response_visibility(&mut self) -> bool {
        match &mut self.interaction_mode {
            InteractionMode::Agent(AgentInteractionMetadata {
                long_running_control_state:
                    Some(LongRunningCommandControlState::Agent {
                        ref mut should_hide_responses,
                        ..
                    }),
                ..
            }) => {
                *should_hide_responses = !*should_hide_responses;
                true
            }
            _ => false,
        }
    }
}
// R23: InteractionMode 类型族提升 terminal_grid_model::interaction;本文件仅存 Block 交互方法。
pub use terminal_grid_model::interaction::{
    AgentInteractionMetadata, InteractionMode, LongRunningCommandControlState,
    UpdateInteractionModeError, UserMode, UserTakeOverReason,
};

/// String representation of the cursor to interpolate in the terminal contents string.
pub const CURSOR_MARKER: &str = "<|cursor|>";

/// Returns a string representation of the terminal contents (represented by the `grid_handler`),
/// limited to `max_row_count` rows in the grid.
///
/// This function returns a string representation of the terminal contents, with a cursor "marker" substring
/// interpolated at the same position in the string as it appears in the grid.
pub fn formatted_terminal_contents_for_input(
    grid_handler: &GridHandler,
    max_row_count: Option<usize>,
    cursor_pattern: &'static str,
) -> String {
    let cursor_point = grid_handler.cursor_point();

    let max_column_index = grid_handler.columns().saturating_sub(1);
    let (context_start_point, context_end_point) = match max_row_count {
        Some(max_count) => {
            // Return start and end points such that the range is of size max_count, bounded to the
            // max row value of the grid.
            let end_point = Point::new(grid_handler.max_content_row(), max_column_index).min(
                Point::new(cursor_point.row + max_count / 2, max_column_index),
            );
            let start_point = Point::new(end_point.row.saturating_sub(max_count), 0);
            (start_point, end_point)
        }
        None => (
            Point::new(0, 0),
            Point::new(
                grid_handler.total_rows().saturating_sub(1),
                grid_handler.columns().saturating_sub(1),
            ),
        ),
    };

    format!(
        "{}{}{cursor_pattern}{}",
        grid_handler.bounds_to_string(
            context_start_point,
            if cursor_point.col == 0 {
                Point::new(
                    cursor_point.row.saturating_sub(1),
                    grid_handler.columns().saturating_sub(1),
                )
            } else {
                Point::new(cursor_point.row, cursor_point.col.saturating_sub(1))
            },
            false,
            RespectObfuscatedSecrets::Yes,
            true,
            RespectDisplayedOutput::No,
        ),
        if cursor_point.col == 0 { "\n" } else { "" },
        grid_handler.bounds_to_string(
            cursor_point,
            context_end_point,
            false,
            RespectObfuscatedSecrets::Yes,
            true,
            RespectDisplayedOutput::No,
        )
    )
}
