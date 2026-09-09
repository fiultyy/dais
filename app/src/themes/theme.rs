use super::default_themes::*;
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::HashMap;
use std::iter::FromIterator;
use warpui::{
    color::ColorU,
    elements::{
        Align, Border, ConstrainedBox, Container, Element, Empty, Flex, ParentElement, Rect,
        Shrinkable, Stack, Text,
    },
    fonts::FamilyId,
};

pub use warp_core::ui::theme::*;

pub use warp_core::ui::color::blend::Blend;

const THUMBNAIL_MARGIN: f32 = 10.;

#[derive(
    Clone,
    Debug,
    Default,
    PartialEq,
    Serialize,
    Hash,
    Eq,
    Ord,
    PartialOrd,
    schemars::JsonSchema,
)]
#[schemars(description = "The color theme.", rename_all = "snake_case")]
pub enum ThemeKind {
    #[default]
    #[schemars(description = "Dark")]
    Dark,
    #[schemars(description = "Light")]
    Light,
}

impl<'de> Deserialize<'de> for ThemeKind {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct Fallback;

        impl<'de> serde::de::Visitor<'de> for Fallback {
            type Value = ThemeKind;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("a theme name")
            }

            fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<ThemeKind, E> {
                match value {
                    // 兼容新旧两种拼写:旧 TOML 用 PascalCase("Dark"/"Light"),
                    // 新序列化输出小写。
                    "light" | "Light" => Ok(ThemeKind::Light),
                    _ => Ok(ThemeKind::Dark),
                }
            }

            // Legacy persisted shapes for removed variants (e.g. `{"custom": …}`)
            // also fall back to Dark instead of failing to deserialize.
            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut access: A,
            ) -> Result<ThemeKind, A::Error> {
                while access.next_key::<serde::de::IgnoredAny>()?.is_some() {
                    access.next_value::<serde::de::IgnoredAny>()?;
                }
                Ok(ThemeKind::Dark)
            }
        }

        deserializer.deserialize_any(Fallback)
    }
}

/// Unknown values from legacy settings files map to `Dark` instead of failing.
impl settings_value::SettingsValue for ThemeKind {
    fn from_file_value(value: &serde_json::Value) -> Option<Self> {
        match value.as_str() {
            Some("light") => Some(ThemeKind::Light),
            _ => Some(ThemeKind::Dark),
        }
    }
}

impl std::fmt::Display for ThemeKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let value = match &self {
            ThemeKind::Light => "Light",
            ThemeKind::Dark => "Dark",
        };
        write!(f, "{value}")
    }
}

impl ThemeKind {
    pub fn matches(&self, query: &str) -> bool {
        let theme_name = format!("{self}").to_lowercase();
        theme_name.contains(&query.to_lowercase())
    }
}

#[derive(Debug, Clone)]
pub struct WarpThemeConfig {
    theme_map: HashMap<ThemeKind, WarpTheme>,
}

impl WarpThemeConfig {
    pub fn new() -> Self {
        // preload with built-in themes
        let theme_map: HashMap<ThemeKind, WarpTheme> = HashMap::from_iter([
            (ThemeKind::Dark, dark_theme()),
            (ThemeKind::Light, light_theme()),
        ]);
        WarpThemeConfig { theme_map }
    }

    pub fn add_new_theme(&mut self, theme_name: ThemeKind, theme: WarpTheme) {
        self.theme_map.insert(theme_name, theme);
    }

    pub fn theme_items(&self) -> impl Iterator<Item = (&ThemeKind, &WarpTheme)> {
        self.theme_map.iter()
    }

    pub fn theme(&self, name: &ThemeKind) -> WarpTheme {
        self.theme_map.get(name).cloned().unwrap_or_else(dark_theme)
    }
}

impl Default for WarpThemeConfig {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum RespectSystemTheme {
    #[default]
    Off,
    On(SelectedSystemThemes),
}

#[derive(
    Clone,
    Debug,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    schemars::JsonSchema,
    settings_value::SettingsValue,
)]
#[schemars(description = "Themes to use when following the system light/dark mode.")]
pub struct SelectedSystemThemes {
    #[schemars(description = "The theme to use in light mode.")]
    pub light: ThemeKind,
    #[schemars(description = "The theme to use in dark mode.")]
    pub dark: ThemeKind,
}

impl RespectSystemTheme {
    pub fn selected_system_themes(&self) -> Option<&SelectedSystemThemes> {
        match self {
            RespectSystemTheme::Off => None,
            RespectSystemTheme::On(selected) => Some(selected),
        }
    }
}

impl Default for SelectedSystemThemes {
    fn default() -> Self {
        Self {
            light: ThemeKind::Light,
            dark: ThemeKind::Dark,
        }
    }
}

#[derive(Copy, Clone, Debug)]
pub struct PromptColors {
    pub input_prompt_conversation_management: ColorU,
    pub input_prompt_pwd: ColorU,
    pub input_prompt_git: ColorU,
    pub input_prompt_branch: ColorU,
    pub input_prompt_agent_mode_hint: ColorU,
    pub input_prompt_agent_mode_tasks: ColorU,
    pub input_prompt_dirty_color: ColorU,
    pub input_prompt_virtual_env: ColorU,
    pub input_prompt_user_and_host: ColorU,
    pub input_prompt_date: ColorU,
    pub input_prompt_time: ColorU,
    pub input_prompt_kubernetes: ColorU,
    pub input_prompt_svn: ColorU,
    pub input_prompt_separator: ColorU,
    pub input_prompt_subshell: ColorU,
    pub input_prompt_ssh: ColorU,
}

