//! Cockpit 面板视图 — CockpitPanelView。
//!
//! 视图不持有业务状态,只渲染 `CockpitModel` 快照 + 派发意图(observatory 同款)。
//! P1 交互(spec §4.2)全部 typed Action(六环接线):
//! - 刷新:agent 会话事件(`CLIAgentSessionsModelEvent` 订阅,事件粒度反映)+
//!   10s 低频 timer 对账(终端开合无会话事件);
//! - 点击卡片 → `FocusCard` → 跨 tab/窗口聚焦 terminal pane
//!   (`WorkspaceAction::FocusTerminalViewInWorkspace`,PaneViewLocator 复用);
//! - 勾选框 → `ToggleCardSelection`(multi-select)+ 底部注入条 → `BeginInjection`
//!   → 确认对话框(列目标清单)→ `ConfirmInjection`;
//! - 筛选/排序/分组:`SetFilter` + `Cycle*` 便捷 action(落到同一批 setter)。

use std::cell::RefCell;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use warpui::elements::{
    ChildAnchor, ClippedScrollStateHandle, ClippedScrollable, ConstrainedBox, Container,
    CornerRadius, CrossAxisAlignment, Dismiss, Empty, Expanded, Fill as ElementFill, Flex,
    Hoverable, MainAxisAlignment, MouseStateHandle, OffsetPositioning, ParentAnchor,
    ParentElement, ScrollbarWidth, Stack, Text, Wrap,
};
use warpui::r#async::SpawnedFutureHandle;
use warpui::r#async::Timer;
use warpui::scene::Radius;
use warpui::{
    AppContext, Element, Entity, EntityId, ModelHandle, SingletonEntity, TypedActionView,
    View, ViewContext, ViewHandle,
};

use warp_core::ui::appearance::Appearance;
use warp_core::ui::theme::WarpTheme;

use super::model::{
    CockpitCard, CockpitCardStatus, CockpitGroupBy, CockpitModel, CockpitSort,
    CockpitStatusFilter,
};
use crate::ai::blocklist::agent_view::agent_input_footer::AgentInputButtonTheme;
use crate::ai::observatory::row::status_dot_element;
use crate::terminal::cli_agent_sessions::{CLIAgentSessionsModel, CLIAgentSessionsModelEvent};
use crate::ui_components::dialog::{dialog_styles, Dialog};
use crate::view_components::action_button::{ActionButton, ButtonSize};
use crate::view_components::{SubmittableTextInput, SubmittableTextInputEvent};
use warpui::ui_components::components::UiComponent;
use crate::workspace::WorkspaceAction;
use warpui::geometry::vector::vec2f;

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
/// 低频对账 timer 间隔(ms)。P1:主刷新走 `CLIAgentSessionsModelEvent`
/// 事件订阅(agent 状态即时);本 timer 对账事件覆盖不到的字段:
/// 无 agent 终端的 Busy/Idle、preview_tail、git branch、cwd。
///
/// [cockpit-slow] 2026-08-22 排查定论:采集路径零瓶颈(8 终端全量
/// refresh 实测 ~350µs,单卡 FairMutex 等待 ~100ns、尾行提取 ~1.3µs,
/// 见 model.rs snapshot_card 注释),感知延迟全部来自本间隔——原 10s
/// 让终端状态最长 10s 不可见。压到 2s:每秒成本 ~0.2ms(9 卡),可忽略;
/// observatory 为 5s,cockpit 作为交互操作面板取更紧的 2s。
const COCKPIT_RECONCILE_INTERVAL_MS: u64 = 2_000;
/// OutputChanged 合并窗(ms)。preview 尾行/分支随输出增长的刷新合并:
/// 窗内事件吸收,窗到单次 refresh(风暴上限 ~6.7 刷新/s,单事件延迟上界
/// 150ms——人眼对 preview 文本的感知粒度足够)。StateChanged 不进此窗。
const OUTPUT_COALESCE_WINDOW_MS: u64 = 150;
/// 多选勾选框边长。
const CHECKBOX_SIZE: f32 = 14.;
/// 注入确认对话框宽度。
const CONFIRM_DIALOG_WIDTH: f32 = 420.;

// ── Action ────────────────────────────────────────────────────────────────────

