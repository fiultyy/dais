//! context_chip 纯模型层 — 自 app/src/context_chips/context_chip.rs 同刀切出
//! (P-ARC R5)。仅承载零 app 依赖的类型面:
//! - ShellCommandGenerator / ShellCommand(serde POD,ShellType 经
//!   warp_terminal::shell 引入);
//! - ExternalCommandsAvailability($PATH 可执行位图);
//! - ChipDisabledReason / ChipAvailability(可用性三态);
//! - ChipFingerprintInput / ChipRuntimePolicy(缓存指纹策略;
//!   `availability(&ChipRuntimeCapabilities)` 因参数为宿主类型留守
//!   app 侧 ext trait);
//! - RefreshConfig(刷新策略枚举);
//! - Environment(会话环境快照 POD;`from_block(&Block)` 因 Block 为宿主
//!   terminal::model 类型留守 app 侧 ext trait)。
//!
//! 留守 app 的部分(物理依赖约束):ChipRuntimeCapabilities(from_session*
//! 读宿主 Session 且被 workspace/view.rs 全限定关联函数调用,R2-b
//! WorkflowViewMode 同型裁决)、GeneratorContext / PromptGenerator(持有
//! 宿主 Block/Session 引用与 ChipValue)、ContextChip(构造器面全接宿主)。

use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    time::Duration,
};

use warp_terminal::shell::ShellType;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ShellCommandGenerator {
    command: ShellCommand,
    dependencies: Vec<String>,
}

/// Representation of a shell command. The command may or may not be supported on all shells.
///
/// In YAML (as a hypothetical example), this should work with a variable format like:
/// ```yaml
/// # This parses to ShellCommand::Portable
/// - "this is a portable command"
/// # This parses to ShellCommand::ShellSpecific
/// - bash: "this works on bash"
///   zsh: "this works on zsh"
///   # this command does not support Fish
/// ```
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ShellCommand {
    /// A shell command that works on all shells.
    Portable(String),
    /// A shell command that only works on specific shells.
    ShellSpecific(HashMap<ShellType, String>),
}

impl ShellCommandGenerator {
    pub fn command(&self) -> &ShellCommand {
        &self.command
    }

    pub fn dependencies(&self) -> &[String] {
        &self.dependencies
    }

    pub fn new(command: ShellCommand, dependencies: Option<Vec<String>>) -> Self {
        Self {
            command,
            dependencies: dependencies.unwrap_or_default(),
        }
    }
}

impl ShellCommand {
    /// Construct a new portable shell command.
    pub fn portable(command: impl Into<String>) -> Self {
        Self::Portable(command.into())
    }

    /// Construct a set of shell-specific commands.
    pub fn shell_specific(commands: impl Into<HashMap<ShellType, String>>) -> Self {
        Self::ShellSpecific(commands.into())
    }

    /// Gets the variant of this command that works on the given shell. If this command does not
    /// support the shell, returns `None`.
    pub fn for_shell(&self, shell_type: ShellType) -> Option<&str> {
        match self {
            Self::Portable(command) => Some(command.as_str()),
            Self::ShellSpecific(commands) => commands.get(&shell_type).map(String::as_str),
        }
    }
}

/// Tracks whether the set of external commands (executables on `$PATH`) has been loaded for a
/// session, and if so, which required commands are present.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum ExternalCommandsAvailability {
    #[default]
    Unknown,
    Known {
        command_count: usize,
        required_command_presence: HashMap<String, bool>,
    },
}

impl ExternalCommandsAvailability {
    pub fn contains(&self, command: &str) -> Option<bool> {
        match self {
            Self::Unknown => None,
            Self::Known {
                required_command_presence,
                ..
            } => required_command_presence.get(command).copied(),
        }
    }

