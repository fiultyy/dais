//! Issue #13 UI — 拦截配置栏:
//!
//! * [`InterceptModeSelector`] — InterceptMode 下拉 (Full / HooksOnly / Bypass),
//!   结构复刻 [`HarnessSelector`](super::harness_selector::HarnessSelector)。
//! * [`InterceptConfigBar`] — 渲染在 agent 输入框上方的配置行:
//!   模式选择器 + 已捕获 block 计数 + Upstream 配置面板开关。
//!   Upstream 面板含 API Base 输入框(留空=自动探测)与 Auth Env 输入框,
//!   并展示按三级优先解析出的探测结果。

use std::sync::Arc;

use pathfinder_geometry::vector::vec2f;
use warpui::r#async::Timer;
use warpui::r#async::SpawnedFutureHandle;
use warpui::{
    elements::{
        Border, ChildAnchor, ChildView, Container, CrossAxisAlignment, Empty, Expanded, Flex,
        MainAxisSize, MainAxisAlignment, OffsetPositioning, ParentAnchor, ParentElement,
        ParentOffsetBounds, Stack,
    },
    AppContext, Element, Entity, ModelHandle, SingletonEntity, TypedActionView, View, ViewContext,
    ViewHandle,
};

use harness_integration::{HarnessType, InterceptMode};
use warp_cli::agent::Harness;
use warp_core::ui::appearance::Appearance;
use warp_core::ui::theme::color::internal_colors;
use warp_core::ui::theme::Fill;

use crate::ai::blocklist::agent_view::agent_input_footer::AgentInputButtonTheme;
use crate::menu::{Event as MenuEvent, Menu, MenuItem, MenuItemFields};
use crate::terminal::input::{MenuPositioning, MenuPositioningProvider};
use crate::terminal::intercept_sessions::InterceptSessionsModel;
use crate::terminal::view::ambient_agent::model::AmbientAgentViewModel;
use crate::view_components::action_button::{ActionButton, ButtonSize};
use crate::ui_components::icons::Icon;
use crate::view_components::{SubmittableTextInput, SubmittableTextInputEvent};

/// Width of the intercept-mode dropdown panel in logical pixels.
const MENU_WIDTH: f32 = 208.;
/// Horizontal padding inside dropdown rows.
const MENU_HORIZONTAL_PADDING: f32 = 16.;
/// Vertical padding on dropdown rows.
const ITEM_VERTICAL_PADDING: f32 = 8.;
/// Vertical padding on the dropdown header row.
const HEADER_VERTICAL_PADDING: f32 = 6.;
/// Font size for the dropdown header row.
const HEADER_FONT_SIZE: f32 = 12.;
/// Font size for dropdown item rows.
const ITEM_FONT_SIZE: f32 = 14.;
/// Spacing between the controls in the config bar row.
const BAR_SPACING: f32 = 8.;
/// How often (ms) the block counter re-queries the BlockStore while the bar is visible.
const BLOCK_COUNT_REFRESH_INTERVAL_MS: u64 = 2_000;

// ── InterceptModeSelector ──────────────────────────────────────────────────

/// Actions dispatched by the [`InterceptModeSelector`].
#[derive(Clone, Debug, PartialEq)]
pub enum InterceptModeSelectorAction {
    /// Toggle the visibility of the dropdown menu.
    ToggleMenu,
    /// The user picked an intercept mode from the dropdown.
    SelectMode(InterceptMode),
}

/// Events emitted by the [`InterceptModeSelector`].
pub enum InterceptModeSelectorEvent {
    /// The dropdown visibility changed.
    MenuVisibilityChanged { open: bool },
}

/// A dropdown selector for the harness LLM-traffic intercept mode.
pub struct InterceptModeSelector {
    button: ViewHandle<ActionButton>,
    menu: ViewHandle<Menu<InterceptModeSelectorAction>>,
    is_menu_open: bool,
    menu_positioning_provider: Arc<dyn MenuPositioningProvider>,
}

