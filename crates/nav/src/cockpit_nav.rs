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
//! 分组:nav 固定按 `CwdProject` 分组——纯内存态,不改 model 持久化偏好,
//! 每次挂载时由本视图重设(重启后 model 回默认,由挂载路径覆盖)。
//!
//! 布局参照 `vertical_tabs` 紧凑导航形态,选中态样式照抄
//! `cockpit/view.rs render_card` 的 focused_selected 分支
//! (accent 边框 + `surface_overlay_1` 背景)。

use std::cell::RefCell;
use std::collections::HashSet;
use std::path::PathBuf;

use ui_components::combo_button::combo_inner_button;
use warp_core::ui::appearance::Appearance;
use warp_core::ui::theme::WarpTheme;
use warpui::elements::{
    ClippedScrollStateHandle, ClippedScrollable, Container, CornerRadius, CrossAxisAlignment,
    Empty, Expanded, Fill as ElementFill, Flex, Hoverable, MainAxisAlignment, MainAxisSize,
    MouseStateHandle, ParentElement, ScrollbarWidth, Shrinkable, Text,
};
use warpui::platform::Cursor;
use warpui::scene::Radius;
use warpui::ui_components::components::{UiComponent, UiComponentStyles};
use warpui::{
    AppContext, Element, Entity, EntityId, ModelHandle, SingletonEntity, TypedActionView, View,
    ViewContext, ViewHandle,
};

use cockpit_model::{CockpitCard, CockpitCardGroup, CockpitModel};
use crate::status_row::status_dot_element;

// ── 布局常量 ──────────────────────────────────────────────────────────────────

/// 头部/空态水平内边距。
const HEADER_PADDING: f32 = 10.;
/// 元素间距。
const SPACING: f32 = 6.;
/// 卡片行间垂直间距。≥6: 1px 边框相邻卡贴排时两条边框会在间隙里
/// 压成一条粗线(高 DPI 下尤甚),视觉上呈"行重叠"。
const CARD_GAP: f32 = 6.;
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
/// tree 缩进:实例行相对组头(项目行)的左缩进,表达从属关系(v1 缩进即
/// 连接线,不画线)。
const TREE_INDENT: f32 = 14.;
/// 卡片行2 recap 截断上限(字符)。
const RECAP_MAX_CHARS: usize = 48;

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
    /// 组头/空组"加项目"按钮 (原 OpenAddProjectPicker dispatch, 拆分事件化;
    /// handler 转 emit 事件, app 订阅后打开 picker)。
    AddProjectRequested,
}

// ── Event(主线程接线契约)─────────────────────────────────────────────────────