/// 面板 typed action:on_click 分发、`on_action` 处理(warpui 六环接线)。
#[derive(Clone, Debug, PartialEq)]
pub enum CockpitPanelAction {
    Refresh,
    /// 点击卡片主体:跨 tab/窗口聚焦对应 terminal pane。
    FocusCard(EntityId),
    /// 勾选框切换 multi-select(批量注入目标)。
    ToggleCardSelection(EntityId),
    /// 清空单选 + multi-select。
    ClearSelection,
    /// 文本筛选(标题/cwd/agent/recap/tool)。
    SetFilter(String),
    /// 循环按钮便捷 action:从当前状态切到下一模式(落到 Set* setter)。
    CycleStatusFilter,
    CycleSortMode,
    CycleGroupBy,
    /// 提交注入文本(注入条 Enter / 注入按钮)→ 进入确认态。
    BeginInjection(String),
    /// 注入按钮:文本从注入输入框读取。
    BeginInjectionFromInput,
    /// 确认注入(执行发送)。
    ConfirmInjection,
    /// 取消注入(保留选中集)。
    CancelInjection,
}

// ── CockpitPanelView ────────────────────────────────────────────────────────

/// 驾驶舱面板视图。只渲染,不持有业务状态(渲染缓存:卡片/勾选框悬停句柄)。
pub struct CockpitPanelView {
    model: ModelHandle<CockpitModel>,
    refresh_button: ViewHandle<ActionButton>,
    /// 排序模式循环按钮(标签随模式更新,见 handle_action)。
    sort_button: ViewHandle<ActionButton>,
    /// 分组模式循环按钮。
    group_button: ViewHandle<ActionButton>,
    /// 状态筛选循环按钮。
    status_filter_button: ViewHandle<ActionButton>,
    /// 清空选中按钮(选中集非空时渲染)。
    clear_selection_button: ViewHandle<ActionButton>,
    /// 注入提交按钮(选中集非空时渲染)。
    inject_button: ViewHandle<ActionButton>,
    confirm_inject_button: ViewHandle<ActionButton>,
    cancel_inject_button: ViewHandle<ActionButton>,
    /// 文本筛选输入框。
    filter_input: ViewHandle<SubmittableTextInput>,
    /// 注入文本输入框。
    inject_input: ViewHandle<SubmittableTextInput>,
    /// 低频对账 timer 句柄,Drop 时中止。
    reconcile_timer_handle: Option<SpawnedFutureHandle>,
    /// OutputChanged 合并窗 timer 句柄(在跑 = 窗口开启,新事件被吸收;
    /// 窗到即单次刷新。Drop 时中止,防泄漏)。
    output_coalesce_handle: Option<SpawnedFutureHandle>,
    /// 卡片悬停句柄(渲染缓存,数量对齐卡片数)。
    card_handles: RefCell<Vec<MouseStateHandle>>,
    /// 勾选框悬停句柄(渲染缓存,数量对齐卡片数)。
    checkbox_handles: RefCell<Vec<MouseStateHandle>>,
    /// 卡片网格滚动句柄。必须是 view 字段:滚动偏移存在句柄内部,
    /// 若在 render 里每次重建句柄,10s 对账 timer + agent 事件驱动的高频
    /// 重渲染会把列表每次都滚回顶部(照抄 observatory 的同构做法)。
    card_grid_scroll: ClippedScrollStateHandle,
}

