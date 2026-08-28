//! 左栏 cockpit 导航视图 — CockpitNavView。
//!
//! 纯导航视图(v1):渲染 `CockpitModel` 单例快照的项目组卡片 + 实例卡片,
//! 带单选选中态与组折叠;不含 composer/注入/批量选择(完整面板见
//! `crate::ai::cockpit::view::CockpitPanelView`)。
//!
//! 交互(六环接线,照抄 cockpit view 先例):
//! - 点击实例卡 → `CockpitNavAction::ActivateCard` → model `select_card`
//!   + emit `CockpitNavEvent::CardActivated`(主线程订阅该事件后接
//!   activate_tab 完成"右侧内容跟随切换";EntityId→tab index 映射需要
//!   遍历 workspace tabs,归主线程);
//! - 点击组头 → `CockpitNavAction::ToggleGroupCollapsed`(折叠集存 view 内)。
//!
//! 数据刷新:视图订阅 `CockpitEvent::SnapshotUpdated` → notify → rerender;
//! 新鲜度自驱——挂载时首刷 + 低频对账 timer(左栏常驻,不依赖 cockpit
//! panel_open;cockpit 面板自身的 timer 在面板关闭时空转,见其 view.rs)。
//!
//! 布局参照 `vertical_tabs` 紧凑导航形态,选中态样式照抄
//! `cockpit/view.rs render_card` 的 focused_selected 分支
//! (accent 边框 + `surface_overlay_1` 背景)。

use std::cell::RefCell;
use std::collections::HashSet;

use warp_core::ui::appearance::Appearance;
use warp_core::ui::theme::WarpTheme;
use warpui::elements::{
    ClippedScrollStateHandle, ClippedScrollable, Container, CornerRadius, CrossAxisAlignment,
    Empty, Expanded, Fill as ElementFill, Flex, Hoverable, MainAxisAlignment, MainAxisSize,
    MouseStateHandle, ParentElement, ScrollbarWidth, Text,
};
use warpui::platform::Cursor;
use warpui::scene::Radius;
use warpui::{
    AppContext, Element, Entity, EntityId, ModelHandle, SingletonEntity, TypedActionView, View,
    ViewContext, ViewHandle,
};

use crate::ai::cockpit::model::{CockpitCard, CockpitCardGroup, CockpitModel};
use crate::ai::observatory::row::status_dot_element;

// ── 布局常量 ──────────────────────────────────────────────────────────────────

/// 头部/空态水平内边距。
const HEADER_PADDING: f32 = 10.;
/// 元素间距。
const SPACING: f32 = 6.;
/// 卡片行间垂直间距(导航紧凑形态)。
const CARD_GAP: f32 = 2.;
/// 左栏对账刷新间隔(与 cockpit 面板 COCKPIT_RECONCILE_INTERVAL_MS 一致)。
const COCKPIT_NAV_RECONCILE_INTERVAL_MS: u64 = 2_000;
/// 行水平内边距。
const CARD_PADDING: f32 = 8.;
/// 行垂直内边距。
const CARD_ROW_VERTICAL_PADDING: f32 = 4.;
/// 行圆角半径。
const CARD_RADIUS: f32 = 6.;
/// 主文本字号。
const FONT_SIZE: f32 = 13.;
/// 辅助文本字号。
const SMALL_FONT_SIZE: f32 = 11.;
/// 卡片标题截断上限(字符)。
const TITLE_MAX_CHARS: usize = 28;

/// 顶部标题。
// TODO(i18n): 文案硬编码中文,主线程统一补 ftl key 后替换。
const NAV_TITLE: &str = "导航";

// ── Action ────────────────────────────────────────────────────────────────────

/// 导航视图 typed action:on_click 分发、`on_action` 处理(六环接线)。
#[derive(Clone, Debug, PartialEq)]
pub enum CockpitNavAction {
    /// 点击实例卡:单选高亮 + 请求右侧切换到该实例所在 tab。
    ActivateCard(EntityId),
    /// 点击组头:折叠/展开该组(键 = 分组 key)。
    ToggleGroupCollapsed(String),
}

