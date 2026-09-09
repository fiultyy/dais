//! R15: agent view 状态三元组(自 app/ai/blocklist/agent_view/controller.rs 提升)。
//! doc 自认 "Terminal view-scoped state",Block 可见性/高度计算的内生输入;
//! 变更权(AgentViewController)留守 app ai 域。
use agent_ids::AIConversationId;

/// The display mode for an active agent view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentViewDisplayMode {
    /// Full-screen agent view (navstack-based).
    FullScreen,
    /// Inline agent view (e.g., for long-running commands).
    Inline,
}

impl AgentViewDisplayMode {
    pub fn is_inline(self) -> bool {
        matches!(self, AgentViewDisplayMode::Inline)
    }

    pub fn is_fullscreen(self) -> bool {
        matches!(self, AgentViewDisplayMode::FullScreen)
    }
}

/// R15: 自 app/terminal/input/slash_commands 提升(AgentViewEntryOrigin 内嵌;
/// slash 菜单 UI 留守 app)。
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum SlashCommandTrigger {
    Input { cmd_or_ctrl_enter: bool },
    Keybinding,
}

impl SlashCommandTrigger {
    pub fn cmd_or_ctrl_enter() -> Self {
        Self::Input {
            cmd_or_ctrl_enter: true,
        }
    }

    pub fn input() -> Self {
        Self::Input {
            cmd_or_ctrl_enter: false,
        }
    }

    pub fn keybinding() -> Self {
        Self::Keybinding
    }

    pub fn is_keybinding(&self) -> bool {
        matches!(self, Self::Keybinding)
    }

    pub fn is_cmd_or_ctrl_enter(&self) -> bool {
        matches!(
            self,
            Self::Input {
                cmd_or_ctrl_enter: true
            }
        )
    }
}

/// The different types of agent view entrypoints.
///
/// Depending on the entrypoint, an `AgentView` block representing the entry may be inserted into
/// the terminal blocklist.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentViewEntryOrigin {
    /// Entered agent view from user input (e.g. /agent or cmd-enter keypress).
    Input {
        was_prompt_autodetected: bool,
    },
    PromptChip,
    /// Entered agent view by selecting a conversation (e.g. selector).
    ConversationSelector,
    /// Entered agent view clicking a conversation item in the Agent Mode homepage (tab zero state).
    AgentModeHomepage,
    /// Entered agent view by clicking an existing agent view block.
    AgentViewBlock,
    /// Entered agent view from the AI document pane.
    AIDocument,
    /// Entered agent view due to an automatic follow-up (not a direct user selection).
    AutoFollowUp,
    /// Entered agent view due to conversation restoration on startup or forking.
    RestoreExistingConversation,
    /// Entered agent view due to shared-session synchronization.
    SharedSessionSelection,
    /// Entered agent view due to a server-driven conversation split (StartNewConversation client action).
    AgentRequestedNewConversation,
    /// Entered agent view via accepting a prompt suggestion.
    AcceptedPromptSuggestion,
    /// Entered agent view via accepting a unit test suggestion.
    AcceptedUnitTestSuggestion,
    /// Entered agent view via accepting a passive code diff.
    AcceptedPassiveCodeDiff,
    /// Entered agent view by starting conversation with an inline code review submission.
    InlineCodeReview,
    /// Entered agent view via the CLI (e.g. `warp agent run`).
    Cli,
    /// Entered agent view by adding an image (drag-and-drop or paste).
    ImageAdded,
    /// Entered agent view by executing a slash command that requires agent mode.
    SlashCommand {
        trigger: SlashCommandTrigger,
    },
    SlashInit,
    /// Entered agent view by executing a slash command that requires agent mode.
    Keybinding,
    /// Entered agent view by attaching context from the code review panel.
    CodeReviewContext,
    /// Entered agent view by selecting a conversation from the inline history menu.
    InlineHistoryMenu,
    InlineConversationMenu,
    OnboardingCallout,
    ConversationListView,
    /// Entered agent view because the default session mode setting is Agent.
    DefaultSessionMode,

    /// Entered agent view by long-running command.
    LongRunningCommand,

    /// Entered agent view from the onboarding flow.
    Onboarding,

    /// Entered agent view because a parent agent started this child agent.
    ChildAgent,

    /// Entered agent view after opening project from OS directory picker.
    ProjectEntry,

    /// Entered agent view via a Linear "work on issue" deeplink.
    LinearDeepLink,

    /// Entered agent view by clearing the buffer (Cmd+K) while already in agent view.
    ClearBuffer,

    // The variants below actually correspond to callsites where the selected conversation is
    // updated, but don't actually correspond to entering the agent view. They exist so we can
    // continue to call `set_pending_query_state_for_(new|existing)_conversation`, but you'll find
    // that their callsites are actually gated on `AgentView` being disabled. Once `AgentView` is
    // launched, those callsites for updating the selected conversation will be removed along with
    // these variants.
    ContinueConversationButton,
    ViewPassiveCodeDiffDetails,
    ResumeConversationButton,
}