impl CockpitPanelView {
    pub fn new(model: ModelHandle<CockpitModel>, ctx: &mut ViewContext<Self>) -> Self {
        // 控制按钮(六环 #2:元素回调 dispatch Action)。
        let refresh_button = ctx.add_typed_action_view(|_ctx| {
            ActionButton::new(crate::t!("cockpit-refresh"), AgentInputButtonTheme)
                .with_size(ButtonSize::AgentInputButton)
                .on_click(|ctx| {
                    ctx.dispatch_typed_action(CockpitPanelAction::Refresh);
                })
        });
        let sort_button = ctx.add_typed_action_view(|_ctx| {
            ActionButton::new(sort_button_label(CockpitSort::default()), AgentInputButtonTheme)
                .with_size(ButtonSize::AgentInputButton)
                .on_click(|ctx| {
                    ctx.dispatch_typed_action(CockpitPanelAction::CycleSortMode);
                })
        });
        let group_button = ctx.add_typed_action_view(|_ctx| {
            ActionButton::new(
                group_button_label(CockpitGroupBy::default()),
                AgentInputButtonTheme,
            )
            .with_size(ButtonSize::AgentInputButton)
            .on_click(|ctx| {
                ctx.dispatch_typed_action(CockpitPanelAction::CycleGroupBy);
            })
        });
        let status_filter_button = ctx.add_typed_action_view(|_ctx| {
            ActionButton::new(status_filter_button_label(None), AgentInputButtonTheme)
                .with_size(ButtonSize::AgentInputButton)
                .on_click(|ctx| {
                    ctx.dispatch_typed_action(CockpitPanelAction::CycleStatusFilter);
                })
        });
        let clear_selection_button = ctx.add_typed_action_view(|_ctx| {
            ActionButton::new(crate::t!("cockpit-clear-selection"), AgentInputButtonTheme)
                .with_size(ButtonSize::AgentInputButton)
                .on_click(|ctx| {
                    ctx.dispatch_typed_action(CockpitPanelAction::ClearSelection);
                })
        });
        let inject_button = ctx.add_typed_action_view(|_ctx| {
            ActionButton::new(crate::t!("cockpit-inject"), AgentInputButtonTheme)
                .with_size(ButtonSize::AgentInputButton)
                .on_click(|ctx| {
                    ctx.dispatch_typed_action(CockpitPanelAction::BeginInjectionFromInput);
                })
        });
        let confirm_inject_button = ctx.add_typed_action_view(|_ctx| {
            ActionButton::new(crate::t!("cockpit-inject-confirm"), AgentInputButtonTheme)
                .with_size(ButtonSize::AgentInputButton)
                .on_click(|ctx| {
                    ctx.dispatch_typed_action(CockpitPanelAction::ConfirmInjection);
                })
        });
        let cancel_inject_button = ctx.add_typed_action_view(|_ctx| {
            ActionButton::new(crate::t!("common-cancel"), AgentInputButtonTheme)
                .with_size(ButtonSize::AgentInputButton)
                .on_click(|ctx| {
                    ctx.dispatch_typed_action(CockpitPanelAction::CancelInjection);
                })
        });

        // 文本筛选输入框:提交 → SetFilter(observatory 同款回填)。
        let filter_input = ctx.add_typed_action_view(|ctx| {
            let mut input = SubmittableTextInput::new(ctx)
                .validate_on_edit(|_| true)
                .with_allow_empty_submit();
            input.set_placeholder_text(crate::t!("cockpit-filter-placeholder"), ctx);
            input
        });
        let filter_editor_handle = filter_input.clone();
        ctx.subscribe_to_view(&filter_input, move |_me, _, event, ctx| {
            if let SubmittableTextInputEvent::Submit(content) = event {
                ctx.dispatch_typed_action(&CockpitPanelAction::SetFilter(content.clone()));
                filter_editor_handle.update(ctx, |input, ctx| {
                    let editor = input.editor().clone();
                    let text = content.clone();
                    editor.update(ctx, |ed, ctx| ed.set_buffer_text(&text, ctx));
                });
            }
        });

        // 注入文本输入框:提交(Enter)→ BeginInjection 进入确认态。
        let inject_input = ctx.add_typed_action_view(|ctx| {
            let mut input = SubmittableTextInput::new(ctx).validate_on_edit(|_| true);
            input.set_placeholder_text(crate::t!("cockpit-inject-placeholder"), ctx);
            input
        });
        let inject_editor_handle = inject_input.clone();
        ctx.subscribe_to_view(&inject_input, move |_me, _, event, ctx| {
            if let SubmittableTextInputEvent::Submit(content) = event {
                // 回填:确认态取消后用户可改文本重试。
                inject_editor_handle.update(ctx, |input, ctx| {
                    let editor = input.editor().clone();
                    let text = content.clone();
                    editor.update(ctx, |ed, ctx| ed.set_buffer_text(&text, ctx));
                });
                ctx.dispatch_typed_action(&CockpitPanelAction::BeginInjection(content.clone()));
            }
        });

        // 订阅 model 事件(六环 #5→#6:状态更新 → notify → rerender)。
        ctx.subscribe_to_model(&model, |_me, _handle, _event, ctx| {
            ctx.notify();
        });

        // P1 主刷新:agent 会话事件订阅(事件粒度反映;面板关闭时跳过)。
        // Started/StatusChanged/Ended/SessionUpdated 会改变卡片数据;
        // InputSessionChanged(富输入开合)不影响卡片,跳过以减少刷新抖动。
        // 订阅随本 view 生命周期:pane 关闭 → view Drop → 自动退订(防泄漏)。
        ctx.subscribe_to_model(&CLIAgentSessionsModel::handle(ctx), |_me, _h, event, ctx| {
            match event {
                CLIAgentSessionsModelEvent::Started { .. }
                | CLIAgentSessionsModelEvent::StatusChanged { .. }
                | CLIAgentSessionsModelEvent::Ended { .. }
                | CLIAgentSessionsModelEvent::SessionUpdated { .. } => {
                    if CockpitModel::handle(ctx).as_ref(ctx).panel_open() {
                        CockpitModel::handle(ctx).update(ctx, |m, ctx| crate::ai::cockpit::model::refresh_model(m, ctx));
                    }
                }
                CLIAgentSessionsModelEvent::InputSessionChanged { .. } => {}
            }
        });

        // cockpit-instant:非 agent 终端事件订阅(per-view 事件经全局
        // TerminalActivityModel 聚合推来)。StateChanged(Busy/Idle 转移)与
        // ViewMembershipChanged(列表成员:tab/pane 增删)零延迟即时刷;
        // OutputChanged(输出增长,高频)进 150ms 合并窗。
        // 订阅随本 view 生命周期:pane 关闭 → view Drop → 自动退订(防泄漏)。
        #[cfg(not(target_family = "wasm"))]
        ctx.subscribe_to_model(
            &crate::terminal::terminal_activity::TerminalActivityModel::handle(ctx),
            |me, _h, event, ctx| {
                if !CockpitModel::handle(ctx).as_ref(ctx).panel_open() {
                    return;
                }
                match event {
                    crate::terminal::terminal_activity::TerminalActivityEvent::StateChanged {
                        ..
                    }
                    | crate::terminal::terminal_activity::TerminalActivityEvent::ViewMembershipChanged => {
                        // 列表成员变化(卡片出现/消失)与状态转移同级——
                        // 零合并窗,即时全量刷新。
                        CockpitModel::handle(ctx).update(ctx, |m, ctx| crate::ai::cockpit::model::refresh_model(m, ctx));
                    }
                    crate::terminal::terminal_activity::TerminalActivityEvent::OutputChanged {
                        ..
                    } => {
                        me.schedule_output_coalesced_refresh(ctx);
                    }
                }
            },
        );

        let mut me = Self {
            model,
            refresh_button,
            sort_button,
            group_button,
            status_filter_button,
            clear_selection_button,
            inject_button,
            confirm_inject_button,
            cancel_inject_button,
            filter_input,
            inject_input,
            reconcile_timer_handle: None,
            output_coalesce_handle: None,
            card_handles: RefCell::new(Vec::new()),
            checkbox_handles: RefCell::new(Vec::new()),
            card_grid_scroll: ClippedScrollStateHandle::default(),
        };
        // 控制按钮初始标签对齐 model 现存状态(pane 重开时 model 状态保留)。
        me.sync_control_labels(ctx);
        // 首次启动 timer(warpui 陷阱#1:render 是 &self 无法启动,
        // start_reconcile_timer 只在回调内自续期,new() 末尾必须首调)。
        me.start_reconcile_timer(ctx);
        me
    }

