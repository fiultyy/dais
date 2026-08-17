//! Cockpit 面板视图 — CockpitPanelView。
//!
//! 视图不持有业务状态,只渲染 `CockpitModel` 快照 + 派发意图
//! (observatory 同款)。P0 交互两个 Action:
//! - `Refresh`(刷新按钮 / 1s timer 兜底)→ model.refresh
//! - `SelectCard`(卡片点击)→ model.select_card
//! 六环接线(Action→dispatch→handler→状态→notify→rerender)逐环可验,
//! 见 dais-cockpit-spec.md §4.1。

use std::cell::RefCell;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use warpui::elements::{
    ConstrainedBox, Container, CornerRadius, CrossAxisAlignment, Empty, Expanded,
    Fill as ElementFill, Flex, Hoverable, MainAxisAlignment,
    MouseStateHandle, ParentElement, ScrollbarWidth, Text, Wrap,
};
use warpui::elements::{ClippedScrollStateHandle, ClippedScrollable};
use warpui::r#async::SpawnedFutureHandle;
use warpui::r#async::Timer;
use warpui::scene::Radius;
use warpui::{
    AppContext, Element, Entity, EntityId, ModelHandle, SingletonEntity, TypedActionView,
    View, ViewContext, ViewHandle,
};

use warp_core::ui::appearance::Appearance;
use warp_core::ui::theme::WarpTheme;

use super::model::{CockpitCard, CockpitCardStatus, CockpitModel};
use crate::ai::blocklist::agent_view::agent_input_footer::AgentInputButtonTheme;
use crate::ai::observatory::row::status_dot_element;
use crate::view_components::action_button::{ActionButton, ButtonSize};

// ── 布局常量 ──────────────────────────────────────────────────────────────────

/// 面板内边距。
const PANEL_PADDING: f32 = 12.;
/// 元素间距。
const SPACING: f32 = 6.;
/// 卡片宽度(固定;Wrap 自动折行成网格)。
const CARD_WIDTH: f32 = 300.;
/// 卡片内边距。
const CARD_PADDING: f32 = 10.;
/// 卡片圆角半径。
const CARD_RADIUS: f32 = 8.;
/// 卡片间水平/垂直间距。
const CARD_GAP: f32 = 10.;
/// 主文本字号。
const FONT_SIZE: f32 = 13.;
/// 辅助文本字号。
const SMALL_FONT_SIZE: f32 = 11.;
/// recap/路径截断上限(字符)。
const RECAP_MAX_CHARS: usize = 64;
const PATH_MAX_CHARS: usize = 42;
/// 周期自动刷新间隔(ms)。P0 全量快照;P1 换 CLIAgentSessionsModel 事件化。
const COCKPIT_REFRESH_INTERVAL_MS: u64 = 1_000;

// ── Action ────────────────────────────────────────────────────────────────────

/// 面板 typed action:on_click 分发、`on_action` 处理(warpui 六环接线)。
#[derive(Clone, Debug, PartialEq)]
pub enum CockpitPanelAction {
    Refresh,
    /// 选中/取消选中卡片(None = 清空选中)。
    SelectCard(Option<EntityId>),
}

// ── CockpitPanelView ────────────────────────────────────────────────────────

/// 驾驶舱面板视图。只渲染,不持有业务状态(渲染缓存:卡片悬停句柄)。
pub struct CockpitPanelView {
    model: ModelHandle<CockpitModel>,
    refresh_button: ViewHandle<ActionButton>,
    /// 周期刷新 timer 句柄,Drop 时中止。
    refresh_timer_handle: Option<SpawnedFutureHandle>,
    /// 卡片悬停句柄(渲染缓存,数量对齐卡片数)。
    card_handles: RefCell<Vec<MouseStateHandle>>,
}

