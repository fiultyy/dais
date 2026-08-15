//! 观测台面板视图 — ObservatoryPanelView
//!
//! 全部用户交互经 `ModelHandle<ObservatoryModel>`（业务状态）或
//! `InterceptSessionsModel`（代理配置单例）派发，视图不持有业务状态，
//! 仅维护渲染缓存（鼠标悬停句柄、子输入框句柄等纯 UI 状态）。

use std::cell::RefCell;
use warpui::r#async::SpawnedFutureHandle;
use warpui::r#async::Timer;
use warpui::elements::{
    Border, ChildView, ConstrainedBox, Container, CornerRadius, CrossAxisAlignment, Empty,
    Expanded, Flex, Hoverable, MainAxisSize, MainAxisAlignment, MouseStateHandle, ParentElement,
    Shrinkable, Text,
};
use warpui::scene::Radius;
use warpui::{
    AppContext, Element, Entity, ModelHandle, SingletonEntity, TypedActionView, View, ViewContext,
    ViewHandle,
};

use warp_core::ui::appearance::Appearance;
use warp_core::ui::theme::color::internal_colors;
use warp_core::ui::theme::Fill;
use warp_core::ui::theme::WarpTheme;
use warpui::color::ColorU;


use harness_integration::{HarnessType, InterceptMode};

use crate::view_components::action_button::{ActionButton, ButtonSize};
use crate::view_components::{SubmittableTextInput, SubmittableTextInputEvent};
use crate::ai::blocklist::agent_view::agent_input_footer::AgentInputButtonTheme;
use crate::terminal::intercept_sessions::InterceptSessionsModel;
use super::model::{
    format_datetime_sqlite, ActiveInterceptRowGui, BlockDetailGui, BlockRowGui, DraftField,
    MessageDetailGui, ObservatoryModel, ObservatoryTab, RawDetailGui, RawRowGui, RunRowGui,
    SessionRowGui, TaskRowGui,
};

// ── 布局常量 ─────────────────────────────────────────────────────────────────

/// 面板内边距。
const PANEL_PADDING: f32 = 12.;
/// 元素间距。
const SPACING: f32 = 6.;
/// 区块间距（tab 切换行与内容之间）。
const SECTION_SPACING: f32 = 10.;
/// Tab 按钮水平内边距。
const TAB_H_PADDING: f32 = 12.;
/// Tab 按钮垂直内边距。
const TAB_V_PADDING: f32 = 6.;
/// Session 行垂直内边距。
const ROW_V_PADDING: f32 = 6.;
/// Composer 输入框间距。
const COMPOSER_SPACING: f32 = 8.;
/// Block type badge 角半径。
const BADGE_RADIUS: f32 = 4.;
/// 小号字体（详情/辅助文本）。
const SMALL_FONT_SIZE: f32 = 12.;

/// 观测台周期自动刷新间隔（ms）。
const OBSERVATORY_REFRESH_INTERVAL_MS: u64 = 5_000;


// ── Action ────────────────────────────────────────────────────────────────────

/// 面板视图的 typed action，由 on_click 分发、handle_action 处理。
#[derive(Clone, Debug)]
pub enum ObservatoryPanelAction {
    Refresh,
    /// 派发当前选中的 task（读取 model.selected_task）。
    DispatchSelectedTask,
    /// 用输入框内容解决当前选中的 gate。
    ResolveSelectedGate(String),
    SendMessage,
    SetTab(ObservatoryTab),
    SelectSession(Option<String>),
    SetSearch(String),
    SelectBlock(Option<String>),
    SelectTask(Option<String>),
    /// 选中 run（composer 发送目标）。
    SelectRun(Option<String>),
    DispatchTask(String),
    SelectGate(Option<String>),
    SelectRaw(Option<String>),
    /// 选中消息（sequence PK）加载详情。
    SelectMessage(Option<i64>),
    ResolveGate(String, String),
    /// 代理配置：切换拦截模式。
    SetInterceptMode(InterceptMode),
    /// 代理配置：设置 upstream base 覆盖（空 = 自动探测）。
    SetUpstreamBase(String),
    /// 代理配置：设置 auth env var 覆盖。
    SetUpstreamAuthEnv(String),
    /// 代理配置：重查 block 计数。
    RefreshBlockCount,
}

// ── ObservatoryPanelView ────────────────────────────────────────────────────

/// 观测台面板视图。只渲染，不持有业务状态。
pub struct ObservatoryPanelView {
    /// 单例模型句柄。
    model: ModelHandle<ObservatoryModel>,
    /// 刷新按钮。
    refresh_button: ViewHandle<ActionButton>,
    /// Session 行悬停状态句柄列表（按 snapshot.sessions 长度缓存）。
    session_row_handles: RefCell<Vec<MouseStateHandle>>,
    /// Block 行悬停状态句柄列表。
    block_row_handles: RefCell<Vec<MouseStateHandle>>,
    /// Tab 切换行的鼠标句柄：[Sessions, Orchestration, Proxy]。
    tab_handles: [MouseStateHandle; 3],
    /// Composer: To 输入框。
    draft_to_input: ViewHandle<SubmittableTextInput>,
    /// Composer: Subject 输入框。
    draft_subject_input: ViewHandle<SubmittableTextInput>,
    /// Composer: Body 输入框。
    draft_body_input: ViewHandle<SubmittableTextInput>,
    /// 发送按钮。
    send_button: ViewHandle<ActionButton>,
    /// Message 行悬停状态句柄列表。
    message_row_handles: RefCell<Vec<MouseStateHandle>>,
    /// Task 行悬停状态句柄列表。
    task_row_handles: RefCell<Vec<MouseStateHandle>>,
    /// Run 行悬停状态句柄列表（composer 目标选中）。
    run_row_handles: RefCell<Vec<MouseStateHandle>>,
    /// Gate 行悬停状态句柄列表。
    gate_row_handles: RefCell<Vec<MouseStateHandle>>,
    /// Gate 选项 chip 悬停句柄列表（所有 gate 的 options 扁平展开）。
    gate_option_handles: RefCell<Vec<MouseStateHandle>>,
    /// Raw 流量行悬停状态句柄列表。
    raw_row_handles: RefCell<Vec<MouseStateHandle>>,
    /// 搜索框。
    search_input: ViewHandle<SubmittableTextInput>,
    /// 代理 tab: gate 自定义 resolution 输入框。
    gate_resolution_input: ViewHandle<SubmittableTextInput>,
    /// 代理 tab: mode 选项 chip 句柄（Full/HooksOnly/Bypass）。
    mode_chip_handles: [MouseStateHandle; 3],
    /// 代理 tab: upstream base 输入框。
    upstream_base_input: ViewHandle<SubmittableTextInput>,
    /// 代理 tab: auth env 输入框。
    upstream_auth_env_input: ViewHandle<SubmittableTextInput>,
    /// 任务派发按钮。
    dispatch_button: ViewHandle<ActionButton>,
    /// 代理 tab: 刷新 block 计数按钮。
    refresh_count_button: ViewHandle<ActionButton>,
    /// 周期自动刷新 timer 句柄。Drop 时中止。
    refresh_timer_handle: Option<SpawnedFutureHandle>,
    /// 上一帧 busy 状态（渲染缓存：检测 send 完成边沿，同步 composer 输入框）。
    prev_busy: std::cell::Cell<bool>,
}

