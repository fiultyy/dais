//! R24: Block 本体迁移 terminal_grid_model::block;本文件为 app 侧 mod 级 shim
//! + FromPersisted 留守件(依赖 app persistence 模型, 对 crate 类型合法)。

use chrono::{Local, TimeZone as _};
use crate::persistence;
use warp_core::command::ExitCode;
pub use terminal_grid_model::block_serialized::{SerializedBlock, SerializedBlockListItem};

pub trait SerializedBlockFromPersisted {
    fn from_persisted(value: crate::persistence::model::Block) -> Self;
}

impl SerializedBlockFromPersisted for SerializedBlock {
    fn from_persisted(block: crate::persistence::model::Block) -> Self {
        let exit_code = ExitCode::from(block.exit_code);
        Self {
            shell_host: crate::terminal::history::try_from_persisted_block(&block),
            id: block.block_id.into(),
            stylized_command: block.stylized_command,
            stylized_output: block.stylized_output,
            pwd: block.pwd,
            git_head: block.git_branch,
            git_branch_name: block.git_branch_name,
            virtual_env: block.virtual_env,
            conda_env: block.conda_env,
            node_version: None, // Database does not store node_version yet
            exit_code,
            did_execute: block.did_execute,
            completed_ts: block
                .completed_ts
                .map(|naive_ts| Local.from_utc_datetime(&naive_ts)),
            start_ts: block
                .start_ts
                .map(|naive_ts| Local.from_utc_datetime(&naive_ts)),
            ps1: block.ps1,
            rprompt: block.rprompt,
            honor_ps1: block.honor_ps1,
            session_id: None,
            is_background: block.is_background,
            prompt_snapshot: block.prompt_snapshot,
            ai_metadata: block.ai_metadata,
            is_local: block.is_local,
            agent_view_visibility: block
                .agent_view_visibility
                .and_then(|json| serde_json::from_str(&json).ok()),
        }
    }
}

pub trait SerializedBlockListItemFromPersisted {
    fn from_persisted(value: crate::persistence::model::Block) -> Self;
}

impl SerializedBlockListItemFromPersisted for SerializedBlockListItem {
    fn from_persisted(value: crate::persistence::model::Block) -> Self {
        Self::Command {
            block: Box::new(SerializedBlock::from_persisted(value)),
        }
    }
}

#[cfg(test)]
#[path = "block_test.rs"]
mod tests;
