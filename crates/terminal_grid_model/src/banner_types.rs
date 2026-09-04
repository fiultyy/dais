//! R17: banner 域轻件(自 app/terminal/view + view/block_banner 提升)。
//! WithinBlockBanner 完整 WarpifyBannerState(含 MouseStateHandle/Keystroke 等
//! UI 对象)留守 app;本模块提供模型侧判别投影与 POD 类型。
pub const BLOCK_BANNER_HEIGHT: f32 = 48. + 16.;
pub const BLOCK_BANNER_DESCRIPTION_MAX_HEIGHT: f32 = 24.;

/// R17: Block.block_banner 字段的模型侧投影(view 完整状态留 TerminalView 侧)。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WithinBlockBannerKind {
    Ssh,
    Subshell,
}

impl WithinBlockBannerKind {
    pub fn banner_height(self) -> f32 {
        match self {
            WithinBlockBannerKind::Ssh => {
                BLOCK_BANNER_HEIGHT + BLOCK_BANNER_DESCRIPTION_MAX_HEIGHT
            }
            WithinBlockBannerKind::Subshell => BLOCK_BANNER_HEIGHT,
        }
    }
}

/// A unique identifier for an inline banner.
pub type InlineBannerId = usize;

/// Type of inline banner - determines behavior like visibility in agent view.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum InlineBannerType {
    NotificationsDiscovery,
    NotificationsError,
    Ssh,
    PromptSuggestions,
    AliasExpansion,
    SharedSessionStart,
    SharedSessionEnd,
    ShellProcessTerminated,
    OpenInWarp,
    VimMode,
    AgentModeSetup,
    AwsBedrockLogin,
    AwsCliNotInstalled,
}

impl InlineBannerType {
    /// Returns whether this banner type should be visible when agent view is active.
    pub fn is_visible_in_agent_view(&self) -> bool {
        matches!(
            self,
            Self::PromptSuggestions
                | Self::AgentModeSetup
                | Self::AwsBedrockLogin
                | Self::AwsCliNotInstalled
        )
    }
}

/// An inline banner with its unique ID and type metadata.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct InlineBannerItem {
    pub id: InlineBannerId,
    pub banner_type: InlineBannerType,
}

impl InlineBannerItem {
    pub fn new(id: InlineBannerId, banner_type: InlineBannerType) -> Self {
        Self { id, banner_type }
    }
}

/// A unique identifier for a subshell separator.
pub type SeparatorId = usize;