/// 导航视图事件。主线程订阅 `CockpitNavEvent` 完成右侧跟随切换。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CockpitNavEvent {
    /// 用户激活了某张实例卡。主线程应把右侧内容切换到该终端所在 tab
    /// (EntityId→tab index 映射需要遍历 workspace tabs,归主线程)。
    CardActivated { terminal_view_id: EntityId },
    /// 组头/空组"加项目"按钮 (原 OpenAddProjectPicker dispatch, 拆分事件化;
    /// app 订阅后打开 picker)。
    AddProjectRequested,
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
    /// 组头"加项目" + 按钮悬停句柄(独立于组头句柄:点击加号时鼠标
    /// 状态落在自己句柄上,组头 on_click 不触发,天然防冒泡)。
    group_add_button_mouse_state: MouseStateHandle,
    /// 列表滚动句柄(复用防丢滚动位)。
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
    /// nav 固定按 `CwdProject` 分组——纯内存态,不持久化;model 的 group_by
    /// 重启后回默认值,由本视图每次挂载重设。

    pub fn new(model: ModelHandle<CockpitModel>, ctx: &mut ViewContext<Self>) -> Self {
        // 订阅 model 事件(六环 #5→#6:选中态/快照更新 → notify → rerender)。
        ctx.subscribe_to_model(&model, |_me, _handle, _event, ctx| {
            ctx.notify();
        });

        model.update(ctx, |m, ctx| {
            m.set_group_by(cockpit_model::CockpitGroupBy::CwdProject, ctx)
        });
        model.update(ctx, |m, ctx| /* app 半边 refresh 在 v1 上移 */ m.replace_snapshot(Vec::new(), m.last_window_count(), ctx));
        let mut me = Self {
            model,
            collapsed_groups: HashSet::new(),
            card_handles: RefCell::new(Vec::new()),
            group_handles: RefCell::new(Vec::new()),
            group_add_button_mouse_state: MouseStateHandle::default(),
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
                me.model.update(ctx, |m, ctx| /* app 半边 refresh 在 v1 上移 */ m.replace_snapshot(Vec::new(), m.last_window_count(), ctx));
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

        let count_text = i18n::t!(
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
    /// 合并持久化项目(ProjectManagementModel): 已注册但当前无实例的
    /// 项目显示为空组——关闭项目内所有 tab 后项目不从导航消失。
    fn render_group_list(&self, app: &AppContext) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        let theme = appearance.theme();
        let model = self.model.as_ref(app);
        let cards = model.cards();
        let selected = model.selected();
        let groups = model.groups();

        // 持久化项目名集(目录名,与 cwd_group_key 同粒度);已出现在活动
        // 组里的跳过,只补"空组"。
        let active_keys: HashSet<&str> =
            groups.iter().map(|g| g.key.as_str()).collect();
        // 持久化项目名集: app refresh 时经 CockpitModel::set_empty_project_names
        // 推入 (拔钉 F — nav 不 import ProjectManagementModel/persistence)。
        // 过滤掉已出现在活动组里的 (与旧 PMM 直读逻辑等价)。
        let empty_projects: Vec<String> = {
            let active: HashSet<&str> = groups.iter().map(|g| g.key.as_str()).collect();
            model
                .empty_project_names()
                .iter()
                .filter(|name| !name.is_empty() && !active.contains(name.as_str()))
                .cloned()
                .collect()
        };

        Self::ensure_handles(
            &mut self.group_handles.borrow_mut(),
            groups.len() + empty_projects.len(),
        );
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
                cards,
                group_handle.clone(),
                self.group_add_button_mouse_state.clone(),
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
                    list.add_child(
                        Container::new(self.render_card_row(
                            card,
                            handle.clone(),
                            selected == Some(card.terminal_view_id),
                            appearance,
                            theme,
                        ))
                        // tree 型层级:实例行整体左缩进表达从属(v1 不画线)。
                        .with_padding_left(TREE_INDENT)
                        .finish(),
                    );
                }
                col.add_child(list.finish());
            }
        }

        // 空项目组(持久化项目,无活动实例): 组头 + "无实例"占位。
        for (ei, name) in empty_projects.iter().enumerate() {
            let Some(handle) = group_handles.get(groups.len() + ei) else {
                continue;
            };
            let empty_group = cockpit_model::CockpitCardGroup {
                key: name.clone(),
                range: 0..0,
            };
            col.add_child(self.render_group_header(
                &empty_group,
                cards,
                handle.clone(),
                self.group_add_button_mouse_state.clone(),
                false,
                appearance,
                theme,
            ));
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
        .with_overlayed_scrollbar()
        .finish()
    }

    /// 组头:折叠箭头 + 分组 key(空 key = 未分组)+ 组内实例数。
    /// 点击 → 折叠/展开;组含选中实例时高亮(照抄 render_card
    /// focused_selected 分支:accent 边框 + surface_overlay_1 背景)。
    fn render_group_header(
        &self,
        group: &CockpitCardGroup,
        cards: &[CockpitCard],
        handle: MouseStateHandle,
        add_button_mouse_state: MouseStateHandle,
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
        // 组内 working 计数(项目行右侧 ●n;Blocked 也视为等待,计入)。
        // TODO(i18n): 纯数字角标,无需 ftl。
        let working_count = group
            .range
            .clone()
            .filter_map(|idx| cards.get(idx))
            .filter(|c| {
                matches!(
                    c.status,
                    cockpit_model::CockpitCardStatus::Working
                        | cockpit_model::CockpitCardStatus::Blocked(_)
                )
            })
            .count();
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
            // working 计数点(组内 working/blocked > 0 时显示 ●n)。
            if working_count > 0 {
                row.add_child(
                    Text::new(format!("●{working_count}"), font_family, SMALL_FONT_SIZE)
                        .with_color(theme.ansi_fg_yellow())
                        .soft_wrap(false)
                        .finish(),
                );
            }
            row.add_child(Expanded::new(1., Empty::new().finish()).finish());
            row.add_child(
                Text::new(count_label.clone(), font_family, SMALL_FONT_SIZE)
                    .with_color(theme.disabled_ui_text_color().into_solid())
                    .soft_wrap(false)
                    .finish(),
            );
            // "加项目" + 按钮:独立 MouseStateHandle + combo_inner_button
            // (照抄 vertical_tabs render_project_section 同款)。按钮是独立
            // Hoverable,click 在子元素处理完即返回 handled,组头的
            // on_click(折叠)不会再触发——防冒泡。
            let add_button = combo_inner_button(
                theme,
                warp_core::ui::icons::Icon::Plus,
                false,
                add_button_mouse_state.clone(),
            )
            .with_style(
                UiComponentStyles::default()
                    .set_border_radius(CornerRadius::with_all(Radius::Pixels(CARD_RADIUS)))
                    .set_font_color(theme.disabled_ui_text_color().into_solid()),
            )
            .build()
            .on_click(|ctx, _, _| {
                ctx.dispatch_typed_action(CockpitNavAction::AddProjectRequested);
            })
            .finish();
            row.add_child(add_button);
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
            // 子元素(加号按钮)处理过 click 时跳过本 Hoverable 的 click。
            .with_defer_events_to_children()
            .with_cursor(Cursor::PointingHand)
            .finish()
    }

    /// 实例卡(两行布局):行1 = 状态点 + 标题 + 右侧 agent 名;行2 =
    /// 小号次级色辅助信息(branch ▸ + cwd 末段 + recap 截断,全空则单行)。
    /// 选中/悬停高亮(照抄 render_card focused_selected 分支)。点击 →
    /// ActivateCard。
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
        // 卡片 ID(EntityId 稳定哈希 4-hex,照抄 cockpit card_tag):身份辨识,
        // 与日志/编排器侧 tag 对账。TODO(i18n): # 前缀符号无需 ftl。
        let id_label = {
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            std::hash::Hash::hash(&card.terminal_view_id, &mut hasher);
            format!("#{:04x}", (std::hash::Hasher::finish(&hasher) & 0xffff) as u16)
        };
        let dot_key = card.status.dot_key().map(str::to_owned);
        let border_color = if selected {
            theme.accent()
        } else {
            theme.nonactive_ui_detail().into()
        };
        let card_id = card.terminal_view_id;

        // 行2 辅助段:ID(恒有)+branch(有则 ▸branch)+ cwd 末段(有则)+
        // recap 截断(有则)。
        // TODO(i18n): 辅助分隔符为符号,无需 ftl。
        let mut aux_parts: Vec<String> = vec![id_label];
        if let Some(branch) = card.branch.as_deref().filter(|b| !b.trim().is_empty()) {
            aux_parts.push(format!("▸{branch}"));
        }
        if let Some(cwd) = card.cwd.as_deref().filter(|c| !c.trim().is_empty()) {
            let tail = cwd
                .rsplit('/')
                .find(|seg| !seg.trim().is_empty())
                .unwrap_or(cwd);
            aux_parts.push(tail.to_string());
        }
        if let Some(recap) = card.recap.as_deref().filter(|r| !r.trim().is_empty()) {
            aux_parts.push(truncate_str(recap, RECAP_MAX_CHARS));
        }
        let aux_label = if aux_parts.is_empty() {
            None
        } else {
            Some(aux_parts.join("  "))
        };

        let content = move |hovered: bool| {
            // 行1:状态点 + 标题 + agent 名。
            let mut first_line = Flex::row()
                .with_main_axis_size(MainAxisSize::Max)
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(SPACING);
            if let Some(key) = &dot_key {
                first_line.add_child(status_dot_element(key, theme));
            }
            first_line.add_child(
                // Expanded(Tight) 而非 Shrinkable(Loose): 长标题测量宽会
                // 撑爆行宽把卡片推出栏缘(右缘被裁,排版错乱);Tight 钳到
                // 分配空间,配合 soft_wrap(false) 溢出部分不绘制。
                Expanded::new(
                    1.,
                    Text::new(title.clone(), font_family, FONT_SIZE)
                        .with_color(theme.main_text_color(theme.background()).into())
                        .soft_wrap(false)
                        .finish(),
                )
                .finish(),
            );
            first_line.add_child(
                Text::new(agent_label.to_string(), font_family, SMALL_FONT_SIZE)
                    .with_color(theme.disabled_ui_text_color().into_solid())
                    .soft_wrap(false)
                    .finish(),
            );

            // 行2(可选):小号次级色辅助信息。aux 行放 Expanded(Tight) 的
            // row 里而不是 column 裸 Text: 纵向裁切实测(行2 只画 2px)来自
            // column 直接量测 Loose 高度;Tight 行 + ellipsis 文本稳定。
            let mut body = Flex::column()
                .with_main_axis_size(MainAxisSize::Min)
                .with_cross_axis_alignment(CrossAxisAlignment::Start);
            body.add_child(first_line.finish());
            if let Some(aux) = &aux_label {
                let mut aux_row = Flex::row()
                    .with_main_axis_size(MainAxisSize::Max)
                    .with_cross_axis_alignment(CrossAxisAlignment::Center);
                aux_row.add_child(
                    Expanded::new(
                        1.,
                        Text::new(aux.clone(), font_family, SMALL_FONT_SIZE)
                            .with_color(theme.sub_text_color(theme.background()).into())
                            .soft_wrap(false)
                            .with_clip(warpui::text_layout::ClipConfig::ellipsis())
                            .finish(),
                    )
                    .finish(),
                );
                body.add_child(aux_row.finish());
            }

            let mut container = Container::new(body.finish())
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
                i18n::t!("cockpit-empty"),
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
            CockpitNavAction::AddProjectRequested => {
                // EventContext 无 emit;经 typed action 中转到 ViewContext emit。
                ctx.emit(CockpitNavEvent::AddProjectRequested);
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
