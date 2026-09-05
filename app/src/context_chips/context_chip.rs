//! context_chip 留守层(P-ARC R5):纯模型类型面(ShellCommandGenerator /
//! ShellCommand / ExternalCommandsAvailability / ChipDisabledReason /
//! ChipAvailability / ChipFingerprintInput / ChipRuntimePolicy /
//! RefreshConfig / Environment)已下沉 crates/context_chips_ui,经下方
//! `pub use` shim 保持 `crate::context_chips::context_chip::*` 旧路径。
//!
//! 留守本文件的宿主耦合部分(物理依赖约束,R4 injector 同型裁决):
//! - `ChipRuntimeCapabilities`:`from_session*` 读宿主
//!   `terminal::model::session::Session`,且被 workspace/view.rs 以全限定
//!   `ChipRuntimeCapabilities::from_session_with_external_command_queries`
//!   关联函数语法调用(R2-b WorkflowViewMode 同型裁决,下沉将强制散改
//!   域外调用点);
//! - `GeneratorContext<'a>` / `PromptGenerator`:持有宿主
//!   `terminal::model::{Block, BlockMetadata, Session}` 引用与 `ChipValue`;
//! - `ContextChip`:构造器面(generator fn / ShellCommandGenerator /
//!   RefreshConfig)全接宿主 PromptGenerator;
//! - `EnvironmentHostExt::from_block` / `ChipRuntimePolicyHostExt::availability`:
//!   参数为宿主类型的构造/判定方法,以宿主 ext trait 留守(R3
//!   PaneDataHost 模式)。

use std::collections::{HashMap, HashSet};

use super::ChipValue;
use crate::terminal::model::session::{Session, SessionId};
use terminal_grid_model::block::{Block, BlockMetadata};

// R5 路径兼容 shim:纯模型类型面已下沉 crates/context_chips_ui,
// `crate::context_chips::context_chip::*` 旧路径全部可解析。
pub use context_chips_ui::context_chip::{
    ChipAvailability, ChipDisabledReason, ChipFingerprintInput, ChipRuntimePolicy, Environment,
    ExternalCommandsAvailability, RefreshConfig, ShellCommand, ShellCommandGenerator,
};

/// 宿主 ext trait(R3 PaneDataHost 模式):`availability` 判定依赖留守的
/// `ChipRuntimeCapabilities`(宿主 Session 派生),无法随纯模型下沉。
/// 当前无调用点(与 ContextChip::availability 一致保留 API 面)。
pub trait ChipRuntimePolicyHostExt {
    fn availability(&self, capabilities: &ChipRuntimeCapabilities) -> ChipAvailability;
}

impl ChipRuntimePolicyHostExt for ChipRuntimePolicy {
    fn availability(&self, capabilities: &ChipRuntimeCapabilities) -> ChipAvailability {
        if self.local_only() && matches!(capabilities.session_is_local, Some(false)) {
            return ChipAvailability::Disabled(ChipDisabledReason::RequiresLocalSession);
        }

        for command in self.required_executables() {
            if matches!(
                capabilities.external_commands.contains(command),
                Some(false)
            ) {
                return ChipAvailability::Disabled(ChipDisabledReason::RequiresExecutable {
                    command: command.clone(),
                });
            }
        }

        ChipAvailability::Enabled
    }
}

/// A snapshot of session-level capabilities that a chip's runtime policy uses to determine
/// availability (e.g. whether the session is local, which executables are present).
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct ChipRuntimeCapabilities {
    pub session_id: Option<SessionId>,
    pub session_is_local: Option<bool>,
    pub external_commands: ExternalCommandsAvailability,
}

impl ChipRuntimeCapabilities {
    pub fn from_session(session: &Session) -> Self {
        Self::from_session_with_external_command_queries(session, std::iter::empty::<&str>(), false)
    }

    pub fn from_session_with_external_command_queries<'a>(
        session: &Session,
        required_executables: impl IntoIterator<Item = &'a str>,
        include_external_command_count: bool,
    ) -> Self {
        let external_commands = if session.has_loaded_external_commands() {
            let mut required_command_presence = HashMap::new();
            let required_executables = required_executables.into_iter().collect::<HashSet<_>>();

            let should_scan_executables =
                include_external_command_count || !required_executables.is_empty();
            let mut command_count = 0;
            if should_scan_executables {
                for executable in session.executable_names() {
                    command_count += 1;
                    if required_executables.contains(executable) {
                        required_command_presence.insert(executable.to_string(), true);
                    }
                }
            }

            for required_executable in required_executables {
                required_command_presence
                    .entry(required_executable.to_string())
                    .or_insert(false);
            }

            ExternalCommandsAvailability::Known {
                command_count,
                required_command_presence,
            }
        } else {
            ExternalCommandsAvailability::Unknown
        };

        Self {
            session_id: Some(session.id()),
            session_is_local: Some(session.is_local()),
            external_commands,
        }
    }
}

/// 宿主 ext trait(R3 PaneDataHost 模式):`from_block` 快照宿主
/// `terminal::model::block::Block`,无法随 Environment POD 下沉。
pub trait EnvironmentHostExt {
    fn from_block(block: &Block) -> Self;
}

