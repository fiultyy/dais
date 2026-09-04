//! R16: block 域事件载荷(自 app/terminal/event.rs + model/block.rs 提升)。
//! BlockCompletedEvent/AfterBlockCompletedEvent/UserBlockCompleted/BlockType
//! 仍锁 app(经 BlockType→SerializedBlock→ShellHost 链,见 R16 报告)。
use instant::Instant;
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