impl CockpitPanelView {
    pub fn new(model: ModelHandle<CockpitModel>, ctx: &mut ViewContext<Self>) -> Self {
        // 刷新按钮(六环 #2:元素回调 dispatch Action)
        let refresh_button = ctx.add_typed_action_view(|_ctx| {
            ActionButton::new(crate::t!("cockpit-refresh"), AgentInputButtonTheme)
                .with_size(ButtonSize::AgentInputButton)
                .on_click(|ctx| {
                    ctx.dispatch_typed_action(CockpitPanelAction::Refresh);
                })
        });

        // 订阅 model 事件(六环 #5→#6:状态更新 → notify → rerender)
        ctx.subscribe_to_model(&model, |_me, _handle, _event, ctx| {
            ctx.notify();
        });

        let mut me = Self {
            model,
            refresh_button,
            refresh_timer_handle: None,
            card_handles: RefCell::new(Vec::new()),
        };
        // 首次启动 timer(warpui 陷阱#1:render 是 &self 无法启动,
        // start_refresh_timer 只在回调内自续期,new() 末尾必须首调)。
        me.start_refresh_timer(ctx);
        me
    }

    /// 启动周期刷新 timer(已在跑则 no-op;随视图 Drop 中止)。
    fn start_refresh_timer(&mut self, ctx: &mut ViewContext<Self>) {
        if self.refresh_timer_handle.is_some() {
            return;
        }
        let handle = ctx.spawn(
            async move {
                Timer::after(std::time::Duration::from_millis(
                    COCKPIT_REFRESH_INTERVAL_MS,
                ))
                .await;
            },
            |me, _unit, ctx| {
                me.refresh_timer_handle = None;
                // 面板关闭时跳过刷新(timer 空转一个 wake,开销可忽略)
                if !CockpitModel::handle(ctx).as_ref(ctx).panel_open() {
                    me.start_refresh_timer(ctx);
                    return;
                }
                CockpitModel::handle(ctx).update(ctx, |model, ctx| {
                    model.refresh(ctx);
                });
                me.start_refresh_timer(ctx);
            },
        );
        self.refresh_timer_handle = Some(handle);
    }

    /// 确保卡片悬停句柄数量对齐。
    fn ensure_handles(handles: &mut Vec<MouseStateHandle>, target_len: usize) {
        while handles.len() < target_len {
            handles.push(MouseStateHandle::default());
        }
        handles.truncate(target_len);
    }

    /// 头部:标题 + 终端计数 + 刷新按钮。
    fn render_header(&self, app: &AppContext) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        let theme = appearance.theme();
        let model = self.model.as_ref(app);

        let title_text = crate::t!("cockpit-title");
        let count_text = crate::t!(
            "cockpit-terminal-count",
            count = model.cards().len(),
            windows = model.last_window_count()
        );

        let mut row = Flex::row()
            .with_main_axis_size(warpui::elements::MainAxisSize::Max)
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(SPACING);

        row.add_child(
            Text::new(
                title_text,
                appearance.ui_font_family(),
                appearance.ui_font_size(),
            )
            .with_color(theme.active_ui_text_color().into())
            .finish(),
        );
        row.add_child(
            Text::new(
                count_text,
                appearance.ui_font_family(),
                appearance.ui_font_size(),
            )
            .with_color(theme.nonactive_ui_text_color().into_solid())
            .finish(),
        );
        row.add_child(Expanded::new(1., Empty::new().finish()).finish());
        row.add_child(warpui::elements::ChildView::new(&self.refresh_button).finish());