impl InterceptModeSelector {
    pub fn new(
        menu_positioning_provider: Arc<dyn MenuPositioningProvider>,
        ctx: &mut ViewContext<Self>,
    ) -> Self {
        let button = ctx.add_typed_action_view(|_ctx| {
            ActionButton::new("", AgentInputButtonTheme)
                .with_size(ButtonSize::AgentInputButton)
                .with_menu(true)
                .with_tooltip(crate::t!("intercept-mode-tooltip"))
                .on_click(|ctx| {
                    ctx.dispatch_typed_action(InterceptModeSelectorAction::ToggleMenu);
                })
        });

        let menu = ctx.add_typed_action_view(|_ctx| {
            Menu::new()
                .with_width(MENU_WIDTH)
                .with_drop_shadow()
                .prevent_interaction_with_other_elements()
        });

        ctx.subscribe_to_view(&menu, |me, _, event, ctx| match event {
            MenuEvent::Close { .. } => {
                me.set_menu_visibility(false, ctx);
            }
            MenuEvent::ItemSelected | MenuEvent::ItemHovered => {}
        });

        ctx.subscribe_to_model(&InterceptSessionsModel::handle(ctx), |me, _, _, ctx| {
            me.refresh_button(ctx);
        });

        ctx.subscribe_to_model(&Appearance::handle(ctx), |me, _, _, ctx| {
            me.refresh_menu(ctx);
        });

        let mut me = Self {
            button,
            menu,
            is_menu_open: false,
            menu_positioning_provider,
        };
        me.refresh_button(ctx);
        me.refresh_menu(ctx);
        me
    }

    pub fn is_menu_open(&self) -> bool {
        self.is_menu_open
    }

    fn set_menu_visibility(&mut self, is_open: bool, ctx: &mut ViewContext<Self>) {
        if self.is_menu_open == is_open {
            return;
        }
        self.is_menu_open = is_open;
        if is_open {
            ctx.focus(&self.menu);
        }
        ctx.emit(InterceptModeSelectorEvent::MenuVisibilityChanged { open: is_open });
        ctx.notify();
    }

    fn refresh_button(&mut self, ctx: &mut ViewContext<Self>) {
        let mode = InterceptSessionsModel::as_ref(ctx).mode();
        let label = mode_label(mode);
        self.button.update(ctx, |button, ctx| {
            button.set_label(label, ctx);
            button.set_icon(Some(Icon::FilterFunnel), ctx);
        });
    }

    fn refresh_menu(&mut self, ctx: &mut ViewContext<Self>) {
        let appearance = Appearance::as_ref(ctx);
        let theme = appearance.theme();
        let hover_background: Fill = internal_colors::neutral_4(theme).into();
        let header_text_color = theme.disabled_text_color(theme.surface_2()).into_solid();
        let border = Border::all(1.).with_border_fill(theme.outline());
        let items = build_menu_items(hover_background, header_text_color);
        self.menu.update(ctx, |menu, ctx| {
            menu.set_border(Some(border));
            menu.set_items(items, ctx);
        });
    }

    fn menu_positioning(&self, app: &AppContext) -> OffsetPositioning {
        match self.menu_positioning_provider.menu_position(app) {
            MenuPositioning::BelowInputBox => OffsetPositioning::offset_from_parent(
                vec2f(0., 4.),
                ParentOffsetBounds::WindowByPosition,
                ParentAnchor::BottomLeft,
                ChildAnchor::TopLeft,
            ),
            MenuPositioning::AboveInputBox => OffsetPositioning::offset_from_parent(
                vec2f(0., -4.),
                ParentOffsetBounds::WindowByPosition,
                ParentAnchor::TopLeft,
                ChildAnchor::BottomLeft,
            ),
        }
    }
}

/// Localized display label for an intercept mode.
fn mode_label(mode: InterceptMode) -> String {
    match mode {
        InterceptMode::Full => crate::t!("intercept-mode-full"),
        InterceptMode::HooksOnly => crate::t!("intercept-mode-hooks-only"),
        InterceptMode::Bypass => crate::t!("intercept-mode-bypass"),
    }
}