    /// 控制按钮标签同步 model 视图参数(仅 new/Action 回调中调用,不在 render 改状态)。
    fn sync_control_labels(&mut self, ctx: &mut ViewContext<Self>) {
        let (sort, group_by, status_filter) = self.model.as_ref(ctx).read_view_params();
        self.sort_button.update(ctx, |button, ctx| {
            button.set_label(sort_button_label(sort), ctx);
        });
        self.group_button.update(ctx, |button, ctx| {
            button.set_label(group_button_label(group_by), ctx);
        });
        self.status_filter_button.update(ctx, |button, ctx| {
            button.set_label(status_filter_button_label(status_filter), ctx);
        });
    }

    /// 启动低频对账 timer(已在跑则 no-op;随视图 Drop 中止)。
    fn start_reconcile_timer(&mut self, ctx: &mut ViewContext<Self>) {
        if self.reconcile_timer_handle.is_some() {
            return;
        }
        let handle = ctx.spawn(
            async move {
                Timer::after(std::time::Duration::from_millis(
                    COCKPIT_RECONCILE_INTERVAL_MS,
                ))
                .await;
            },
            |me, _unit, ctx| {
                me.reconcile_timer_handle = None;
                // 面板关闭时跳过刷新(timer 空转一个 wake,开销可忽略)
                if !CockpitModel::handle(ctx).as_ref(ctx).panel_open() {
                    me.start_reconcile_timer(ctx);
                    return;
                }
                CockpitModel::handle(ctx).update(ctx, |model, ctx| {
                    crate::ai::cockpit::model::refresh_model(model, ctx);
                });
                me.start_reconcile_timer(ctx);
            },
        );
        self.reconcile_timer_handle = Some(handle);
    }

    /// cockpit-instant:OutputChanged 合并窗。窗口已开(计时器在跑)时
    /// 吸收事件不做事;窗口关闭时开一个 150ms 一次性计时器,窗到即单次
    /// refresh。固定窗(不做 trailing 重置)保证:事件风暴下刷新频率
    /// 上限 ~6.7 次/s,单事件延迟上界 150ms。
    fn schedule_output_coalesced_refresh(&mut self, ctx: &mut ViewContext<Self>) {
        if self.output_coalesce_handle.is_some() {
            return;
        }
        let handle = ctx.spawn(
            async move {
                Timer::after(std::time::Duration::from_millis(
                    OUTPUT_COALESCE_WINDOW_MS,
                ))
                .await;
            },
            |me, _unit, ctx| {
                me.output_coalesce_handle = None;
                if CockpitModel::handle(ctx).as_ref(ctx).panel_open() {
                    CockpitModel::handle(ctx).update(ctx, |m, ctx| crate::ai::cockpit::model::refresh_model(m, ctx));
                }
            },
        );
        self.output_coalesce_handle = Some(handle);
    }