impl EnvironmentHostExt for Environment {
    /// Snapshot the environment from a block.
    fn from_block(block: &Block) -> Self {
        Environment::from_parts(
            block.git_branch().cloned(),
            block.virtual_env_short_name(),
            block.conda_env().cloned(),
            block.node_version().cloned(),
        )
    }
}


/// Context for built-in contextual [`PromptGenerator`]s.
pub struct GeneratorContext<'a> {
    /// The latest block in the session. While the prompt is shown, this block should have precmd
    /// metadata but will not have executed yet.
    pub active_block_metadata: &'a BlockMetadata,
    /// The session that the active block is part of. This should always be available once the
    /// session is bootstrapped. However, it may be missing due to errors restoring previous
    /// sessions or extracting session info from the shell.
    pub active_session: Option<&'a Session>,
    /// The most-recently-available environment data for the terminal session. Unlike the username,
    /// hostname, and other session-level info, this can change over the lifetime of a session -
    /// users can activate/deactivate virtualenvs, change branches, and so on.
    pub current_environment: &'a Environment,

}

#[derive(Clone, Debug)]
pub enum PromptGenerator {
    ShellCommand(ShellCommandGenerator),
    Contextual {
        /// A function that extracts the chip value from the prompt-generation context.
        from_context_fn: fn(&GeneratorContext) -> Option<ChipValue>,
    },
}


#[derive(Clone, Debug)]
pub struct ContextChip {
    title: String,
    generator: PromptGenerator,
    on_click_generator: Option<PromptGenerator>,
    /// TODO: this likely needs to move to a config state.
    icon_path: Option<&'static str>,
    refresh_config: RefreshConfig,
    runtime_policy: ChipRuntimePolicy,
    /// When `true`, a shell command that succeeds with empty output produces `Some("")` instead of `None`.
    /// Useful for chips like `GitDiffStats` where empty output can still mean
    /// a valid and clean working tree rather than "not applicable".
    allow_empty_value: bool,
}

impl ContextChip {
    /// Create a new built-in context chip using the given generator function.
    pub fn builtin(
        title: impl Into<String>,
        generator: fn(&GeneratorContext) -> Option<ChipValue>,
        refresh_config: RefreshConfig,
    ) -> Self {
        Self::builtin_with_runtime_policy(
            title,
            generator,
            refresh_config,
            ChipRuntimePolicy::default(),
        )
    }

    pub fn builtin_with_runtime_policy(
        title: impl Into<String>,
        generator: fn(&GeneratorContext) -> Option<ChipValue>,
        refresh_config: RefreshConfig,
        runtime_policy: ChipRuntimePolicy,
    ) -> Self {
        Self {
            title: title.into(),
            generator: PromptGenerator::Contextual {
                from_context_fn: generator,
            },
            on_click_generator: None,
            icon_path: None,
            refresh_config,
            runtime_policy,
            allow_empty_value: false,
        }
    }

    pub fn shell_builtin(
        title: impl Into<String>,
        generator: ShellCommandGenerator,
        on_click_generator: Option<ShellCommandGenerator>,
        refresh_config: RefreshConfig,
    ) -> Self {
        let runtime_policy = ChipRuntimePolicy::for_shell_generator(&generator);
        Self::shell_builtin_with_runtime_policy(
            title,
            generator,
            on_click_generator,
            refresh_config,
            runtime_policy,
        )
    }

    pub fn shell_builtin_with_runtime_policy(
        title: impl Into<String>,
        generator: ShellCommandGenerator,
        on_click_generator: Option<ShellCommandGenerator>,
        refresh_config: RefreshConfig,
        runtime_policy: ChipRuntimePolicy,
    ) -> Self {
        Self {
            title: title.into(),
            generator: PromptGenerator::ShellCommand(generator),
            on_click_generator: on_click_generator.map(PromptGenerator::ShellCommand),
            icon_path: None,
            refresh_config,
            runtime_policy,
            allow_empty_value: false,
        }
    }

    pub fn new_custom_chip(title: String, shell_command_generator: ShellCommandGenerator) -> Self {
        let runtime_policy = ChipRuntimePolicy::for_shell_generator(&shell_command_generator);
        Self {
            title,
            generator: PromptGenerator::ShellCommand(shell_command_generator),
            on_click_generator: None,
            icon_path: None,
            refresh_config: Default::default(),
            runtime_policy,
            allow_empty_value: false,
        }
    }

    pub fn title(&self) -> &str {
        self.title.as_str()
    }

    pub fn generator(&self) -> &PromptGenerator {
        &self.generator
    }

    pub fn on_click_generator(&self) -> Option<&PromptGenerator> {
        self.on_click_generator.as_ref()
    }

    pub fn refresh_config(&self) -> &RefreshConfig {
        &self.refresh_config
    }

    pub fn runtime_policy(&self) -> &ChipRuntimePolicy {
        &self.runtime_policy
    }

    pub fn availability(&self, capabilities: &ChipRuntimeCapabilities) -> ChipAvailability {
        self.runtime_policy.availability(capabilities)
    }

    pub fn icon_path(&self) -> Option<&'static str> {
        self.icon_path
    }

    pub fn allow_empty_value(&self) -> bool {
        self.allow_empty_value
    }

    pub fn with_allow_empty_value(mut self) -> Self {
        self.allow_empty_value = true;
        self
    }
}

