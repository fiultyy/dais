//! R23: agent 交互控制类型族(自 app ai/blocklist/block/cli_controller 与
//! terminal/model/block/interaction_mode 提升)。字段全部为
//! agent_ids 单源原语或纯数据, 无 app 依赖。

use agent_ids::{AIAgentActionId, AIConversationId, TaskId};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub enum UserTakeOverReason {
    Manual,
    Stop,
    /// The agent explicitly transferred control to the user via the
    /// TransferShellCommandControlToUser tool call.
    TransferFromAgent {
        /// The reason the agent gave for transferring control.
        reason: String,
    },
}

impl UserTakeOverReason {
pub fn is_stop(&self) -> bool {
        matches!(self, Self::Stop)
    }
pub fn is_transfer_from_agent(&self) -> bool {
        matches!(self, Self::TransferFromAgent { .. })
    }
pub fn transfer_reason(&self) -> Option<&str> {
        match self {
            Self::TransferFromAgent { reason } => Some(reason.as_str()),
            _ => None,
        }
    }
}
/// Represents which party is in control of the active long running command.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub enum LongRunningCommandControlState {
    /// The agent is in control.
    ///
    /// When the agent has control, the user cannot submit input to the command.
    Agent {
        /// `true` if the agent is blocked on approval from the user for submitting input.
        is_blocked: bool,
        /// `true` if agent responses should be hidden in the UI.
        should_hide_responses: bool,
    },
    /// The user is in control.
    User { reason: UserTakeOverReason },
}

impl LongRunningCommandControlState {
pub fn is_agent_in_control(&self) -> bool {
        matches!(self, Self::Agent { .. })
    }
pub fn is_agent_blocked(&self) -> bool {
        matches!(
            self,
            Self::Agent {
                is_blocked: true,
                ..
            }
        )
    }
pub fn is_user_in_control(&self) -> bool {
        matches!(self, Self::User { .. })
    }
pub fn should_hide_responses(&self) -> bool {
        matches!(
            self,
            Self::Agent {
                should_hide_responses: true,
                ..
            }
        )
    }
pub fn user_take_over_reason(&self) -> Option<&UserTakeOverReason> {
        match &self {
            LongRunningCommandControlState::Agent { .. } => None,
            LongRunningCommandControlState::User { reason } => Some(reason),
        }
    }
}

// ---- R23: InteractionMode 类型族(自 app block/interaction_mode 提升) ----

// ---- R23: InteractionMode 类型族(自 app block/interaction_mode 提升) ----

#[derive(Debug, Clone, thiserror::Error)]
pub enum UpdateInteractionModeError {
    #[error("Attempted to update interaction mode from agent with requested command to agent-monitored for mismatched conversation IDs.")]
    UnexpectedConversationId,
    #[error("Attempted to take over control for user when block was not already agent controlled")]
    InvalidTakeOver,
    #[error("Attempted to handoff control to agent when block was not already user controlled")]
    InvalidHandOff,
}

#[derive(Debug, Clone)]
pub struct UserMode {
    // `true` if the user executed the command themself and the agent mode input should be shown.
    //
    // This does _not_ mean an agent is in control of the command. This merely means the user has
    // opted to show the agent input, indicated intent to send a query to give control.
    //
    // If the user executes a command, shows the input, then hides the input, this reverts to
    // `false`.
    pub did_user_tag_in_agent: bool,
}

/// Represents the 'interaction mode' for a command block with respect to the agent.
///
/// There are 4 user-perceived states:
///
/// 1) The command was executed by the user; if long-running, they are in control and the input is hidden.
/// 2) The command was executed by the user, but is long-running and the user has toggled on the
///    agent input and may send a query to trigger the Agent (the CLI subagent). We refer to this
///    state, where the user executed the command and deliberately opened the agent input, as the
///    agent being 'tagged in'. Note that this is distinct from the agent actually having control.
///    "Tagged in" merely means the command is running and the agent mode input is visible
/// 3) The command was executed by the agent (is a requested command) and is not long running. No CLI subagent was triggered.
/// 4) The command was executed by the agent and is long running, and thus the CLI subagent was triggered.
///   a) The agent is in control of the command (actively reading the command's output or writing input to the command)
///   b) The agent was in the control of the command, but the user took over.
///
/// The `User` variant represents modes where the user executed the original command and the agent has yet to take control.
/// The `Agent` variant represents modes where the agent either ran the command itself, or the user tagged in the agent and
/// passed control to the CLI subagent by sending a query during its execution
#[derive(Debug, Clone)]
pub enum InteractionMode {
    User(UserMode),
    Agent(AgentInteractionMetadata),
}