    /// 聚焦筛选输入框(pane `focus_contents` 入口,observatory focus_search 同款)。
    pub fn focus_filter(&mut self, ctx: &mut ViewContext<Self>) {
        ctx.focus(&self.filter_input);
    }

    /// 确保卡片悬停句柄数量对齐。
    fn ensure_handles(handles: &mut Vec<MouseStateHandle>, target_len: usize) {
        while handles.len() < target_len {
            handles.push(MouseStateHandle::default());
        }
        handles.truncate(target_len);
    }

    /// 头部:标题 + 终端计数 + 控制(状态筛选/排序/分组/刷新)。
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
        row.add_child(warpui::elements::ChildView::new(&self.status_filter_button).finish());
        row.add_child(warpui::elements::ChildView::new(&self.sort_button).finish());
        row.add_child(warpui::elements::ChildView::new(&self.group_button).finish());
        row.add_child(warpui::elements::ChildView::new(&self.refresh_button).finish());

        Container::new(row.finish())
            .with_horizontal_padding(PANEL_PADDING)
            .with_vertical_padding(SPACING)
            .finish()
    }

    /// 筛选行:文本筛选输入框。
    fn render_filter_row(&self, _app: &AppContext) -> Box<dyn Element> {
        Container::new(warpui::elements::ChildView::new(&self.filter_input).finish())
            .with_horizontal_padding(PANEL_PADDING)
            .finish()
    }

    /// 占位文案。
    fn render_empty_state(
        &self,
        text: String,
        appearance: &Appearance,
        theme: &WarpTheme,
    ) -> Box<dyn Element> {
        Container::new(
            Text::new(
                text,
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

    /// 多选勾选框元素:accent 实心 = 选中;点击 → ToggleCardSelection。
    /// 关联 fn(非 &self):render_card 的渲染闭包内构造。
    fn checkbox_element(
        card_id: EntityId,
        checked: bool,
        handle: MouseStateHandle,
        theme: &WarpTheme,
    ) -> Box<dyn Element> {
        let background = if checked {
            Some(theme.accent())
        } else {
            None
        };
        let border_fill = theme.accent();
        let content = move || {
            let mut container = Container::new(Empty::new().finish())
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(3.)))
                .with_border(warpui::elements::Border::all(1.).with_border_fill(border_fill));
            if let Some(bg) = background {
                container = container.with_background(bg);
            }
            ConstrainedBox::new(container.finish())
                .with_width(CHECKBOX_SIZE)
                .with_height(CHECKBOX_SIZE)
                .finish()
        };
        Hoverable::new(handle, move |_state| content()).on_click(
            move |ctx, _app, _position| {
                // 勾选框是独立交互目标,与卡片主体点击(FocusCard)分开。
                ctx.dispatch_typed_action(CockpitPanelAction::ToggleCardSelection(card_id));
            },
        )
        .finish()
    }

    /// 单张卡片:勾选框 + 状态点 + 标题 / agent·branch·flags 行 / recap 行 /
    /// tool 行 / cwd+状态行。卡片主体点击 → `FocusCard`(跨 tab 聚焦)。
    #[allow(clippy::too_many_arguments)]
    fn render_card(
        &self,
        card: &CockpitCard,
        handle: MouseStateHandle,
        checkbox_handle: MouseStateHandle,
        checked: bool,
        focused_selected: bool,
        appearance: &Appearance,
        theme: &WarpTheme,
    ) -> Box<dyn Element> {
        let font_family = appearance.ui_font_family();
        let tag = card_tag(card.terminal_view_id);
        let title = truncate_str(&card.title, 30);
        // row1:agent 名(或 Shell)+ branch + 连接/只读标记。
        let mut meta_parts: Vec<String> = vec![match card.agent_name {
            Some(name) => name.to_string(),
            None => "Shell".to_string(),
        }];
        if let Some(branch) = &card.branch {
            meta_parts.push(format!(
                "{}{}",
                crate::t!("cockpit-branch-prefix"),
                truncate_str(branch, 20)
            ));
        }
        if card.connected {
            meta_parts.push(crate::t!("cockpit-badge-shared").to_string());
        }
        if !card.writable {
            meta_parts.push(crate::t!("cockpit-badge-readonly").to_string());
        }
        let agent_line = meta_parts.join(" · ");
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
        let border_color = if focused_selected {
            theme.accent()
        } else {
            theme.nonactive_ui_detail().into()
        };

        let content = move |hovered: bool| {
            let mut col = Flex::column()
                .with_main_axis_alignment(MainAxisAlignment::Start)
                .with_cross_axis_alignment(CrossAxisAlignment::Start)
                .with_spacing(4.);

            // row0: 勾选框 + 状态点 + 标题 + 右侧 tag
            let mut row0 = Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(SPACING);
            row0.add_child(Self::checkbox_element(
                card_id,
                checked,
                checkbox_handle,
                theme,
            ));
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

            // row1: agent 名(或 Shell)· branch · shared/readonly
            col.add_child(
                Text::new(agent_line.clone(), font_family, SMALL_FONT_SIZE)
                    .with_color(theme.sub_text_color(theme.background()).into())
                    .soft_wrap(false)
                    .finish(),
            );

            // row2: recap(response > query > summary > preview_tail 回退链)
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
            if hovered || focused_selected {
                container = container.with_background(theme.surface_overlay_1());
            }
            container.finish()
        };
        let hoverable = Hoverable::new(handle, move |state| content(state.is_hovered())).on_click(
            move |ctx, _app, _position| {
                ctx.dispatch_typed_action(CockpitPanelAction::FocusCard(card_id));
            },
        );

        ConstrainedBox::new(hoverable.finish())
            .with_width(CARD_WIDTH)
            .finish()
    }

    /// 卡片网格(按 groups 分节):每组标题 + Wrap 折行;外层纵向裁剪滚动。
    fn render_card_grid(&self, app: &AppContext) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        let theme = appearance.theme();
        let model = self.model.as_ref(app);
        let cards = model.cards();
        let selected = model.selected();
        let grouped = model.group_by() == CockpitGroupBy::CwdProject;

        Self::ensure_handles(&mut self.card_handles.borrow_mut(), cards.len());
        Self::ensure_handles(&mut self.checkbox_handles.borrow_mut(), cards.len());
        let card_handles = self.card_handles.borrow().clone();
        let checkbox_handles = self.checkbox_handles.borrow().clone();

        let mut sections = Flex::column()
            .with_main_axis_alignment(MainAxisAlignment::Start)
            .with_cross_axis_alignment(CrossAxisAlignment::Start)
            .with_spacing(SPACING);
        for group in model.groups() {
            if grouped {
                let count = group.range.len();
                let header = crate::t!(
                    "cockpit-group-header",
                    key = group.key.as_str(),
                    count = count
                );
                sections.add_child(
                    Container::new(
                        Text::new(header, appearance.ui_font_family(), SMALL_FONT_SIZE)
                            .with_color(theme.nonactive_ui_text_color().into_solid())
                            .soft_wrap(false)
                            .finish(),
                    )
                    .with_horizontal_padding(PANEL_PADDING)
                    .finish(),
                );
            }
            let mut grid = Wrap::row()
                .with_spacing(CARD_GAP)
                .with_run_spacing(CARD_GAP)
                .with_cross_axis_alignment(CrossAxisAlignment::Start);
            for idx in group.range.clone() {
                let (Some(card), Some(handle), Some(checkbox_handle)) = (
                    cards.get(idx),
                    card_handles.get(idx),
                    checkbox_handles.get(idx),
                ) else {
                    continue;
                };
                let checked = model.selected_set().contains(&card.terminal_view_id);
                grid.add_child(self.render_card(
                    card,
                    handle.clone(),
                    checkbox_handle.clone(),
                    checked,
                    selected == Some(card.terminal_view_id),
                    appearance,
                    theme,
                ));
            }
            sections.add_child(
                Container::new(grid.finish())
                    .with_horizontal_padding(PANEL_PADDING)
                    .finish(),
            );
        }

        // 必须复用 self.card_grid_scroll 字段:滚动偏移存句柄内部,
        // 每次 render 新建句柄会丢滚动位置(高频重渲染下列表滚回顶部)。
        ClippedScrollable::vertical(
            self.card_grid_scroll.clone(),
            Container::new(sections.finish())
                .with_vertical_padding(SPACING)
                .finish(),
            ScrollbarWidth::Auto,
            ElementFill::Solid(theme.nonactive_ui_detail().into()),
            ElementFill::Solid(theme.active_ui_detail().into()),
            ElementFill::None,
        )
        .finish()
    }

    /// 底部选中条:选中计数 + 清空 + 注入输入框 + 注入按钮。
    fn render_selection_bar(&self, app: &AppContext) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        let theme = appearance.theme();
        let count = self.model.as_ref(app).selected_set().len();

        let mut row = Flex::row()
            .with_main_axis_size(warpui::elements::MainAxisSize::Max)
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(SPACING);
        row.add_child(
            Text::new(
                crate::t!("cockpit-selected-count", count = count),
                appearance.ui_font_family(),
                SMALL_FONT_SIZE,
            )
            .with_color(theme.active_ui_text_color().into())
            .soft_wrap(false)
            .finish(),
        );
        row.add_child(warpui::elements::ChildView::new(&self.clear_selection_button).finish());
        row.add_child(Expanded::new(
            1.,
            warpui::elements::ChildView::new(&self.inject_input).finish(),
        ).finish());
        row.add_child(warpui::elements::ChildView::new(&self.inject_button).finish());

        Container::new(row.finish())
            .with_horizontal_padding(PANEL_PADDING)
            .with_vertical_padding(SPACING)
            .with_border(warpui::elements::Border::top(1.).with_border_fill(
                theme.nonactive_ui_detail(),
            ))
            .finish()
    }

    /// 注入确认对话框(列目标终端清单;Dialog + Dismiss 遮罩,
    /// destructive_mcp_confirmation_dialog 同款)。
    fn render_confirm_overlay(&self, app: &AppContext) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        let theme = appearance.theme();
        let model = self.model.as_ref(app);
        let Some(pending) = model.pending_injection() else {
            return Empty::new().finish();
        };

        // 目标清单:标题 + agent(可识别终端;目标已从卡片集消失时列 EntityId)。
        let cards = model.cards();
        let mut list_items = Vec::new();
        for id in &pending.target_ids {
            let line = match cards.iter().find(|c| c.terminal_view_id == *id) {
                Some(card) => format!(
                    "• {} ({})",
                    truncate_str(&card.title, 36),
                    card.agent_name.unwrap_or("Shell")
                ),
                None => format!("• {}", id),
            };
            list_items.push(line);
        }
        let list_text = list_items.join("\n");

        let mut list_col = Flex::column().with_spacing(2.);
        list_col.add_child(
            Text::new(truncate_str(&pending.text, 120), appearance.ui_font_family(), FONT_SIZE)
                .with_color(theme.main_text_color(theme.background()).into())
                .soft_wrap(true)
                .finish(),
        );
        list_col.add_child(
            Text::new(list_text, appearance.ui_font_family(), SMALL_FONT_SIZE)
                .with_color(theme.sub_text_color(theme.background()).into())
                .soft_wrap(false)
                .finish(),
        );

        let dialog = Dialog::new(
            crate::t!(
                "cockpit-inject-confirm-title",
                count = pending.target_ids.len()
            ),
            Some(crate::t!("cockpit-inject-confirm-body").to_string()),
            dialog_styles(appearance),
        )
        .with_child(list_col.finish())
        .with_bottom_row_child(warpui::elements::ChildView::new(&self.cancel_inject_button).finish())
        .with_bottom_row_child(
            Container::new(warpui::elements::ChildView::new(&self.confirm_inject_button).finish())
                .with_margin_left(12.)
                .finish(),
        )
        .with_width(CONFIRM_DIALOG_WIDTH)
        .build()
        .finish();

        Dismiss::new(dialog)
            .prevent_interaction_with_other_elements()
            .on_dismiss(|ctx, _app| {
                ctx.dispatch_typed_action(CockpitPanelAction::CancelInjection);
            })
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
    fn handle_action(&mut self, action: &Self::Action, ctx: &mut ViewContext<Self>) {
        match action {
            CockpitPanelAction::Refresh => {
                self.model.update(ctx, |model, ctx| {
                    crate::ai::cockpit::model::refresh_model(model, ctx);
                });
            }
            CockpitPanelAction::FocusCard(id) => {
                // 单选高亮同步 + 跨 tab/窗口聚焦(workspace 侧定位 pane:
                // 本 tab → activate_tab + focus_pane;他窗口 → show_window)。
                self.model.update(ctx, |model, ctx| {
                    model.select_card(Some(*id), ctx);
                });
                ctx.dispatch_typed_action(&WorkspaceAction::FocusTerminalViewInWorkspace {
                    terminal_view_id: *id,
                });
            }
            CockpitPanelAction::ToggleCardSelection(id) => {
                self.model.update(ctx, |model, ctx| {
                    model.toggle_card_selection(*id, ctx);
                });
            }
            CockpitPanelAction::ClearSelection => {
                self.model.update(ctx, |model, ctx| {
                    model.clear_selection(ctx);
                });
            }
            CockpitPanelAction::SetFilter(filter) => {
                self.model.update(ctx, |model, ctx| {
                    model.set_filter(filter.clone(), ctx);
                });
            }
            CockpitPanelAction::CycleStatusFilter => {
                let next = CockpitStatusFilter::cycle(self.model.as_ref(ctx).status_filter());
                self.model.update(ctx, |model, ctx| {
                    model.set_status_filter(next, ctx);
                });
                self.sync_control_labels(ctx);
            }
            CockpitPanelAction::CycleSortMode => {
                let next = self.model.as_ref(ctx).sort().cycle();
                self.model.update(ctx, |model, ctx| {
                    model.set_sort(next, ctx);
                });
                self.sync_control_labels(ctx);
            }
            CockpitPanelAction::CycleGroupBy => {
                let next = self.model.as_ref(ctx).group_by().cycle();
                self.model.update(ctx, |model, ctx| {
                    model.set_group_by(next, ctx);
                });
                self.sync_control_labels(ctx);
            }
            CockpitPanelAction::BeginInjection(text) => {
                self.model.update(ctx, |model, ctx| {
                    model.begin_injection(text.clone(), ctx);
                });
            }
            CockpitPanelAction::BeginInjectionFromInput => {
                let text = self.inject_input.read(ctx, |input, ctx| input.editor().as_ref(ctx).buffer_text(ctx));
                self.model.update(ctx, |model, ctx| {
                    model.begin_injection(text.clone(), ctx);
                });
            }
            CockpitPanelAction::ConfirmInjection => {
                self.model.update(ctx, |model, ctx| {
                    crate::ai::cockpit::model::confirm_injection_model(model, ctx);
                });
                // 文本已发送,清空注入输入框。
                self.inject_input.update(ctx, |input, ctx| {
                    let editor = input.editor().clone();
                    editor.update(ctx, |ed, ctx| ed.clear_buffer(ctx));
                });
            }
            CockpitPanelAction::CancelInjection => {
                self.model.update(ctx, |model, ctx| {
                    model.cancel_injection(ctx);
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
        let model = self.model.as_ref(app);
        let has_cards = !model.cards().is_empty();
        let has_selection = !model.selected_set().is_empty();

        let mut col = Flex::column()
            .with_main_axis_alignment(MainAxisAlignment::Start)
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_spacing(SPACING);

        col.add_child(self.render_header(app));
        col.add_child(self.render_filter_row(app));
        if has_cards {
            col.add_child(Box::new(warpui::elements::Expanded::new(
                1.,
                self.render_card_grid(app),
            )));
        } else {
            // 无卡区分两态:无终端 / 有终端但被筛选全部排除。
            let text = if model.all_card_count() == 0 {
                crate::t!("cockpit-empty")
            } else {
                crate::t!("cockpit-no-match")
            };
            col.add_child(Box::new(warpui::elements::Expanded::new(
                1.,
                self.render_empty_state(text, appearance, theme),
            )));
        }
        if has_selection && model.pending_injection().is_none() {
            col.add_child(self.render_selection_bar(app));
        }

        let mut stack = Stack::new();
        stack.add_child(
            Container::new(col.finish())
                .with_background(theme.background())
                .finish(),
        );
        if model.pending_injection().is_some() {
            stack.add_positioned_overlay_child(
                self.render_confirm_overlay(app),
                OffsetPositioning::offset_from_parent(
                    vec2f(0., 0.),
                    warpui::elements::ParentOffsetBounds::Unbounded,
                    ParentAnchor::Center,
                    ChildAnchor::Center,
                ),
            );
        }
        stack.finish()
    }
}

// ── 控制按钮标签 ──────────────────────────────────────────────────────────────

fn sort_button_label(sort: CockpitSort) -> String {
    match sort {
        CockpitSort::Activity => crate::t!("cockpit-sort-activity"),
        CockpitSort::Title => crate::t!("cockpit-sort-title"),
        CockpitSort::Cwd => crate::t!("cockpit-sort-cwd"),
    }
    .to_string()
}

fn group_button_label(group_by: CockpitGroupBy) -> String {
    match group_by {
        CockpitGroupBy::None => crate::t!("cockpit-group-none"),
        CockpitGroupBy::CwdProject => crate::t!("cockpit-group-project"),
    }
    .to_string()
}

fn status_filter_button_label(kind: Option<CockpitStatusFilter>) -> String {
    match kind {
        None => crate::t!("cockpit-filter-status-all"),
        Some(CockpitStatusFilter::Working) => crate::t!("cockpit-filter-status-working"),
        Some(CockpitStatusFilter::Done) => crate::t!("cockpit-filter-status-done"),
        Some(CockpitStatusFilter::Blocked) => crate::t!("cockpit-filter-status-blocked"),
        Some(CockpitStatusFilter::Busy) => crate::t!("cockpit-filter-status-busy"),
        Some(CockpitStatusFilter::Idle) => crate::t!("cockpit-filter-status-idle"),
    }
    .to_string()
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

    #[test]
    fn cycle_actions_advance_from_current_mode() {
        // 循环按钮 Cycle* 的语义:从当前模式推进(model 侧 cycle),
        // 非"从默认值出发" — pane 重开后仍正确。
        assert_eq!(CockpitSort::Title.cycle(), CockpitSort::Cwd);
        assert_eq!(CockpitGroupBy::CwdProject.cycle(), CockpitGroupBy::None);
        assert_eq!(
            CockpitStatusFilter::cycle(Some(CockpitStatusFilter::Idle)),
            None
        );
    }
}
