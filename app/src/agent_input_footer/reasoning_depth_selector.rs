//! 输入框右下角"思考深度"picker。
//!
//! 与 `EnvironmentSelector` 同模板。外部 provider 接入剥离后已无 reasoning
//! 档位数据源:目标解析恒为 `None`,组件以空菜单呈现,按钮禁用、不渲染。

use pathfinder_color::ColorU;
use pathfinder_geometry::vector::vec2f;
use warp_core::ui::color::blend::Blend;
use warp_core::ui::theme::Fill;
use warpui::{
    elements::{
        ChildAnchor, ChildView, OffsetPositioning, ParentAnchor, ParentElement, ParentOffsetBounds,
        Stack,
    },
    AppContext, Element, Entity, EntityId, TypedActionView, View, ViewContext, ViewHandle,
};

use std::sync::Arc;

use crate::{
    appearance::Appearance,
    context_chips::display_menu::{
        ChipMenuType, DisplayChipMenu, GenericMenuItem, PromptDisplayMenuEvent,
    },
    terminal::input::{MenuPositioning, MenuPositioningProvider},
    ui_components::icons::Icon,
    view_components::action_button::{ActionButton, ActionButtonTheme, ButtonSize},
};

use super::AgentInputButtonTheme;

/// 输入框 toolbar 的"Reasoning Depth"选择器。
pub struct ReasoningDepthSelector {
    button: ViewHandle<ActionButton>,
    dropdown: ViewHandle<DisplayChipMenu>,
    is_menu_open: bool,
    menu_positioning_provider: Arc<dyn MenuPositioningProvider>,
    terminal_view_id: EntityId,
    /// 当前 picker 关联的 model_id;外部 provider 接入剥离后恒为 `None`。
    current_target: Option<String>,
}

pub enum ReasoningDepthSelectorEvent {
    MenuVisibilityChanged { open: bool },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReasoningDepthSelectorAction {
    ToggleMenu,
}

/// 菜单条目占位类型;菜单恒为空,仅为满足 `DisplayChipMenu` 的条目类型约束而保留。
#[derive(Debug)]
struct ReasoningDepthMenuItem;

impl GenericMenuItem for ReasoningDepthMenuItem {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn name(&self) -> String {
        String::new()
    }

    fn icon(&self, _app: &AppContext) -> Option<Icon> {
        None
    }

    fn action_data(&self) -> String {
        String::new()
    }
}

impl ReasoningDepthSelector {
    pub fn new(
        menu_positioning_provider: Arc<dyn MenuPositioningProvider>,
        terminal_view_id: EntityId,
        ctx: &mut ViewContext<Self>,
    ) -> Self {
        let button = ctx.add_typed_action_view(|_ctx| {
            ActionButton::new("", AgentInputButtonTheme)
                .with_icon(Icon::Stars)
                .with_tooltip(crate::t!("ai-footer-reasoning-depth-tooltip"))
                .with_size(ButtonSize::AgentInputButton)
                .with_disabled_theme(DisabledTheme)
                .on_click(|ctx| {
                    ctx.dispatch_typed_action(ReasoningDepthSelectorAction::ToggleMenu);
                })
        });

        let dropdown = ctx.add_typed_action_view(move |ctx| {
            // 用 CodeReview 风格(无 search input、无 environment sidecar、视觉紧凑)。
            // 不能用 Environments — 它的 sidecar 路径会把 action_data() 喂给
            // `ServerId::from_string_lossy`,任意非 22 字符字符串都会在 debug
            // build 触发 panic。
            DisplayChipMenu::new(
                Vec::<ReasoningDepthMenuItem>::new(),
                None,
                ChipMenuType::CodeReview,
                ctx,
            )
        });

        ctx.subscribe_to_view(&dropdown, |me, _, event, ctx| match event {
            PromptDisplayMenuEvent::CloseMenu => {
                me.set_menu_visibility(false, ctx);
            }
            PromptDisplayMenuEvent::MenuAction(_) => {}
        });

        let mut me = Self {
            button,
            dropdown,
            is_menu_open: false,
            menu_positioning_provider,
            terminal_view_id,
            current_target: None,
        };
        me.refresh(ctx);
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
            ctx.focus(&self.dropdown);
        }
        ctx.emit(ReasoningDepthSelectorEvent::MenuVisibilityChanged { open: is_open });
        ctx.notify();
    }

    /// 解析当前选中模型对应的目标;外部 provider 接入剥离后已无 provider,恒返回 `None`。
    fn resolve_current_target(&self) -> Option<String> {
        None
    }

    fn refresh(&mut self, ctx: &mut ViewContext<Self>) {
        self.current_target = self.resolve_current_target();

        // 目标恒为空 → 菜单无条目,按钮保持禁用,后续 render 走空分支。
        self.button.update(ctx, |button, ctx| {
            button.set_disabled(true, ctx);
        });

        ctx.notify();
    }

    /// 当前是否应渲染;目标恒为空 → 恒不渲染。
    fn is_visible(&self) -> bool {
        self.current_target.is_some()
    }

    fn get_menu_positioning(&self, app: &AppContext) -> OffsetPositioning {
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

impl TypedActionView for ReasoningDepthSelector {
    type Action = ReasoningDepthSelectorAction;

    fn handle_action(&mut self, action: &Self::Action, ctx: &mut ViewContext<Self>) {
        match action {
            ReasoningDepthSelectorAction::ToggleMenu => {
                if self.is_visible() {
                    self.set_menu_visibility(!self.is_menu_open, ctx);
                }
            }
        }
    }
}

impl View for ReasoningDepthSelector {
    fn ui_name() -> &'static str {
        "ReasoningDepthSelector"
    }

    fn render(&self, app: &AppContext) -> Box<dyn Element> {
        if !self.is_visible() {
            // 不支持 reasoning 的模型 → 渲染空
            return Stack::new().finish();
        }

        let mut stack = Stack::new();
        stack.add_child(ChildView::new(&self.button).finish());

        if self.is_menu_open {
            let menu = ChildView::new(&self.dropdown).finish();
            let positioning = self.get_menu_positioning(app);
            stack.add_positioned_overlay_child(menu, positioning);
        }

        stack.finish()
    }
}

impl Entity for ReasoningDepthSelector {
    type Event = ReasoningDepthSelectorEvent;
}

struct DisabledTheme;

impl ActionButtonTheme for DisabledTheme {
    fn background(&self, hovered: bool, appearance: &Appearance) -> Option<Fill> {
        AgentInputButtonTheme.background(hovered, appearance)
    }

    fn text_color(
        &self,
        _hovered: bool,
        background: Option<Fill>,
        appearance: &Appearance,
    ) -> ColorU {
        let base_bg = appearance.theme().surface_1();
        let effective_bg = match background {
            Some(overlay) => base_bg.blend(&overlay),
            None => base_bg,
        };
        appearance
            .theme()
            .disabled_text_color(effective_bg)
            .into_solid()
    }

    fn border(&self, appearance: &Appearance) -> Option<ColorU> {
        AgentInputButtonTheme.border(appearance)
    }

    fn should_opt_out_of_contrast_adjustment(&self) -> bool {
        AgentInputButtonTheme.should_opt_out_of_contrast_adjustment()
    }
}