fn build_menu_items(
    hover_background: Fill,
    header_text_color: pathfinder_color::ColorU,
) -> Vec<MenuItem<InterceptModeSelectorAction>> {
    let header = MenuItem::Header {
        fields: MenuItemFields::new(crate::t!("intercept-mode-header"))
            .with_font_size_override(HEADER_FONT_SIZE)
            .with_override_text_color(header_text_color)
            .with_padding_override(HEADER_VERTICAL_PADDING, MENU_HORIZONTAL_PADDING)
            .with_no_interaction_on_hover(),
        clickable: false,
        right_side_fields: None,
    };
    let item_for = |mode: InterceptMode| {
        MenuItem::Item(
            MenuItemFields::new(mode_label(mode))
                .with_font_size_override(ITEM_FONT_SIZE)
                .with_padding_override(ITEM_VERTICAL_PADDING, MENU_HORIZONTAL_PADDING)
                .with_override_hover_background_color(hover_background)
                .with_on_select_action(InterceptModeSelectorAction::SelectMode(mode)),
        )
    };

    vec![
        header,
        item_for(InterceptMode::Full),
        item_for(InterceptMode::HooksOnly),
        item_for(InterceptMode::Bypass),
    ]
}

impl Entity for InterceptModeSelector {
    type Event = InterceptModeSelectorEvent;
}

impl TypedActionView for InterceptModeSelector {
    type Action = InterceptModeSelectorAction;

    fn handle_action(&mut self, action: &Self::Action, ctx: &mut ViewContext<Self>) {
        match action {
            InterceptModeSelectorAction::ToggleMenu => {
                let new_state = !self.is_menu_open;
                self.set_menu_visibility(new_state, ctx);
            }
            InterceptModeSelectorAction::SelectMode(mode) => {
                let mode = *mode;
                InterceptSessionsModel::handle(ctx).update(ctx, |model, ctx| {
                    model.set_mode(mode, ctx);
                });
                self.set_menu_visibility(false, ctx);
            }
        }
    }
}

impl View for InterceptModeSelector {
    fn ui_name() -> &'static str {
        "InterceptModeSelector"
    }

    fn render(&self, app: &AppContext) -> Box<dyn Element> {
        let mut stack = Stack::new();
        stack.add_child(ChildView::new(&self.button).finish());

        if self.is_menu_open {
            let positioning = self.menu_positioning(app);
            stack.add_positioned_overlay_child(ChildView::new(&self.menu).finish(), positioning);
        }

        stack.finish()
    }
}

// ── InterceptConfigBar ─────────────────────────────────────────────────────

/// Actions dispatched by the [`InterceptConfigBar`].
#[derive(Clone, Debug, PartialEq)]
pub enum InterceptConfigBarAction {
    /// Toggle the upstream configuration panel.
    TogglePanel,
}

/// Events emitted by the [`InterceptConfigBar`].
pub enum InterceptConfigBarEvent {
    /// The panel visibility changed.
    PanelVisibilityChanged { open: bool },
}

/// The intercept configuration bar shown above the agent input:
/// intercept-mode selector + captured-block counter + upstream panel toggle.
pub struct InterceptConfigBar {
    mode_selector: ViewHandle<InterceptModeSelector>,
    upstream_button: ViewHandle<ActionButton>,
    api_base_input: ViewHandle<SubmittableTextInput>,
    auth_env_input: ViewHandle<SubmittableTextInput>,
    /// Handle for the periodic block-count refresh timer. Aborted on drop.
    refresh_timer_handle: Option<SpawnedFutureHandle>,
    panel_open: bool,
    ambient_agent_model: ModelHandle<AmbientAgentViewModel>,
}