impl ObservatoryPanelView {
    pub fn new(model: ModelHandle<ObservatoryModel>, ctx: &mut ViewContext<Self>) -> Self {
        // 刷新按钮
        let refresh_button = ctx.add_typed_action_view(|_ctx| {
            ActionButton::new(crate::t!("observatory-refresh"), AgentInputButtonTheme)
                .with_size(ButtonSize::AgentInputButton)
                .on_click(|ctx| {
                    ctx.dispatch_typed_action(ObservatoryPanelAction::Refresh);
                })
        });

        // Composer 输入框
        let draft_to_input = ctx.add_typed_action_view(|ctx| {
            let mut input = SubmittableTextInput::new(ctx)
                .validate_on_edit(|_| true)
                .with_allow_empty_submit();
            input.set_placeholder_text(crate::t!("observatory-send-to"), ctx);
            input
        });
        let draft_subject_input = ctx.add_typed_action_view(|ctx| {
            let mut input = SubmittableTextInput::new(ctx)
                .validate_on_edit(|_| true)
                .with_allow_empty_submit();
            input.set_placeholder_text(crate::t!("observatory-send-subject"), ctx);
            input
        });
        let draft_body_input = ctx.add_typed_action_view(|ctx| {
            let mut input = SubmittableTextInput::new(ctx)
                .validate_on_edit(|_| true)
                .with_allow_empty_submit();
            input.set_placeholder_text(crate::t!("observatory-send-body"), ctx);
            input
        });

        // 订阅 composer 提交事件 → 写入 model draft
        let to_input = draft_to_input.clone();
        let subject_input = draft_subject_input.clone();
        let body_input = draft_body_input.clone();
        ctx.subscribe_to_view(&draft_to_input, move |_me, _, event, ctx| {
            if let SubmittableTextInputEvent::Submit(content) = event {
                ObservatoryModel::handle(ctx).update(ctx, |model, ctx| {
                    model.set_draft(DraftField::To, content.clone(), ctx);
                });
                // 回填已存 draft（可见反馈；发送后才清空）
                to_input.update(ctx, |input, ctx| {
                    let editor = input.editor().clone();
                    let text = content.clone();
                    editor.update(ctx, |ed, ctx| ed.set_buffer_text(&text, ctx));
                });
            }
        });
        ctx.subscribe_to_view(&draft_subject_input, move |_me, _, event, ctx| {
            if let SubmittableTextInputEvent::Submit(content) = event {
                ObservatoryModel::handle(ctx).update(ctx, |model, ctx| {
                    model.set_draft(DraftField::Subject, content.clone(), ctx);
                });
                subject_input.update(ctx, |input, ctx| {
                    let editor = input.editor().clone();
                    let text = content.clone();
                    editor.update(ctx, |ed, ctx| ed.set_buffer_text(&text, ctx));
                });
            }
        });
        ctx.subscribe_to_view(&draft_body_input, move |_me, _, event, ctx| {
            if let SubmittableTextInputEvent::Submit(content) = event {
                ObservatoryModel::handle(ctx).update(ctx, |model, ctx| {
                    model.set_draft(DraftField::Body, content.clone(), ctx);
                });
                body_input.update(ctx, |input, ctx| {
                    let editor = input.editor().clone();
                    let text = content.clone();
                    editor.update(ctx, |ed, ctx| ed.set_buffer_text(&text, ctx));
                });
            }
        });

        // 发送按钮
        let send_button = ctx.add_typed_action_view(|_ctx| {
            ActionButton::new(crate::t!("observatory-send"), AgentInputButtonTheme)
                .with_size(ButtonSize::AgentInputButton)
                .on_click(|ctx| {
                    ctx.dispatch_typed_action(ObservatoryPanelAction::SendMessage);
                })
        });

        // 搜索框：提交 → SetSearch
        let search_input = ctx.add_typed_action_view(|ctx| {
            let mut input = SubmittableTextInput::new(ctx)
                .validate_on_edit(|_| true)
                .with_allow_empty_submit();
            input.set_placeholder_text(crate::t!("observatory-search-placeholder"), ctx);
            input
        });
        let search_handle = search_input.clone();
        ctx.subscribe_to_view(&search_input, move |_me, _, event, ctx| {
            if let SubmittableTextInputEvent::Submit(content) = event {
                ctx.dispatch_typed_action(&ObservatoryPanelAction::SetSearch(content.clone()));
                // 搜索词回填到框内，便于继续编辑
                search_handle.update(ctx, |input, ctx| {
                    let editor = input.editor().clone();
                    let text = content.clone();
                    editor.update(ctx, |ed, ctx| ed.set_buffer_text(&text, ctx));
                });
            }
        });

        // Gate 自定义 resolution 输入框：提交 → ResolveGate(选中 gate, draft)
        let gate_resolution_input = ctx.add_typed_action_view(|ctx| {
            let mut input = SubmittableTextInput::new(ctx)
                .validate_on_edit(|_| true)
                .with_allow_empty_submit();
            input.set_placeholder_text(crate::t!("observatory-gate-resolve"), ctx);
            input
        });
        let gate_input_handle = gate_resolution_input.clone();
        ctx.subscribe_to_view(&gate_resolution_input, move |_me, _, event, ctx| {
            if let SubmittableTextInputEvent::Submit(content) = event {
                // 草稿写入 model；提交的 content 作为自定义 resolution
                ObservatoryModel::handle(ctx).update(ctx, |model, ctx| {
                    model.set_gate_draft(content.clone(), ctx);
                });
                ctx.dispatch_typed_action(&ObservatoryPanelAction::ResolveSelectedGate(
                    content.clone(),
                ));
                gate_input_handle.update(ctx, |input, ctx| {
                    let editor = input.editor().clone();
                    editor.update(ctx, |ed, ctx| ed.set_buffer_text("", ctx));
                });
            }
        });

        // 代理 tab: upstream 输入框
        let upstream_base_input = ctx.add_typed_action_view(|ctx| {
            let mut input = SubmittableTextInput::new(ctx)
                .validate_on_edit(|_| true)
                .with_allow_empty_submit();
            input.set_placeholder_text(crate::t!("observatory-proxy-base"), ctx);
            input
        });
        let base_handle = upstream_base_input.clone();
        ctx.subscribe_to_view(&upstream_base_input, move |_me, _, event, ctx| {
            if let SubmittableTextInputEvent::Submit(content) = event {
                ctx.dispatch_typed_action(&ObservatoryPanelAction::SetUpstreamBase(content.clone()));
                base_handle.update(ctx, |input, ctx| {
                    let editor = input.editor().clone();
                    let text = content.clone();
                    editor.update(ctx, |ed, ctx| ed.set_buffer_text(&text, ctx));
                });
            }
        });
        let upstream_auth_env_input = ctx.add_typed_action_view(|ctx| {
            let mut input = SubmittableTextInput::new(ctx)
                .validate_on_edit(|_| true)
                .with_allow_empty_submit();
            input.set_placeholder_text(crate::t!("observatory-proxy-auth-env"), ctx);
            input
        });
        let auth_env_handle = upstream_auth_env_input.clone();
        ctx.subscribe_to_view(&upstream_auth_env_input, move |_me, _, event, ctx| {
            if let SubmittableTextInputEvent::Submit(content) = event {
                ctx.dispatch_typed_action(&ObservatoryPanelAction::SetUpstreamAuthEnv(content.clone()));
                auth_env_handle.update(ctx, |input, ctx| {
                    let editor = input.editor().clone();
                    let text = content.clone();
                    editor.update(ctx, |ed, ctx| ed.set_buffer_text(&text, ctx));
                });
            }
        });

        // 任务派发按钮（选中 task 由 handle_action 侧读取）
        let dispatch_button = ctx.add_typed_action_view(|_ctx| {
            ActionButton::new(crate::t!("observatory-task-dispatch"), AgentInputButtonTheme)
                .with_size(ButtonSize::AgentInputButton)
                .on_click(|ctx| {
                    ctx.dispatch_typed_action(ObservatoryPanelAction::DispatchSelectedTask);
                })
        });

        // 代理 tab: 刷新 block 计数按钮
        let refresh_count_button = ctx.add_typed_action_view(|_ctx| {
            ActionButton::new(crate::t!("observatory-proxy-refresh-count"), AgentInputButtonTheme)
                .with_size(ButtonSize::AgentInputButton)
                .on_click(|ctx| {
                    ctx.dispatch_typed_action(ObservatoryPanelAction::RefreshBlockCount);
                })
        });

        // 订阅 model 事件 → 重绘；busy true→false 边沿（send 完成）时
        // 将 composer 输入框同步为 model 当前 draft（成功路径 body 已清）。
        ctx.subscribe_to_model(&model, |me, handle, _event, ctx| {
            let busy = handle.as_ref(ctx).busy();
            let was_busy = me.prev_busy.replace(busy);
            if was_busy && !busy {
                let to = handle.as_ref(ctx).draft_to().to_string();
                let subject = handle.as_ref(ctx).draft_subject().to_string();
                let body = handle.as_ref(ctx).draft_body().to_string();
                me.draft_to_input.update(ctx, |input, ctx| {
                    let editor = input.editor().clone();
                    editor.update(ctx, |ed, ctx| ed.set_buffer_text(&to, ctx));
                });
                me.draft_subject_input.update(ctx, |input, ctx| {
                    let editor = input.editor().clone();
                    editor.update(ctx, |ed, ctx| ed.set_buffer_text(&subject, ctx));
                });
                me.draft_body_input.update(ctx, |input, ctx| {
                    let editor = input.editor().clone();
                    editor.update(ctx, |ed, ctx| ed.set_buffer_text(&body, ctx));
                });
            }
            ctx.notify();
        });
        // 订阅拦截配置单例变化（代理 tab 展示）→ 重绘
        ctx.subscribe_to_model(
            &InterceptSessionsModel::handle(ctx),
            |_me, _handle, _event, ctx| {
                ctx.notify();
            },
        );

        let mut me = Self {
            model,
            refresh_button,
            session_row_handles: RefCell::new(Vec::new()),
            block_row_handles: RefCell::new(Vec::new()),
            tab_handles: [
                MouseStateHandle::default(),
                MouseStateHandle::default(),
                MouseStateHandle::default(),
            ],
            draft_to_input,
            draft_subject_input,
            draft_body_input,
            send_button,
            message_row_handles: RefCell::new(Vec::new()),
            task_row_handles: RefCell::new(Vec::new()),
            run_row_handles: RefCell::new(Vec::new()),
            gate_row_handles: RefCell::new(Vec::new()),
            gate_option_handles: RefCell::new(Vec::new()),
            raw_row_handles: RefCell::new(Vec::new()),
            search_input,
            gate_resolution_input,
            mode_chip_handles: [
                MouseStateHandle::default(),
                MouseStateHandle::default(),
                MouseStateHandle::default(),
            ],
            upstream_base_input,
            upstream_auth_env_input,
            dispatch_button,
            refresh_count_button,
            refresh_timer_handle: None,
            prev_busy: std::cell::Cell::new(false),
        };
        me
    }

    /// 启动周期自动刷新 timer（已在跑则 no-op；随视图 Drop 中止）。
    fn start_refresh_timer(&mut self, ctx: &mut ViewContext<Self>) {
        if self.refresh_timer_handle.is_some() {
            return;
        }
        let handle = ctx.spawn(
            async move {
                Timer::after(std::time::Duration::from_millis(
                    OBSERVATORY_REFRESH_INTERVAL_MS,
                ))
                .await;
            },
            |me, _unit, ctx| {
                me.refresh_timer_handle = None;
                // flag 中途关闭时停止续期
                if !crate::features::FeatureFlag::AgentHarness.is_enabled() {
                    return;
                }
                // 面板关闭时跳过 DB 轮询（timer 空转一个 wake，开销可忽略）
                if !ObservatoryModel::handle(ctx).as_ref(ctx).panel_open() {
                    me.start_refresh_timer(ctx);
                    return;
                }
                ObservatoryModel::handle(ctx).update(ctx, |model, ctx| {
                    model.refresh_auto(ctx);
                });
                InterceptSessionsModel::handle(ctx).update(ctx, |model, ctx| {
                    model.refresh_block_count(ctx);
                });
                me.start_refresh_timer(ctx);
            },
        );
        self.refresh_timer_handle = Some(handle);
    }

    // ── 渲染子方法 ──────────────────────────────────────────────────────────

    /// 确保鼠标悬停句柄数量匹配数据行数。
    fn ensure_handles(handles: &mut Vec<MouseStateHandle>, target_len: usize) {
        while handles.len() < target_len {
            handles.push(MouseStateHandle::default());
        }
        handles.truncate(target_len);
    }

    /// 头部: 标题 + mode + block 计数 + 刷新按钮。
    fn render_header(&self, app: &AppContext) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        let theme = appearance.theme();
        let model = self.model.as_ref(app);

        let title_text = crate::t!("observatory-title");
        let mode_label = match model.mode(app) {
            InterceptMode::Full => crate::t!("intercept-mode-full"),
            InterceptMode::HooksOnly => crate::t!("intercept-mode-hooks-only"),
            InterceptMode::Bypass => crate::t!("intercept-mode-bypass"),
        };
        let count = model.block_count_total(app);
        let blocks_text = crate::t!("observatory-blocks-captured", count = count);

        let mut row = Flex::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(SPACING);

