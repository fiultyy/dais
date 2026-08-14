//! Core data types for harness blocks.
//!
//! A harness block is an immutable record of one interaction unit (prompt,
//! response, tool call, ...) captured from an agent harness session.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

/// The kind of content a block carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BlockType {
    Spawn,
    SystemPrompt,
    PromptSegment,
    UserPrompt,
    Response,
    ResponseChunk,
    ToolCall,
    ToolResult,
    PtyRaw,
    Exit,
}

impl BlockType {
    pub const ALL: [BlockType; 10] = [
        BlockType::Spawn,
        BlockType::SystemPrompt,
        BlockType::PromptSegment,
        BlockType::UserPrompt,
        BlockType::Response,
        BlockType::ResponseChunk,
        BlockType::ToolCall,
        BlockType::ToolResult,
        BlockType::PtyRaw,
        BlockType::Exit,
    ];

    pub fn as_str(&self) -> &'static str {
        match self {
            BlockType::Spawn => "spawn",
            BlockType::SystemPrompt => "system_prompt",
            BlockType::PromptSegment => "prompt_segment",
            BlockType::UserPrompt => "user_prompt",
            BlockType::Response => "response",
            BlockType::ResponseChunk => "response_chunk",
            BlockType::ToolCall => "tool_call",
            BlockType::ToolResult => "tool_result",
            BlockType::PtyRaw => "pty_raw",
            BlockType::Exit => "exit",
        }
    }
}

impl fmt::Display for BlockType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for BlockType {
    type Err = UnknownBlockType;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        BlockType::ALL
            .iter()
            .copied()
            .find(|t| t.as_str() == s)
            .ok_or_else(|| UnknownBlockType(s.to_string()))
    }
}

/// Error returned when a string does not name a known [`BlockType`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownBlockType(pub String);

impl fmt::Display for UnknownBlockType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "unknown block type: {}", self.0)
    }
}

impl std::error::Error for UnknownBlockType {}

/// How a harness session's traffic is intercepted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InterceptMode {
    Full,
    HooksOnly,
    Bypass,
}

impl InterceptMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            InterceptMode::Full => "full",
            InterceptMode::HooksOnly => "hooks_only",
            InterceptMode::Bypass => "bypass",
        }
    }
}

impl fmt::Display for InterceptMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for InterceptMode {
    type Err = UnknownInterceptMode;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "full" => Ok(InterceptMode::Full),
            "hooks_only" => Ok(InterceptMode::HooksOnly),
            "bypass" => Ok(InterceptMode::Bypass),
            other => Err(UnknownInterceptMode(other.to_string())),
        }
    }
}

/// Error returned when a string does not name a known [`InterceptMode`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownInterceptMode(pub String);

impl fmt::Display for UnknownInterceptMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "unknown intercept mode: {}", self.0)
    }
}

impl std::error::Error for UnknownInterceptMode {}

/// One immutable interaction unit from a harness session.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HarnessBlock {
    pub id: String,
    pub session_id: String,
    pub parent_id: Option<String>,
    /// Which harness the block came from (e.g. "claude", "codex", "aider").
    pub harness_type: String,
    pub block_type: BlockType,
    /// Monotonic per-session sequence number.
    pub sequence: u32,
    /// Raw payload bytes; interpretation depends on `block_type`.
    pub content: Vec<u8>,
    /// Free-form structured metadata (JSON object expected, any Value allowed).
    pub metadata: serde_json::Value,
    /// Unix epoch milliseconds.
    pub timestamp: i64,
}

impl HarnessBlock {
    /// Create a block with a fresh UUID and `metadata` defaulting to `{}`.
    /// Other fields are taken as-is.
    #[must_use]
    pub fn new(
        session_id: impl Into<String>,
        harness_type: impl Into<String>,
        block_type: BlockType,
        sequence: u32,
        content: Vec<u8>,
        timestamp: i64,
    ) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            session_id: session_id.into(),
            parent_id: None,
            harness_type: harness_type.into(),
            block_type,
            sequence,
            content,
            metadata: serde_json::Value::Null,
            timestamp,
        }
    }
}