impl InterceptConfigBar {
    pub fn new(
        menu_positioning_provider: Arc<dyn MenuPositioningProvider>,
        ambient_agent_model: ModelHandle<AmbientAgentViewModel>,
        ctx: &mut ViewContext<Self>,
    ) -> Self {
        let mode_selector = ctx.add_typed_action_view(|ctx| {
            InterceptModeSelector::new(menu_positioning_provider.clone(), ctx)
        });
        ctx.subscribe_to_view(&mode_selector, |_me, _, _, ctx| {
            ctx.notify();
        });

        let upstream_button = ctx.add_typed_action_view(|_ctx| {
            ActionButton::new(crate::t!("intercept-upstream-button"), AgentInputButtonTheme)
                .with_size(ButtonSize::AgentInputButton)
                .with_tooltip(crate::t!("intercept-upstream-tooltip"))
                .with_menu(true)
                .on_click(|ctx| {
                    ctx.dispatch_typed_action(InterceptConfigBarAction::TogglePanel);
                })
        });

        let api_base_input = ctx.add_typed_action_view(|ctx| {
            let mut input = SubmittableTextInput::new(ctx)
                .validate_on_edit(|_| true)
                // 空提交 = 清除显式覆盖,恢复 auto-detect。
                .with_allow_empty_submit();
            input.set_placeholder_text(crate::t!("intercept-api-base-placeholder"), ctx);
            input
        });
        let auth_env_input = ctx.add_typed_action_view(|ctx| {
            let mut input = SubmittableTextInput::new(ctx)
                .validate_on_edit(|_| true)
                // 空提交 = 清除显式覆盖,恢复解析默认。
                .with_allow_empty_submit();
            input.set_placeholder_text(crate::t!("intercept-auth-env-placeholder"), ctx);
            input
        });

        // 分别订阅:提交来源与字段一一对应,不靠比较值猜测来源;
        // 空提交经 with_allow_empty_submit 放行,作为清除覆盖的显式入口。
        for (input, is_base) in [(&api_base_input, true), (&auth_env_input, false)] {
            ctx.subscribe_to_view(input, move |me, _, event, ctx| match event {
                SubmittableTextInputEvent::Submit(content) => {
                    InterceptSessionsModel::handle(ctx).update(ctx, |model, ctx| {
                        if is_base {
                            model.set_upstream_base(content.clone(), ctx);
                        } else {
                            model.set_upstream_auth_env(content.clone(), ctx);
                        }
                    });
                    me.refresh_upstream_input_prefills(ctx);
                    ctx.notify();
                }
                SubmittableTextInputEvent::Escape => {
                    me.set_panel_open(false, ctx);
                }
            });
        }

        // Re-render when the intercept config or block count changes.
        ctx.subscribe_to_model(&InterceptSessionsModel::handle(ctx), |_me, _, _, ctx| {
            ctx.notify();
        });

        // Periodically re-query the block store so the counter stays fresh.
        // Self-rescheduling one-shot timer; dropping the view aborts it.
        let mut me = Self {
            mode_selector,
            upstream_button,
            api_base_input,
            auth_env_input,
            panel_open: false,
            refresh_timer_handle: None,
            ambient_agent_model,
        };
        // flag 未开启时不启动轮询 timer:UI 渲染同样被 flag 挡住,
        // 避免每个 pane 常驻一个空转的 2s 唤醒。
        if crate::features::FeatureFlag::AgentHarness.is_enabled() {
            me.start_refresh_timer(ctx);
        }
        me
    }

    /// Starts the periodic block-count refresh timer (no-op if already running).
    fn start_refresh_timer(&mut self, ctx: &mut ViewContext<Self>) {
        if self.refresh_timer_handle.is_some() {
            return;
        }
        let handle = ctx.spawn(
            async move {
                Timer::after(std::time::Duration::from_millis(
                    BLOCK_COUNT_REFRESH_INTERVAL_MS,
                ))
                .await;
            },
            |me, _unit, ctx| {
                me.refresh_timer_handle = None;
                // flag 中途被关闭时停止续期(下次 render 若仍显示会重启)。
                if !crate::features::FeatureFlag::AgentHarness.is_enabled() {
                    return;
                }
                InterceptSessionsModel::handle(ctx).update(ctx, |model, ctx| {
                    model.refresh_block_count(ctx);
                });
                me.start_refresh_timer(ctx);
            },
        );
        self.refresh_timer_handle = Some(handle);
    }

    /// True when the upstream configuration panel is open.
    pub fn is_panel_open(&self) -> bool {
        self.panel_open
    }

    fn set_panel_open(&mut self, open: bool, ctx: &mut ViewContext<Self>) {
        if self.panel_open == open {
            return;
        }
        self.panel_open = open;
        if open {
            // Prefill both inputs with the current explicit overrides.
            let model = InterceptSessionsModel::handle(ctx);
            let base = model.as_ref(ctx).upstream_base().to_string();
            let auth_env = model.as_ref(ctx).upstream_auth_env().to_string();
            self.api_base_input.update(ctx, |input, ctx| {
                let editor = input.editor().clone();
                editor.update(ctx, |ed, ctx| ed.set_buffer_text(&base, ctx));
            });
            self.auth_env_input.update(ctx, |input, ctx| {
                let editor = input.editor().clone();
                editor.update(ctx, |ed, ctx| ed.set_buffer_text(&auth_env, ctx));
            });
        }
        ctx.emit(InterceptConfigBarEvent::PanelVisibilityChanged { open });
        ctx.notify();
    }

