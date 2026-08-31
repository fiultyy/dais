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
    ClippedScrollStateHandle, ClippedScrollable, ConstrainedBox, Container, CornerRadius,
    CrossAxisAlignment, Empty, Expanded, Fill as ElementFill, Flex, Hoverable, MainAxisAlignment,
    MainAxisSize, MouseStateHandle, ParentElement, ScrollbarWidth, Shrinkable, Text,
};
use warpui::platform::Cursor;
use warpui::scene::Radius;
use warpui::ui_components::components::{UiComponent, UiComponentStyles};
use warpui::{
    AppContext, Element, Entity, EntityId, ModelHandle, SingletonEntity, TypedActionView, View,
    ViewContext, ViewHandle,
};

use cockpit_model::{CockpitCard, CockpitCardGroup, CockpitCardStatus, CockpitModel};
use warpui::color::ColorU;

// ── 布局常量(Orca sidebar 对齐: 无边框卡片 + 背景 wash 分层 + 紧凑密度)──────

/// 头部/空态水平内边距。
const HEADER_PADDING: f32 = 10.;
/// 元素间距。
const SPACING: f32 = 6.;
/// 卡片行间垂直间距(Orca space-y-1 = 4px)。
const CARD_GAP: f32 = 4.;
/// 组间距(去边框后组边界全靠留白,必须显著大于卡间距)。
const GROUP_GAP: f32 = 12.;
/// 左栏对账刷新间隔(与 cockpit 面板 COCKPIT_RECONCILE_INTERVAL_MS 一致)。
const COCKPIT_NAV_RECONCILE_INTERVAL_MS: u64 = 2_000;
/// 行水平内边距。
const CARD_PADDING: f32 = 8.;
/// 行垂直内边距(Orca pt-1.25/pb-1.5 ≈ 5/6px)。
const CARD_ROW_VERTICAL_PADDING: f32 = 5.;
/// 行圆角半径(Orca rounded-lg 档)。
const CARD_RADIUS: f32 = 6.;
/// 主文本字号(标题)。
const FONT_SIZE: f32 = 13.;
/// 组头字号。
const GROUP_FONT_SIZE: f32 = 12.;
/// 辅助文本字号。
const SMALL_FONT_SIZE: f32 = 11.;
/// mono 微字号(#id/agent 名右槽)。
const MONO_FONT_SIZE: f32 = 10.;
/// 状态点尺寸(Orca AgentStateDot sm = 10px)。
const STATUS_ICON_SIZE: f32 = 10.;
/// 状态 gutter 槽宽:状态点 10 + 右间距 4。五态恒占位(含 Idle),
/// 保证所有行状态点/标题起点同列(扫视竖线)。
const STATUS_GUTTER: f32 = 14.;
/// 身份图标尺寸(agent 品牌 logo 14px,「谁」与「什么状态」两个独立 glyph)。
const IDENTITY_ICON_SIZE: f32 = 14.;
/// agent 名右槽宽(固定宽:内容换入换出不引发行宽变化)。
const AGENT_SLOT_WIDTH: f32 = 56.;
/// 卡片标题截断上限(字符)。
const TITLE_MAX_CHARS: usize = 28;
/// recap 截断上限(字符)。
const RECAP_MAX_CHARS: usize = 48;
/// tree 缩进:实例行相对组头的左缩进,表达从属关系。
const TREE_INDENT: f32 = 13.;

/// 顶部标题。
// TODO(i18n): 文案硬编码中文,主线程统一补 ftl key 后替换。
const NAV_TITLE: &str = "导航";

// ── Orca 化呈现 helper(状态五态词汇表 / agent 身份图标 / 选中 wash)──────────