// ── Event(主线程接线契约)─────────────────────────────────────────────────────

/// 导航视图事件。主线程订阅 `CockpitNavEvent` 完成右侧跟随切换。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CockpitNavEvent {
    /// 用户激活了某张实例卡。主线程应把右侧内容切换到该终端所在 tab
    /// (EntityId→tab index 映射需要遍历 workspace tabs,归主线程)。
    CardActivated { terminal_view_id: EntityId },
}

// ── CockpitNavView ────────────────────────────────────────────────────────────

/// 左栏 cockpit 导航视图。只渲染,不持有业务状态(本地状态:组折叠集;
/// 渲染缓存:行/组头悬停句柄、滚动句柄)。
pub struct CockpitNavView {
    model: ModelHandle<CockpitModel>,
    /// 折叠的项目组 key 集(v1 本地状态,pane 重开即复位;不持久化)。
    collapsed_groups: HashSet<String>,
    /// 卡片行悬停句柄(渲染缓存,数量对齐卡片数)。
    card_handles: RefCell<Vec<MouseStateHandle>>,
    /// 组头悬停句柄(渲染缓存,数量对齐分组数)。
    group_handles: RefCell<Vec<MouseStateHandle>>,
    /// 列表滚动句柄。必须是 view 字段:滚动偏移存在句柄内部,若在 render
    /// 里每次重建句柄,高频重渲染会把列表滚回顶部(照抄 cockpit 同构做法)。
    list_scroll: ClippedScrollStateHandle,
}

impl CockpitNavView {
    /// 主线程挂载入口:在任意 parent view 的 ViewContext 里创建本视图
    /// (注册 typed action handler 并返回句柄,供 `subscribe_to_view`
    /// 订阅 `CockpitNavEvent`)。
    pub fn init<A: View>(ctx: &mut ViewContext<A>) -> ViewHandle<Self> {
        let model = CockpitModel::handle(ctx);
        ctx.add_typed_action_view(|ctx| Self::new(model, ctx))
    }

    /// 构造(照抄 `CockpitPanelView::new` 形态):订阅 model 事件驱动
    /// rerender;另自驱快照新鲜度(挂载首刷 + 低频对账 timer)。

    pub fn new(model: ModelHandle<CockpitModel>, ctx: &mut ViewContext<Self>) -> Self {
        // 订阅 model 事件(六环 #5→#6:选中态/快照更新 → notify → rerender)。
        ctx.subscribe_to_model(&model, |_me, _handle, _event, ctx| {
            ctx.notify();
        });

        model.update(ctx, |m, ctx| m.refresh(ctx));
        let mut me = Self {
            model,
            collapsed_groups: HashSet::new(),
            card_handles: RefCell::new(Vec::new()),
            group_handles: RefCell::new(Vec::new()),
            list_scroll: ClippedScrollStateHandle::default(),
        };
        me.start_reconcile_timer(ctx);
        me
    }

