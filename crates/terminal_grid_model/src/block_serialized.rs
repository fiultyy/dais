//! R17: SerializedBlock 数据面(自 app/terminal/model/block/serialized_block.rs 半下沉)。
//! From<&Block>(依赖 15 个 app Block 输出方法)与 SerializedAIMetadata(依赖
//! app ai cli_controller 类型)留守 app;From<persistence::Block> 经 app 扩展
//! trait 承载(persistence 类型双向皆外)。
//! serde 兼容: 字段名/alias(shell/git_branch/conversation_id)/serde_bytes/
//! default 逐一原样迁移,持久化字节级不变。
use std::collections::HashSet;

use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};
use warp_core::SessionId;
use crate::block_id::BlockId;
use warp_terminal::shell::ShellType;

/// R17: 自 app/terminal/history.rs 提升(3 字段纯数据)。
/// serde alias "shell" 保留——旧 JSON/列兼容。
#[derive(Debug, PartialEq, Eq, Hash, Clone, Serialize, Deserialize)]
pub struct ShellHost {
    #[serde(alias = "shell")]
    pub shell_type: ShellType,
    pub user: String,
    pub hostname: String,
}

impl ShellHost {
    /// 参数化构造(替代 try_from_persisted_block,不引 persistence 依赖)。
    pub fn from_parts(
        shell_name: Option<&str>,
        user: Option<&str>,
        hostname: Option<&str>,
    ) -> Option<Self> {
        let shell_type = ShellType::from_name(shell_name?)?;
        Some(Self {
            shell_type,
            user: user.unwrap_or("local:user").to_owned(),
            hostname: hostname.unwrap_or("local:host").to_owned(),
        })
    }
}

/// Tracks which views (terminal and/or agent conversations) this block should be visible in.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum SerializedAgentViewVisibility {
    Terminal {
        #[serde(default)]
        pending_conversation_ids: HashSet<agent_ids::AIConversationId>,
        conversation_ids: HashSet<agent_ids::AIConversationId>,
    },
    Agent {
        #[serde(alias = "conversation_id")]
        origin_conversation_id: agent_ids::AIConversationId,
        #[serde(default)]
        pending_other_conversation_ids: HashSet<agent_ids::AIConversationId>,
        #[serde(default)]
        other_conversation_ids: HashSet<agent_ids::AIConversationId>,
    },
}

#[derive(Clone, Debug, Serialize, Default, Deserialize, PartialEq)]
pub struct SerializedBlock {
    pub id: BlockId,
    /// The input lines with their corresponding escape sequences so it can be rendered outside of
    /// the terminal.
    #[serde(with = "serde_bytes")]
    pub stylized_command: Vec<u8>,

    /// The output lines with their corresponding escape sequences so it can be rendered outside of
    /// the terminal.
    /// They are truncated to MAX_SERIALIZED_STYLIZED_OUTPUT_LINES lines.
    #[serde(with = "serde_bytes")]
    pub stylized_output: Vec<u8>,

    /// The current working directory of the block.
    pub pwd: Option<String>,

    #[serde(alias = "git_branch")]
    pub git_head: Option<String>,

    #[serde(default)]
    pub git_branch_name: Option<String>,

    pub virtual_env: Option<String>,

    pub conda_env: Option<String>,

    pub node_version: Option<String>,

    pub exit_code: warp_core::command::ExitCode,

    /// True iff the block _started_ executing (i.e. preexec was received) or it's a static block.
    pub did_execute: bool,

    pub completed_ts: Option<DateTime<Local>>,

    pub start_ts: Option<DateTime<Local>>,

    pub ps1: Option<String>,

    pub rprompt: Option<String>,

    pub honor_ps1: bool,

    pub is_background: bool,

    pub session_id: Option<SessionId>,

    pub shell_host: Option<ShellHost>,

    /// JSON-serialized representation of the Dais prompt snapshot (Context Chips). Note that this
    /// is different from PS1 and RPROMPT1
    pub prompt_snapshot: Option<String>,

    /// JSON-serialized representation of [`crate::block_serialized_app_shim note:
    /// SerializedAIMetadata 留守 app]。
    pub ai_metadata: Option<String>,

    /// Whether this block was created locally (true) or remotely (false)
    #[serde(default)]
    pub is_local: Option<bool>,

    /// Tracks which views (terminal and/or agent conversations) this block should be visible in.
    #[serde(default)]
    pub agent_view_visibility: Option<SerializedAgentViewVisibility>,
}

impl SerializedBlock {
    /// Sets the command & output and `did_execute` to true.
    /// Everything else is a default value.
    #[cfg(any(test, feature = "test-support"))]
    pub fn new_for_test(stylized_command: Vec<u8>, stylized_output: Vec<u8>) -> SerializedBlock {
        SerializedBlock {
            stylized_command,
            stylized_output,
            did_execute: true,
            start_ts: Some(Local::now()),
            completed_ts: Some(Local::now()),
            ..Default::default()
        }
    }

    /// Sets only the command with no output, and `did_execute` to false.
    /// Everything else is a default value.
    #[cfg(any(test, feature = "test-support"))]
    pub fn new_active_block_for_test() -> SerializedBlock {
        SerializedBlock::default()
    }

    /// Serialize this block to JSON bytes.
    ///
    /// The command and output contents are base64-encoded. This is *not* the default serde behavior,
    /// and blocks encoded this way must be deserialized with [`Self::from_json`].
    pub fn to_json(&self) -> anyhow::Result<Vec<u8>> {
        let mut buf = Vec::new();
        let mut ser = serde_json::Serializer::new(&mut buf);
        let base64_config = base64::engine::GeneralPurposeConfig::new();
        let base64_ser =
            serde_bytes_repr::ByteFmtSerializer::base64(&mut ser, base64::alphabet::STANDARD, base64_config);
        self.serialize(base64_ser)
            .map_err(|e| anyhow::anyhow!("Failed to serialize block to JSON: {e}"))?;
        Ok(buf)
    }

    /// Deserialize a block from JSON bytes produced by [`Self::to_json`]
    /// or [`serde_json`].
    pub fn from_json(json: &[u8]) -> anyhow::Result<Self> {
        let mut de = serde_json::Deserializer::from_slice(json);
        let base64_config = base64::engine::GeneralPurposeConfig::new();
        let base64_de =
            serde_bytes_repr::ByteFmtDeserializer::new_base64(&mut de, base64::alphabet::STANDARD, base64_config);
        Self::deserialize(base64_de)
            .map_err(|e| anyhow::anyhow!("Failed to deserialize block from JSON: {e}"))
    }

    /// R17: has_block_failed(app pub(super))语义内联——与原
    /// `did_execute→DoneWithExecution + has_block_failed` 等价。
    pub fn has_failed(&self) -> bool {
        self.did_execute && !self.exit_code.was_successful()
    }
}

/// R16 定义提升(R17 随 SerializedBlock 归位 crate)。
#[derive(Debug, Clone, PartialEq)]
pub enum SerializedBlockListItem {
    Command { block: Box<SerializedBlock> },
}

impl SerializedBlockListItem {
    pub fn start_ts(&self) -> Option<DateTime<Local>> {
        match self {
            Self::Command { block } => block.start_ts,
        }
    }
}

impl From<SerializedBlock> for SerializedBlockListItem {
    fn from(value: SerializedBlock) -> Self {
        Self::Command {
            block: Box::new(value),
        }
    }
}
