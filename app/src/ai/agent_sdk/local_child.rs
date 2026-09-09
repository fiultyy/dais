//! Local child agent run types: runtime config snapshot and task identity.
//!
//! R41-T5: migrated from `ai::ambient_agents` (now deleted). The cloud ambient
//! task machinery was stripped; only the types still consumed by the local
//! runner stack (agent_sdk driver, third-party harnesses, local child pane
//! launch) survive here. The former ambient task ID was renamed to `LocalChildTaskId`
//! to reflect its remaining purpose: identifying a locally spawned child run.

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt::Display;
use std::str::FromStr;
use uuid::{NonNilUuid, Uuid};
use warp_cli::agent::Harness;

/// Runtime configuration snapshot for agent execution.
///
/// This is the merged/resolved config used when spawning or running an agent.
/// It combines settings from config files and CLI args.
/// Unlike persisted agent profile/config records, field names here use the runtime format
/// (e.g. `model_id` instead of `base_model_id`).
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct AgentConfigSnapshot {
    /// Config name for searchability/traceability.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_prompt: Option<String>,
    /// MCP server configuration map (unwrapped; no `mcpServers` wrapper).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mcp_servers: Option<serde_json::Map<String, serde_json::Value>>,
    /// Profile ID for local agent runs. This configures the terminal session
    /// with the specified execution profile.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_id: Option<String>,
    /// Local worker ID that should execute this task.
    /// If None or Some("warp"), the default local runtime is used.
    /// Otherwise, the task will only be assigned to a connected local worker with matching ID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker_host: Option<String>,
    /// Skill spec to use as the base prompt for the agent.
    /// Format: "skill_name", "repo:skill_name", or "org/repo:skill_name".
    /// The skill is resolved at runtime in the agent environment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skill_spec: Option<String>,
    /// Whether computer use is enabled for this agent run.
    /// If None, the default behavior is used.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub computer_use_enabled: Option<bool>,
    /// Execution harness for the agent run.
    /// If None, we use Dais's default ("oz").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness: Option<HarnessConfig>,
    /// Authentication secrets for third-party harnesses.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness_auth_secrets: Option<HarnessAuthSecretsConfig>,
}

impl AgentConfigSnapshot {
    /// Returns true if this config is empty (no options are set).
    pub fn is_empty(&self) -> bool {
        let Self {
            name,
            environment_id,
            model_id,
            base_prompt,
            mcp_servers,
            profile_id,
            worker_host,
            skill_spec,
            computer_use_enabled,
            harness,
            harness_auth_secrets,
        } = self;

        name.is_none()
            && environment_id.is_none()
            && model_id.is_none()
            && base_prompt.is_none()
            && mcp_servers.is_none()
            && profile_id.is_none()
            && worker_host.is_none()
            && skill_spec.is_none()
            && computer_use_enabled.is_none()
            && harness.is_none()
            && harness_auth_secrets.is_none()
    }
}

/// Configuration for a third-party execution harness.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct HarnessConfig {
    /// The harness type, e.g. [`Harness::Claude`].
    #[serde(
        rename = "type",
        serialize_with = "serialize_harness",
        deserialize_with = "deserialize_harness"
    )]
    pub harness_type: Harness,
}

impl HarnessConfig {
    /// Builds a harness config from just the harness type.
    pub fn from_harness_type(harness_type: Harness) -> Self {
        Self { harness_type }
    }
}

/// Parses a harness type name (e.g. `"claude"`) into a [`Harness`] variant.
/// Unknown values fall back to [`Harness::Unknown`] so we don't
/// misrepresent a future harness as Oz; UI surfaces should treat
/// `Unknown` as a non-runnable harness.
fn harness_from_name(name: &str) -> Harness {
    match name {
        "claude" => Harness::Claude,
        "opencode" => Harness::OpenCode,
        "gemini" => Harness::Gemini,
        "oz" => Harness::Oz,
        other => {
            log::warn!("Unknown harness config name: {other:?}; treating as Unknown");
            Harness::Unknown
        }
    }
}

fn serialize_harness<S: Serializer>(harness: &Harness, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(&harness.to_string())
}

fn deserialize_harness<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Harness, D::Error> {
    let name = String::deserialize(deserializer)?;
    Ok(harness_from_name(&name))
}

/// Authentication secrets for third-party harnesses.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct HarnessAuthSecretsConfig {
    /// Name of a managed secret for Claude Code harness authentication.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claude_auth_secret_name: Option<String>,
}

#[derive(Debug, thiserror::Error)]
#[error("Invalid task ID: {0}")]
pub struct ParseLocalChildTaskIdError(#[from] uuid::Error);

/// A globally unique ID for a locally spawned child agent run.
///
/// R41-T5: renamed from the former ambient task ID; generated locally (UUID v4) when
/// the GUI/CLI launches a third-party child harness so the run does not depend
/// on a remote pre-created task.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LocalChildTaskId(NonNilUuid);

impl Display for LocalChildTaskId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl FromStr for LocalChildTaskId {
    type Err = ParseLocalChildTaskIdError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let uuid = Uuid::try_parse(s)?;
        Ok(Self(NonNilUuid::try_from(uuid)?))
    }
}

impl LocalChildTaskId {
    /// Dais(本地化,Phase 3b-4):本地生成一个 UUID v4 作为 task_id,避免本地
    /// harness 启动子 task 时依赖远端预创建任务接口。
    pub fn new_local() -> Self {
        let uuid = Uuid::new_v4();
        // UUID v4 几乎不可能产生 nil(概率 ~ 1/2^122),采用 expect 表示逻辑不可达。
        let non_nil =
            NonNilUuid::try_from(uuid).expect("freshly generated UUID v4 must be non-nil");
        Self(non_nil)
    }
}