    /// Re-sync both upstream input editors with the committed model values.
    /// Called after a submit clears the buffer so the (possibly empty) field
    /// shows the effective override again.
    fn refresh_upstream_input_prefills(&self, ctx: &mut ViewContext<Self>) {
        let model = InterceptSessionsModel::handle(ctx);
        let base = model.as_ref(ctx).upstream_base().to_string();
        let auth_env = model.as_ref(ctx).upstream_auth_env().to_string();
        self.api_base_input.update(ctx, |input, ctx| {
            let editor = input.editor().clone();
            editor.update(ctx, |ed, ctx| ed.set_buffer_text(&base, ctx));
        });
        self.auth_env_input.update(ctx, |input, ctx| {
            let editor = input.editor().clone();
            editor.update(ctx, |ed, ctx| ed.set_buffer_text(&auth_env, ctx));
        });
    }


    fn render_counter(&self, app: &AppContext) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        let count = InterceptSessionsModel::as_ref(app).block_count();
        let label = crate::t!("intercept-blocks-captured", count = count);
        warpui::elements::Text::new(
            label,
            appearance.ui_font_family(),
            appearance.ui_font_size(),
        )
        .with_color(appearance.theme().nonactive_ui_text_color().into_solid())
        .finish()
    }

    fn render_upstream_panel(&self, app: &AppContext) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        let theme = appearance.theme();

        let mut column = Flex::column().with_spacing(BAR_SPACING);
        column.add_child(ChildView::new(&self.api_base_input).finish());
        column.add_child(ChildView::new(&self.auth_env_input).finish());

        // Probe result: the effective upstream after three-tier resolution.
        let harness_type = self.upstream_harness_type(app);
        let probe = InterceptSessionsModel::as_ref(app).resolve_upstream(harness_type);
        let probe_text = match &probe {
            Some(config) => format!(
                "{} {} · {} {}",
                crate::t!("intercept-probe-prefix"),
                config.api_base,
                config.auth_header,
                config.api_key_env,
            ),
            None => crate::t!("intercept-probe-unavailable"),
        };
        column.add_child(
            warpui::elements::Text::new(
                probe_text,
                appearance.ui_font_family(),
                appearance.ui_font_size(),
            )
            .with_color(theme.disabled_text_color(theme.background()).into_solid())
            .finish(),
        );

        Container::new(column.finish())
            .with_border(Border::all(1.).with_border_fill(theme.outline()))
            .with_horizontal_padding(BAR_SPACING)
            .with_vertical_padding(BAR_SPACING)
            .finish()
    }

    /// Map the selected ambient-agent harness to a proxy upstream harness type.
    fn upstream_harness_type(&self, app: &AppContext) -> HarnessType {
        match self.ambient_agent_model.as_ref(app).selected_harness() {
            Harness::Claude => HarnessType::ClaudeCode,
            Harness::Oz | Harness::OpenCode | Harness::Gemini | Harness::Unknown => {
                HarnessType::Generic
            }
        }
    }
}

impl Entity for InterceptConfigBar {
    type Event = InterceptConfigBarEvent;
}

impl TypedActionView for InterceptConfigBar {
    type Action = InterceptConfigBarAction;

    fn handle_action(&mut self, action: &Self::Action, ctx: &mut ViewContext<Self>) {
        match action {
            InterceptConfigBarAction::TogglePanel => {
                let new_state = !self.panel_open;
                self.set_panel_open(new_state, ctx);
            }
        }
    }
}

impl View for InterceptConfigBar {
    fn ui_name() -> &'static str {
        "InterceptConfigBar"
    }

    fn render(&self, app: &AppContext) -> Box<dyn Element> {
        let mut column = Flex::column().with_spacing(BAR_SPACING);

        let mut row = Flex::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_main_axis_alignment(MainAxisAlignment::Start)
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(BAR_SPACING);
        row.add_child(ChildView::new(&self.mode_selector).finish());
        row.add_child(self.render_counter(app));
        row.add_child(Expanded::new(1., Empty::new().finish()).finish());
        row.add_child(ChildView::new(&self.upstream_button).finish());
        column.add_child(row.finish());

        if self.panel_open {
            column.add_child(self.render_upstream_panel(app));
        }

        column.finish()
    }
}