impl InteractionMode {
pub fn to_agent_monitored(
        &self,
        task_id: &TaskId,
        conversation_id: AIConversationId,
    ) -> Result<Self, UpdateInteractionModeError> {
        let requested_command_action_id = match self {
            InteractionMode::User(_) => None,
            InteractionMode::Agent(metadata) => {
                if metadata.conversation_id != conversation_id {
                    return Err(UpdateInteractionModeError::UnexpectedConversationId);
                }
                metadata.requested_command_action_id.clone()
            }
        };

        Ok(Self::Agent(AgentInteractionMetadata {
            requested_command_action_id,
            conversation_id,
            subagent_task_id: Some(task_id.clone()),
            long_running_control_state: Some(LongRunningCommandControlState::Agent {
                is_blocked: false,
                should_hide_responses: false,
            }),
            has_agent_written_to_block: false,
            should_hide_block: false,
        }))
    }
pub fn new_agent(metadata: AgentInteractionMetadata) -> Self {
        Self::Agent(metadata)
    }
pub fn agent_interaction_metadata(&self) -> Option<&AgentInteractionMetadata> {
        match self {
            Self::Agent(agent_interaction_metadata) => Some(agent_interaction_metadata),
            Self::User(_) => None,
        }
    }
pub fn should_hide_block(&self) -> bool {
        match self {
            Self::Agent(metadata) => metadata.should_hide_block,
            _ => false,
        }
    }
pub fn long_running_control_state(&self) -> Option<&LongRunningCommandControlState> {
        match self {
            Self::Agent(metadata) => metadata.long_running_control_state.as_ref(),
            _ => None,
        }
    }
pub fn is_agent_tagged_in(&self) -> bool {
        matches!(
            self,
            Self::User(UserMode {
                did_user_tag_in_agent: true
            })
        )
    }
pub fn set_should_hide_block(&mut self, value: bool) {
        if let Self::Agent(metadata) = self {
            metadata.should_hide_block = value;
        }
    }

    pub fn take_over_for_user(
        &mut self,
        reason: UserTakeOverReason,
    ) -> Result<(), UpdateInteractionModeError> {
        let Self::Agent(AgentInteractionMetadata {
            ref mut long_running_control_state,
            ..
        }) = self
        else {
            return Err(UpdateInteractionModeError::InvalidTakeOver);
        };

        if !long_running_control_state
            .as_ref()
            .is_some_and(|state| state.is_agent_in_control())
        {
            return Err(UpdateInteractionModeError::InvalidTakeOver);
        }

        *long_running_control_state = Some(LongRunningCommandControlState::User { reason });
        Ok(())
    }

    pub fn handoff_to_agent(&mut self) -> Result<(), UpdateInteractionModeError> {
        let Self::Agent(AgentInteractionMetadata {
            ref mut long_running_control_state,
            ..
        }) = self
        else {
            return Err(UpdateInteractionModeError::InvalidHandOff);
        };

        if !long_running_control_state
            .as_ref()
            .is_some_and(|state| state.is_user_in_control())
        {
            return Err(UpdateInteractionModeError::InvalidHandOff);
        }

        *long_running_control_state = Some(LongRunningCommandControlState::Agent {
            is_blocked: false,
            should_hide_responses: false,
        });
        Ok(())
    }
}

impl Default for InteractionMode {
    fn default() -> Self {
        Self::User(UserMode {
            did_user_tag_in_agent: false,
        })
    }
}

/// Blocklist AI metadata associated with this block.
#[derive(Debug, Clone)]
pub struct AgentInteractionMetadata {
    /// The ID of the `AIAgentAction` associated with this block's requested command execution.
    /// This is optional because not all AI-related blocks are associated with a requested command.
    pub requested_command_action_id: Option<AIAgentActionId>,

    /// The ID of the conversation to which this action belongs.
    pub conversation_id: AIConversationId,

    /// The task ID for the CLI subagent interaction with this block if any.
    pub subagent_task_id: Option<TaskId>,

    /// State governing user/agent interaction with the command in this block.
    pub long_running_control_state: Option<LongRunningCommandControlState>,

    /// `true` if the agent has previously written to this block.
    pub has_agent_written_to_block: bool,

    /// `true` if this block should be hidden from the user (as is the case with AI-requested
    /// commands, for example).
    pub should_hide_block: bool,
}

impl AgentInteractionMetadata {
    /// Creates a new metadata instance with fully specified fields.
pub fn new(
        requested_command_action_id: Option<AIAgentActionId>,
        conversation_id: AIConversationId,
        subagent_task_id: Option<TaskId>,
        long_running_control_state: Option<LongRunningCommandControlState>,
        has_agent_written_to_block: bool,
        should_hide_block: bool,
    ) -> Self {
        AgentInteractionMetadata {
            requested_command_action_id,
            conversation_id,
            subagent_task_id,
            long_running_control_state,
            has_agent_written_to_block,
            should_hide_block,
        }
    }

    /// Convenience constructor for the common "hidden by default" case used for requested commands.
pub fn new_hidden(
        requested_command_action_id: AIAgentActionId,
        conversation_id: AIConversationId,
    ) -> Self {
        Self::new(
            Some(requested_command_action_id),
            conversation_id,
            None,
            None,
            false,
            true,
        )
    }
pub fn requested_command_action_id(&self) -> Option<&AIAgentActionId> {
        self.requested_command_action_id.as_ref()
    }
pub fn conversation_id(&self) -> &AIConversationId {
        &self.conversation_id
    }
pub fn subagent_task_id(&self) -> Option<&TaskId> {
        self.subagent_task_id.as_ref()
    }
pub fn is_agent_in_control(&self) -> bool {
        self.long_running_control_state
            .as_ref()
            .is_some_and(|state| state.is_agent_in_control())
    }
pub fn long_running_control_state(&self) -> Option<&LongRunningCommandControlState> {
        self.long_running_control_state.as_ref()
    }
pub fn has_agent_written_to_block(&self) -> bool {
        self.has_agent_written_to_block
    }
pub fn should_hide_block(&self) -> bool {
        self.should_hide_block
    }
}
