//! R21: PromptSnapshot 数据族(自 app/context_chips + settings/editor 提升)。
//! 生成器(from_current_prompt, 依赖 CurrentPrompt/AppContext)与交互面
//! (to_chip/should_render/udi_icon/GitLineChanges::from_diff_stats)留 app 扩展。
//! serde 面逐字段原样迁移: ChipValue untagged、ContextChipKind snake_case+
//! alias RemoteLogin、WarpPromptSeparator snake_case+#[default]。

use serde::{Deserialize, Serialize};

/// R21: 自 app/settings/editor.rs 提升。
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    Eq,
    PartialEq,
    Deserialize,
    Serialize,
    schemars::JsonSchema,
    settings_value::SettingsValue,
)]
#[schemars(
    description = "Trailing separator character displayed at the end of the prompt.",
    rename_all = "snake_case"
)]
pub enum WarpPromptSeparator {
    /// No separator for the prompt.
    #[default]
    None,
    /// "%" separator for the prompt. Note this is the default separator used in zsh traditionally.
    PercentSign,
    /// "$" separator for the prompt. Note this is the default separator used in bash traditionally.
    DollarSign,
    /// ">" separator for the prompt. Note this is the default separator used in fish traditionally.
    ChevronSymbol,
}

impl WarpPromptSeparator {
    pub fn dropdown_item_label(&self) -> &'static str {
        match self {
            Self::None => "None",
            Self::PercentSign => "%",
            Self::DollarSign => "$",
            Self::ChevronSymbol => ">",
        }
    }

    pub fn renderable_string(&self) -> Option<&'static str> {
        match self {
            Self::None => None,
            Self::PercentSign => Some("%"),
            Self::DollarSign => Some("$"),
            Self::ChevronSymbol => Some(">"),
        }
    }
}

/// R21: 自 app/context_chips/display_chip.rs 提升(纯 3×u32)。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitLineChanges {
    pub files_changed: u32,
    pub lines_added: u32,
    pub lines_removed: u32,
}

impl GitLineChanges {
    /// R21: 自 app display_chip 提升(纯 git diff --shortstat 解析)。
    pub fn parse_from_git_output(raw_output: &str) -> Option<Self> {
        let line = raw_output.trim();

        if line.is_empty() {
            return None;
        }

        let mut files_changed = 0;
        let mut lines_added = 0;
        let mut lines_removed = 0;

        let words: Vec<&str> = line.split_whitespace().collect();
        for (i, word) in words.iter().enumerate() {
            if let Ok(num) = word.parse::<u32>() {
                if let Some(next_word) = words.get(i + 1) {
                    if next_word.starts_with("file") {
                        files_changed = num;
                    } else if next_word.starts_with("insertion") {
                        lines_added = num;
                    } else if next_word.starts_with("deletion") {
                        lines_removed = num;
                    }
                }
            }
        }

        Some(Self {
            files_changed,
            lines_added,
            lines_removed,
        })
    }
}

/// The value of a context chip. Most chips produce plain text, but some
/// (like `GitDiffStats`) carry structured data to avoid string round-trips.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ChipValue {
    Text(String),
    GitDiffStats(GitLineChanges),
}

impl ChipValue {
    /// Returns the text representation, or `None` for non-text variants.
    pub fn as_text(&self) -> Option<&str> {
        match self {
            ChipValue::Text(s) => Some(s),
            ChipValue::GitDiffStats(_) => None,
        }
    }

    /// Returns the `GitLineChanges` payload, if this is the `GitDiffStats` variant.
    pub fn as_git_diff_stats(&self) -> Option<&GitLineChanges> {
        match self {
            ChipValue::GitDiffStats(g) => Some(g),
            ChipValue::Text(_) => None,
        }
    }
}

impl Default for ChipValue {
    fn default() -> Self {
        ChipValue::Text(String::new())
    }
}

impl std::fmt::Display for ChipValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ChipValue::Text(s) => f.write_str(s),
            ChipValue::GitDiffStats(g) => {
                write!(
                    f,
                    "{} • +{} -{}",
                    g.files_changed, g.lines_added, g.lines_removed
                )
            }
        }
    }
}

