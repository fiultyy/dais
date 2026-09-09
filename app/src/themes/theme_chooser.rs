use settings::{Setting as _, ToggleableSetting as _};
use warpui::{
    accessibility::{AccessibilityContent, WarpA11yRole},
    elements::{
        Align, Border, ConstrainedBox, Container, CornerRadius, CrossAxisAlignment, Element, Empty,
        Flex, Hoverable, MainAxisAlignment, MainAxisSize, MouseStateHandle, ParentElement, Radius,
        Shrinkable,
    },
    fonts::Weight,
    keymap::FixedBinding,
    platform::{Cursor, SystemTheme},
    ui_components::{
        components::{UiComponent, UiComponentStyles},
        switch::SwitchStateHandle,
    },
    AppContext, Entity, SingletonEntity, Tracked, TypedActionView, UpdateModel, View, ViewContext,
};

use crate::{
    appearance::{Appearance, AppearanceManager},
    ui_components::buttons::close_button,
    report_if_error,
    settings::{respect_system_theme, ThemeSettings},
    themes::theme::{RespectSystemTheme, SelectedSystemThemes, ThemeKind, WarpTheme},
    util::traffic_lights::{traffic_light_data, TrafficLightData, TrafficLightSide},
    window_settings::WindowSettings,
    workspace::PANEL_HEADER_HEIGHT,
};

use super::{default_themes, theme};

// All units in px
const THEME_CHOOSER_TITLE: &str = "主题";
const FOLLOW_SYSTEM_LABEL: &str = "跟随系统";
const CLOSE_BUTTON_MARGIN_RIGHT: f32 = 6.;
const THEME_NAME_FONT_SIZE: f32 = 14.;
const TITLE_MARGIN: f32 = 12.;
const CARD_PADDING: f32 = 8.;
const CARD_PREVIEW_SCALE: f32 = 0.7;
const CARD_CORNER_RADIUS: f32 = 6.;

pub enum ThemeChooserEvent {
    Click,
    Close(ThemeChooserMode),
}

#[derive(Clone, Copy, Debug)]
#[allow(clippy::enum_variant_names)]
pub enum ThemeChooserMode {
    /// Select a single fixed theme, independent of whether the system is using
    /// a light or dark theme.
    SystemAgnostic,
    /// Select a theme to use when the system is using a light theme.
    SystemLight,
    /// Select a theme to use when the system is using a dark theme.
    SystemDark,
}

impl ThemeChooserMode {
    /// Returns the mode the theme chooser should use if the aim is to change
    /// the active theme, as opposed to changing a specific theme option.
    pub fn for_active_theme(app: &AppContext) -> Self {
        match respect_system_theme(ThemeSettings::as_ref(app)) {
            RespectSystemTheme::On(_) => match app.system_theme() {
                SystemTheme::Dark => ThemeChooserMode::SystemDark,
                SystemTheme::Light => ThemeChooserMode::SystemLight,
            },
            RespectSystemTheme::Off => ThemeChooserMode::SystemAgnostic,
        }
    }

    pub fn into_theme_kind(self, ctx: &AppContext) -> ThemeKind {
        let theme_settings = ThemeSettings::as_ref(ctx);
        let theme_kind = theme_settings.theme_kind.value();
        match (self, &respect_system_theme(theme_settings)) {
            (ThemeChooserMode::SystemAgnostic, _) => theme_kind.clone(),
            (ThemeChooserMode::SystemLight, RespectSystemTheme::On(system_themes)) => {
                system_themes.light.clone()
            }
            (ThemeChooserMode::SystemDark, RespectSystemTheme::On(system_themes)) => {
                system_themes.dark.clone()
            }
            (_, _) => ThemeKind::default(),
        }
    }
}

pub struct ThemeChooser {
    close_button_mouse_state: MouseStateHandle,
    dark_card_mouse_state: MouseStateHandle,
    light_card_mouse_state: MouseStateHandle,
    follow_system_switch_state: SwitchStateHandle,
    selected_theme: Tracked<Option<ThemeKind>>,
    mode: ThemeChooserMode,
    window_id: warpui::WindowId,
}

