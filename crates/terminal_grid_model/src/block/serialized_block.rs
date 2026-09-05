//! R17: SerializedBlock 数据面半下沉 crate::block_serialized。
//! 留守件: SerializedAIMetadata(依赖 app ai cli_controller 类型)、
//! From<&Block>(依赖 15 个 app Block 输出方法)、
//! persistence 转换经扩展 trait(孤儿规则)。
use serde::{Deserialize, Serialize};
use chrono::{DateTime, Local, TimeZone as _};
use warp_core::command::ExitCode;

fn default_as_true() -> bool {
    true
}

fn skip_hide_when_true(value: &bool) -> bool {
    *value
}


use crate::interaction::LongRunningCommandControlState;
use agent_ids::{AIAgentActionId, AIConversationId, TaskId};

use super::{AgentInteractionMetadata, AgentViewVisibility, Block, BlockState, PromptInfo, MAX_SERIALIZED_STYLIZED_OUTPUT_LINES};

pub use crate::block_serialized::{
    SerializedAgentViewVisibility, SerializedBlock, SerializedBlockListItem, ShellHost,
};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SerializedAIMetadata {
    /// The ID of the `AIAgentAction` associated with this block's requested command execution.
    /// This is optional because not all AI-related blocks are associated with a requested command.
    #[serde(alias = "action_id")]
    requested_command_action_id: Option<AIAgentActionId>,

    /// The ID of the conversation to which this action belongs.
    conversation_id: AIConversationId,

    subagent_task_id: Option<TaskId>,

    /// State governing user/agent interaction with the command in this block.
    long_running_control_state: Option<LongRunningCommandControlState>,

    /// `true` if the agent has previously written to this block.
    has_agent_written_to_block: bool,

    /// `true` if this block should be hidden from the user (as is the case with AI-requested
    /// commands, for example).
    #[serde(default = "default_as_true", skip_serializing_if = "skip_hide_when_true")]
    should_hide_block: bool,
}

impl From<AgentInteractionMetadata> for SerializedAIMetadata {
    fn from(value: AgentInteractionMetadata) -> Self {
        SerializedAIMetadata {
            requested_command_action_id: value.requested_command_action_id().cloned(),
            conversation_id: *value.conversation_id(),
            subagent_task_id: value.subagent_task_id().cloned(),
            long_running_control_state: value.long_running_control_state().cloned(),
            has_agent_written_to_block: value.has_agent_written_to_block(),
            should_hide_block: value.should_hide_block(),
        }
    }
}

impl From<SerializedAIMetadata> for AgentInteractionMetadata {
    fn from(value: SerializedAIMetadata) -> Self {
        AgentInteractionMetadata::new(
            value.requested_command_action_id,
            value.conversation_id,
            value.subagent_task_id,
            value.long_running_control_state,
            value.has_agent_written_to_block,
            value.should_hide_block,
        )
    }
}

impl From<&Block> for SerializedBlock {
    fn from(block: &Block) -> Self {
        let stylized_command = block
            .command_with_secrets_unobfuscated(true /*include_escape_sequences*/)
            .into_bytes();
        let stylized_output = block
            .output_grid()
            .contents_to_string_with_secrets_unobfuscated(
                true, /*include_escape_sequences*/
                Some(MAX_SERIALIZED_STYLIZED_OUTPUT_LINES),
            )
            .into_bytes();
        let ps1 =
            (!block.is_prompt_empty()).then(|| hex::encode(block.prompt_contents_to_string(true)));
        let rprompt = (!block.rprompt_grid().is_empty()).then(|| {
            let mut grid_content = block.rprompt_grid().contents_to_string(true, None);
            // We mustn't allow trailing newlines in the rprompt grid. This is b/c a trailing
            // newline will cause the Grid::max_cursor::column to go to 0. That is a problem b/c we
            // assume that the Grid::max_cursor::column is the length of the rprompt, and that
            // value is used to calculate the left-alignment position when painting the block.
            while grid_content.ends_with('\n') || grid_content.ends_with('\r') {
                grid_content.pop();
            }
            hex::encode(grid_content)
        });
        let prompt_snapshot = block
            .prompt_snapshot
            .as_ref()
            .and_then(|prompt_snapshot| serde_json::to_string(prompt_snapshot).ok());
        let prompt_info = PromptInfo {
            pwd: block.pwd().map(String::from),
            git_branch: block.git_branch.clone(),
            git_branch_name: block.git_branch_name.clone(),
            virtual_env: block.virtual_env.clone(),
            conda_env: block.conda_env.clone(),
            node_version: block.node_version.clone(),
            ps1,
            rprompt,
            honor_ps1: block.honor_ps1(),
            prompt_snapshot,
        };

        let ai_metadata = block
            .agent_interaction_metadata()
            .cloned()
            .map(Into::<SerializedAIMetadata>::into)
            .and_then(|metadata| serde_json::to_string(&metadata).ok());

        SerializedBlock {
            id: block.id.clone(),
            stylized_command,
            stylized_output,
            pwd: prompt_info.pwd,
            git_head: prompt_info.git_branch,
            git_branch_name: prompt_info.git_branch_name,
            virtual_env: prompt_info.virtual_env,
            conda_env: prompt_info.conda_env,
            node_version: prompt_info.node_version,
            exit_code: block.exit_code,
            did_execute: block.state == BlockState::Executing
                || block.state == BlockState::DoneWithExecution
                || block.state == BlockState::Static,
            completed_ts: block.completed_ts,
            start_ts: block.start_ts,
            is_background: block.is_background(),
            ps1: prompt_info.ps1,
            rprompt: prompt_info.rprompt,
            honor_ps1: prompt_info.honor_ps1,
            session_id: block.session_id,
            shell_host: block.shell_host.clone(),
            prompt_snapshot: prompt_info.prompt_snapshot,
            ai_metadata,
            is_local: None,
            agent_view_visibility: Some(block.agent_view_visibility().clone().into()),
        }
    }
}

impl From<AgentViewVisibility> for SerializedAgentViewVisibility {
    fn from(value: AgentViewVisibility) -> Self {
        match value {
            AgentViewVisibility::Terminal {
                pending_conversation_ids,
                conversation_ids,
            } => SerializedAgentViewVisibility::Terminal {
                pending_conversation_ids,
                conversation_ids,
            },
            AgentViewVisibility::Agent {
                origin_conversation_id,
                pending_other_conversation_ids,
                other_conversation_ids,
            } => SerializedAgentViewVisibility::Agent {
                origin_conversation_id,
                pending_other_conversation_ids,
                other_conversation_ids,
            },
        }
    }
}

impl From<SerializedAgentViewVisibility> for AgentViewVisibility {
    fn from(value: SerializedAgentViewVisibility) -> Self {
        match value {
            SerializedAgentViewVisibility::Terminal {
                pending_conversation_ids,
                conversation_ids,
            } => AgentViewVisibility::Terminal {
                pending_conversation_ids,
                conversation_ids,
            },
            SerializedAgentViewVisibility::Agent {
                origin_conversation_id,
                pending_other_conversation_ids,
                other_conversation_ids,
            } => AgentViewVisibility::Agent {
                origin_conversation_id,
                pending_other_conversation_ids,
                other_conversation_ids,
            },
        }
    }
}