impl From<String> for ChipValue {
    fn from(s: String) -> Self {
        ChipValue::Text(s)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChipResult {
    pub kind: ContextChipKind,
    pub value: Option<ChipValue>,
    pub on_click_values: Vec<String>,
}

impl ChipResult {
    pub fn kind(&self) -> &ContextChipKind {
        &self.kind
    }

    pub fn value(&self) -> Option<&ChipValue> {
        self.value.as_ref()
    }

    pub fn on_click_values(&self) -> &[String] {
        &self.on_click_values
    }
}

#[derive(
    Serialize,
    Deserialize,
    Clone,
    Debug,
    Eq,
    PartialEq,
    Hash,
    schemars::JsonSchema,
    settings_value::SettingsValue,
)]
#[schemars(
    description = "Type of prompt context chip.",
    rename_all = "snake_case"
)]
pub enum ContextChipKind {
    WorkingDirectory,
    Username,
    Hostname,
    Date,
    Time12,
    Time24,
    VirtualEnvironment,
    CondaEnvironment,
    NodeVersion,
    #[schemars(description = "A user-defined custom chip.")]
    Custom {
        title: String,
    },
    ShellGitBranch,
    GitDiffStats,
    GithubPullRequest,
    KubernetesContext,
    SvnBranch,
    SvnDirtyItems,
    // This is for backwards compatibility with the old "RemoteLogin" chip.
    // We originally had two different chips for different input types, this has since been consolidated.
    #[serde(alias = "RemoteLogin")]
    Ssh,
    Subshell,
    /// A chip that shows the plan and todo list for the current conversation.
    AgentPlanAndTodoList,
}

pub(crate) fn github_pr_number_from_url(url: &str) -> Option<&str> {
    let (_, tail) = url.trim().rsplit_once("/pull/")?;
    let number = tail.split(['/', '?', '#']).next()?;
    (!number.is_empty() && number.chars().all(|c| c.is_ascii_digit())).then_some(number)
}

pub(crate) fn github_pr_display_text_from_url(url: &str) -> Option<String> {
    github_pr_number_from_url(url).map(|number| format!("PR #{number}"))
}

impl ContextChipKind {
    pub fn display_value(&self, value: &ChipValue) -> String {
        let text = value.to_string();
        match self {
            Self::ShellGitBranch => format!("git:({text})"),
            Self::GithubPullRequest => github_pr_display_text_from_url(&text).unwrap_or(text),
            Self::KubernetesContext => format!("⎈ {text}"),
            Self::SvnBranch => format!("svn:({text})"),
            Self::SvnDirtyItems => format!("±{text}"),
            _ => text,
        }
    }
}

/// Formats given context chips as an unstylized string.
/// Compared to displaying individual chips, certain chips when combined are displayed differently.
pub fn chips_to_string(chips: impl Iterator<Item = ChipResult>) -> String {
    let mut prompt = String::new();
    let mut visible_chips = chips
        .into_iter()
        .filter_map(|chip_result| Some((chip_result.kind, chip_result.value?)))
        .peekable();
    while let Some((chip_kind, current_value)) = visible_chips.next() {
        // This is temporary, until we design more generic chip formatting.
        let next_chip_kind = visible_chips.peek().map(|(next_kind, _)| next_kind);
        let chip_display_value = chip_kind.display_value(&current_value);
        prompt.push_str(&chip_display_value);
        match (chip_kind, next_chip_kind) {
            // Omit the space between adjacent Svn chips.
            (ContextChipKind::SvnBranch, Some(ContextChipKind::SvnDirtyItems)) => (),
            (_, Some(_)) => {
                // Add padding after non-empty chips.
                if !chip_display_value.is_empty() {
                    prompt.push(' ');
                }
            }
            _ => (),
        }
    }
    prompt
}

/// Struct that holds a point in time snapshot of a prompt (chips are no longer interactive)
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PromptSnapshot {
    pub chips: Vec<ChipResult>,

    pub same_line_prompt_enabled: bool,
    /// The separator to use as a trailing character at the end of Dais prompt, if any.
    pub separator: WarpPromptSeparator,
}

impl PromptSnapshot {
    pub fn from_chips(
        chips: Vec<ChipResult>,
        same_line_prompt_enabled: bool,
        separator: WarpPromptSeparator,
    ) -> Self {
        Self {
            chips,
            same_line_prompt_enabled,
            separator,
        }
    }