/// 状态 → (图标, 颜色)。五态全占位(Idle 也有中性弱化点),保证 gutter
/// 扫视竖线成立。色语义对齐 Orca AgentStateDot:
/// - Working = ClockLoader magenta(agent 在跑;Orca 为 spinner,此处静态帧)
/// - Busy = ClockLoader blue(机器/长命令在跑;Orca 词汇表无此态,dais 扩展)
/// - Blocked = HelpCircle amber(等用户输入/权限;≡ Orca waiting/permission,
///   **不是** Orca 的 blocked=红——dais 无失败态,红留给未来 Failed)
/// - Done = Check green(与 idle 灰点显式区分)
/// - Idle = Circle 中性 40% alpha
fn status_glyph(
    status: &CockpitCardStatus,
    theme: &WarpTheme,
) -> (warp_core::ui::icons::Icon, ColorU) {
    use cockpit_model::CockpitCardStatus;
    use warp_core::ui::icons::Icon;
    match status {
        CockpitCardStatus::Working => (Icon::ClockLoader, theme.ansi_fg_magenta()),
        CockpitCardStatus::Busy => (Icon::ClockLoader, theme.ansi_fg_blue()),
        CockpitCardStatus::Blocked(_) => (Icon::HelpCircle, theme.ansi_fg_yellow()),
        CockpitCardStatus::Done => (Icon::Check, theme.ansi_fg_green()),
        CockpitCardStatus::Idle => {
            let c = theme.disabled_ui_text_color().into_solid();
            (Icon::Circle, ColorU::new(c.r, c.g, c.b, 102))
        }
    }
}

/// 状态 gutter 槽元素:固定宽 ConstrainedBox 包状态点。所有行恒占位。
fn status_gutter(status: &CockpitCardStatus, theme: &WarpTheme) -> Box<dyn Element> {
    let (icon, color) = status_glyph(status, theme);
    ConstrainedBox::new(icon.to_warpui_icon(warp_core::ui::theme::Fill::Solid(color)).finish())
        .with_width(STATUS_GUTTER)
        .with_height(STATUS_GUTTER)
        .finish()
}

/// agent 身份图标:agent_name 匹配品牌 logo(「谁」),无匹配的 agent 用
/// AiAssistant 兜底,无 agent(纯 shell)用 Terminal。与状态点互为独立 glyph。
fn identity_icon(card: &CockpitCard) -> warp_core::ui::icons::Icon {
    use warp_core::ui::icons::Icon;
    let Some(name) = card.agent_name else {
        return Icon::Terminal;
    };
    let n = name.to_ascii_lowercase();
    if n.contains("claude") {
        Icon::ClaudeLogo
    } else if n.contains("codex") || n.contains("openai") || n.contains("gpt") {
        Icon::OpenAILogo
    } else if n.contains("gemini") {
        Icon::GeminiLogo
    } else if n.contains("cursor") {
        Icon::CursorLogo
    } else if n.contains("copilot") {
        Icon::CopilotLogo
    } else if n.contains("goose") {
        Icon::GooseLogo
    } else if n.contains("droid") {
        Icon::DroidLogo
    } else if n.contains("amp") {
        Icon::AmpLogo
    } else if n.contains("deepseek") {
        Icon::DeepSeekLogo
    } else if n.contains("pi") && n.len() <= 4 {
        Icon::PiLogo
    } else if n.contains("omp") {
        Icon::OmpLogo
    } else {
        Icon::AiAssistant
    }
}

/// 选中卡背景 wash:accent 低透明(Orca isActiveSurface 语义)。预混在
/// 侧栏纯色底上与 alpha 混合数学等价,这里直接用 ColorU alpha 通道。
fn selected_wash(theme: &WarpTheme) -> ColorU {
    let a = theme.accent().into_solid();
    ColorU::new(a.r, a.g, a.b, 38)
}

/// 查看端/只读标记的轻量描述:仅受限端有值(只读=Lock;共享查看端=Eye)。
/// 返回 (icon, muted 色) 而非元素——元素不可克隆,轻量描述可被 content
/// 闭包按 hover 重建时反复物化。
fn restricted_glyph(card: &CockpitCard, theme: &WarpTheme) -> Option<(warp_core::ui::icons::Icon, ColorU)> {
    use warp_core::ui::icons::Icon;
    let icon = if !card.writable {
        Icon::Lock
    } else if card.connected {
        Icon::Eye
    } else {
        return None;
    };
    Some((icon, theme.disabled_ui_text_color().into_solid()))
}

// ── 刷新钩子(app 半边注入)────────────────────────────────────────────────────

/// app 半边全量刷新钩子(签名同 `app::ai::cockpit::model::refresh_model`)。
/// nav 在 crate 内不能依赖 app 类型,与 panel_api::AvailabilityHooks 同模式:
/// app 初始化时注入一次,nav 的挂载首刷与对账 timer 经此触发真实刷新。
pub type CockpitRefreshFn = fn(
    &mut cockpit_model::CockpitModel,
    &mut warpui::ModelContext<cockpit_model::CockpitModel>,
);