#[derive(Debug)]
pub enum ThemeChooserAction {
    Close,
    Enter,
    Click(ThemeKind),
    ToggleFollowSystem,
}

pub fn init(app: &mut AppContext) {
    use warpui::keymap::macros::*;

    app.register_fixed_bindings(vec![
        FixedBinding::new("escape", ThemeChooserAction::Close, id!("ThemeChooser")),
        FixedBinding::new("enter", ThemeChooserAction::Enter, id!("ThemeChooser")),
    ]);
}

impl ThemeChooser {
    pub fn new(ctx: &mut ViewContext<Self>) -> Self {
        Self {
            close_button_mouse_state: Default::default(),
            dark_card_mouse_state: Default::default(),
            light_card_mouse_state: Default::default(),
            follow_system_switch_state: Default::default(),
            selected_theme: Tracked::new(None),
            mode: ThemeChooserMode::for_active_theme(ctx),
            window_id: ctx.window_id(),
        }
    }

    pub fn set_mode(&mut self, mode: ThemeChooserMode) {
        self.mode = mode;
    }

    /// Re-syncs the highlighted card with the persisted theme settings. Called
    /// by the workspace when the chooser is open and settings changed elsewhere
    /// (e.g. cloud preferences sync).
    pub fn handle_theme_change(&mut self, ctx: &mut ViewContext<Self>) {
        let theme_settings = ThemeSettings::as_ref(ctx);
        let kind = match (self.mode, respect_system_theme(theme_settings)) {
            (ThemeChooserMode::SystemLight, RespectSystemTheme::On(system_themes)) => {
                system_themes.light.clone()
            }
            (ThemeChooserMode::SystemDark, RespectSystemTheme::On(system_themes)) => {
                system_themes.dark.clone()
            }
            _ => theme_settings.theme_kind.value().clone(),
        };
        self.select_theme(kind, ctx);
    }

    /// Highlights a card and live-previews it without committing to settings.
    pub fn select_theme(&mut self, kind: ThemeKind, ctx: &mut ViewContext<Self>) {
        *self.selected_theme = Some(kind.clone());
        AppearanceManager::handle(ctx).update(ctx, |appearance_manager, ctx| {
            appearance_manager.set_transient_theme(kind, ctx);
        });
        ctx.notify();
    }

    /// Click-to-apply: highlights the card, writes the theme into
    /// `ThemeSettings` for the chooser's mode, and leaves the live preview in
    /// place (the settings change makes it permanent).
    fn apply_selection(&mut self, kind: ThemeKind, ctx: &mut ViewContext<Self>) {
        self.select_theme(kind.clone(), ctx);

        let mode = self.mode;
        ThemeSettings::handle(ctx).update(ctx, move |theme_settings, ctx| {
            match mode {
                ThemeChooserMode::SystemAgnostic => {
                    report_if_error!(theme_settings.theme_kind.set_value(kind.clone(), ctx));
                }
                ThemeChooserMode::SystemLight => {
                    let dark = theme_settings.selected_system_themes.value().dark.clone();
                    report_if_error!(theme_settings.selected_system_themes.set_value(
                        SelectedSystemThemes {
                            light: kind.clone(),
                            dark
                        },
                        ctx,
                    ));
                }
                ThemeChooserMode::SystemDark => {
                    let light = theme_settings.selected_system_themes.value().light.clone();
                    report_if_error!(theme_settings.selected_system_themes.set_value(
                        SelectedSystemThemes {
                            light,
                            dark: kind.clone()
                        },
                        ctx,
                    ));
                }
            }
        });
    }

    fn click(&mut self, kind: ThemeKind, ctx: &mut ViewContext<Self>) {
        self.apply_selection(kind, ctx);
        ctx.emit(ThemeChooserEvent::Click);
    }