        Container::new(row.finish())
            .with_horizontal_padding(PANEL_PADDING)
            .with_vertical_padding(SPACING)
            .finish()
    }

    /// 空态占位。
    fn render_empty_state(&self, appearance: &Appearance, theme: &WarpTheme) -> Box<dyn Element> {
        Container::new(
            Text::new(
                crate::t!("cockpit-empty"),
                appearance.ui_font_family(),
                appearance.ui_font_size(),
            )
            .with_color(theme.disabled_ui_text_color().into_solid())
            .finish(),
        )
        .with_horizontal_padding(PANEL_PADDING)
        .with_vertical_padding(PANEL_PADDING)
        .finish()
    }

    /// 单张卡片:状态点 + 标题 / agent 行 / recap 行 / tool 行 / cwd+状态行。
    /// 点击 → `SelectCard`(选中卡 accent 边框)。
    fn render_card(
        &self,
        card: &CockpitCard,
        handle: MouseStateHandle,
        selected: bool,
        appearance: &Appearance,
        theme: &WarpTheme,
    ) -> Box<dyn Element> {
        let font_family = appearance.ui_font_family();
        let tag = card_tag(card.terminal_view_id);
        let title = truncate_str(&card.title, 30);
        let agent_line = match card.agent_name {
            Some(name) => name.to_string(),
            None => "Shell".to_string(),
        };
        let recap = card
            .recap
            .as_deref()
            .map(|r| truncate_str(r, RECAP_MAX_CHARS))
            .unwrap_or_else(|| "—".to_string());
        let tool_line = card
            .tool_name
            .as_deref()
            .map(|t| format!("tool: {}", truncate_str(t, 24)));
        let cwd_line = card
            .cwd
            .as_deref()
            .map(|c| truncate_str(c, PATH_MAX_CHARS))
            .unwrap_or_else(|| "~".to_string());
        let status_label = match &card.status {
            CockpitCardStatus::Blocked(Some(message)) => {
                format!("{} · {}", card.status.label(), truncate_str(message, 24))
            }
            _ => card.status.label().to_string(),
        };
        let dot_key = card.status.dot_key().map(str::to_owned);

        let card_id = card.terminal_view_id;
        let border_color = if selected {
            theme.accent()
        } else {
            theme.nonactive_ui_detail().into()
        };

        let content = move |hovered: bool| {
            let mut col = Flex::column()
                .with_main_axis_alignment(MainAxisAlignment::Start)
                .with_cross_axis_alignment(CrossAxisAlignment::Start)
                .with_spacing(4.);

            // row0: 状态点 + 标题 + 右侧 tag
            let mut row0 = Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(SPACING);
            if let Some(key) = &dot_key {
                row0.add_child(status_dot_element(key, theme));
            }
            row0.add_child(
                Text::new(title.clone(), font_family, FONT_SIZE)
                    .with_color(theme.main_text_color(theme.background()).into())
                    .soft_wrap(false)
                    .finish(),
            );
            row0.add_child(Expanded::new(1., Empty::new().finish()).finish());
            row0.add_child(
                Text::new(tag.clone(), font_family, SMALL_FONT_SIZE)
                    .with_color(theme.disabled_ui_text_color().into_solid())
                    .soft_wrap(false)
                    .finish(),
            );
            col.add_child(row0.finish());

            // row1: agent 名(或 Shell)
            col.add_child(
                Text::new(agent_line.clone(), font_family, SMALL_FONT_SIZE)
                    .with_color(theme.sub_text_color(theme.background()).into())
                    .soft_wrap(false)
                    .finish(),
            );

            // row2: recap(query > summary 回退链)
            col.add_child(
                Text::new(recap.clone(), font_family, FONT_SIZE)
                    .with_color(theme.main_text_color(theme.background()).into())
                    .soft_wrap(false)
                    .finish(),
            );

            // row3: tool(可无)
            if let Some(tool) = &tool_line {
                col.add_child(
                    Text::new(tool.clone(), font_family, SMALL_FONT_SIZE)
                        .with_color(theme.sub_text_color(theme.background()).into())
                        .soft_wrap(false)
                        .finish(),
                );
            }

            // row4: cwd + 状态标签
            let mut row4 = Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(SPACING);
            row4.add_child(
                Text::new(cwd_line.clone(), font_family, SMALL_FONT_SIZE)
                    .with_color(theme.sub_text_color(theme.background()).into())
                    .soft_wrap(false)
                    .finish(),
            );
            row4.add_child(Expanded::new(1., Empty::new().finish()).finish());
            row4.add_child(
                Text::new(status_label.clone(), font_family, SMALL_FONT_SIZE)
                    .with_color(theme.nonactive_ui_text_color().into_solid())
                    .soft_wrap(false)
                    .finish(),
            );
            col.add_child(row4.finish());
            let mut container = Container::new(col.finish())
                .with_horizontal_padding(CARD_PADDING)
                .with_vertical_padding(CARD_PADDING)
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(CARD_RADIUS)))
                .with_border(
                    warpui::elements::Border::all(1.).with_border_fill(border_color),
                );
            if hovered || selected {
                container = container.with_background(theme.surface_overlay_1());
            }
            container.finish()
        };
        let hoverable = Hoverable::new(handle, move |state| content(state.is_hovered())).on_click(
            move |ctx, _app, _position| {
                ctx.dispatch_typed_action(CockpitPanelAction::SelectCard(Some(card_id)));
            },
        );

        ConstrainedBox::new(hoverable.finish())
            .with_width(CARD_WIDTH)
            .finish()
    }

    /// 卡片网格:Wrap 自动折行;外层纵向裁剪滚动。
    fn render_card_grid(&self, app: &AppContext) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        let theme = appearance.theme();
        let model = self.model.as_ref(app);
        let cards = model.cards();
        let selected = model.selected();

        Self::ensure_handles(&mut self.card_handles.borrow_mut(), cards.len());
        let handles = self.card_handles.borrow().clone();

        let mut grid = Wrap::row()
            .with_spacing(CARD_GAP)
            .with_run_spacing(CARD_GAP)
            .with_cross_axis_alignment(CrossAxisAlignment::Start);
        for (card, handle) in cards.iter().zip(handles) {
            grid.add_child(self.render_card(
                card,
                handle,
                selected == Some(card.terminal_view_id),
                appearance,
                theme,
            ));
        }

        let scroll_state = ClippedScrollStateHandle::new();
        ClippedScrollable::vertical(
            scroll_state,
            Container::new(grid.finish())
                .with_horizontal_padding(PANEL_PADDING)
                .with_vertical_padding(SPACING)
                .finish(),
            ScrollbarWidth::Auto,
            ElementFill::Solid(theme.nonactive_ui_detail().into()),
            ElementFill::Solid(theme.active_ui_detail().into()),
            ElementFill::None,
        )
        .finish()
    }

}