    pub fn command_count(&self) -> Option<usize> {
        match self {
            Self::Unknown => None,
            Self::Known { command_count, .. } => Some(*command_count),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChipDisabledReason {
    RequiresLocalSession,
    RequiresExecutable { command: String },
}

impl ChipDisabledReason {
    pub fn tooltip_text(&self) -> String {
        match self {
            Self::RequiresLocalSession => "Requires a local session".to_string(),
            Self::RequiresExecutable { command } if command == "gh" => {
                "Requires the GitHub CLI".to_string()
            }
            Self::RequiresExecutable { command } => format!("Requires the `{command}` command"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum ChipAvailability {
    #[default]
    Enabled,
    Disabled(ChipDisabledReason),
    Hidden,
}

impl ChipAvailability {
    pub fn is_enabled(&self) -> bool {
        matches!(self, Self::Enabled)
    }

    pub fn tooltip_override_text(&self) -> Option<String> {
        match self {
            Self::Disabled(reason) => Some(reason.tooltip_text()),
            Self::Enabled | Self::Hidden => None,
        }
    }
}

/// An input that contributes to a chip's fingerprint hash. When all fingerprint inputs match
/// the previously computed fingerprint, the chip can skip re-fetching its value.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ChipFingerprintInput {
    SessionId,
    SessionIsLocal,
    WorkingDirectory,
    GitBranch,
    PythonVirtualenv,
    CondaEnvironment,
    NodeVersion,
    SessionUser,
    SessionHostname,
    ExternalCommandsState,
    RequiredExecutablesPresence,
    /// A per-chip monotonic counter that increments each time a user command matching
    /// the chip's `invalidate_on_commands` list completes, causing the fingerprint to change.
    InvalidatingCommandCount,
}

/// Configuration that governs how a chip interacts with the runtime environment: which
/// executables it requires, whether it's restricted to local sessions, its shell command
/// timeout, and which inputs form its cache fingerprint.
#[derive(Clone, Debug, Default)]
pub struct ChipRuntimePolicy {
    required_executables: Vec<String>,
    local_only: bool,
    shell_command_timeout: Option<Duration>,
    fingerprint_inputs: Vec<ChipFingerprintInput>,
    /// When true, if the chip's shell command fails (or times out), the chip records the current
    /// fingerprint and skips re-execution on future fetches — including periodic refreshes —
    /// until the fingerprint changes (e.g. branch or directory change).
    suppress_on_failure: bool,
    /// Top-level command names (e.g. `["git", "gh", "gt"]`) whose execution should
    /// invalidate this chip's fingerprint. Pair with `ChipFingerprintInput::InvalidatingCommandCount`.
    invalidate_on_commands: Vec<String>,
}

impl ChipRuntimePolicy {
    pub fn new(
        required_executables: impl IntoIterator<Item = impl Into<String>>,
        local_only: bool,
        shell_command_timeout: Option<Duration>,
        fingerprint_inputs: impl IntoIterator<Item = ChipFingerprintInput>,
    ) -> Self {
        Self {
            required_executables: required_executables.into_iter().map(Into::into).collect(),
            local_only,
            shell_command_timeout,
            fingerprint_inputs: fingerprint_inputs.into_iter().collect(),
            suppress_on_failure: false,
            invalidate_on_commands: Vec::new(),
        }
    }

    pub fn for_shell_generator(generator: &ShellCommandGenerator) -> Self {
        Self::new(
            generator.dependencies().to_vec(),
            false,
            None,
            std::iter::empty(),
        )
    }

    pub fn required_executables(&self) -> &[String] {
        &self.required_executables
    }

    /// Whether the chip is restricted to local sessions(宿主 ext trait
    /// `ChipRuntimePolicyHostExt::availability` 经此读取,字段保持私有)。
    pub fn local_only(&self) -> bool {
        self.local_only
    }

    pub fn shell_command_timeout(&self) -> Option<Duration> {
        self.shell_command_timeout
    }

    pub fn fingerprint_inputs(&self) -> &[ChipFingerprintInput] {
        &self.fingerprint_inputs
    }

    pub fn suppress_on_failure(&self) -> bool {
        self.suppress_on_failure
    }

    pub fn with_suppress_on_failure(mut self) -> Self {
        self.suppress_on_failure = true;
        self
    }

    pub fn invalidate_on_commands(&self) -> &[String] {
        &self.invalidate_on_commands
    }

    pub fn with_invalidate_on_commands(
        mut self,
        commands: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        self.invalidate_on_commands = commands.into_iter().map(Into::into).collect();
        self
    }
}

#[derive(Clone, Debug, Default)]
pub enum RefreshConfig {
    #[default]
    OnDemandOnly,
    #[allow(dead_code)]
    Periodically { interval: Duration },
    #[allow(dead_code)]
    OnFileChanges { filepath: String },
}

/// Environment information for a terminal session.
#[derive(Debug, Clone, Default)]
pub struct Environment {
    /// The Git branch that's checked out.
    git_branch: Option<String>,
    /// The name of the active Python virtual environment.
    python_virtualenv: Option<String>,
    /// The Anaconda environment name.
    conda_environment: Option<String>,
    /// The Node.js version.
    node_version: Option<String>,
}

impl Environment {
    /// Create a new environment with the given values.
    pub fn new(
        python_virtualenv: Option<String>,
        conda_environment: Option<String>,
        node_version: Option<String>,
    ) -> Self {
        Self {
            git_branch: None,
            python_virtualenv,
            conda_environment,
            node_version,
        }
    }

    /// Full-field constructor(宿主 ext trait `EnvironmentHostExt::from_block`
    /// 经此构造,字段保持私有)。
    pub fn from_parts(
        git_branch: Option<String>,
        python_virtualenv: Option<String>,
        conda_environment: Option<String>,
        node_version: Option<String>,
    ) -> Self {
        Self {
            git_branch,
            python_virtualenv,
            conda_environment,
            node_version,
        }
    }

    pub fn git_branch(&self) -> Option<&String> {
        self.git_branch.as_ref()
    }

    pub fn python_virtualenv(&self) -> Option<&String> {
        self.python_virtualenv.as_ref()
    }

    pub fn conda_environment(&self) -> Option<&String> {
        self.conda_environment.as_ref()
    }

    pub fn node_version(&self) -> Option<&String> {
        self.node_version.as_ref()
    }
}