        row.add_child(
            Text::new(title_text, appearance.ui_font_family(), appearance.ui_font_size())
                .with_color(theme.active_ui_text_color().into())
                .finish(),
        );
        row.add_child(
            Text::new(mode_label, appearance.ui_font_family(), appearance.ui_font_size())
                .with_color(theme.nonactive_ui_text_color().into_solid())
                .finish(),
        );
        row.add_child(
            Text::new(blocks_text, appearance.ui_font_family(), appearance.ui_font_size())
                .with_color(theme.nonactive_ui_text_color().into_solid())
                .finish(),
        );
        row.add_child(Expanded::new(1., Empty::new().finish()).finish());
        row.add_child(ChildView::new(&self.refresh_button).finish());

        Container::new(row.finish())
            .with_horizontal_padding(PANEL_PADDING)
            .with_vertical_padding(SPACING)
            .finish()
    }

    /// Tab 切换行: Sessions / Orchestration / Proxy（可点击文字标签）。
    fn render_tab_bar(&self, app: &AppContext) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        let theme = appearance.theme();
        let active_tab = self.model.as_ref(app).active_tab();

        let mut row = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(SPACING);

        let tabs: [(ObservatoryTab, String, MouseStateHandle); 3] = [
            (
                ObservatoryTab::Sessions,
                crate::t!("observatory-tab-sessions"),
                self.tab_handles[0].clone(),
            ),
            (
                ObservatoryTab::Orchestration,
                crate::t!("observatory-tab-orchestration"),
                self.tab_handles[1].clone(),
            ),
            (
                ObservatoryTab::Proxy,
                crate::t!("observatory-tab-proxy"),
                self.tab_handles[2].clone(),
            ),
        ];
        for (tab, label, handle) in tabs {
            let is_active = active_tab == tab;
            let tab_to_set = tab;
            let hoverable = Hoverable::new(handle, move |state| {
                let text_color = if is_active {
                    theme.active_ui_text_color().into()
                } else if state.is_hovered() {
                    theme.nonactive_ui_text_color().into()
                } else {
                    theme.disabled_ui_text_color().into_solid()
                };
                let mut container = Container::new(
                    Text::new(label.clone(), appearance.ui_font_family(), appearance.ui_font_size())
                        .with_color(text_color)
                        .finish(),
                )
                .with_horizontal_padding(TAB_H_PADDING)
                .with_vertical_padding(TAB_V_PADDING);
                if is_active {
                    container =
                        container.with_border(Border::bottom(2.).with_border_fill(theme.accent()));
                }
                container.finish()
            })
            .on_click(move |ctx, _, _| {
                ctx.dispatch_typed_action(ObservatoryPanelAction::SetTab(tab_to_set));
            })
            .finish();
            row.add_child(hoverable);
        }

        Container::new(row.finish())
            .with_horizontal_padding(PANEL_PADDING)
            .finish()
    }

    fn render_sessions_tab(&self, app: &AppContext) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        let theme = appearance.theme();
        let model = self.model.as_ref(app);
        let snapshot = model.snapshot();

        let mut col = Flex::column()
            .with_main_axis_size(MainAxisSize::Max)
            .with_spacing(SECTION_SPACING);

        // ── 搜索框 ──
        col.add_child(
            Container::new(ChildView::new(&self.search_input).finish())
                .with_horizontal_padding(PANEL_PADDING)
                .finish(),
        );

        // ── 会话列表 ──
        if snapshot.sessions.is_empty() {
            col.add_child(self.render_empty_state(
                &crate::t!("observatory-sessions-empty"),
                appearance,
                theme,
            ));
        } else {
            Self::ensure_handles(&mut self.session_row_handles.borrow_mut(), snapshot.sessions.len());
            let handles = self.session_row_handles.borrow();
            for (i, session) in snapshot.sessions.iter().enumerate() {
                let is_selected = model
                    .selected_session()
                    .is_some_and(|s| s == session.session_id);
                let handle = handles[i].clone();
                let session_id = session.session_id.clone();

                let inner = self.render_session_row(session, is_selected, appearance, theme);
                let hoverable = Hoverable::new(handle, move |state| {
                    let mut container = Container::new(inner)
                        .with_horizontal_padding(PANEL_PADDING)
                        .with_vertical_padding(ROW_V_PADDING);
                    if is_selected {
                        container = container.with_background(internal_colors::fg_overlay_1(theme));
                    } else if state.is_hovered() {
                        container = container.with_background(Fill::Solid(internal_colors::neutral_3(theme)));
                    }
                    container.finish()
                })
        .on_click(move |ctx, _, _| {
                    ctx.dispatch_typed_action(ObservatoryPanelAction::SelectSession(Some(session_id.clone())));
                })
                .finish();
                col.add_child(hoverable);
            }
        }

        // ── Block 时间线（选中 session 时展示） ──
        if model.selected_session().is_some() {
            if snapshot.blocks.is_empty() {
                col.add_child(self.render_empty_state(
                    &crate::t!("observatory-blocks-empty"),
                    appearance,
                    theme,
                ));
            } else {
                Self::ensure_handles(&mut self.block_row_handles.borrow_mut(), snapshot.blocks.len());
                let handles = self.block_row_handles.borrow();
                for (i, block) in snapshot.blocks.iter().enumerate() {
                    let handle = handles[i].clone();
                    let is_selected = model
                        .selected_block()
                        .is_some_and(|b| b == block.id);
                    let block_id = block.id.clone();
                    let block_el = self.render_block_row(block, is_selected, appearance, theme);
                    let hoverable = Hoverable::new(handle, move |state| {
                        let mut container = Container::new(block_el)
                            .with_horizontal_padding(PANEL_PADDING)
                            .with_vertical_padding(ROW_V_PADDING);
                        if is_selected {
                            container = container.with_background(internal_colors::fg_overlay_1(theme));
                        } else if state.is_hovered() {
                            container = container.with_background(Fill::Solid(internal_colors::neutral_3(theme)));
                        }
                        container.finish()
                    })
                    .on_click(move |ctx, _, _| {
                        ctx.dispatch_typed_action(ObservatoryPanelAction::SelectBlock(Some(block_id.clone())));
                    })
                    .finish();
                    col.add_child(hoverable);
                }
            }
        }

        // ── Block 详情（选中 block 时展示） ──
        if let Some(detail) = model.block_detail() {
            col.add_child(self.render_block_detail(detail, appearance, theme));
        }

        // ── Raw 代理流量（选中 session 时展示） ──
        if model.selected_session().is_some() {
            col.add_child(self.render_raw_section(app));
        }

        Shrinkable::new(1., col.finish()).finish()
    }

    /// Raw 代理流量列表（时间升序）+ 选中详情卡片。
    fn render_raw_section(&self, app: &AppContext) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        let theme = appearance.theme();
        let model = self.model.as_ref(app);
        let snapshot = model.snapshot();

        let mut col = Flex::column()
            .with_main_axis_size(MainAxisSize::Max)
            .with_spacing(SPACING);

        col.add_child(
            Text::new(
                crate::t!("observatory-raw-title"),
                appearance.ui_font_family(),
                appearance.ui_font_size(),
            )
            .with_color(theme.active_ui_text_color().into())
            .finish(),
        );

        if snapshot.raw_entries.is_empty() {
            col.add_child(
                Text::new(
                    crate::t!("observatory-raw-empty"),
                    appearance.ui_font_family(),
                    SMALL_FONT_SIZE,
                )
                .with_color(theme.disabled_ui_text_color().into_solid())
                .finish(),
            );
        } else {
            Self::ensure_handles(
                &mut self.raw_row_handles.borrow_mut(),
                snapshot.raw_entries.len(),
            );
            let handles = self.raw_row_handles.borrow();
            for (i, entry) in snapshot.raw_entries.iter().enumerate() {
                let handle = handles[i].clone();
                let is_selected = model.selected_raw().is_some_and(|r| r == entry.id);
                let raw_id = entry.id.clone();
                let row_el = self.render_raw_row(entry, appearance, theme);
                let hoverable = Hoverable::new(handle, move |state| {
                    let mut container = Container::new(row_el)
                        .with_horizontal_padding(PANEL_PADDING)
                        .with_vertical_padding(ROW_V_PADDING);
                    if is_selected {
                        container = container.with_background(internal_colors::fg_overlay_1(theme));
                    } else if state.is_hovered() {
                        container =
                            container.with_background(Fill::Solid(internal_colors::neutral_3(theme)));
                    }
                    container.finish()
                })
                .on_click(move |ctx, _, _| {
                    ctx.dispatch_typed_action(ObservatoryPanelAction::SelectRaw(Some(raw_id.clone())));
                })
                .finish();
                col.add_child(hoverable);
            }
        }

        // 选中 raw 的详情卡片
        if let Some(detail) = model.raw_detail() {
            col.add_child(self.render_raw_detail(detail, appearance, theme));
        }

        Container::new(col.finish())
            .with_vertical_padding(SECTION_SPACING / 2.)
            .finish()
    }

    /// 单行 raw 流量条目。
    fn render_raw_row(
        &self,
        entry: &RawRowGui,
        appearance: &Appearance,
        theme: &WarpTheme,
    ) -> Box<dyn Element> {
        let is_request = entry.direction == "request";
        let dir_color = if is_request {
            theme.accent().into_solid()
        } else {
            theme.nonactive_ui_text_color().into_solid()
        };
        let size_text = format!("{}B", entry.content_len);
        let time_text = format_timestamp(entry.timestamp);
        let preview = truncate_str(&entry.preview, 60);

        let mut row = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(SPACING);

        let badge = Container::new(
            Text::new(
                entry.direction.clone(),
                appearance.ui_font_family(),
                appearance.ui_font_size(),
            )
            .with_color(dir_color)
            .finish(),
        )
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(BADGE_RADIUS)))
        .with_horizontal_padding(4.)
        .with_vertical_padding(2.)
        .finish();
        row.add_child(badge);
        row.add_child(
            Text::new(size_text, appearance.ui_font_family(), appearance.ui_font_size())
                .with_color(theme.disabled_ui_text_color().into_solid())
                .finish(),
        );
        row.add_child(Expanded::new(1., Empty::new().finish()).finish());
        row.add_child(
            Text::new(preview, appearance.ui_font_family(), appearance.ui_font_size())
                .with_color(theme.nonactive_ui_text_color().into_solid())
                .soft_wrap(false)
                .finish(),
        );
        row.add_child(
            Text::new(time_text, appearance.ui_font_family(), SMALL_FONT_SIZE)
                .with_color(theme.disabled_ui_text_color().into_solid())
                .finish(),
        );
        row.finish()
    }

    /// Raw 载荷详情卡片。
    fn render_raw_detail(
        &self,
        detail: &RawDetailGui,
        appearance: &Appearance,
        theme: &WarpTheme,
    ) -> Box<dyn Element> {
        let meta_text = crate::t!(
            "observatory-raw-detail-meta",
            direction = detail.direction.clone(),
            len = detail.content_len,
            ts = format_timestamp(detail.timestamp),
        );

        let mut col = Flex::column()
            .with_main_axis_size(MainAxisSize::Max)
            .with_spacing(SPACING);

        col.add_child(
            Text::new(
                crate::t!("observatory-raw-detail-title"),
                appearance.ui_font_family(),
                appearance.ui_font_size(),
            )
            .with_color(theme.active_ui_text_color().into())
            .finish(),
        );
        col.add_child(
            Text::new(meta_text, appearance.ui_font_family(), SMALL_FONT_SIZE)
                .with_color(theme.sub_text_color(theme.background()).into())
                .finish(),
        );
        col.add_child(
            Text::new(
                crate::t!("observatory-raw-detail-content"),
                appearance.ui_font_family(),
                SMALL_FONT_SIZE,
            )
            .with_color(theme.nonactive_ui_text_color().into_solid())
            .finish(),
        );
        col.add_child(
            Text::new(truncate_str(&detail.content, 16000), appearance.ui_font_family(), SMALL_FONT_SIZE)
                .with_color(theme.sub_text_color(theme.background()).into())
                .finish(),
        );

        Container::new(col.finish())
            .with_horizontal_padding(PANEL_PADDING)
            .with_vertical_padding(SPACING)
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(BADGE_RADIUS)))
            .with_background(Fill::Solid(internal_colors::neutral_2(theme)))
            .finish()
    }

    /// 选中消息详情卡片：from/to/subject 元信息 + type/priority + body 全文。
    fn render_message_detail(
        &self,
        detail: &MessageDetailGui,
        appearance: &Appearance,
        theme: &WarpTheme,
    ) -> Box<dyn Element> {
        let meta_text = crate::t!(
            "observatory-message-detail-meta",
            from = detail.from_handle.clone(),
            to = detail.to_handle.clone(),
            ts = format_datetime_sqlite(&detail.created_at),
        );
        let type_text = crate::t!(
            "observatory-message-detail-kind",
            kind = detail.message_type.clone(),
            priority = detail.priority.clone(),
        );

        let mut col = Flex::column()
            .with_main_axis_size(MainAxisSize::Max)
            .with_spacing(SPACING);

        col.add_child(
            Text::new(
                crate::t!("observatory-message-detail-title"),
                appearance.ui_font_family(),
                appearance.ui_font_size(),
            )
            .with_color(theme.active_ui_text_color().into())
            .finish(),
        );
        col.add_child(
            Text::new(format!("{} · {}", detail.subject, meta_text), appearance.ui_font_family(), SMALL_FONT_SIZE)
                .with_color(theme.sub_text_color(theme.background()).into())
                .finish(),
        );
        col.add_child(
            Text::new(type_text, appearance.ui_font_family(), SMALL_FONT_SIZE)
                .with_color(theme.nonactive_ui_text_color().into_solid())
                .finish(),
        );
        col.add_child(
            Text::new(
                crate::t!("observatory-message-detail-body"),
                appearance.ui_font_family(),
                SMALL_FONT_SIZE,
            )
            .with_color(theme.nonactive_ui_text_color().into_solid())
            .finish(),
        );
        col.add_child(
            Text::new(truncate_str(&detail.body, 16000), appearance.ui_font_family(), SMALL_FONT_SIZE)
                .with_color(theme.sub_text_color(theme.background()).into())
                .finish(),
        );

        Container::new(col.finish())
            .with_horizontal_padding(PANEL_PADDING)
            .with_vertical_padding(SPACING)
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(BADGE_RADIUS)))
            .with_background(Fill::Solid(internal_colors::neutral_2(theme)))
            .finish()
    }

    /// 单行 session 渲染（无悬停/边框包裹——由调用者 Hoverable 负责）。
    fn render_session_row(
        &self,
        session: &SessionRowGui,
        is_selected: bool,
        appearance: &Appearance,
        theme: &WarpTheme,
    ) -> Box<dyn Element> {
        let display_id = truncate_str(&session.session_id, 16);
        let count_text = format!("{} blocks", session.block_count);
        let time_text = format_timestamp(session.last_ts);

        let mut row = Flex::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(SPACING);

        row.add_child(
            Text::new(display_id, appearance.ui_font_family(), appearance.ui_font_size())
                .with_color(theme.main_text_color(theme.background()).into())
                .soft_wrap(false)
                .finish(),
        );
        row.add_child(Expanded::new(1., Empty::new().finish()).finish());
        row.add_child(
            Text::new(count_text, appearance.ui_font_family(), appearance.ui_font_size())
                .with_color(theme.sub_text_color(theme.background()).into())
                .finish(),
        );
        row.add_child(
            Text::new(time_text, appearance.ui_font_family(), appearance.ui_font_size())
                .with_color(theme.disabled_ui_text_color().into_solid())
                .finish(),
        );

        if is_selected {
            Container::new(row.finish())
                .with_border(Border::left(2.).with_border_fill(theme.accent()))
                .finish()
        } else {
            row.finish()
        }
    }

    /// 单行 block 时间线条目渲染。
    fn render_block_row(
        &self,
        block: &BlockRowGui,
        is_selected: bool,
        appearance: &Appearance,
        theme: &WarpTheme,
    ) -> Box<dyn Element> {
        let badge_color = block_type_color(&block.block_type, theme);
        let seq_text = crate::t!("observatory-block-seq", seq = block.sequence);
        let content_text = format!("{}B", block.content_len);
        let preview = truncate_str(&block.preview, 80);

        let mut row = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(SPACING);

        // block_type badge
        let badge = Container::new(
            Text::new(
                block.block_type.clone(),
                appearance.ui_font_family(),
                appearance.ui_font_size(),
            )
            .with_color(badge_color)
            .finish(),
        )
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(BADGE_RADIUS)))
        .with_horizontal_padding(4.)
        .with_vertical_padding(2.)
        .finish();
        row.add_child(badge);

        row.add_child(
            Text::new(seq_text, appearance.ui_font_family(), appearance.ui_font_size())
                .with_color(theme.sub_text_color(theme.background()).into())
                .finish(),
        );
        row.add_child(
            Text::new(content_text, appearance.ui_font_family(), appearance.ui_font_size())
                .with_color(theme.disabled_ui_text_color().into_solid())
                .finish(),
        );

        // 预览占满剩余空间，截断
        row.add_child(Expanded::new(1., Empty::new().finish()).finish());
        row.add_child(
            Text::new(preview, appearance.ui_font_family(), appearance.ui_font_size())
                .with_color(theme.nonactive_ui_text_color().into_solid())
                .soft_wrap(false)
                .finish(),
        );

        if is_selected {
            Container::new(row.finish())
                .with_border(Border::left(2.).with_border_fill(theme.accent()))
                .finish()
        } else {
            row.finish()
        }
    }

    /// Block 详情卡片：元信息 + metadata + content 全文（换行渲染）。
    fn render_block_detail(
        &self,
        detail: &BlockDetailGui,
        appearance: &Appearance,
        theme: &WarpTheme,
    ) -> Box<dyn Element> {
        let meta_text = crate::t!(
            "observatory-block-detail-meta",
            block_type = detail.block_type.clone(),
            seq = detail.sequence,
            len = detail.content_len,
            ts = format_timestamp(detail.timestamp),
        );

        let mut col = Flex::column()
            .with_main_axis_size(MainAxisSize::Max)
            .with_spacing(SPACING);

        // 标题行：标题 + 关闭提示（点击已选 block 行即可取消选中）
        let mut title_row = Flex::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(SPACING);
        title_row.add_child(
            Text::new(
                crate::t!("observatory-block-detail-title"),
                appearance.ui_font_family(),
                appearance.ui_font_size(),
            )
            .with_color(theme.active_ui_text_color().into())
            .finish(),
        );
        title_row.add_child(Expanded::new(1., Empty::new().finish()).finish());
        title_row.add_child(
            Text::new(
                crate::t!("observatory-block-detail-close"),
                appearance.ui_font_family(),
                SMALL_FONT_SIZE,
            )
            .with_color(theme.disabled_ui_text_color().into_solid())
            .finish(),
        );
        col.add_child(title_row.finish());

        col.add_child(
            Text::new(meta_text, appearance.ui_font_family(), SMALL_FONT_SIZE)
                .with_color(theme.sub_text_color(theme.background()).into())
                .finish(),
        );

        if let Some(parent) = &detail.parent_id {
            col.add_child(
                Text::new(
                    crate::t!("observatory-block-detail-parent", parent = parent.clone()),
                    appearance.ui_font_family(),
                    SMALL_FONT_SIZE,
                )
                .with_color(theme.sub_text_color(theme.background()).into())
                .finish(),
            );
        }

        // Metadata
        col.add_child(
            Text::new(
                crate::t!("observatory-block-detail-metadata"),
                appearance.ui_font_family(),
                SMALL_FONT_SIZE,
            )
            .with_color(theme.nonactive_ui_text_color().into_solid())
            .finish(),
        );
        col.add_child(
            Text::new(truncate_str(&detail.metadata, 4000), appearance.ui_font_family(), SMALL_FONT_SIZE)
                .with_color(theme.sub_text_color(theme.background()).into())
                .finish(),
        );

        // Content
        col.add_child(
            Text::new(
                crate::t!("observatory-block-detail-content"),
                appearance.ui_font_family(),
                SMALL_FONT_SIZE,
            )
            .with_color(theme.nonactive_ui_text_color().into_solid())
            .finish(),
        );
        col.add_child(
            Text::new(truncate_str(&detail.content, 16000), appearance.ui_font_family(), SMALL_FONT_SIZE)
                .with_color(theme.sub_text_color(theme.background()).into())
                .finish(),
        );

        Container::new(col.finish())
            .with_horizontal_padding(PANEL_PADDING)
            .with_vertical_padding(SPACING)
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(BADGE_RADIUS)))
            .with_background(Fill::Solid(internal_colors::neutral_2(theme)))
            .finish()
    }

    /// Orchestration tab 内容: runs + tasks + gates + messages + composer。
    fn render_orchestration_tab(&self, app: &AppContext) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        let theme = appearance.theme();
        let model = self.model.as_ref(app);
        let snapshot = model.snapshot();

        let mut col = Flex::column()
            .with_main_axis_size(MainAxisSize::Max)
            .with_spacing(SECTION_SPACING);

        // ── Runs + Tasks（task 行可点击选中） ──
        if snapshot.runs.is_empty() {
            col.add_child(self.render_empty_state(
                &crate::t!("observatory-runs-empty"),
                appearance,
                theme,
            ));
        } else {
            let selected_task = model.selected_task();
            let selected_run = model.selected_run();
            Self::ensure_handles(
                &mut self.run_row_handles.borrow_mut(),
                snapshot.runs.len(),
            );
            let run_handles = self.run_row_handles.borrow();
            let mut task_idx = 0usize;
            for (ri, run) in snapshot.runs.iter().enumerate() {
                let run_tasks: Vec<&TaskRowGui> = snapshot
                    .tasks
                    .iter()
                    .filter(|t| t.run_id == run.id)
                    .collect();
                // 预增长句柄池
                Self::ensure_handles(
                    &mut self.task_row_handles.borrow_mut(),
                    task_idx + run_tasks.len(),
                );
                let handles = self.task_row_handles.borrow();
                let rows: Vec<(usize, &TaskRowGui)> =
                    run_tasks.into_iter().enumerate().map(|(i, t)| (task_idx + i, t)).collect();
                task_idx += rows.len();
                col.add_child(self.render_run_entry(
                    run,
                    &rows,
                    selected_task,
                    selected_run,
                    run_handles[ri].clone(),
                    &handles,
                    appearance,
                    theme,
                ));
            }
        }

        // ── 选中 task 详情 + 派发 ──
        col.add_child(self.render_task_panel(app));

        // ── Pending gates ──
        col.add_child(self.render_gates_section(app));

        // ── 最近 Messages ──
        if !snapshot.recent_messages.is_empty() {
            Self::ensure_handles(
                &mut self.message_row_handles.borrow_mut(),
                snapshot.recent_messages.len(),
            );
            let handles = self.message_row_handles.borrow();
            for (i, msg) in snapshot.recent_messages.iter().enumerate() {
                let handle = handles[i].clone();
                let is_selected = model
                    .selected_message()
                    .is_some_and(|s| s == msg.seq);
                let msg_text = format!("{} → {}: {}", msg.from_handle, msg.to_handle, msg.subject);
                let seq = msg.seq;
                let hoverable = Hoverable::new(handle, move |state| {
                    let mut container = Container::new(
                        Text::new(
                            msg_text.clone(),
                            appearance.ui_font_family(),
                            appearance.ui_font_size(),
                        )
                        .with_color(theme.sub_text_color(theme.background()).into())
                        .soft_wrap(false)
                        .finish(),
                    )
                    .with_horizontal_padding(PANEL_PADDING)
                    .with_vertical_padding(ROW_V_PADDING);
                    if is_selected {
                        container = container.with_background(internal_colors::fg_overlay_1(theme));
                    } else if state.is_hovered() {
                        container = container.with_background(Fill::Solid(internal_colors::neutral_3(theme)));
                    }
                    container.finish()
                })
                .on_click(move |ctx, _, _| {
                    ctx.dispatch_typed_action(ObservatoryPanelAction::SelectMessage(Some(seq)));
                })
                .finish();
                col.add_child(hoverable);
            }
        }

        // ── 选中消息详情卡片 ──
        if let Some(detail) = model.message_detail() {
            col.add_child(self.render_message_detail(detail, appearance, theme));
        }

        // ── Worker 终端输出归档（最新 5） ──
        if !snapshot.archives.is_empty() {
            col.add_child(
                Text::new(
                    crate::t!("observatory-archives-title"),
                    appearance.ui_font_family(),
                    SMALL_FONT_SIZE,
                )
                .with_color(theme.nonactive_ui_text_color().into_solid())
                .finish(),
            );
            for archive in &snapshot.archives {
                col.add_child(
                    Text::new(
                        crate::t!(
                            "observatory-archive-meta",
                            id = truncate_str(&archive.dispatch_id, 22),
                            kind = archive.kind.clone(),
                            time = archive.created_at.clone(),
                        ),
                        appearance.ui_font_family(),
                        SMALL_FONT_SIZE,
                    )
                    .with_color(theme.disabled_ui_text_color().into_solid())
                    .soft_wrap(false)
                    .finish(),
                );
                // tail 语义：只显示最后 3 行
                for line in archive.lines.iter().rev().take(3).rev() {
                    col.add_child(
                        Text::new(
                            truncate_str(line, 120),
                            appearance.ui_font_family(),
                            SMALL_FONT_SIZE,
                        )
                        .with_color(theme.sub_text_color(theme.background()).into())
                        .soft_wrap(false)
                        .finish(),
                    );
                }
            }
        }
        // ── Composer ──
        col.add_child(self.render_composer(app));

        Shrinkable::new(1., col.finish()).finish()
    }
    /// 单个 run 及其下属 tasks 渲染（run/task 行 Hoverable + 点击选中）。
    #[allow(clippy::too_many_arguments)]
    fn render_run_entry(
        &self,
        run: &RunRowGui,
        run_tasks: &[(usize, &TaskRowGui)],
        selected_task: Option<&str>,
        selected_run: Option<&str>,
        run_handle: MouseStateHandle,
        handles: &[MouseStateHandle],
        appearance: &Appearance,
        theme: &WarpTheme,
    ) -> Box<dyn Element> {
        let objective = truncate_str(&run.objective, 40);
        let created_at = &run.created_at;
        let is_run_selected = selected_run.is_some_and(|s| s == run.id);
        let run_id = run.id.clone();

        let mut col = Flex::column()
            .with_main_axis_size(MainAxisSize::Max)
            .with_spacing(SPACING / 2.);

        // Run header（可点击选中 → composer 目标）
        let mut run_row = Flex::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(SPACING);
        run_row.add_child(
            Text::new(objective, appearance.ui_font_family(), appearance.ui_font_size())
                .with_color(theme.main_text_color(theme.background()).into())
                .soft_wrap(false)
                .finish(),
        );
        run_row.add_child(Expanded::new(1., Empty::new().finish()).finish());
        run_row.add_child(
            Text::new(created_at.clone(), appearance.ui_font_family(), appearance.ui_font_size())
                .with_color(theme.disabled_ui_text_color().into_solid())
                .finish(),
        );
        let run_header_row = run_row.finish();
        let run_header = Hoverable::new(run_handle, move |state| {
            let mut container = Container::new(run_header_row)
                .with_horizontal_padding(PANEL_PADDING)
                .with_vertical_padding(ROW_V_PADDING);
            if is_run_selected {
                container = container.with_background(internal_colors::fg_overlay_1(theme));
            } else if state.is_hovered() {
                container = container.with_background(Fill::Solid(internal_colors::neutral_3(theme)));
            }
            container.finish()
        })
        .on_click(move |ctx, _, _| {
            ctx.dispatch_typed_action(ObservatoryPanelAction::SelectRun(Some(run_id.clone())));
        })
        .finish();
        col.add_child(run_header);

        // 嵌套 tasks（可点击选中）
        for (handle_idx, task) in run_tasks {
            let handle = handles[*handle_idx].clone();
            let is_selected = selected_task.is_some_and(|s| s == task.id);
            let task_id = task.id.clone();
            let status_color = task_status_color(&task.status);
            let title = truncate_str(&task.title, 36);

            let mut task_row = Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(SPACING);
            // 状态色点
            let dot = Container::new(
                ConstrainedBox::new(Empty::new().finish())
                    .with_width(6.)
                    .with_height(6.)
                    .finish(),
            )
            .with_background(status_color.clone())
            .finish();
            task_row.add_child(dot);
            task_row.add_child(
                Text::new(title, appearance.ui_font_family(), appearance.ui_font_size())
                    .with_color(theme.sub_text_color(theme.background()).into())
                    .soft_wrap(false)
                    .finish(),
            );
            task_row.add_child(
                Text::new(
                    task.status.clone(),
                    appearance.ui_font_family(),
                    appearance.ui_font_size(),
                )
                .with_color(status_color.clone().into_solid())
                .finish(),
            );
            let task_row = task_row.finish();

            let hoverable = Hoverable::new(handle, move |state| {
                let mut container = Container::new(task_row)
                    .with_margin_left(PANEL_PADDING + 8.)
                    .with_horizontal_padding(4.)
                    .with_vertical_padding(2.);
                if is_selected {
                    container = container.with_background(internal_colors::fg_overlay_1(theme));
                } else if state.is_hovered() {
                    container =
                        container.with_background(Fill::Solid(internal_colors::neutral_3(theme)));
                }
                container.finish()
            })
            .on_click(move |ctx, _, _| {
                ctx.dispatch_typed_action(ObservatoryPanelAction::SelectTask(Some(task_id.clone())));
            })
            .finish();
            col.add_child(hoverable);
        }

        Container::new(col.finish()).finish()
    }

    /// 选中 task 的详情面板：id/status + 派发按钮 + 最近 dispatch 反馈。
    fn render_task_panel(&self, app: &AppContext) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        let theme = appearance.theme();
        let model = self.model.as_ref(app);
        let snapshot = model.snapshot();

        let mut col = Flex::column()
            .with_main_axis_size(MainAxisSize::Max)
            .with_spacing(SPACING);

        col.add_child(
            Text::new(
                crate::t!("observatory-task-panel-title"),
                appearance.ui_font_family(),
                appearance.ui_font_size(),
            )
            .with_color(theme.active_ui_text_color().into())
            .finish(),
        );

        let selected_task = model
            .selected_task()
            .and_then(|id| snapshot.tasks.iter().find(|t| t.id == id));

        if let Some(task) = selected_task {
            col.add_child(
                Text::new(
                    format!("{} · {}", truncate_str(&task.id, 24), task.status),
                    appearance.ui_font_family(),
                    SMALL_FONT_SIZE,
                )
                .with_color(theme.sub_text_color(theme.background()).into())
                .soft_wrap(false)
                .finish(),
            );
            // 任务规格
            if !task.spec.is_empty() {
                col.add_child(
                    Text::new(
                        crate::t!("observatory-task-spec"),
                        appearance.ui_font_family(),
                        SMALL_FONT_SIZE,
                    )
                    .with_color(theme.nonactive_ui_text_color().into_solid())
                    .finish(),
                );
                col.add_child(
                    Text::new(truncate_str(&task.spec, 4000), appearance.ui_font_family(), SMALL_FONT_SIZE)
                        .with_color(theme.sub_text_color(theme.background()).into())
                        .finish(),
                );
            }
            // 依赖（非空时）
            if !task.deps.is_empty() && task.deps != "[]" {
                col.add_child(
                    Text::new(
                        crate::t!("observatory-task-deps", deps = task.deps.clone()),
                        appearance.ui_font_family(),
                        SMALL_FONT_SIZE,
                    )
                    .with_color(theme.disabled_ui_text_color().into_solid())
                    .soft_wrap(false)
                    .finish(),
                );
            }
            // 结果（完成后展示）
            if let Some(result) = &task.result {
                col.add_child(
                    Text::new(
                        crate::t!("observatory-task-result"),
                        appearance.ui_font_family(),
                        SMALL_FONT_SIZE,
                    )
                    .with_color(theme.nonactive_ui_text_color().into_solid())
                    .finish(),
                );
                col.add_child(
                    Text::new(truncate_str(result, 16000), appearance.ui_font_family(), SMALL_FONT_SIZE)
                        .with_color(theme.sub_text_color(theme.background()).into())
                        .finish(),
                );
            }
            col.add_child(ChildView::new(&self.dispatch_button).finish());
        } else {
            col.add_child(
                Text::new(
                    crate::t!("observatory-task-no-selection"),
                    appearance.ui_font_family(),
                    SMALL_FONT_SIZE,
                )
                .with_color(theme.disabled_ui_text_color().into_solid())
                .finish(),
            );
        }

        if let Some(dispatch_id) = model.last_dispatch() {
            col.add_child(
                Text::new(
                    crate::t!("observatory-task-dispatched", id = dispatch_id),
                    appearance.ui_font_family(),
                    SMALL_FONT_SIZE,
                )
                .with_color(theme.accent().into_solid())
                .finish(),
            );
        }

        // ── 该 task 的 dispatch 明细（最新 20） ──
        col.add_child(
            Text::new(
                crate::t!("observatory-dispatches-title"),
                appearance.ui_font_family(),
                SMALL_FONT_SIZE,
            )
            .with_color(theme.nonactive_ui_text_color().into_solid())
            .finish(),
        );
        if snapshot.dispatches.is_empty() {
            col.add_child(
                Text::new(
                    crate::t!("observatory-dispatches-empty"),
                    appearance.ui_font_family(),
                    SMALL_FONT_SIZE,
                )
                .with_color(theme.disabled_ui_text_color().into_solid())
                .finish(),
            );
        } else {
            for d in &snapshot.dispatches {
                let status_color = task_status_color(&d.status);
                let mut row = Flex::row()
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_spacing(SPACING);
                row.add_child(
                    Text::new(
                        truncate_str(&d.dispatch_id, 20),
                        appearance.ui_font_family(),
                        SMALL_FONT_SIZE,
                    )
                    .with_color(theme.sub_text_color(theme.background()).into())
                    .soft_wrap(false)
                    .finish(),
                );
                row.add_child(
                    Text::new(
                        format!("{}/{}", d.status, if d.state.is_empty() { "-" } else { &d.state }),
                        appearance.ui_font_family(),
                        SMALL_FONT_SIZE,
                    )
                    .with_color(status_color.into_solid())
                    .finish(),
                );
                row.add_child(Expanded::new(1., Empty::new().finish()).finish());
                row.add_child(
                    Text::new(
                        d.created_at.clone(),
                        appearance.ui_font_family(),
                        SMALL_FONT_SIZE,
                    )
                    .with_color(theme.disabled_ui_text_color().into_solid())
                    .finish(),
                );
                col.add_child(row.finish());
            }
        }

        Container::new(col.finish())
            .with_horizontal_padding(PANEL_PADDING)
            .with_vertical_padding(SPACING)
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(BADGE_RADIUS)))
            .with_background(Fill::Solid(internal_colors::neutral_2(theme)))
            .finish()
    }

    /// Pending gates 列表：选中 gate → 选项 chip 一键解决 / 自定义 resolution。
    fn render_gates_section(&self, app: &AppContext) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        let theme = appearance.theme();
        let model = self.model.as_ref(app);
        let snapshot = model.snapshot();

        let mut col = Flex::column()
            .with_main_axis_size(MainAxisSize::Max)
            .with_spacing(SPACING);

        col.add_child(
            Text::new(
                crate::t!("observatory-gates-title"),
                appearance.ui_font_family(),
                appearance.ui_font_size(),
            )
            .with_color(theme.active_ui_text_color().into())
            .finish(),
        );

        if snapshot.gates.is_empty() {
            col.add_child(
                Text::new(
                    crate::t!("observatory-gates-empty"),
                    appearance.ui_font_family(),
                    SMALL_FONT_SIZE,
                )
                .with_color(theme.disabled_ui_text_color().into_solid())
                .finish(),
            );
        } else {
            Self::ensure_handles(
                &mut self.gate_row_handles.borrow_mut(),
                snapshot.gates.len(),
            );
            // 选项 chip 句柄扁平展开
            let total_options: usize = snapshot.gates.iter().map(|g| g.options.len()).sum();
            Self::ensure_handles(
                &mut self.gate_option_handles.borrow_mut(),
                total_options,
            );
            let row_handles = self.gate_row_handles.borrow();
            let option_handles = self.gate_option_handles.borrow();
            let mut option_idx = 0usize;

            for (i, gate) in snapshot.gates.iter().enumerate() {
                let is_selected = model.selected_gate().is_some_and(|g| g == gate.id);
                let gate_id = gate.id.clone();
                let question = truncate_str(&gate.question, 80);
                let created = &gate.created_at;

                let mut gate_col = Flex::column().with_spacing(SPACING / 2.);
                let mut header = Flex::row()
                    .with_main_axis_size(MainAxisSize::Max)
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_spacing(SPACING);
                header.add_child(
                    Text::new(question.clone(), appearance.ui_font_family(), appearance.ui_font_size())
                        .with_color(theme.main_text_color(theme.background()).into())
                        .soft_wrap(false)
                        .finish(),
                );
                header.add_child(Expanded::new(1., Empty::new().finish()).finish());
                header.add_child(
                    Text::new(created.clone(), appearance.ui_font_family(), SMALL_FONT_SIZE)
                        .with_color(theme.disabled_ui_text_color().into_solid())
                        .finish(),
                );
                gate_col.add_child(header.finish());

                // 选项 chips（点击 → ResolveGate(gate_id, option)）
                let mut chip_row = Flex::row()
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_spacing(SPACING);
                for option in &gate.options {
                    let handle = option_handles[option_idx].clone();
                    option_idx += 1;
                    let gate_id_for_chip = gate_id.clone();
                    let option_text = option.clone();
                    let option_text_click = option.clone();
                    let chip = Hoverable::new(handle, move |state| {
                        let mut container = Container::new(
                            Text::new(
                                option_text.clone(),
                                appearance.ui_font_family(),
                                SMALL_FONT_SIZE,
                            )
                            .with_color(theme.active_ui_text_color().into())
                            .finish(),
                        )
                        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(BADGE_RADIUS)))
                        .with_horizontal_padding(8.)
                        .with_vertical_padding(3.)
                        .with_border(Border::all(1.).with_border_fill(theme.accent()));
                        if state.is_hovered() {
                            container =
                                container.with_background(Fill::Solid(internal_colors::neutral_3(theme)));
                        }
                        container.finish()
                    })
                    .on_click(move |ctx, _, _| {
                        ctx.dispatch_typed_action(ObservatoryPanelAction::ResolveGate(
                            gate_id_for_chip.clone(),
                            option_text_click.clone(),
                        ));
                    })
                    .finish();
                    chip_row.add_child(chip);
                }
                if !gate.options.is_empty() {
                    gate_col.add_child(chip_row.finish());
                }

                let inner = gate_col.finish();
                let hoverable = Hoverable::new(row_handles[i].clone(), move |state| {
                    let mut container = Container::new(inner)
                        .with_horizontal_padding(PANEL_PADDING)
                        .with_vertical_padding(ROW_V_PADDING);
                    if is_selected {
                        container = container.with_background(internal_colors::fg_overlay_1(theme));
                    } else if state.is_hovered() {
                        container =
                            container.with_background(Fill::Solid(internal_colors::neutral_3(theme)));
                    }
                    container.finish()
                })
                .on_click(move |ctx, _, _| {
                    ctx.dispatch_typed_action(ObservatoryPanelAction::SelectGate(Some(gate_id.clone())));
                })
                .finish();
                col.add_child(hoverable);
            }

            // 选中 gate 的自定义 resolution 输入
            if model.selected_gate().is_some() {
                col.add_child(
                    Container::new(ChildView::new(&self.gate_resolution_input).finish())
                        .with_horizontal_padding(PANEL_PADDING)
                        .finish(),
                );
            }
        }

        // ── 最近已决 gates（决策历史：resolution + 时间） ──
        if !snapshot.resolved_gates.is_empty() {
            col.add_child(
                Text::new(
                    crate::t!("observatory-gates-resolved-title"),
                    appearance.ui_font_family(),
                    SMALL_FONT_SIZE,
                )
                .with_color(theme.nonactive_ui_text_color().into_solid())
                .finish(),
            );
            for gate in &snapshot.resolved_gates {
                let resolution = gate
                    .resolution
                    .clone()
                    .unwrap_or_else(|| "-".to_string());
                let time = gate
                    .resolved_at
                    .as_deref()
                    .map(format_datetime_sqlite)
                    .unwrap_or_else(|| "-".to_string());
                col.add_child(
                    Text::new(
                        crate::t!(
                            "observatory-gate-resolved-row",
                            status = gate.status.clone(),
                            resolution = truncate_str(&resolution, 40),
                            time = time,
                        ),
                        appearance.ui_font_family(),
                        SMALL_FONT_SIZE,
                    )
                    .with_color(theme.disabled_ui_text_color().into_solid())
                    .soft_wrap(false)
                    .finish(),
                );
            }
        }

        Container::new(col.finish())
            .with_horizontal_padding(0.)
            .finish()
    }

    /// Proxy tab：透明代理配置（模式 chips + upstream 覆盖 + 解析探测 + 计数刷新）。
    fn render_proxy_tab(&self, app: &AppContext) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        let theme = appearance.theme();
        let intercept = InterceptSessionsModel::as_ref(app);
        let current_mode = intercept.mode();

        let mut col = Flex::column()
            .with_main_axis_size(MainAxisSize::Max)
            .with_spacing(SECTION_SPACING);

        // ── 拦截模式 chips ──
        col.add_child(
            Text::new(
                crate::t!("observatory-proxy-mode"),
                appearance.ui_font_family(),
                appearance.ui_font_size(),
            )
            .with_color(theme.active_ui_text_color().into())
            .finish(),
        );

        let modes: [(InterceptMode, String, MouseStateHandle); 3] = [
            (
                InterceptMode::Full,
                crate::t!("observatory-proxy-mode-full"),
                self.mode_chip_handles[0].clone(),
            ),
            (
                InterceptMode::HooksOnly,
                crate::t!("observatory-proxy-mode-hooks"),
                self.mode_chip_handles[1].clone(),
            ),
            (
                InterceptMode::Bypass,
                crate::t!("observatory-proxy-mode-bypass"),
                self.mode_chip_handles[2].clone(),
            ),
        ];
        let mut mode_row = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(SPACING);
        for (mode, label, handle) in modes {
            let is_active = mode == current_mode;
            let chip = Hoverable::new(handle, move |state| {
                let text_color = if is_active {
                    theme.active_ui_text_color().into()
                } else if state.is_hovered() {
                    theme.nonactive_ui_text_color().into()
                } else {
                    theme.disabled_ui_text_color().into_solid()
                };
                let mut container = Container::new(
                    Text::new(label.clone(), appearance.ui_font_family(), SMALL_FONT_SIZE)
                        .with_color(text_color)
                        .finish(),
                )
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(BADGE_RADIUS)))
                .with_horizontal_padding(8.)
                .with_vertical_padding(3.);
                if is_active {
                    container = container.with_border(Border::all(1.).with_border_fill(theme.accent()));
                }
                container.finish()
            })
            .on_click(move |ctx, _, _| {
                ctx.dispatch_typed_action(ObservatoryPanelAction::SetInterceptMode(mode));
            })
            .finish();
            mode_row.add_child(chip);
        }
        col.add_child(
            Container::new(mode_row.finish())
                .with_horizontal_padding(PANEL_PADDING)
                .finish(),
        );


        // ── 活跃拦截会话（proxy 运行态） ──
        let snapshot = self.model.as_ref(app).snapshot();
        let mut active_col = Flex::column().with_spacing(SPACING);
        active_col.add_child(
            Text::new(
                crate::t!("observatory-active-title"),
                appearance.ui_font_family(),
                appearance.ui_font_size(),
            )
            .with_color(theme.active_ui_text_color().into())
            .finish(),
        );
        if snapshot.active_intercepts.is_empty() {
            active_col.add_child(
                Text::new(
                    crate::t!("observatory-active-empty"),
                    appearance.ui_font_family(),
                    SMALL_FONT_SIZE,
                )
                .with_color(theme.disabled_ui_text_color().into_solid())
                .finish(),
            );
        } else {
            for ic in &snapshot.active_intercepts {
                active_col.add_child(render_active_intercept_row(ic, appearance, theme));
            }
        }
        col.add_child(
            Container::new(active_col.finish())
                .with_vertical_padding(SPACING)
                .finish(),
        );
        // ── Upstream 覆盖输入 ──
        col.add_child(
            Container::new(ChildView::new(&self.upstream_base_input).finish())
                .with_horizontal_padding(PANEL_PADDING)
                .finish(),
        );
        col.add_child(
            Container::new(ChildView::new(&self.upstream_auth_env_input).finish())
                .with_horizontal_padding(PANEL_PADDING)
                .finish(),
        );

        // ── 解析探测（ClaudeCode / Codex 双视角，与 harness_intercept 同基准） ──
        for (label, harness) in [
            ("Claude Code", HarnessType::ClaudeCode),
            ("Codex", HarnessType::Codex),
        ] {
            let probe_el = match intercept.resolve_upstream(harness) {
                Some(config) => Text::new(
                    format!(
                        "{} · {}",
                        label,
                        crate::t!(
                            "observatory-proxy-resolved",
                            base = config.api_base.clone(),
                            env = config.api_key_env.clone(),
                        ),
                    ),
                    appearance.ui_font_family(),
                    SMALL_FONT_SIZE,
                )
                .with_color(theme.sub_text_color(theme.background()).into())
                .soft_wrap(false)
                .finish(),
                None => Text::new(
                    format!("{} · {}", label, crate::t!("observatory-proxy-resolve-failed")),
                    appearance.ui_font_family(),
                    SMALL_FONT_SIZE,
                )
                .with_color(theme.ui_error_color())
                .finish(),
            };
            col.add_child(
                Container::new(probe_el)
                    .with_horizontal_padding(PANEL_PADDING)
                    .finish(),
            );
        }

        // ── 刷新计数按钮 ──
        col.add_child(
            Container::new(ChildView::new(&self.refresh_count_button).finish())
                .with_horizontal_padding(PANEL_PADDING)
                .finish(),
        );

        // ── 当前覆盖值回显 ──
        let base = intercept.upstream_base();
        let auth_env = intercept.upstream_auth_env();
        let override_text = format!(
            "base: {} · auth env: {}",
            if base.is_empty() { "(auto)" } else { base },
            if auth_env.is_empty() { "(default)" } else { auth_env },
        );
        col.add_child(
            Container::new(
                Text::new(override_text, appearance.ui_font_family(), SMALL_FONT_SIZE)
                    .with_color(theme.disabled_ui_text_color().into_solid())
                    .soft_wrap(false)
                    .finish(),
            )
            .with_horizontal_padding(PANEL_PADDING)
            .finish(),
        );

        // ── 持久化反馈：写盘成功显示时间，失败显示原因 ──
        if let Some(err) = intercept.last_persist_error() {
            col.add_child(
                Container::new(
                    Text::new(
                        crate::t!("observatory-proxy-save-failed", err = err),
                        appearance.ui_font_family(),
                        SMALL_FONT_SIZE,
                    )
                    .with_color(theme.ui_error_color())
                    .soft_wrap(false)
                    .finish(),
                )
                .with_horizontal_padding(PANEL_PADDING)
                .finish(),
            );
        } else if let Some(ts) = intercept.last_saved_at() {
            col.add_child(
                Container::new(
                    Text::new(
                        crate::t!(
                            "observatory-proxy-saved",
                            time = format_timestamp(ts),
                        ),
                        appearance.ui_font_family(),
                        SMALL_FONT_SIZE,
                    )
                    .with_color(theme.disabled_ui_text_color().into_solid())
                    .soft_wrap(false)
                    .finish(),
                )
                .with_horizontal_padding(PANEL_PADDING)
                .finish(),
            );
        }

        // ── 提示 ──
        col.add_child(
            Container::new(
                Text::new(
                    crate::t!("observatory-proxy-applies-to"),
                    appearance.ui_font_family(),
                    SMALL_FONT_SIZE,
                )
                .with_color(theme.disabled_ui_text_color().into_solid())
                .finish(),
            )
            .with_horizontal_padding(PANEL_PADDING)
            .finish(),
        );

        Shrinkable::new(1., col.finish()).finish()
    }

    /// Composer 区域: draft_to / subject / body 输入框 + 发送按钮。
    fn render_composer(&self, app: &AppContext) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        let theme = appearance.theme();
        let model = self.model.as_ref(app);
        let busy = model.busy();

        let mut col = Flex::column()
            .with_main_axis_size(MainAxisSize::Max)
            .with_spacing(COMPOSER_SPACING);

        // 发送目标 run（选中 run 优先，否则最新；无 run 时提示）
        let target_text = match (
            model.selected_run(),
            model.snapshot().runs.first(),
        ) {
            (Some(sel), _) => crate::t!(
                "observatory-send-target",
                run = truncate_str(sel, 16),
            ),
            (None, Some(latest)) => crate::t!(
                "observatory-send-target",
                run = truncate_str(&latest.id, 16),
            ),
            (None, None) => crate::t!("observatory-send-target-none"),
        };
        col.add_child(
            Text::new(target_text, appearance.ui_font_family(), SMALL_FONT_SIZE)
                .with_color(theme.disabled_ui_text_color().into_solid())
                .soft_wrap(false)
                .finish(),
        );

        col.add_child(ChildView::new(&self.draft_to_input).finish());
        col.add_child(ChildView::new(&self.draft_subject_input).finish());
        col.add_child(ChildView::new(&self.draft_body_input).finish());

        let mut btn_row = Flex::row()
            .with_main_axis_alignment(MainAxisAlignment::End)
            .with_spacing(SPACING);
        if busy {
            btn_row.add_child(
                Text::new(
                    crate::t!("observatory-send-busy"),
                    appearance.ui_font_family(),
                    appearance.ui_font_size(),
                )
                .with_color(appearance.theme().disabled_ui_text_color().into_solid())
                .finish(),
            );
        } else {
            btn_row.add_child(ChildView::new(&self.send_button).finish());
        }
        col.add_child(btn_row.finish());

        Container::new(col.finish())
            .with_horizontal_padding(PANEL_PADDING)
            .finish()
    }

    /// 空态占位文字。
    fn render_empty_state(
        &self,
        text: &str,
        appearance: &Appearance,
        theme: &WarpTheme,
    ) -> Box<dyn Element> {
        Container::new(
            Text::new(
                text.to_string(),
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
}

impl Entity for ObservatoryPanelView {
    type Event = ObservatoryPanelAction;
}
impl TypedActionView for ObservatoryPanelView {
    type Action = ObservatoryPanelAction;

    fn handle_action(&mut self, action: &Self::Action, ctx: &mut ViewContext<Self>) {
        match action {
            ObservatoryPanelAction::Refresh => {
                ObservatoryModel::handle(ctx).update(ctx, |model, ctx| {
                    model.refresh(ctx);
                });
                InterceptSessionsModel::handle(ctx).update(ctx, |model, ctx| {
                    model.refresh_block_count(ctx);
                });
            }
            ObservatoryPanelAction::SendMessage => {
                ObservatoryModel::handle(ctx).update(ctx, |model, ctx| {
                    model.send_message(ctx);
                });
            }
            ObservatoryPanelAction::SetTab(tab) => {
                let tab = *tab;
                ObservatoryModel::handle(ctx).update(ctx, |model, ctx| {
                    model.set_active_tab(tab, ctx);
                });
            }
            ObservatoryPanelAction::SelectSession(id) => {
                // 点击已选中项 → 取消选中（toggle）
                let id = toggle_id(
                    id,
                    ObservatoryModel::handle(ctx)
                        .as_ref(ctx)
                        .selected_session()
                        .map(str::to_string),
                );
                ObservatoryModel::handle(ctx).update(ctx, |model, ctx| {
                    model.select_session(id, ctx);
                });
            }

            ObservatoryPanelAction::SetSearch(filter) => {
                let filter = filter.clone();
                ObservatoryModel::handle(ctx).update(ctx, |model, ctx| {
                    model.set_search_filter(filter, ctx);
                });
            }
            ObservatoryPanelAction::SelectBlock(id) => {
                let id = toggle_id(
                    id,
                    ObservatoryModel::handle(ctx)
                        .as_ref(ctx)
                        .selected_block()
                        .map(str::to_string),
                );
                ObservatoryModel::handle(ctx).update(ctx, |model, ctx| {
                    model.select_block(id, ctx);
                });
            }
            ObservatoryPanelAction::SelectTask(id) => {
                let id = toggle_id(
                    id,
                    ObservatoryModel::handle(ctx)
                        .as_ref(ctx)
                        .selected_task()
                        .map(str::to_string),
                );
                ObservatoryModel::handle(ctx).update(ctx, |model, ctx| {
                    model.select_task(id, ctx);
                });
            }

            ObservatoryPanelAction::SelectRun(id) => {
                let id = toggle_id(
                    id,
                    ObservatoryModel::handle(ctx)
                        .as_ref(ctx)
                        .selected_run()
                        .map(str::to_string),
                );
                ObservatoryModel::handle(ctx).update(ctx, |model, ctx| {
                    model.select_run(id, ctx);
                });
            }
            ObservatoryPanelAction::DispatchTask(task_id) => {
                let task_id = task_id.clone();
                ObservatoryModel::handle(ctx).update(ctx, |model, ctx| {
                    model.dispatch_task(&task_id, ctx);
                });
            }

            ObservatoryPanelAction::DispatchSelectedTask => {
                let selected = ObservatoryModel::handle(ctx)
                    .as_ref(ctx)
                    .selected_task()
                    .map(str::to_string);
                if let Some(task_id) = selected {
                    ObservatoryModel::handle(ctx).update(ctx, |model, ctx| {
                        model.dispatch_task(&task_id, ctx);
                    });
                }
            }
            ObservatoryPanelAction::ResolveSelectedGate(resolution) => {
                let resolution = resolution.clone();
                let selected = ObservatoryModel::handle(ctx)
                    .as_ref(ctx)
                    .selected_gate()
                    .map(str::to_string);
                if let Some(gate_id) = selected {
                    ObservatoryModel::handle(ctx).update(ctx, |model, ctx| {
                        model.resolve_gate(&gate_id, &resolution, ctx);
                    });
                }
            }
            ObservatoryPanelAction::SelectGate(id) => {
                let id = toggle_id(
                    id,
                    ObservatoryModel::handle(ctx)
                        .as_ref(ctx)
                        .selected_gate()
                        .map(str::to_string),
                );
                ObservatoryModel::handle(ctx).update(ctx, |model, ctx| {
                    model.select_gate(id, ctx);
                });
            }

            ObservatoryPanelAction::ResolveGate(gate_id, resolution) => {
                let gate_id = gate_id.clone();
                let resolution = resolution.clone();
                ObservatoryModel::handle(ctx).update(ctx, |model, ctx| {
                    model.resolve_gate(&gate_id, &resolution, ctx);
                });
            }
            ObservatoryPanelAction::SelectRaw(id) => {
                let id = toggle_id(
                    id,
                    ObservatoryModel::handle(ctx)
                        .as_ref(ctx)
                        .selected_raw()
                        .map(str::to_string),
                );
                ObservatoryModel::handle(ctx).update(ctx, |model, ctx| {
                    model.select_raw(id, ctx);
                });
            }
            ObservatoryPanelAction::SelectMessage(seq) => {
                let seq = toggle_seq(
                    *seq,
                    ObservatoryModel::handle(ctx).as_ref(ctx).selected_message(),
                );
                ObservatoryModel::handle(ctx).update(ctx, |model, ctx| {
                    model.select_message(seq, ctx);
                });
            }
            ObservatoryPanelAction::SetInterceptMode(mode) => {
                let mode = *mode;
                InterceptSessionsModel::handle(ctx).update(ctx, |model, ctx| {
                    model.set_mode(mode, ctx);
                });
            }
            ObservatoryPanelAction::SetUpstreamBase(base) => {
                let base = base.clone();
                InterceptSessionsModel::handle(ctx).update(ctx, |model, ctx| {
                    model.set_upstream_base(base, ctx);
                });
            }
            ObservatoryPanelAction::SetUpstreamAuthEnv(env) => {
                let env = env.clone();
                InterceptSessionsModel::handle(ctx).update(ctx, |model, ctx| {
                    model.set_upstream_auth_env(env, ctx);
                });
            }
            ObservatoryPanelAction::RefreshBlockCount => {
                InterceptSessionsModel::handle(ctx).update(ctx, |model, ctx| {
                    model.refresh_block_count(ctx);
                });
            }
        }
    }
}

// ── Entity / View ────────────────────────────────────────────────────────────

impl View for ObservatoryPanelView {
    fn ui_name() -> &'static str {
        "ObservatoryPanelView"
    }

    fn render(&self, app: &AppContext) -> Box<dyn Element> {
        let active_tab = self.model.as_ref(app).active_tab();

        let mut col = Flex::column()
            .with_main_axis_size(MainAxisSize::Max)
            .with_spacing(SPACING);

        // 头部
        col.add_child(self.render_header(app));

        // Tab 切换行
        col.add_child(self.render_tab_bar(app));

        // Tab 内容
        match active_tab {
            ObservatoryTab::Sessions => col.add_child(self.render_sessions_tab(app)),
            ObservatoryTab::Orchestration => col.add_child(self.render_orchestration_tab(app)),
            ObservatoryTab::Proxy => col.add_child(self.render_proxy_tab(app)),
        }

        // 错误态（面板级：refresh 为三 tab 共用的全量刷新，任何 tab 下都需可见）
        if let Some(err) = self.model.as_ref(app).last_error() {
            let appearance = Appearance::as_ref(app);
            let theme = appearance.theme();
            col.add_child(
                Text::new(
                    crate::t!("observatory-last-error", err = err),
                    appearance.ui_font_family(),
                    appearance.ui_font_size(),
                )
                .with_color(theme.ui_error_color())
                .finish(),
            );
        }

        Shrinkable::new(1., col.finish()).finish()
    }
}

// ── 辅助函数 ────────────────────────────────────────────────────────────────

/// 字符串截断，超过 max_len 字符加 "…"。
fn truncate_str(s: &str, max_len: usize) -> String {
    if s.chars().count() <= max_len {
        s.to_string()
    } else {
        let truncated: String = s.chars().take(max_len).collect();
        format!("{}…", truncated)
    }
}

/// Unix timestamp 格式化为简洁时间文本。
fn format_timestamp(ts: i64) -> String {
    let dt = if ts > 1_000_000_000_000 {
        chrono::DateTime::from_timestamp_millis(ts)
    } else {
        chrono::DateTime::from_timestamp(ts, 0)
    };
    match dt {
        Some(dt) => dt.format("%m-%d %H:%M").to_string(),
        None => format!("{}", ts),
    }
}

/// block 类型 → badge 颜色。
fn block_type_color(block_type: &str, theme: &WarpTheme) -> ColorU {
    match block_type {
        "request" => theme.accent().into_solid(),
        "response" => internal_colors::neutral_6(theme),
        "event" => internal_colors::neutral_4(theme),
        "error" => theme.ui_error_color(),
        _ => theme.nonactive_ui_text_color().into_solid(),
    }
}

/// task status → 颜色（不依赖 theme，使用固定语义色）。
fn task_status_color(status: &str) -> Fill {
    match status {
        "completed" => Fill::Solid(ColorU { r: 80, g: 200, b: 120, a: 255 }),
        "failed" => Fill::Solid(ColorU { r: 220, g: 80, b: 80, a: 255 }),
        "ready" => Fill::Solid(ColorU { r: 220, g: 180, b: 60, a: 255 }),
        "dispatched" => Fill::Solid(ColorU { r: 80, g: 140, b: 220, a: 255 }),
        _ => Fill::Solid(ColorU { r: 150, g: 150, b: 150, a: 255 }),
    }
}

/// 活跃拦截会话单行：session 短 id · proxy 端口 · hook URL。
fn render_active_intercept_row(
    ic: &ActiveInterceptRowGui,
    appearance: &Appearance,
    theme: &WarpTheme,
) -> Box<dyn Element> {
    let mut col = Flex::column().with_spacing(SPACING / 2.);

    let mut row = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_spacing(SPACING);
    row.add_child(
        Text::new(
            truncate_str(&ic.session_id, 20),
            appearance.ui_font_family(),
            SMALL_FONT_SIZE,
        )
        .with_color(theme.sub_text_color(theme.background()).into())
        .soft_wrap(false)
        .finish(),
    );
    let port_text = match ic.proxy_port {
        Some(p) => format!("proxy 127.0.0.1:{p}"),
        None => "proxy (hooks only)".to_string(),
    };
    row.add_child(
        Text::new(port_text, appearance.ui_font_family(), SMALL_FONT_SIZE)
            .with_color(theme.accent().into_solid())
            .finish(),
    );
    col.add_child(row.finish());

    if let Some(url) = &ic.hook_url {
        col.add_child(
            Text::new(
                crate::t!("observatory-active-hook", url = url.clone()),
                appearance.ui_font_family(),
                SMALL_FONT_SIZE,
            )
            .with_color(theme.disabled_ui_text_color().into_solid())
            .soft_wrap(false)
            .finish(),
        );
    }

    Container::new(col.finish())
        .with_horizontal_padding(PANEL_PADDING)
        .with_vertical_padding(2.)
        .finish()
}

/// 选中 toggle：点击当前已选中项 → `None`（取消选中），否则透传。
fn toggle_id(id: &Option<String>, current: Option<String>) -> Option<String> {
    match (id, current) {
        (Some(new), Some(cur)) if *new == cur => None,
        (other, _) => other.clone(),
    }
}

/// 消息选中 toggle（seq 版 [`toggle_id`]）：点击已选中消息 → 取消。
fn toggle_seq(seq: Option<i64>, current: Option<i64>) -> Option<i64> {
    match (seq, current) {
        (Some(new), Some(cur)) if new == cur => None,
        (other, _) => other,
    }
}