impl Entity for CockpitPanelView {
    /// pane 体系关闭通道(header X 按钮 → PaneEvent::Close)。
    type Event = crate::pane_group::PaneEvent;
}

impl TypedActionView for CockpitPanelView {
    type Action = CockpitPanelAction;

    /// typed action 处理(六环 #3→#4:handler 注册 + 状态更新)。
    /// dispatch 源:刷新按钮 on_click / 卡片 on_click。
    fn handle_action(&mut self, action: &Self::Action, ctx: &mut ViewContext<Self>) {
        match action {
            CockpitPanelAction::Refresh => {
                self.model.update(ctx, |model, ctx| {
                    model.refresh(ctx);
                });
            }
            CockpitPanelAction::SelectCard(id) => {
                // 点击已选中卡 → 取消选中(toggle)。
                let id = match id {
                    Some(id) if self.model.as_ref(ctx).selected() == Some(*id) => None,
                    other => *other,
                };
                self.model.update(ctx, |model, ctx| {
                    model.select_card(id, ctx);
                });
            }
        }
    }
}

impl View for CockpitPanelView {
    fn ui_name() -> &'static str {
        "CockpitPanelView"
    }

    fn render(&self, app: &AppContext) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        let theme = appearance.theme();
        let has_cards = !self.model.as_ref(app).cards().is_empty();

        let mut col = Flex::column()
            .with_main_axis_alignment(MainAxisAlignment::Start)
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_spacing(SPACING);

        col.add_child(self.render_header(app));
        if has_cards {
            col.add_child(Box::new(warpui::elements::Expanded::new(
                1.,
                self.render_card_grid(app),
            )));
        }
        Container::new(col.finish())
            .with_background(theme.background())
            .finish()
    }
}

/// 卡片 tag:EntityId 稳定哈希低 16 bit 的 4-hex(hub-tui 8-hex tag 的等价物,
/// 身份辨识用,不暴露内部计数器)。
fn card_tag(id: EntityId) -> String {
    let mut hasher = DefaultHasher::new();
    id.hash(&mut hasher);
    format!("#{:04x}", (hasher.finish() & 0xffff) as u16)
}

/// 字符串截断,超过 max_len 字符加 "…"(observatory row.rs 同款策略)。
fn truncate_str(s: &str, max_len: usize) -> String {
    if s.chars().count() <= max_len {
        s.to_string()
    } else {
        let mut truncated: String = s.chars().take(max_len.saturating_sub(1)).collect();
        truncated.push('…');
        truncated
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_respects_char_boundaries() {
        assert_eq!(truncate_str("abc", 5), "abc");
        assert_eq!(truncate_str("abcdef", 5), "abcd…");
        // CJK 按字符计,不切字节
        assert_eq!(truncate_str("中文测试文本", 3), "中文…");
    }

    #[test]
    fn card_tag_is_stable_and_short() {
        let id = EntityId::from_usize(42);
        let tag = card_tag(id);
        assert_eq!(tag.len(), 5, "tag = # + 4 hex");
        assert_eq!(tag, card_tag(id));
        assert_ne!(tag, card_tag(EntityId::from_usize(43)));
    }
}