    /// 低频对账 timer(照抄 cockpit view `start_reconcile_timer`;**无
    /// panel_open 短路**——左栏导航常驻,关着 cockpit 面板也要刷新)。
    fn start_reconcile_timer(&mut self, ctx: &mut ViewContext<Self>) {
        ctx.spawn(
            async move {
                warpui::r#async::Timer::after(std::time::Duration::from_millis(
                    COCKPIT_NAV_RECONCILE_INTERVAL_MS,
                ))
                .await;
            },
            |me, _unit, ctx| {
                me.model.update(ctx, |m, ctx| m.refresh(ctx));
                me.start_reconcile_timer(ctx);
            },
        );
    }
    /// 确保悬停句柄数量对齐(照抄 cockpit `ensure_handles`)。
    fn ensure_handles(handles: &mut Vec<MouseStateHandle>, target_len: usize) {
        while handles.len() < target_len {
            handles.push(MouseStateHandle::default());
        }
        handles.truncate(target_len);
    }
    /// 头部:紧凑标题行(标题 + 终端计数),无搜索框(v1)。
    fn render_header(&self, app: &AppContext) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        let theme = appearance.theme();
        let model = self.model.as_ref(app);

        let count_text = crate::t!(
            "cockpit-terminal-count",
            count = model.cards().len(),
            windows = model.last_window_count()
        );

        let mut row = Flex::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(SPACING);
        row.add_child(
            Text::new(
                NAV_TITLE,
                appearance.ui_font_family(),
                appearance.ui_font_size(),
            )
            .with_color(theme.active_ui_text_color().into())
            .finish(),
        );
        row.add_child(Expanded::new(1., Empty::new().finish()).finish());
        row.add_child(
            Text::new(count_text, appearance.ui_font_family(), SMALL_FONT_SIZE)
                .with_color(theme.nonactive_ui_text_color().into_solid())
                .soft_wrap(false)
                .finish(),
        );

        Container::new(row.finish())
            .with_horizontal_padding(HEADER_PADDING)
            .with_vertical_padding(SPACING)
            .finish()
    }

    /// 组列表(纵向裁剪滚动):每组 = 组头(可折叠)+ 组内实例卡行。
    fn render_group_list(&self, app: &AppContext) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        let theme = appearance.theme();
        let model = self.model.as_ref(app);
        let cards = model.cards();
        let selected = model.selected();
        let groups = model.groups();

        Self::ensure_handles(&mut self.group_handles.borrow_mut(), groups.len());
        Self::ensure_handles(&mut self.card_handles.borrow_mut(), cards.len());
        let group_handles = self.group_handles.borrow().clone();
        let card_handles = self.card_handles.borrow().clone();

        let mut col = Flex::column()
            .with_main_axis_alignment(MainAxisAlignment::Start)
            .with_cross_axis_alignment(CrossAxisAlignment::Start)
            .with_spacing(SPACING);

        for (gi, group) in groups.iter().enumerate() {
            let Some(group_handle) = group_handles.get(gi) else {
                continue;
            };
            // 组选中态:组内含当前单选实例(照抄 brief 要求 #3)。
            let group_selected = group
                .range
                .clone()
                .any(|idx| cards.get(idx).map(|c| c.terminal_view_id) == selected);
            col.add_child(self.render_group_header(
                group,
                group_handle.clone(),
                group_selected,
                appearance,
                theme,
            ));

            if !self.collapsed_groups.contains(&group.key) {
                let mut list = Flex::column()
                    .with_main_axis_alignment(MainAxisAlignment::Start)
                    .with_cross_axis_alignment(CrossAxisAlignment::Start)
                    .with_spacing(CARD_GAP);
                for idx in group.range.clone() {
                    let (Some(card), Some(handle)) = (cards.get(idx), card_handles.get(idx)) else {
                        continue;
                    };
                    list.add_child(self.render_card_row(
                        card,
                        handle.clone(),
                        selected == Some(card.terminal_view_id),
                        appearance,
                        theme,
                    ));
                }
                col.add_child(list.finish());
            }
        }

        // 必须复用 self.list_scroll 字段:滚动偏移存句柄内部,每次 render
        // 新建句柄会丢滚动位置(照抄 cockpit card_grid_scroll 注释)。
        ClippedScrollable::vertical(
            self.list_scroll.clone(),
            Container::new(col.finish())
                .with_vertical_padding(SPACING)
                .finish(),
            ScrollbarWidth::Auto,
            ElementFill::Solid(theme.nonactive_ui_detail().into()),
            ElementFill::Solid(theme.active_ui_detail().into()),
            ElementFill::None,
        )
        .finish()
    }

    /// 组头:折叠箭头 + 分组 key(空 key = 未分组)+ 组内实例数。
    /// 点击 → 折叠/展开;组含选中实例时高亮(照抄 render_card
    /// focused_selected 分支:accent 边框 + surface_overlay_1 背景)。
    fn render_group_header(
        &self,
        group: &CockpitCardGroup,
        handle: MouseStateHandle,
        group_selected: bool,
        appearance: &Appearance,
        theme: &WarpTheme,
    ) -> Box<dyn Element> {
        let font_family = appearance.ui_font_family();
        let collapsed = self.collapsed_groups.contains(&group.key);
        // TODO(i18n): 未分组文案待 ftl 增加 cockpit-group-ungrouped 后替换,
        // 当前硬编码中文(主线程统一补 i18n)。
        let key_label = if group.key.is_empty() {
            "未分组".to_string()
        } else {
            group.key.clone()
        };
        let count_label = group.range.len().to_string();
        let chevron = if collapsed { "▸" } else { "▾" };
        let key_color = if group_selected {
            theme.accent().into()
        } else {
            theme.sub_text_color(theme.background()).into()
        };
        let border_color = if group_selected {
            theme.accent()
        } else {
            theme.nonactive_ui_detail().into()
        };
        let key_for_action = group.key.clone();

        let content = move |hovered: bool| {
            let mut row = Flex::row()
                .with_main_axis_size(MainAxisSize::Max)
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(SPACING);
            row.add_child(
                Text::new(chevron, font_family, SMALL_FONT_SIZE)
                    .with_color(key_color)
                    .finish(),
            );
            row.add_child(
                Text::new(key_label.clone(), font_family, SMALL_FONT_SIZE)
                    .with_color(key_color)
                    .soft_wrap(false)
                    .finish(),
            );
            row.add_child(Expanded::new(1., Empty::new().finish()).finish());
            row.add_child(
                Text::new(count_label.clone(), font_family, SMALL_FONT_SIZE)
                    .with_color(theme.disabled_ui_text_color().into_solid())
                    .soft_wrap(false)
                    .finish(),
            );
            let mut container = Container::new(row.finish())
                .with_horizontal_padding(CARD_PADDING)
                .with_vertical_padding(CARD_ROW_VERTICAL_PADDING)
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(CARD_RADIUS)))
                .with_border(warpui::elements::Border::all(1.).with_border_fill(border_color));
            if hovered || group_selected {
                container = container.with_background(theme.surface_overlay_1());
            }
            container.finish()
        };
        Hoverable::new(handle, move |state| content(state.is_hovered()))
            .on_click(move |ctx, _app, _position| {
                ctx.dispatch_typed_action(CockpitNavAction::ToggleGroupCollapsed(
                    key_for_action.clone(),
                ));
            })
            .with_cursor(Cursor::PointingHand)
            .finish()
    }

    /// 单行实例卡:状态点 + 标题 + 右侧 agent 名。选中/悬停高亮(照抄
    /// render_card focused_selected 分支)。点击 → ActivateCard。
    fn render_card_row(
        &self,
        card: &CockpitCard,
        handle: MouseStateHandle,
        selected: bool,
        appearance: &Appearance,
        theme: &WarpTheme,
    ) -> Box<dyn Element> {
        let font_family = appearance.ui_font_family();
        let title = truncate_str(&card.title, TITLE_MAX_CHARS);
        let agent_label = card.agent_name.unwrap_or("Shell");
        let dot_key = card.status.dot_key().map(str::to_owned);
        let border_color = if selected {
            theme.accent()
        } else {
            theme.nonactive_ui_detail().into()
        };
        let card_id = card.terminal_view_id;

        let content = move |hovered: bool| {
            let mut row = Flex::row()
                .with_main_axis_size(MainAxisSize::Max)
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(SPACING);
            if let Some(key) = &dot_key {
                row.add_child(status_dot_element(key, theme));
            }
            row.add_child(
                Text::new(title.clone(), font_family, FONT_SIZE)
                    .with_color(theme.main_text_color(theme.background()).into())
                    .soft_wrap(false)
                    .finish(),
            );
            row.add_child(Expanded::new(1., Empty::new().finish()).finish());
            row.add_child(
                Text::new(agent_label.to_string(), font_family, SMALL_FONT_SIZE)
                    .with_color(theme.disabled_ui_text_color().into_solid())
                    .soft_wrap(false)
                    .finish(),
            );
            let mut container = Container::new(row.finish())
                .with_horizontal_padding(CARD_PADDING)
                .with_vertical_padding(CARD_ROW_VERTICAL_PADDING)
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(CARD_RADIUS)))
                .with_border(warpui::elements::Border::all(1.).with_border_fill(border_color));
            if hovered || selected {
                container = container.with_background(theme.surface_overlay_1());
            }
            container.finish()
        };
        Hoverable::new(handle, move |state| content(state.is_hovered()))
            .on_click(move |ctx, _app, _position| {
                ctx.dispatch_typed_action(CockpitNavAction::ActivateCard(card_id));
            })
            .with_cursor(Cursor::PointingHand)
            .finish()
    }

    /// 空态文案(复用既有 cockpit-empty key)。
    fn render_empty_state(&self, app: &AppContext) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        let theme = appearance.theme();
        Container::new(
            Text::new(
                crate::t!("cockpit-empty"),
                appearance.ui_font_family(),
                appearance.ui_font_size(),
            )
            .with_color(theme.disabled_ui_text_color().into_solid())
            .finish(),
        )
        .with_horizontal_padding(HEADER_PADDING)
        .with_vertical_padding(HEADER_PADDING)
        .finish()
    }
}

