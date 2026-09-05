use std::time::Duration;

use instant::Instant;
use objects::ids::SyncId;

use crate::block_serialized::SerializedBlock;

use warp_core::SessionId;
use warp_terminal::model::{BlockId, BlockIndex};

#[derive(Clone)]
pub struct BlockLatencyData {
    pub command: &'static str,
    /// When the block's command grid was started (i.e. when the user hit enter).
    pub started_at: Instant,
}

#[derive(Clone, Debug)]
pub struct BlockMetadata {
    session_id: Option<SessionId>,
    current_working_directory: Option<String>,
}

impl BlockMetadata {
    pub fn new(session_id: Option<SessionId>, current_working_directory: Option<String>) -> Self {
        BlockMetadata {
            session_id,
            current_working_directory,
        }
    }

    pub fn session_id(&self) -> Option<SessionId> {
        self.session_id
    }

    pub fn current_working_directory(&self) -> Option<&str> {
        self.current_working_directory.as_deref()
    }
}

#[derive(Clone, Debug)]
pub struct BlockMetadataReceivedEvent {
    pub block_metadata: BlockMetadata,
    pub block_index: BlockIndex,
    /// Whether the previous block was an in-band command.
    pub is_after_in_band_command: bool,
    /// Whether the session has fully completed the bootstrapping process.
    pub is_done_bootstrapping: bool,
}

/// R17: 自 app session/command_executor 提升(零状态自由函数)。
/// In-band commands are prefixed with a leading space for Fish, which is done to omit them from
/// fish's command history.  Thus we strip leading whitespace before matching the `command`.
pub fn is_in_band_command(command: &str) -> bool {
    let trimmed = command.trim_start();
    trimmed.starts_with("Zap-Run-GeneratorCommand ")
        || trimmed.starts_with("warp_run_generator_command ")
}

// ---- R20: BlockType 载荷族(SerializedBlock crate 化后链解锁, 自 app event.rs 提升) ----

#[derive(Clone)]
pub struct BlockCompletedEvent {
    /// This will be None when we don't want to collect telemetry
    /// for this block's latency.
    pub block_latency_data: Option<BlockLatencyData>,
    pub block_type: BlockType,
    pub num_secrets_obfuscated: usize,
    pub block_index: BlockIndex,
    pub block_id: BlockId,
    pub session_id: Option<SessionId>,
    pub restored_block_was_local: Option<bool>,
}
#[derive(Clone)]
pub struct AfterBlockCompletedEvent {
    /// The delay from the CommandFinished ansi hook to the Precmd hook.
    /// This value is only provided for the blocks that the user directly
    /// executes (so it's not provided if this is a restored block from session
    /// restoration or a bootstrapping block).
    pub command_finished_to_precmd_delay: Option<Duration>,
    pub block_type: BlockType,
    pub num_secrets_obfuscated: usize,

    /// The exit code of the completed command. 0 = success.
    pub exit_code: i32,

    /// If the completed block was a workflow, this is its id.
    pub cloud_workflow_id: Option<SyncId>,

    /// If the completed block had an env var object associated.
    pub cloud_env_var_collection_id: Option<SyncId>,
}
#[derive(Clone, Debug)]
/// Different types of blocks. `User` is for normal execution.
/// Everything else is earlier in the bootstrapping sequence.
pub enum BlockType {
    /// When there are blocks that finish in the bootstrap sequence,
    /// we don't want to propagate the event around our app.
    BootstrapHidden,
    /// This is a special case where the user's rcfiles resulted in outputs. We
    /// will want some of the view logic to execute, and not all of it.
    BootstrapVisible(std::sync::Arc<crate::block_serialized::SerializedBlock>),
    /// This was a block we restored through session restoration.
    Restored,
    /// This was a block created for execution of an in-band command.
    InBandCommand,
    /// This is a normal block that the user executed.
    User(UserBlockCompleted),

    /// This is a block containing background process output.
    Background(std::sync::Arc<crate::block_serialized::SerializedBlock>),

    /// This is a block containing static/hardcoded content (e.g. the subshell Warpification
    /// welcome block).
    Static,
}

impl BlockType {
    pub fn is_bootstrap_block(&self) -> bool {
        matches!(self, Self::BootstrapHidden | Self::BootstrapVisible(_))
    }
}
/// Contents of a normal block that a user executed.
#[derive(Clone, Debug)]
pub struct UserBlockCompleted {
    pub index: BlockIndex,

    pub serialized_block: std::sync::Arc<crate::block_serialized::SerializedBlock>,

    /// The input lines for a block without any escape sequences.
    pub command: String,

    /// The command with secrets obfuscated.
    pub command_with_obfuscated_secrets: String,

    /// The output lines for a block without any escape sequences.
    /// They are truncated to the number of lines specificed by the caller.
    pub output_truncated: String,

    /// The output lines for a block without any escape sequences.
    /// They are truncated to the number of lines specificed by the caller.
    /// Forced secrets to be obfuscated as well.
    pub output_truncated_with_obfuscated_secrets: String,

    /// `true` if the block was run as a requested command or was part of a CLI subagent interaction.
    pub was_part_of_agent_interaction: bool,

    /// Time that we started the command grid (i.e. immediately after the user
    /// hit enter).
    pub started_at: Option<Instant>,

    /// The number of lines in the output grid when it was finished.
    pub num_output_lines: u64,

    /// The number of lines of output that were truncated while the block
    /// was active and receiving output.
    pub num_output_lines_truncated: u64,
}

/// R22: Block 发射点载荷(自 app event.rs AfterBlockStarted 字面量结构定形)。
#[derive(Clone, Debug)]
pub struct AfterBlockStartedEvent {
    pub block_id: warp_terminal::model::BlockId,
    pub command: String,
    pub is_for_in_band_command: bool,
}