    fn toggle_follow_system(&mut self, ctx: &mut ViewContext<Self>) {
        ThemeSettings::handle(ctx).update(ctx, |theme_settings, ctx| {
            report_if_error!(theme_settings.use_system_theme.toggle_and_save_value(ctx));
        });
        ctx.notify();
    }

    fn close(&mut self, ctx: &mut ViewContext<Self>) {
        *self.selected_theme = None;
        AppearanceManager::handle(ctx).update(ctx, |appearance_manager, ctx| {
            appearance_manager.clear_transient_theme(ctx);
        });
        ctx.emit(ThemeChooserEvent::Close(self.mode));
    }

    fn render_header(
        &self,
        traffic_light_data: Option<&TrafficLightData>,
        appearance: &Appearance,
        app: &AppContext,
    ) -> Box<dyn Element> {
        let mut margin_left = 16.;

        let zoom_factor = WindowSettings::as_ref(app).zoom_level.as_zoom_factor();
        // Since this panel is always on the left, only account for left-side traffic lights.
        if let Some(width) = traffic_light_data
            .filter(|data| data.side == TrafficLightSide::Left)
            .map(|data| data.width(zoom_factor))
        {
            margin_left += width;
        }

        let close_button = close_button(appearance, self.close_button_mouse_state.clone())
            .build()
            .on_click(|ctx: &mut warpui::EventContext<'_>, _, _| {
                ctx.dispatch_typed_action(ThemeChooserAction::Close)
            })
            .finish();

        ConstrainedBox::new(
            Flex::row()
                .with_child(
                    Container::new(Align::new(close_button).finish())
                        .with_margin_left(margin_left)
                        .with_margin_right(CLOSE_BUTTON_MARGIN_RIGHT)
                        .finish(),
                )
                .with_main_axis_size(MainAxisSize::Max)
                .with_main_axis_alignment(MainAxisAlignment::End)
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .finish(),
        )
        .with_height(PANEL_HEADER_HEIGHT)
        .finish()
    }

    fn render_title(&self, appearance: &Appearance) -> Box<dyn Element> {
        Container::new(
            appearance
                .ui_builder()
                .span(THEME_CHOOSER_TITLE.to_string())
                .with_style(UiComponentStyles {
                    font_family_id: Some(appearance.ui_font_family()),
                    font_size: Some(appearance.ui_font_heading_3()),
                    font_weight: Some(Weight::Semibold),
                    ..Default::default()
                })
                .build()
                .finish(),
        )
        .with_margin_left(TITLE_MARGIN)
        .with_margin_right(TITLE_MARGIN)
        .with_margin_bottom(6.)
        .finish()
    }

    fn render_cards(&self, appearance: &Appearance) -> Box<dyn Element> {
        Flex::row()
            .with_main_axis_alignment(MainAxisAlignment::SpaceBetween)
            .with_main_axis_size(MainAxisSize::Max)
            .with_child(self.render_card(
                ThemeKind::Dark,
                default_themes::dark_theme(),
                self.dark_card_mouse_state.clone(),
                appearance,
            ))
            .with_child(self.render_card(
                ThemeKind::Light,
                default_themes::light_theme(),
                self.light_card_mouse_state.clone(),
                appearance,
            ))
            .finish()
    }

    fn render_card(
        &self,
        kind: ThemeKind,
        warp_theme: WarpTheme,
        mouse_state: MouseStateHandle,
        appearance: &Appearance,
    ) -> Box<dyn Element> {
        let is_selected = self.selected_theme.as_ref() == Some(&kind);
        let accent = appearance.theme().accent();
        let kind_for_click = kind.clone();

        Hoverable::new(mouse_state, move |hover_state| {
            let thumbnail = theme::render_preview(
                &warp_theme,
                appearance.monospace_font_family(),
                Some(CARD_PREVIEW_SCALE),
            );
            let label = appearance
                .ui_builder()
                .span(kind.to_string())
                .with_style(UiComponentStyles {
                    font_family_id: Some(appearance.ui_font_family()),
                    font_size: Some(THEME_NAME_FONT_SIZE),
                    ..Default::default()
                })
                .build()
                .finish();

            let border_width = if is_selected {
                1.5
            } else if hover_state.is_hovered() {
                0.5
            } else {
                0.
            };

            Container::new(
                Flex::column()
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_child(thumbnail)
                    .with_child(Container::new(label).with_margin_top(8.).finish())
                    .finish(),
            )
            .with_border(Border::all(border_width).with_border_fill(accent))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(CARD_CORNER_RADIUS)))
            .with_padding_top(CARD_PADDING - border_width)
            .with_padding_bottom(CARD_PADDING - border_width)
            .with_padding_left(CARD_PADDING - border_width)
            .with_padding_right(CARD_PADDING - border_width)
            .finish()
        })
        .with_cursor(Cursor::PointingHand)
        .on_click(move |ctx, _, _| {
            ctx.dispatch_typed_action(ThemeChooserAction::Click(kind_for_click.clone()))
        })
        .finish()
    }

    fn render_follow_system_row(&self, appearance: &Appearance, app: &AppContext) -> Box<dyn Element> {
        let enabled = matches!(
            respect_system_theme(ThemeSettings::as_ref(app)),
            RespectSystemTheme::On(_)
        );

        Container::new(
            Flex::row()
                .with_main_axis_alignment(MainAxisAlignment::SpaceBetween)
                .with_main_axis_size(MainAxisSize::Max)
                .with_child(
                    appearance
                        .ui_builder()
                        .span(FOLLOW_SYSTEM_LABEL.to_string())
                        .with_style(UiComponentStyles {
                            font_family_id: Some(appearance.ui_font_family()),
                            font_size: Some(THEME_NAME_FONT_SIZE),
                            ..Default::default()
                        })
                        .build()
                        .finish(),
                )
                .with_child(
                    appearance
                        .ui_builder()
                        .switch(self.follow_system_switch_state.clone())
                        .check(enabled)
                        .build()
                        .on_click(|ctx, _, _| {
                            ctx.dispatch_typed_action(ThemeChooserAction::ToggleFollowSystem)
                        })
                        .finish(),
                )
                .finish(),
        )
        .with_margin_left(TITLE_MARGIN)
        .with_margin_right(TITLE_MARGIN)
        .with_margin_bottom(12.)
        .finish()
    }
}