static REFRESH_HOOK: std::sync::LazyLock<std::sync::OnceLock<CockpitRefreshFn>> =
    std::sync::LazyLock::new(std::sync::OnceLock::new);

/// app 初始化时挂接刷新钩子;幂等(首次生效)。
pub fn set_cockpit_refresh_hook(hook: CockpitRefreshFn) {
    let _ = REFRESH_HOOK.set(hook);
}

/// 读取刷新钩子;未注入时返回 None(此时不做任何事,避免误清数据)。
fn refresh_hook() -> Option<CockpitRefreshFn> {
    REFRESH_HOOK.get().copied()
}

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
        // v1 修复: 挂载不再清空快照(共享单例, 清空会打掉 cockpit 面板正在
        // 显示的卡片); 改为请求 app 半边做一次真实 refresh。
        model.update(ctx, |m, ctx| {
            if let Some(refresh) = refresh_hook() {
                refresh(m, ctx);
            }
        });
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
                // v1 修复: 对账 = 请求 app 半边真实 refresh(原实现错误地
                // 硬清空快照且无回填, 2s 后把共享单例打空 → 面板空白)。
                me.model.update(ctx, |m, ctx| {
                    if let Some(refresh) = refresh_hook() {
                        refresh(m, ctx);
                    }
                });
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
            Text::new(NAV_TITLE, appearance.ui_font_family(), GROUP_FONT_SIZE)
                .with_color(theme.sub_text_color(theme.background()).into())
                .finish(),
        );
        row.add_child(Expanded::new(1., Empty::new().finish()).finish());
        row.add_child(
            Text::new(count_text, appearance.monospace_font_family(), MONO_FONT_SIZE)
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
            .with_spacing(GROUP_GAP);

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

    /// 组头(Orca 化轻量):无框标题行 = 折叠图标 + 项目名(12px semibold
    /// muted)+ working/blocked 分列计数 + 右端实例数 + hover 加号。
    /// 去边框去背景;层级全靠字重/颜色/留白(Orca SidebarHeader 语义)。
    /// 点击 → 折叠/展开。
    fn render_group_header(
        &self,
        group: &CockpitCardGroup,
        cards: &[CockpitCard],
        handle: MouseStateHandle,
        add_button_mouse_state: MouseStateHandle,
        _group_selected: bool,
        appearance: &Appearance,
        theme: &WarpTheme,
    ) -> Box<dyn Element> {
        use warp_core::ui::icons::Icon;
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
        // 组级活动计数,拆开双语义:working=agent 在跑(magenta),
        // blocked=有人等我(amber,行动项)。分列渲染,不再混计一个黄点。
        let mut working_count = 0usize;
        let mut blocked_count = 0usize;
        for idx in group.range.clone() {
            match cards.get(idx).map(|c| &c.status) {
                Some(cockpit_model::CockpitCardStatus::Working) => working_count += 1,
                Some(cockpit_model::CockpitCardStatus::Blocked(_)) => blocked_count += 1,
                _ => {}
            }
        }
        let chevron_icon = if collapsed {
            Icon::ListCollapsed
        } else {
            Icon::ListOpen
        };
        let chevron_color = theme.sub_text_color(theme.background()).into();
        let key_color = theme.sub_text_color(theme.background()).into();
        let key_for_action = group.key.clone();

        let content = move |hovered: bool| {
            let mut row = Flex::row()
                .with_main_axis_size(MainAxisSize::Max)
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(SPACING);
            row.add_child(
                ConstrainedBox::new(
                    chevron_icon
                        .to_warpui_icon(warp_core::ui::theme::Fill::Solid(chevron_color))
                        .finish(),
                )
                .with_width(GROUP_FONT_SIZE)
                .with_height(GROUP_FONT_SIZE)
                .finish(),
            );
            row.add_child(
                Text::new(key_label.clone(), font_family, GROUP_FONT_SIZE)
                    .with_color(key_color)
                    .soft_wrap(false)
                    .finish(),
            );
            // 组级活动计数:●n magenta(在跑)+ ?n amber(等输入)。
            if working_count > 0 {
                row.add_child(
                    Text::new(format!("●{working_count}"), font_family, SMALL_FONT_SIZE)
                        .with_color(theme.ansi_fg_magenta())
                        .soft_wrap(false)
                        .finish(),
                );
            }
            if blocked_count > 0 {
                row.add_child(
                    Text::new(format!("?{blocked_count}"), font_family, SMALL_FONT_SIZE)
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
                hovered,
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
            // 无边框组头;hover 仅极淡 wash(Orca 组头无背景,这里给一点
            // 可点性反馈,弱于卡片 hover)。
            let mut container = Container::new(row.finish())
                .with_horizontal_padding(CARD_PADDING)
                .with_vertical_padding(CARD_ROW_VERTICAL_PADDING)
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(CARD_RADIUS)));
            if hovered {
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

    /// 实例卡(Orca 化三行制,无边框):
    /// 行1 标题行 = [状态 gutter(恒占位)] + [身份图标 14px] + [标题 13px,
    /// Blocked 时提亮强调] + 右端 [受限 glyph(仅查看端/只读)] + [agent 名右槽
    /// 56px 固定宽];
    /// 行2 meta = [#id 10px mono] + [▸branch 11px](cwd 末段删除——组头已
    /// 表达,卡内重复);
    /// 行3 内容行(按状态分流):Blocked → 行动行「tool — message」amber;
    /// Working/Done → recap 11px;Busy → recap mono(命令尾);Idle → 无。
    /// 选中 = accent wash 背景;hover = surface_overlay_1(弱一档)。
    /// 无边框:层级靠背景 wash + 间距 + 字重(Orca 卡面语义)。点击 →
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
        let mono_family = appearance.monospace_font_family();
        let title = truncate_str(&card.title, TITLE_MAX_CHARS);
        // 卡片 ID(EntityId 稳定哈希 4-hex,照抄 cockpit card_tag):身份辨识,
        // 与日志/编排器侧 tag 对账。TODO(i18n): # 前缀符号无需 ftl。
        let id_label = {
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            std::hash::Hash::hash(&card.terminal_view_id, &mut hasher);
            format!("#{:04x}", (std::hash::Hasher::finish(&hasher) & 0xffff) as u16)
        };
        let agent_label = card.agent_name.unwrap_or("Shell").to_string();
        let card_id = card.terminal_view_id;
        // 行1 标题色:Blocked/Working 提亮(需要我/在跑),其余降一档灰。
        let title_color = match card.status {
            cockpit_model::CockpitCardStatus::Working
            | cockpit_model::CockpitCardStatus::Blocked(_) => {
                theme.main_text_color(theme.background()).into()
            }
            _ => theme.sub_text_color(theme.background()).into(),
        };
        // 行3 内容行:按状态分流(见 fn 注释)。
        let content_line: Option<(String, ColorU, bool)> = match &card.status {
            cockpit_model::CockpitCardStatus::Blocked(_) => {
                // 行动行:工具名 — message,amber 系,压过一切 recap。
                let tool = card.tool_name.as_deref().unwrap_or("");
                let msg = card
                    .status
                    .blocked_message()
                    .unwrap_or("等待输入");
                let text = if tool.is_empty() {
                    msg.to_string()
                } else {
                    format!("{tool} — {msg}")
                };
                Some((truncate_str(&text, RECAP_MAX_CHARS), theme.ansi_fg_yellow(), false))
            }
            cockpit_model::CockpitCardStatus::Busy => card
                .recap
                .as_deref()
                .filter(|r| !r.trim().is_empty())
                .map(|r| (truncate_str(r, RECAP_MAX_CHARS), theme.sub_text_color(theme.background()).into(), true)),
            cockpit_model::CockpitCardStatus::Working | cockpit_model::CockpitCardStatus::Done => card
                .recap
                .as_deref()
                .filter(|r| !r.trim().is_empty())
                .map(|r| (truncate_str(r, RECAP_MAX_CHARS), theme.sub_text_color(theme.background()).into(), false)),
            cockpit_model::CockpitCardStatus::Idle => None,
        };
        let identity = identity_icon(card);
        let identity_color = theme.sub_text_color(theme.background()).into();
        let restricted = restricted_glyph(card, theme);

        let content = move |hovered: bool| {
            // 行1:状态 gutter + 身份图标 + 标题 + 受限 glyph + agent 右槽。
            let mut first_line = Flex::row()
                .with_main_axis_size(MainAxisSize::Max)
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(SPACING);
            first_line.add_child(status_gutter(&card.status, theme));
            first_line.add_child(
                ConstrainedBox::new(
                    identity.to_warpui_icon(warp_core::ui::theme::Fill::Solid(identity_color)).finish(),
                )
                .with_width(IDENTITY_ICON_SIZE)
                .with_height(IDENTITY_ICON_SIZE)
                .finish(),
            );
            first_line.add_child(
                // Expanded(Tight): 长标题钳到分配空间,溢出不绘制(防撑爆栏宽)。
                Expanded::new(
                    1.,
                    Text::new(title.clone(), font_family, FONT_SIZE)
                        .with_color(title_color)
                        .soft_wrap(false)
                        .finish(),
                )
                .finish(),
            );
            if let Some((icon, color)) = &restricted {
                first_line.add_child(
                    ConstrainedBox::new(
                        icon.to_warpui_icon(warp_core::ui::theme::Fill::Solid(*color)).finish(),
                    )
                    .with_width(MONO_FONT_SIZE)
                    .with_height(MONO_FONT_SIZE)
                    .finish(),
                );
            }
            // agent 名右槽:固定宽,右对齐(列稳定;未来 hover 动作换入
            // 同槽不 reflow)。
            first_line.add_child(
                ConstrainedBox::new(
                    Text::new(agent_label.clone(), mono_family, MONO_FONT_SIZE)
                        .with_color(theme.disabled_ui_text_color().into_solid())
                        .soft_wrap(false)
                        .with_clip(warpui::text_layout::ClipConfig::ellipsis())
                        .finish(),
                )
                .with_width(AGENT_SLOT_WIDTH)
                .finish(),
            );

            // 行2 meta:#id(mono 弱化,右列第二扫视锚)+branch(有则)。
            let mut body = Flex::column()
                .with_main_axis_size(MainAxisSize::Min)
                .with_cross_axis_alignment(CrossAxisAlignment::Start);
            body.add_child(first_line.finish());
            let branch_part = card
                .branch
                .as_deref()
                .filter(|b| !b.trim().is_empty())
                .map(|b| format!("▸{b}"));
            {
                let mut meta_row = Flex::row()
                    .with_main_axis_size(MainAxisSize::Max)
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_spacing(SPACING);
                // gutter 对齐占位(meta 行 #id 与上行状态点同列)。
                meta_row.add_child(
                    ConstrainedBox::new(Empty::new().finish())
                        .with_width(STATUS_GUTTER)
                        .with_height(1.)
                        .finish(),
                );
                meta_row.add_child(
                    Text::new(id_label.clone(), mono_family, MONO_FONT_SIZE)
                        .with_color(theme.disabled_ui_text_color().into_solid())
                        .soft_wrap(false)
                        .finish(),
                );
                if let Some(branch) = &branch_part {
                    meta_row.add_child(
                        Text::new(branch.clone(), font_family, SMALL_FONT_SIZE)
                            .with_color(theme.sub_text_color(theme.background()).into())
                            .soft_wrap(false)
                            .with_clip(warpui::text_layout::ClipConfig::ellipsis())
                            .finish(),
                    );
                }
                body.add_child(meta_row.finish());
            }
            // 行3 内容行(状态分流,可省略)。
            if let Some((text, color, is_mono)) = &content_line {
                let mut row3 = Flex::row()
                    .with_main_axis_size(MainAxisSize::Max)
                    .with_cross_axis_alignment(CrossAxisAlignment::Center);
                row3.add_child(
                    ConstrainedBox::new(Empty::new().finish())
                        .with_width(STATUS_GUTTER)
                        .with_height(1.)
                        .finish(),
                );
                let family = if *is_mono { mono_family } else { font_family };
                let size = if *is_mono { MONO_FONT_SIZE } else { SMALL_FONT_SIZE };
                row3.add_child(
                    Expanded::new(
                        1.,
                        Text::new(text.clone(), family, size)
                            .with_color(*color)
                            .soft_wrap(false)
                            .with_clip(warpui::text_layout::ClipConfig::ellipsis())
                            .finish(),
                    )
                    .finish(),
                );
                body.add_child(row3.finish());
            }

            let mut container = Container::new(body.finish())
                .with_horizontal_padding(CARD_PADDING)
                .with_vertical_padding(CARD_ROW_VERTICAL_PADDING)
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(CARD_RADIUS)));
            if selected {
                container = container.with_background_color(selected_wash(theme));
            } else if hovered {
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
            .with_background_color(theme.background().into())
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