impl Entity for CockpitNavView {
    /// 主线程接线契约:订阅 `CockpitNavEvent::CardActivated` 接 activate_tab。
    type Event = CockpitNavEvent;
}

impl TypedActionView for CockpitNavView {
    type Action = CockpitNavAction;

    /// typed action 处理(六环 #3→#4:handler 注册 + 状态更新)。
    fn handle_action(&mut self, action: &Self::Action, ctx: &mut ViewContext<Self>) {
        match action {
            CockpitNavAction::ActivateCard(id) => {
                // 单选高亮同步 model(brief 要求 #5),再发右侧跟随事件;
                // 已选中的卡重复点击仍发事件(重激活该 tab,幂等)。
                CockpitModel::handle(ctx).update(ctx, |m, ctx| m.select_card(Some(*id), ctx));
                ctx.emit(CockpitNavEvent::CardActivated {
                    terminal_view_id: *id,
                });
            }
            CockpitNavAction::ToggleGroupCollapsed(key) => {
                // 本地折叠集切换;无 model 事件,显式 notify 驱动 rerender。
                if !self.collapsed_groups.remove(key) {
                    self.collapsed_groups.insert(key.clone());
                }
                ctx.notify();
            }
        }
    }
}

impl View for CockpitNavView {
    fn ui_name() -> &'static str {
        "CockpitNavView"
    }

    fn render(&self, app: &AppContext) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        let theme = appearance.theme();
        let model = self.model.as_ref(app);

        let mut col = Flex::column()
            .with_main_axis_alignment(MainAxisAlignment::Start)
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_spacing(SPACING);

        col.add_child(self.render_header(app));
        if model.cards().is_empty() {
            col.add_child(Box::new(Expanded::new(1., self.render_empty_state(app))));
        } else {
            col.add_child(Box::new(Expanded::new(1., self.render_group_list(app))));
        }

        Container::new(col.finish())
            .with_background(theme.background())
            .finish()
    }
}

/// 字符串截断,超过 max_len 字符加 "…"(照抄 cockpit view.rs 同款策略)。
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
}