impl Entity for ThemeChooser {
    type Event = ThemeChooserEvent;
}

impl TypedActionView for ThemeChooser {
    type Action = ThemeChooserAction;

    fn handle_action(&mut self, action: &ThemeChooserAction, ctx: &mut ViewContext<Self>) {
        use ThemeChooserAction::*;

        match action {
            Click(kind) => self.click(kind.clone(), ctx),
            Close | Enter => self.close(ctx),
            ToggleFollowSystem => self.toggle_follow_system(ctx),
        }
    }
}

impl View for ThemeChooser {
    fn ui_name() -> &'static str {
        "ThemeChooser"
    }

    fn accessibility_contents(&self, _: &AppContext) -> Option<AccessibilityContent> {
        Some(AccessibilityContent::new(
            "主题选择器",
            "点击卡片切换主题,按 Esc 关闭。",
            WarpA11yRole::WindowRole,
        ))
    }

    fn render(&self, app: &AppContext) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        let traffic_lights = traffic_light_data(app, self.window_id);

        Container::new(
            Flex::column()
                .with_child(self.render_header(traffic_lights.as_ref(), appearance, app))
                .with_child(self.render_title(appearance))
                .with_child(self.render_cards(appearance))
                // Filler expands the panel to full height, pinning the
                // follow-system toggle to the bottom.
                .with_child(Shrinkable::new(1., Empty::new().finish()).finish())
                .with_child(self.render_follow_system_row(appearance, app))
                .finish(),
        )
        .finish()
    }
}