impl From<WarpTheme> for PromptColors {
    fn from(theme: WarpTheme) -> Self {
        PromptColors {
            input_prompt_conversation_management: theme.terminal_colors().normal.white.into(),
            input_prompt_pwd: theme.terminal_colors().normal.magenta.into(),
            input_prompt_git: theme.terminal_colors().normal.green.into(),
            input_prompt_agent_mode_hint: theme.terminal_colors().normal.yellow.into(),
            input_prompt_agent_mode_tasks: theme.terminal_colors().normal.yellow.into(),
            input_prompt_branch: theme.terminal_colors().normal.yellow.into(),
            input_prompt_dirty_color: theme.terminal_colors().normal.green.into(),
            input_prompt_virtual_env: theme.terminal_colors().normal.yellow.into(),
            input_prompt_user_and_host: theme.terminal_colors().normal.green.into(),
            input_prompt_date: theme.terminal_colors().normal.cyan.into(),
            input_prompt_time: theme.terminal_colors().normal.red.into(),
            input_prompt_kubernetes: theme.terminal_colors().normal.cyan.into(),
            input_prompt_ssh: theme.terminal_colors().normal.blue.into(),
            input_prompt_subshell: theme.terminal_colors().normal.blue.into(),
            input_prompt_svn: theme.terminal_colors().normal.blue.into(),
            input_prompt_separator: theme.terminal_colors().normal.magenta.into(),
        }
    }
}

pub fn render_preview(
    theme: &WarpTheme,
    font_family: FamilyId,
    form_factor: Option<f32>,
) -> Box<dyn Element> {
    let text_size = 8. * form_factor.unwrap_or(1.);
    let margin = THUMBNAIL_MARGIN * form_factor.unwrap_or(1.);
    let padding = 5. * form_factor.unwrap_or(1.);
    let text_line_1 = Container::new(
        Text::new_inline("ls", font_family, text_size)
            .with_color(theme.foreground().into_solid())
            .finish(),
    )
    .with_margin_left(margin)
    .with_margin_right(margin)
    .finish();

    let text_line_2 = Container::new(
        Flex::row()
            .with_child(
                Text::new_inline("dir   ", font_family, text_size)
                    .with_color(theme.terminal_colors().normal.blue.into())
                    .finish(),
            )
            .with_child(
                Text::new_inline("executable   ", font_family, text_size)
                    .with_color(theme.terminal_colors().normal.red.into())
                    .finish(),
            )
            .with_child(
                Text::new_inline("file", font_family, text_size)
                    .with_color(theme.foreground().into_solid())
                    .finish(),
            )
            .finish(),
    )
    .with_margin_left(margin)
    .with_margin_right(margin)
    .finish();

    let input_box = Shrinkable::new(
        1.,
        Align::new(
            Flex::column()
                // The border above the input box.
                .with_child(
                    Container::new(Empty::new().finish())
                        .with_padding_bottom(padding)
                        .with_border(
                            Border::top(1.2 * form_factor.unwrap_or(1.))
                                .with_border_color(theme.outline().into_solid()),
                        )
                        .finish(),
                )
                // The fake cursor within the input box.
                .with_child(
                    Container::new(
                        ConstrainedBox::new(
                            Rect::new()
                                .with_background_color(theme.accent().into_solid())
                                .finish(),
                        )
                        .with_height(12. * form_factor.unwrap_or(1.))
                        .with_width(2. * form_factor.unwrap_or(1.))
                        .finish(),
                    )
                    .with_margin_left(margin)
                    .with_margin_right(margin)
                    .finish(),
                )
                .finish(),
        )
        .bottom_left()
        .finish(),
    )
    .finish();

    let mut thumbnail = Stack::new();
    let mut background_opacity = 100;
    if let Some(background_image) = theme.background_image() {
        thumbnail.add_child(
            Shrinkable::new(
                1.,
                warpui::elements::Image::new(
                    background_image.source(),
                    warpui::elements::CacheOption::BySize,
                )
                .cover()
                .finish(),
            )
            .finish(),
        );
        background_opacity -= background_image.opacity;
    }

    thumbnail.add_child(
        Container::new(
            Container::new(
                Flex::column()
                    .with_child(text_line_1)
                    .with_child(
                        Container::new(text_line_2)
                            .with_padding_top(padding)
                            .finish(),
                    )
                    .with_child(input_box)
                    .finish(),
            )
            .with_margin_top(margin)
            .with_margin_bottom(margin)
            .finish(),
        )
        .with_background(theme.background().with_opacity(background_opacity))
        .finish(),
    );

    Align::new(
        Container::new(
            ConstrainedBox::new(thumbnail.finish())
                .with_height(100. * form_factor.unwrap_or(1.))
                .with_width(190. * form_factor.unwrap_or(1.))
                .finish(),
        )
        .finish(),
    )
    .finish()
}

#[cfg(test)]
#[path = "theme_test.rs"]
mod tests;