    /// The value of the given chip, in this snapshot.
    pub fn chip_value(&self, chip: &ContextChipKind) -> Option<ChipValue> {
        self.chips.iter().find_map(|chip_result| {
            if chip_result.kind == *chip {
                chip_result.value.clone()
            } else {
                None
            }
        })
    }

    pub fn chips(&self) -> &Vec<ChipResult> {
        &self.chips
    }

    pub fn same_line_prompt_enabled(&self) -> bool {
        self.same_line_prompt_enabled
    }

    pub fn separator(&self) -> WarpPromptSeparator {
        self.separator
    }
}

impl std::fmt::Display for PromptSnapshot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&chips_to_string(self.chips.clone().into_iter()))
    }
}

#[cfg(test)]
mod roundtrip_tests {
    use super::*;

    /// R21 锚1 硬性: 旧持久化 JSON 样本 → 新类型 → 再序列化语义级等价断言。
    /// 样本字段形态覆盖: untagged ChipValue(Text/GitDiffStats)、
    /// PascalCase 枚举名(serde 无 rename_all, snake_case 仅 schemars 层)、alias
    /// RemoteLogin→Ssh、externally-tagged Custom{title}、separator PascalCase。
    #[test]
    fn prompt_snapshot_roundtrip_legacy_json() {
        let legacy = r#"{
            "chips": [
                {"kind": "WorkingDirectory", "value": "~/proj", "on_click_values": []},
                {"kind": "RemoteLogin", "value": "user@host", "on_click_values": []},
                {"kind": "GitDiffStats", "value": {"files_changed": 2, "lines_added": 10, "lines_removed": 3}, "on_click_values": []},
                {"kind": {"Custom": {"title": "label"}}, "value": "x", "on_click_values": ["cmd"]}
            ],
            "same_line_prompt_enabled": true,
            "separator": "PercentSign"
        }"#;
        let snapshot: PromptSnapshot = serde_json::from_str(legacy).expect("legacy JSON 解析");
        assert_eq!(snapshot.chips().len(), 4);
        // alias RemoteLogin → Ssh
        assert!(matches!(snapshot.chips()[1].kind(), ContextChipKind::Ssh));
        // untagged GitDiffStats
        assert!(snapshot.chips()[2].value().unwrap().as_git_diff_stats().is_some());
        assert!(snapshot.same_line_prompt_enabled());
        assert_eq!(snapshot.separator(), WarpPromptSeparator::PercentSign);

        // 再序列化语义等价(往返)
        let re = serde_json::to_string(&snapshot).unwrap();
        let snapshot2: PromptSnapshot = serde_json::from_str(&re).unwrap();
        assert_eq!(snapshot.chips().len(), snapshot2.chips().len());
        assert!(matches!(snapshot2.chips()[1].kind(), ContextChipKind::Ssh));
        assert_eq!(
            snapshot.chips()[2].value().unwrap().to_string(),
            snapshot2.chips()[2].value().unwrap().to_string()
        );
        assert_eq!(snapshot.separator(), snapshot2.separator());
    }

    #[test]
    fn chip_value_untagged_text_serializes_as_bare_string() {
        let v = ChipValue::Text("hello".into());
        assert_eq!(serde_json::to_string(&v).unwrap(), r#""hello""#);
        let g = ChipValue::GitDiffStats(GitLineChanges {
            files_changed: 1,
            lines_added: 2,
            lines_removed: 3,
        });
        assert_eq!(
            serde_json::to_string(&g).unwrap(),
            r#"{"files_changed":1,"lines_added":2,"lines_removed":3}"#
        );
    }

    #[test]
    fn chips_to_string_combined_formatting() {
        let chips = vec![
            ChipResult {
                kind: ContextChipKind::SvnBranch,
                value: Some(ChipValue::Text("trunk".into())),
                on_click_values: vec![],
            },
            ChipResult {
                kind: ContextChipKind::SvnDirtyItems,
                value: Some(ChipValue::Text("3".into())),
                on_click_values: vec![],
            },
            ChipResult {
                kind: ContextChipKind::ShellGitBranch,
                value: Some(ChipValue::Text("main".into())),
                on_click_values: vec![],
            },
        ];
        // Svn 相邻免空格;其余补空格。
        assert_eq!(chips_to_string(chips.into_iter()), "svn:(trunk)±3 git:(main)");
    }
}