/// Controls when `try_enter_agent_view` is allowed to auto-submit an initial prompt
/// to the LLM.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutoTriggerBehavior {
    /// Always auto-submit the prompt, regardless of prior agent-view state.
    Always,
    /// Auto-submit only when the user was already in agent view before this entry.
    InAgentView,
    /// Never auto-submit. The prompt is placed into the input buffer as a draft.
    Never,
}

impl AgentViewEntryOrigin {
    pub fn should_autotrigger_request(&self) -> AutoTriggerBehavior {
        match self {
            AgentViewEntryOrigin::Input {
                was_prompt_autodetected,
            } if *was_prompt_autodetected => AutoTriggerBehavior::Always,
            AgentViewEntryOrigin::SlashCommand { trigger } if !trigger.is_keybinding() => {
                AutoTriggerBehavior::Always
            }
            AgentViewEntryOrigin::Cli => AutoTriggerBehavior::Always,
            AgentViewEntryOrigin::AcceptedPromptSuggestion => AutoTriggerBehavior::Always,
            AgentViewEntryOrigin::LinearDeepLink => AutoTriggerBehavior::Never,
            _ => AutoTriggerBehavior::InAgentView,
        }
    }
}

/// Terminal view-scoped state representing whether or not the user is engaged in an active agent view.
#[derive(Debug, Clone)]
pub enum AgentViewState {
    Active {
        conversation_id: AIConversationId,
        origin: AgentViewEntryOrigin,
        display_mode: AgentViewDisplayMode,

        // The number of exchanges in the conversation when the agent view was entered.
        //
        // This can be used to determine if a conversation has been updated within an agent view
        // 'entry'.
        original_conversation_length: usize,
    },
    Inactive,
}

impl AgentViewState {
    pub fn active_conversation_id(&self) -> Option<AIConversationId> {
        match self {
            AgentViewState::Active {
                conversation_id, ..
            } => Some(*conversation_id),
            AgentViewState::Inactive => None,
        }
    }

    /// Returns the display mode if active, `None` if inactive.
    pub fn display_mode(&self) -> Option<AgentViewDisplayMode> {
        match self {
            AgentViewState::Active { display_mode, .. } => Some(*display_mode),
            AgentViewState::Inactive => None,
        }
    }

    /// Returns `true` if in an active agent view state.
    pub fn is_active(&self) -> bool {
        matches!(self, AgentViewState::Active { .. })
    }

    /// Returns `true` if in inline display mode.
    pub fn is_inline(&self) -> bool {
        self.display_mode().is_some_and(|mode| mode.is_inline())
    }

    /// Returns `true` if in fullscreen display mode.
    pub fn is_fullscreen(&self) -> bool {
        self.display_mode().is_some_and(|mode| mode.is_fullscreen())
    }

    pub fn fullscreen_conversation_id(&self) -> Option<AIConversationId> {
        match self {
            AgentViewState::Active {
                conversation_id,
                display_mode: AgentViewDisplayMode::FullScreen,
                ..
            } => Some(*conversation_id),
            _ => None,
        }
    }

    pub fn is_new(&self) -> bool {
        match self {
            AgentViewState::Active {
                original_conversation_length,
                ..
            } => *original_conversation_length == 0,
            AgentViewState::Inactive => false,
        }
    }

    /// Returns the save position ID for the zero state block, if active.
    pub fn zero_state_position_id(&self) -> Option<String> {
        self.active_conversation_id()
            .map(|id| format!("agent_view_zero_state_{}", id))
    }
}
