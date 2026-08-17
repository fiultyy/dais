//! 观测台面板视图 — ObservatoryPanelView
//!
//! 全部用户交互经 `ModelHandle<ObservatoryModel>`（业务状态）或
//! `InterceptSessionsModel`（代理配置单例）派发，视图不持有业务状态，
//! 仅维护渲染缓存（鼠标悬停句柄、子输入框句柄等纯 UI 状态）。

use std::cell::RefCell;
use warpui::elements::{
    Border, ChildView, ClippedScrollStateHandle, ClippedScrollable, ConstrainedBox, Container,
    CornerRadius, CrossAxisAlignment, DragBarSide, Empty, Expanded, Fill as ElementFill, Flex,
    Hoverable, MainAxisAlignment, MainAxisSize, MouseStateHandle, ParentElement, Resizable,
    ScrollStateHandle, Scrollable, ScrollableElement, ScrollbarWidth, Shrinkable, Text,
    UniformList, UniformListState,
};
use warpui::r#async::SpawnedFutureHandle;
use warpui::r#async::Timer;
use warpui::scene::Radius;
use warpui::{
    AppContext, Element, Entity, ModelHandle, SingletonEntity, TypedActionView, View, ViewContext,
    ViewHandle,
};

use warp_core::ui::appearance::Appearance;
use warp_core::ui::theme::color::internal_colors;
use warp_core::ui::theme::Fill;
use warp_core::ui::theme::WarpTheme;

use harness_integration::{HarnessType, InterceptMode};

use super::model::{
    ActiveInterceptRowGui, BlockDetailGui, BlockRowGui, DraftField, MessageDetailGui,
    ObservatoryModel, ObservatoryTab, RawDetailGui, RawRowGui, RunRowGui, TaskRowGui,
};
use super::format::{
    absolute_time_millis, compact_bytes, compact_count, format_duration_row_ms,
    relative_time_text,
};
use super::row::{list_row, status_dot_element};
use crate::ai::blocklist::agent_view::agent_input_footer::AgentInputButtonTheme;
use crate::terminal::intercept_sessions::InterceptSessionsModel;
use crate::view_components::action_button::{ActionButton, ButtonSize};
use crate::view_components::{SubmittableTextInput, SubmittableTextInputEvent};

// ── 布局常量（集中收口，P0-3） ──────────────────────────────────────────────

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
/// 列表行内边距（水平；垂直方向由 LIST_ROW_HEIGHT 固定行高收口）。
const ROW_H_PADDING: f32 = 8.;
/// Composer 输入框间距。
const COMPOSER_SPACING: f32 = 8.;
/// Block type badge 角半径。
const BADGE_RADIUS: f32 = 4.;
/// 小号字体（详情/辅助文本）。
const SMALL_FONT_SIZE: f32 = 12.;
/// 详情卡内容最大高度（超出内部滚动）。
const DETAIL_MAX_HEIGHT: f32 = 260.;
/// 观测台周期自动刷新间隔（ms）。
const OBSERVATORY_REFRESH_INTERVAL_MS: u64 = 5_000;

// ── 列表硬上限（DV24：达到上限必须可见提示，禁止静默截断） ──
const SESSIONS_CAP: usize = 100;
const BLOCKS_CAP: usize = 500;
const MESSAGES_CAP: usize = 30;

// ── 侧栏几何（sessions tab：主列 + blocks 侧栏 + block 详情侧栏） ──
/// Blocks 侧栏默认/最小宽度。
const BLOCKS_SIDEBAR_DEFAULT_WIDTH: f32 = 320.;
const BLOCKS_SIDEBAR_MIN_WIDTH: f32 = 240.;
/// Block 详情侧栏默认/最小宽度。
const BLOCK_DETAIL_SIDEBAR_DEFAULT_WIDTH: f32 = 360.;
const BLOCK_DETAIL_SIDEBAR_MIN_WIDTH: f32 = 280.;
const RUNS_CAP: usize = 50;
const TASKS_CAP: usize = 200;

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
    /// 外部捕获：切换 pane 级 harness 捕获开关（持久化）。
    ToggleExternalCapture,
    /// 代理配置：重查 block 计数。
    RefreshBlockCount,
    /// SystemPrompt 详情：切换 分段折叠/原文 视图模式。
    SetSystemPromptMode(SystemPromptViewMode),
    /// SystemPrompt 详情：切换第 idx 段展开态（折叠模式下）。
    ToggleSystemPromptSegment(usize),
    /// SystemPrompt 详情：全部展开/全部收起（折叠模式下）。
    ToggleAllSystemPromptSegments,
}

// ── SystemPrompt 分段折叠视图状态（T11） ──────────────────────────────────────

/// SystemPrompt 详情内容区的视图模式。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SystemPromptViewMode {
    /// 标记感知分段折叠（默认：全折叠，仅显标记名 + 摘要，点击展开原文）。
    Folded,
    /// 原文全文（旧行为，即全展开/raw）。
    Raw,
}

/// SystemPrompt 分段折叠的纯 UI 状态（渲染缓存）：模式 + 展开集合 +
/// 段头悬停句柄 + 解析缓存（block id / content 长度失配时重置重解析）。
struct SystemPromptFoldView {
    block_id: Option<String>,
    content_len: usize,
    mode: SystemPromptViewMode,
    expanded: std::collections::HashSet<usize>,
    segment_handles: Vec<MouseStateHandle>,
    mode_chip_handles: [MouseStateHandle; 2],
    expand_all_chip_handle: MouseStateHandle,
    segments: Vec<super::system_prompt_segments::SystemPromptSegment>,
}

impl Default for SystemPromptFoldView {
    fn default() -> Self {
        Self {
            block_id: None,
            content_len: 0,
            mode: SystemPromptViewMode::Folded,
            expanded: std::collections::HashSet::new(),
            segment_handles: Vec::new(),
            mode_chip_handles: [MouseStateHandle::default(), MouseStateHandle::default()],
            expand_all_chip_handle: MouseStateHandle::default(),
            segments: Vec::new(),
        }
    }
}

impl SystemPromptFoldView {
    /// 同步解析缓存：block id / 内容长度失配时重置模式与展开集并重解析
    ///（block 切换防折叠态残留，T11 陷阱#4）；命中缓存则原样保留。
    fn sync(&mut self, block_id: &str, content: &str) {
        if self.block_id.as_deref() == Some(block_id) && self.content_len == content.len() {
            return;
        }
        self.block_id = Some(block_id.to_string());
        self.content_len = content.len();
        self.mode = SystemPromptViewMode::Folded;
        self.expanded.clear();
        self.segments = super::system_prompt_segments::segment_system_prompt(content);
        self.segment_handles.clear();
    }
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
    /// 外部捕获开关 chip 悬停状态。
    external_capture_chip_handle: MouseStateHandle,
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
    // ── P0-1 虚拟化列表状态（Scrollable + UniformList） ──
    /// Sessions tab: sessions 列表滚动状态。
    sessions_scroll_state: ScrollStateHandle,
    /// Sessions tab: sessions 列表 UniformList 状态。
    sessions_list: UniformListState,
    /// Sessions tab: blocks 时间线滚动状态。
    blocks_scroll_state: ScrollStateHandle,
    /// Sessions tab: blocks 时间线 UniformList 状态。
    blocks_list: UniformListState,
    /// Raw 流量滚动状态。
    raw_scroll_state: ScrollStateHandle,
    /// Raw 流量列表状态。
    raw_list: UniformListState,
    /// Orchestration tab: 消息滚动状态。
    messages_scroll_state: ScrollStateHandle,
    /// Orchestration tab: 消息列表状态。
    messages_list: UniformListState,
    /// Orchestration tab: 归档滚动状态。
    archives_scroll_state: ScrollStateHandle,
    /// Orchestration tab: 归档列表状态。
    archives_list: UniformListState,
    /// Orchestration tab: runs/gates 区滚动状态（ClippedScrollable）。
    orchestration_clipped_scroll: ClippedScrollStateHandle,
    // ── 侧栏体系（session → blocks → block 详情） ──
    /// Blocks 侧栏 Resizable 状态。
    blocks_sidebar_resize_state: warpui::elements::ResizableStateHandle,
    /// Block 详情侧栏 Resizable 状态。
    block_detail_sidebar_resize_state: warpui::elements::ResizableStateHandle,
    /// Block 详情侧栏滚动状态。
    block_detail_scroll_state: ClippedScrollStateHandle,
    /// SystemPrompt 分段折叠视图状态（T11；纯 UI 渲染缓存）。
    system_prompt_view: RefCell<SystemPromptFoldView>,
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
                ctx.dispatch_typed_action(&ObservatoryPanelAction::SetUpstreamBase(
                    content.clone(),
                ));
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
                ctx.dispatch_typed_action(&ObservatoryPanelAction::SetUpstreamAuthEnv(
                    content.clone(),
                ));
                auth_env_handle.update(ctx, |input, ctx| {
                    let editor = input.editor().clone();
                    let text = content.clone();
                    editor.update(ctx, |ed, ctx| ed.set_buffer_text(&text, ctx));
                });
            }
        });

        // 任务派发按钮（选中 task 由 handle_action 侧读取）
        let dispatch_button = ctx.add_typed_action_view(|_ctx| {
            ActionButton::new(
                crate::t!("observatory-task-dispatch"),
                AgentInputButtonTheme,
            )
            .with_size(ButtonSize::AgentInputButton)
            .on_click(|ctx| {
                ctx.dispatch_typed_action(ObservatoryPanelAction::DispatchSelectedTask);
            })
        });

        // 代理 tab: 刷新 block 计数按钮
        let refresh_count_button = ctx.add_typed_action_view(|_ctx| {
            ActionButton::new(
                crate::t!("observatory-proxy-refresh-count"),
                AgentInputButtonTheme,
            )
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
            external_capture_chip_handle: MouseStateHandle::default(),
            upstream_base_input,
            upstream_auth_env_input,
            dispatch_button,
            refresh_count_button,
            refresh_timer_handle: None,
            prev_busy: std::cell::Cell::new(false),
            sessions_scroll_state: ScrollStateHandle::default(),
            sessions_list: UniformListState::new(),
            blocks_scroll_state: ScrollStateHandle::default(),
            blocks_list: UniformListState::new(),
            raw_scroll_state: ScrollStateHandle::default(),
            raw_list: UniformListState::new(),
            messages_scroll_state: ScrollStateHandle::default(),
            messages_list: UniformListState::new(),
            archives_scroll_state: ScrollStateHandle::default(),
            archives_list: UniformListState::new(),
            orchestration_clipped_scroll: ClippedScrollStateHandle::default(),
            blocks_sidebar_resize_state: warpui::elements::resizable_state_handle(
                BLOCKS_SIDEBAR_DEFAULT_WIDTH,
            ),
            block_detail_sidebar_resize_state: warpui::elements::resizable_state_handle(
                BLOCK_DETAIL_SIDEBAR_DEFAULT_WIDTH,
            ),
            block_detail_scroll_state: ClippedScrollStateHandle::default(),
            system_prompt_view: RefCell::new(SystemPromptFoldView::default()),
        };
        // 首次启动 5s 自动刷新 timer（此前无首调，timer 从未跑起来——
        // render 是 &self 无法启动，start_refresh_timer 只在回调内自续期）。
        me.start_refresh_timer(ctx);
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
        let blocks_text =
            crate::t!("observatory-blocks-captured", count = compact_count(model.block_count_total(app)));

        let mut row = Flex::row()
            .with_main_axis_size(MainAxisSize::Max)
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
                mode_label,
                appearance.ui_font_family(),
                appearance.ui_font_size(),
            )
            .with_color(theme.nonactive_ui_text_color().into_solid())
            .finish(),
        );
        row.add_child(
            Text::new(
                blocks_text,
                appearance.ui_font_family(),
                appearance.ui_font_size(),
            )
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
                    Text::new(
                        label.clone(),
                        appearance.ui_font_family(),
                        appearance.ui_font_size(),
                    )
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

    /// Sessions tab：搜索框 + 会话主列；点击会话 → 右侧滑出 blocks 侧栏
    /// （Resizable 可拖宽，打开即滚到最新一条）；点击 block → 再滑出
    /// block 详情侧栏（宽度独立）。raw/详情区收进侧栏体系。
    fn render_sessions_tab(&self, app: &AppContext) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        let theme = appearance.theme();
        let model = self.model.as_ref(app);
        let snapshot = model.snapshot();

        let mut main_col = Flex::column()
            .with_main_axis_size(MainAxisSize::Min)
            .with_spacing(SPACING);

        // ── 搜索框（固定区） ──
        main_col.add_child(
            Container::new(ChildView::new(&self.search_input).finish())
                .with_horizontal_padding(PANEL_PADDING)
                .finish(),
        );

        // ── 会话主列（≤100 行；虚拟化滚动，占满剩余高度） ──
        if snapshot.sessions.is_empty() {
            main_col.add_child(self.render_empty_state(
                &crate::t!("observatory-sessions-empty"),
                appearance,
                theme,
            ));
        } else {
            Self::ensure_handles(
                &mut self.session_row_handles.borrow_mut(),
                snapshot.sessions.len(),
            );
            let handles = self.session_row_handles.borrow().clone();
            let now = chrono::Utc::now().timestamp();
            let sessions: Vec<(String, bool, i64)> = snapshot
                .sessions
                .iter()
                .map(|s| {
                    (
                        s.session_id.clone(),
                        model
                            .selected_session()
                            .is_some_and(|id| id == s.session_id),
                        s.last_ts,
                    )
                })
                .collect();
            // 闭包按 'static 捕获：theme 克隆 + 字体参数 Copy
            let theme = theme.clone();
            let theme_for_list = theme.clone();
            let font_family = appearance.ui_font_family();
            let font_size = appearance.ui_font_size();
            let build = move |range: std::ops::Range<usize>, _app: &AppContext| {
                (range.start..range.end)
                    .filter_map(|i| {
                        sessions.get(i).map(|(sid, sel, ts)| (sid.clone(), *sel, *ts, i))
                    })
                    .map(|(session_id, is_selected, last_ts, i)| {
                        let handle = handles[i].clone();
                        let inner = list_row(
                            &theme,
                            font_family,
                            font_size,
                            None,
                            truncate_str(&session_id, 16),
                            None,
                            Some(relative_time_text(now, last_ts)),
                        );
                        let theme = theme.clone();
                        Hoverable::new(handle, move |state| {
                            let mut container =
                                Container::new(inner).with_horizontal_padding(PANEL_PADDING);
                            if is_selected {
                                container = container
                                    .with_background(internal_colors::fg_overlay_1(&theme));
                            } else if state.is_hovered() {
                                container = container.with_background(Fill::Solid(
                                    internal_colors::neutral_3(&theme),
                                ));
                            }
                            container.finish()
                        })
                        .on_click(move |ctx, _, _| {
                            ctx.dispatch_typed_action(ObservatoryPanelAction::SelectSession(Some(
                                session_id.clone(),
                            )));
                        })
                        .finish()
                    })
                    .collect::<Vec<_>>()
                    .into_iter()
            };
            main_col.add_child(
                Expanded::new(
                    1.,
                    self.wrap_virtual_list(
                        self.sessions_scroll_state.clone(),
                        self.sessions_list.clone(),
                        snapshot.sessions.len(),
                        build,
                        &theme_for_list,
                    ),
                )
                .finish(),
            );
            if let Some(hint) =
                Self::truncated_hint(
                    snapshot.sessions.len(),
                    SESSIONS_CAP,
                    appearance,
                    appearance.theme(),
                )
            {
                main_col.add_child(hint);
            }
        }

        // ── 未选会话：主列独占 ──
        if model.selected_session().is_none() {
            return Shrinkable::new(1., main_col.finish()).finish();
        }

        // ── blocks 侧栏（选中会话滑出） ──
        let blocks_sidebar = self.render_blocks_sidebar(app);

        // ── block 详情二级侧栏（选中 block 滑出） ──
        let row = if model.block_detail().is_some() {
            Flex::row()
                .with_main_axis_size(MainAxisSize::Max)
                .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                .with_child(Shrinkable::new(1., main_col.finish()).finish())
                .with_child(blocks_sidebar)
                .with_child(self.render_block_detail_sidebar(app))
                .finish()
        } else {
            Flex::row()
                .with_main_axis_size(MainAxisSize::Max)
                .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                .with_child(Shrinkable::new(1., main_col.finish()).finish())
                .with_child(blocks_sidebar)
                .finish()
        };

        ConstrainedBox::new(row).with_max_height(1600.).finish()
    }

    /// Blocks 侧栏：session 的时间线 + raw 流量（垂直两段，各自滚动）。
    /// Resizable 可拖宽；打开/切换会话时滚到最新一条。
    fn render_blocks_sidebar(&self, app: &AppContext) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        let theme = appearance.theme();
        let model = self.model.as_ref(app);
        let snapshot = model.snapshot();

        let mut col = Flex::column()
            .with_main_axis_size(MainAxisSize::Max)
            .with_spacing(SPACING);

        // 侧栏头：标题 + 会话 id + 关闭按钮语义（再点选中行取消）
        let title = crate::t!(
            "observatory-blocks-sidebar-title",
            session = truncate_str(
                model.selected_session().unwrap_or_default(),
                20
            ),
        );
        col.add_child(
            Container::new(
                Text::new(
                    title,
                    appearance.ui_font_family(),
                    appearance.ui_font_size(),
                )
                .with_color(theme.active_ui_text_color().into())
                .soft_wrap(false)
                .finish(),
            )
            .with_horizontal_padding(PANEL_PADDING)
            .with_vertical_padding(SPACING)
            .finish(),
        );

        // 上下文占用行（T10）：tokens + 模型；窗口已知时加占用率。
        if let Some(ctx_info) = &snapshot.session_context {
            let pct = ctx_info
                .window_tokens
                .filter(|w| *w > 0)
                .map(|w| (ctx_info.used_tokens as f64 / w as f64).clamp(0.0, 1.0));
            let text = match pct {
                Some(p) => crate::t!(
                    "observatory-session-context",
                    model = ctx_info.model.clone(),
                    used = super::format::compact_count(ctx_info.used_tokens),
                    window =
                        super::format::compact_count(ctx_info.window_tokens.unwrap_or_default()),
                    pct = format!("{}%", (p * 100.0).round() as u32),
                ),
                None => crate::t!(
                    "observatory-session-context-unknown-window",
                    model = ctx_info.model.clone(),
                    used = super::format::compact_count(ctx_info.used_tokens),
                ),
            };
            col.add_child(
                Container::new(
                    Text::new(text, appearance.ui_font_family(), SMALL_FONT_SIZE)
                        .with_color(theme.sub_text_color(theme.background()).into())
                        .soft_wrap(false)
                        .finish(),
                )
                .with_horizontal_padding(PANEL_PADDING)
                .finish(),
            );
        }

        // Block 时间线（虚拟化；主滚动区占剩余高度）
        if snapshot.blocks.is_empty() {
            col.add_child(self.render_empty_state(
                &crate::t!("observatory-blocks-empty"),
                appearance,
                theme,
            ));
        } else {
            Self::ensure_handles(
                &mut self.block_row_handles.borrow_mut(),
                snapshot.blocks.len(),
            );
            let handles = self.block_row_handles.borrow().clone();
            let blocks: Vec<(String, bool)> = snapshot
                .blocks
                .iter()
                .map(|b| {
                    (
                        b.id.clone(),
                        model.selected_block().is_some_and(|id| id == b.id),
                    )
                })
                .collect();
            // 闭包按 'static 捕获：theme 克隆 + 行数据克隆 + 字体参数 Copy
            let blocks_full: Vec<BlockRowGui> = snapshot.blocks.clone();
            let theme = theme.clone();
            let theme_for_list = theme.clone();
            let font_family = appearance.ui_font_family();
            let font_size = appearance.ui_font_size();
            let build = move |range: std::ops::Range<usize>, _app: &AppContext| {
                (range.start..range.end)
                    .filter_map(|i| blocks.get(i).cloned().map(|(bid, sel)| (bid, sel, i)))
                    .map(|(block_id, is_selected, i)| {
                        let handle = handles[i].clone();
                        let block = &blocks_full[i];
                        let inner =
                            render_block_list_row(block, font_family, font_size, &theme);
                        let theme = theme.clone();
                        Hoverable::new(handle, move |state| {
                            let mut container =
                                Container::new(inner).with_horizontal_padding(PANEL_PADDING);
                            if is_selected {
                                // DSH 选中：inset 2px brand 描边（行卡外圈）。
                                container = container
                                    .with_background(internal_colors::fg_overlay_1(&theme))
                                    .with_border(
                                        Border::all(2.).with_border_fill(theme.accent()),
                                    );
                            } else if state.is_hovered() {
                                container = container.with_background(Fill::Solid(
                                    internal_colors::neutral_3(&theme),
                                ));
                            }
                            container.finish()
                        })
                        .on_click(move |ctx, _, _| {
                            ctx.dispatch_typed_action(ObservatoryPanelAction::SelectBlock(
                                Some(block_id.clone()),
                            ));
                        })
                        .finish()
                    })
                    .collect::<Vec<_>>()
                    .into_iter()
            };
            // 打开/切换会话：一次性滚动到最新一条。`scroll_to` 是
            // UniformListState 内置通道，layout 时消费，无重入。
            let latest = snapshot.blocks.len().saturating_sub(1);
            if self.model.as_ref(app).take_scroll_blocks_to_latest() {
                self.blocks_list.scroll_to(latest);
            }
            col.add_child(
                Expanded::new(
                    1.,
                    self.wrap_virtual_list(
                        self.blocks_scroll_state.clone(),
                        self.blocks_list.clone(),
                        snapshot.blocks.len(),
                        build,
                        &theme_for_list,
                    ),
                )
                .finish(),
            );
            if let Some(hint) =
                Self::truncated_hint(
                    snapshot.blocks.len(),
                    BLOCKS_CAP,
                    appearance,
                    appearance.theme(),
                )
            {
                col.add_child(hint);
            }
        }

        // Raw 流量（底部固定高度段）
        let raw_section = self.render_raw_list(app);
        col.add_child(ConstrainedBox::new(raw_section).with_max_height(160.).finish());

        // 侧栏容器：Resizable 拖宽（左拉手柄），带背景与分隔边。
        let theme = appearance.theme();
        let sidebar = Container::new(
            ConstrainedBox::new(col.finish())
                .with_min_width(BLOCKS_SIDEBAR_MIN_WIDTH)
                .finish(),
        )
        .with_background(Fill::Solid(internal_colors::neutral_1(theme)))
        .with_border(Border::left(1.).with_border_fill(theme.outline()))
        .finish();

        Resizable::new(self.blocks_sidebar_resize_state.clone(), sidebar)
            .with_dragbar_side(DragBarSide::Left)
            .with_bounds_callback(Box::new(|window_size| {
                let min = BLOCKS_SIDEBAR_MIN_WIDTH;
                let max = (window_size.x() * 0.4).max(min);
                (min, max)
            }))
            .finish()
    }

    /// Block 详情二级侧栏：元信息 + metadata + content 全文（内部滚动）。
    fn render_block_detail_sidebar(&self, app: &AppContext) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        let theme = appearance.theme();
        let model = self.model.as_ref(app);
        let Some(detail) = model.block_detail() else {
            return Empty::new().finish();
        };

        let content = self.render_block_detail_body(detail, appearance, theme);
        // 不加包装 col：flex 非-flex child 通道给主轴 [0,∞]，会把
        // ClippedScrollable 的视口撑成内容全高（constraint.apply 不收
        // 缩），scroll() 的 child_size > clipped_size 恒假 → 滚轮静默
        // no-op。有限高度由 row 的 Stretch 直接传下来，与 blocks 侧栏
        // 同构（orchestration tab 用 Expanded 是同一原理的显式版）。
        let sidebar = Container::new(
            ConstrainedBox::new(content)
                .with_min_width(BLOCK_DETAIL_SIDEBAR_MIN_WIDTH)
                .finish(),
        )
        .with_background(Fill::Solid(internal_colors::neutral_1(theme)))
        .with_border(Border::left(1.).with_border_fill(theme.outline()))
        .finish();

        Resizable::new(self.block_detail_sidebar_resize_state.clone(), sidebar)
            .with_dragbar_side(DragBarSide::Left)
            .with_bounds_callback(Box::new(|window_size| {
                let min = BLOCK_DETAIL_SIDEBAR_MIN_WIDTH;
                let max = (window_size.x() * 0.5).max(min);
                (min, max)
            }))
            .finish()
    }

    /// UniformList 包裹 helper：Scrollable(vertical) + UniformList
    /// （global_search 同款虚拟化滚动模式；Scrollable 传递有限高度约束，
    /// UniformList 实现 ScrollableElement 自管滚动与可见行窗口）。
    fn wrap_virtual_list<F, G>(
        &self,
        scroll_state: ScrollStateHandle,
        list_state: UniformListState,
        item_count: usize,
        build_items: F,
        theme: &WarpTheme,
    ) -> Box<dyn Element>
    where
        F: Fn(std::ops::Range<usize>, &AppContext) -> G + 'static,
        G: Iterator<Item = Box<dyn Element>> + 'static,
    {
        let list = UniformList::new(list_state, item_count, build_items);
        Scrollable::vertical(
            scroll_state,
            list.finish_scrollable(),
            ScrollbarWidth::Auto,
            theme.nonactive_ui_detail().into(),
            theme.active_ui_detail().into(),
            ElementFill::None,
        )
        .with_overlayed_scrollbar()
        .finish()
    }

    /// Raw 代理流量列表（时间升序，虚拟化）。详情卡固定在 tab 底部（见
    /// render_sessions_tab）。
    fn render_raw_list(&self, app: &AppContext) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        let theme = appearance.theme();
        let model = self.model.as_ref(app);
        let snapshot = model.snapshot();

        let mut col = Flex::column()
            .with_main_axis_size(MainAxisSize::Min)
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
            let handles = self.raw_row_handles.borrow().clone();
            let raws: Vec<(String, bool)> = snapshot
                .raw_entries
                .iter()
                .map(|r| {
                    (
                        r.id.clone(),
                        model.selected_raw().is_some_and(|id| id == r.id),
                    )
                })
                .collect();
            // 闭包按 'static 捕获：theme 克隆 + 字体参数 Copy
            let theme = theme.clone();
            let theme_for_list = theme.clone();
            let font_family = appearance.ui_font_family();
            let font_size = appearance.ui_font_size();
            let raws_full: Vec<RawRowGui> = snapshot.raw_entries.clone();
            let build = move |range: std::ops::Range<usize>, _app: &AppContext| {
                (range.start..range.end)
                    .filter_map(|i| raws.get(i).cloned().map(|(rid, sel)| (rid, sel, i)))
                    .map(|(raw_id, is_selected, i)| {
                        let handle = handles[i].clone();
                        let entry = &raws_full[i];
                        let row_el = render_raw_list_row(entry, font_family, font_size, &theme);
                        let theme = theme.clone();
                        Hoverable::new(handle, move |state| {
                            let mut container =
                                Container::new(row_el).with_horizontal_padding(PANEL_PADDING);
                            if is_selected {
                                container = container
                                    .with_background(internal_colors::fg_overlay_1(&theme));
                            } else if state.is_hovered() {
                                container = container.with_background(Fill::Solid(
                                    internal_colors::neutral_3(&theme),
                                ));
                            }
                            container.finish()
                        })
                        .on_click(move |ctx, _, _| {
                            ctx.dispatch_typed_action(ObservatoryPanelAction::SelectRaw(Some(
                                raw_id.clone(),
                            )));
                        })
                        .finish()
                    })
                    .collect::<Vec<_>>()
                    .into_iter()
            };
            col.add_child(
                ConstrainedBox::new(
                    self.wrap_virtual_list(
                        self.raw_scroll_state.clone(),
                        self.raw_list.clone(),
                        snapshot.raw_entries.len(),
                        build,
                        &theme_for_list,
                    ),
                )
                .with_max_height(160.)
                .finish(),
            );
        }

        Container::new(col.finish())
            .with_vertical_padding(SECTION_SPACING / 2.)
            .finish()
    }

    /// 单行 raw 流量条目（row.rs list_row 版本）。
    fn render_raw_row(
        &self,
        entry: &RawRowGui,
        appearance: &Appearance,
        theme: &WarpTheme,
    ) -> Box<dyn Element> {
        render_raw_list_row(
            entry,
            appearance.ui_font_family(),
            appearance.ui_font_size(),
            theme,
        )
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
            ts = absolute_time_millis(Some(detail.timestamp)),
        );

        let mut col = Flex::column()
            .with_main_axis_size(MainAxisSize::Min)
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
            Text::new(
                truncate_str(&detail.content, 16000),
                appearance.ui_font_family(),
                SMALL_FONT_SIZE,
            )
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
            ts = absolute_time_millis(Some(detail.created_at)),
        );
        let type_text = crate::t!(
            "observatory-message-detail-kind",
            kind = detail.message_type.clone(),
            priority = detail.priority.clone(),
        );

        let mut col = Flex::column()
            .with_main_axis_size(MainAxisSize::Min)
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
            Text::new(
                format!("{} · {}", detail.subject, meta_text),
                appearance.ui_font_family(),
                SMALL_FONT_SIZE,
            )
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
            Text::new(
                truncate_str(&detail.body, 16000),
                appearance.ui_font_family(),
                SMALL_FONT_SIZE,
            )
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


    /// Block 详情侧栏滚动体：元信息 + metadata + content 全文。
    fn render_block_detail_body(
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
            ts = absolute_time_millis(Some(detail.timestamp)),
        );

        let mut col = Flex::column()
            .with_main_axis_size(MainAxisSize::Min)
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
            Text::new(
                truncate_str(&detail.metadata, 4000),
                appearance.ui_font_family(),
                SMALL_FONT_SIZE,
            )
            .with_color(theme.sub_text_color(theme.background()).into())
            .finish(),
        );

        // Content：system_prompt 走标记感知分段折叠（T11）；
        // 其余类型保持原文全文。
        col.add_child(
            Text::new(
                crate::t!("observatory-block-detail-content"),
                appearance.ui_font_family(),
                SMALL_FONT_SIZE,
            )
            .with_color(theme.nonactive_ui_text_color().into_solid())
            .finish(),
        );
        if detail.block_type == "system_prompt" {
            col.add_child(self.render_system_prompt_content(detail, appearance, theme));
        } else {
            col.add_child(
                Text::new(
                    truncate_str(&detail.content, 16000),
                    appearance.ui_font_family(),
                    SMALL_FONT_SIZE,
                )
                .with_color(theme.sub_text_color(theme.background()).into())
                .finish(),
            );
        }

        Container::new(
            ClippedScrollable::vertical(
                self.block_detail_scroll_state.clone(),
                col.finish(),
                ScrollbarWidth::Auto,
                theme.disabled_text_color(theme.background()).into(),
                theme.main_text_color(theme.background()).into(),
                ElementFill::None,
            )
            .finish(),
        )
        .with_horizontal_padding(PANEL_PADDING)
        .with_vertical_padding(SPACING)
        .finish()
    }

    /// SystemPrompt 详情内容区（T11）：标记感知分段折叠。
    ///
    /// - 折叠模式（默认）：每段一行「▸ 标记名 · 摘要 · N 行」，点击展开
    ///   段原文；附 全部展开/收起。段数/内容失配时重置折叠状态并重解析
    ///   （block 切换防选中态残留）。
    /// - 原文模式：与旧版一致的全文渲染（即全展开/raw 切换）。
    fn render_system_prompt_content(
        &self,
        detail: &BlockDetailGui,
        appearance: &Appearance,
        theme: &WarpTheme,
    ) -> Box<dyn Element> {
        // ── 同步解析缓存（block id / 内容长度失配 → 重置重解析） ──
        {
            let mut view = self.system_prompt_view.borrow_mut();
            view.sync(&detail.id, &detail.content);
            let seg_count = view.segments.len();
            Self::ensure_handles(&mut view.segment_handles, seg_count);
        }


        let view = self.system_prompt_view.borrow();
        let mut col = Flex::column()
            .with_main_axis_size(MainAxisSize::Min)
            .with_spacing(SPACING);

        // ── 模式 chips：分段（默认）/ 原文 ──
        let mode = view.mode;
        let modes = [
            (
                SystemPromptViewMode::Folded,
                crate::t!("observatory-system-prompt-mode-folded"),
                view.mode_chip_handles[0].clone(),
            ),
            (
                SystemPromptViewMode::Raw,
                crate::t!("observatory-system-prompt-mode-raw"),
                view.mode_chip_handles[1].clone(),
            ),
        ];
        let mut mode_row = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(SPACING);
        for (chip_mode, label, handle) in modes {
            let is_active = chip_mode == mode;
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
                    container =
                        container.with_border(Border::all(1.).with_border_fill(theme.accent()));
                }
                container.finish()
            })
            .on_click(move |ctx, _, _| {
                ctx.dispatch_typed_action(ObservatoryPanelAction::SetSystemPromptMode(
                    chip_mode,
                ));
            })
            .finish();
            mode_row.add_child(chip);
        }
        col.add_child(mode_row.finish());

        if mode == SystemPromptViewMode::Raw {
            // 原文模式：旧版全文渲染（raw/全展开）。
            col.add_child(
                Text::new(
                    truncate_str(&detail.content, 16000),
                    appearance.ui_font_family(),
                    SMALL_FONT_SIZE,
                )
                .with_color(theme.sub_text_color(theme.background()).into())
                .finish(),
            );
            return col.finish();
        }

        // ── 折叠模式 ──
        let segments = &view.segments;
        if segments.is_empty() {
            drop(view);
            col.add_child(
                Text::new(
                    crate::t!("observatory-system-prompt-empty"),
                    appearance.ui_font_family(),
                    SMALL_FONT_SIZE,
                )
                .with_color(theme.disabled_ui_text_color().into_solid())
                .finish(),
            );
            return col.finish();
        }

        let total = segments.len();
        let all_expanded = view.expanded.len() == total;

        // 全部展开/收起 chip + 段数统计
        let expand_chip = Hoverable::new(view.expand_all_chip_handle.clone(), move |state| {
            let text_color = if state.is_hovered() {
                theme.nonactive_ui_text_color().into()
            } else {
                theme.disabled_ui_text_color().into_solid()
            };
            Container::new(
                Text::new(
                    if all_expanded {
                        crate::t!("observatory-system-prompt-collapse-all")
                    } else {
                        crate::t!("observatory-system-prompt-expand-all")
                    },
                    appearance.ui_font_family(),
                    SMALL_FONT_SIZE,
                )
                .with_color(text_color)
                .finish(),
            )
            .finish()
        })
        .on_click(move |ctx, _, _| {
            ctx.dispatch_typed_action(ObservatoryPanelAction::ToggleAllSystemPromptSegments);
        })
        .finish();
        let mut ctrl_row = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(SPACING);
        ctrl_row.add_child(expand_chip);
        ctrl_row.add_child(Expanded::new(
            1.,
            Text::new(
                crate::t!("observatory-system-prompt-segments-count", count = total),
                appearance.ui_font_family(),
                SMALL_FONT_SIZE,
            )
            .with_color(theme.disabled_ui_text_color().into_solid())
            .finish(),
        )
        .finish());
        col.add_child(ctrl_row.finish());

        // 段列表
        for (idx, seg) in segments.iter().enumerate() {
            let expanded = view.expanded.contains(&idx);
            let handle = view.segment_handles[idx].clone();
            let title = match &seg.marker {
                Some(m) => m.clone(),
                None => crate::t!("observatory-system-prompt-preamble"),
            };
            let summary = if seg.summary.is_empty() {
                crate::t!("observatory-system-prompt-empty")
            } else {
                seg.summary.clone()
            };
            let line_count = seg.line_count;
            let body_text = truncate_str(&seg.text, 16000);

            // 段头行：▸/▾ 标记名 · 摘要 · N 行（点击切换展开）。
            let header = Hoverable::new(handle, move |state| {
                let mut row = Flex::row()
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_spacing(SPACING);
                row.add_child(
                    Text::new(
                        format!("{} {}", if expanded { "▾" } else { "▸" }, title),
                        appearance.ui_font_family(),
                        SMALL_FONT_SIZE,
                    )
                    .with_color(theme.active_ui_text_color().into())
                    .finish(),
                );
                if !expanded {
                    row.add_child(
                        Expanded::new(
                            1.,
                            Text::new(
                                summary,
                                appearance.ui_font_family(),
                                SMALL_FONT_SIZE,
                            )
                            .with_color(theme.sub_text_color(theme.background()).into())
                            .finish(),
                        )
                        .finish(),
                    );
                }
                row.add_child(
                    Text::new(
                        crate::t!("observatory-system-prompt-segment-lines", lines = line_count),
                        appearance.ui_font_family(),
                        SMALL_FONT_SIZE,
                    )
                    .with_color(theme.disabled_ui_text_color().into_solid())
                    .finish(),
                );
                let mut container = Container::new(row.finish())
                    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(BADGE_RADIUS)))
                    .with_horizontal_padding(4.)
                    .with_vertical_padding(2.);
                if state.is_hovered() {
                    container = container.with_background(Fill::Solid(internal_colors::neutral_3(
                        &theme,
                    )));
                } else if expanded {
                    container = container.with_background(Fill::Solid(internal_colors::neutral_2(
                        &theme,
                    )));
                }
                container.finish()
            })
            .on_click(move |ctx, _, _| {
                ctx.dispatch_typed_action(ObservatoryPanelAction::ToggleSystemPromptSegment(idx));
            })
            .finish();
            col.add_child(header);

            // 展开态：段原文（左侧 accent 边线 + 缩进）。
            if expanded {
                col.add_child(
                    Container::new(
                        Text::new(
                            body_text,
                            appearance.ui_font_family(),
                            SMALL_FONT_SIZE,
                        )
                        .with_color(theme.sub_text_color(theme.background()).into())
                        .finish(),
                    )
                    .with_border(
                        Border::left(2.).with_border_fill(theme.accent()),
                    )
                    .with_horizontal_padding(8.)
                    .finish(),
                );
            }
        }

        col.finish()
    }

    /// Orchestration tab 内容: 滚动列表区（runs+tasks / gates / messages /
    /// archives，单一滚动口）+ 固定详情区（task 详情 + composer）。
    fn render_orchestration_tab(&self, app: &AppContext) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        let theme = appearance.theme();
        let model = self.model.as_ref(app);
        let snapshot = model.snapshot();
        let now = chrono::Utc::now().timestamp();

        let mut col = Flex::column()
            .with_main_axis_size(MainAxisSize::Min)
            .with_spacing(SECTION_SPACING);

        // ── 滚动列表区 ──
        let mut scroll_col = Flex::column()
            .with_main_axis_size(MainAxisSize::Min)
            .with_spacing(SECTION_SPACING);

        // Runs + Tasks（task 行可点击选中）
        if snapshot.runs.is_empty() {
            scroll_col.add_child(self.render_empty_state(
                &crate::t!("observatory-runs-empty"),
                appearance,
                theme,
            ));
        } else {
            let selected_task = model.selected_task();
            let selected_run = model.selected_run();
            Self::ensure_handles(&mut self.run_row_handles.borrow_mut(), snapshot.runs.len());
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
                let rows: Vec<(usize, &TaskRowGui)> = run_tasks
                    .into_iter()
                    .enumerate()
                    .map(|(i, t)| (task_idx + i, t))
                    .collect();
                task_idx += rows.len();
                scroll_col.add_child(self.render_run_entry(
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
            if let Some(hint) = Self::truncated_hint(
                snapshot.runs.len(),
                RUNS_CAP,
                appearance,
                appearance.theme(),
            )
            {
                scroll_col.add_child(hint);
            }
            if let Some(hint) =
                Self::truncated_hint(
                    snapshot.tasks.len(),
                    TASKS_CAP,
                    appearance,
                    appearance.theme(),
                )
            {
                scroll_col.add_child(hint);
            }
        }

        // Pending gates
        scroll_col.add_child(self.render_gates_section(app));

        // 最近 Messages（UniformList 虚拟化）
        if !snapshot.recent_messages.is_empty() {
            Self::ensure_handles(
                &mut self.message_row_handles.borrow_mut(),
                snapshot.recent_messages.len(),
            );
            let handles = self.message_row_handles.borrow().clone();
            let msgs: Vec<(i64, String, bool, i64)> = snapshot
                .recent_messages
                .iter()
                .map(|m| {
                    (
                        m.seq,
                        format!("{} → {}: {}", m.from_handle, m.to_handle, m.subject),
                        model.selected_message().is_some_and(|s| s == m.seq),
                        m.created_at,
                    )
                })
                .collect();
            // 闭包按 'static 捕获：theme 克隆 + 字体参数 Copy
            let theme = theme.clone();
            let theme_for_list = theme.clone();
            let font_family = appearance.ui_font_family();
            let font_size = appearance.ui_font_size();
            let build = move |range: std::ops::Range<usize>, _app: &AppContext| {
                (range.start..range.end)
                    .filter_map(|i| {
                        msgs.get(i)
                            .cloned()
                            .map(|(seq, text, sel, ts)| (seq, text, sel, ts, i))
                    })
                    .map(|(seq, msg_text, is_selected, msg_ts, i)| {
                        let handle = handles[i].clone();
                        let inner = list_row(
                            &theme,
                            font_family,
                            font_size,
                            None,
                            msg_text,
                            None,
                            Some(relative_time_text(now, msg_ts)),
                        );
                        let theme = theme.clone();
                        Hoverable::new(handle, move |state| {
                            let mut container =
                                Container::new(inner).with_horizontal_padding(PANEL_PADDING);
                            if is_selected {
                                container = container
                                    .with_background(internal_colors::fg_overlay_1(&theme));
                            } else if state.is_hovered() {
                                container = container.with_background(Fill::Solid(
                                    internal_colors::neutral_3(&theme),
                                ));
                            }
                            container.finish()
                        })
                        .on_click(move |ctx, _, _| {
                            ctx.dispatch_typed_action(ObservatoryPanelAction::SelectMessage(Some(
                                seq,
                            )));
                        })
                        .finish()
                    })
                    .collect::<Vec<_>>()
                    .into_iter()
            };
            // 消息列表独立滚动口（Scrollable 有限高度约束）
            scroll_col.add_child(
                ConstrainedBox::new(self.wrap_virtual_list(
                    self.messages_scroll_state.clone(),
                    self.messages_list.clone(),
                    snapshot.recent_messages.len(),
                    build,
                    &theme_for_list,
                ))
                .with_max_height(180.)
                .finish(),
            );
            if let Some(hint) = Self::truncated_hint(
                snapshot.recent_messages.len(),
                MESSAGES_CAP,
                appearance,
                appearance.theme(),
            ) {
                scroll_col.add_child(hint);
            }
        }

        // Worker 终端输出归档（最新 5；meta 行 + tail 3 行扁平展开为虚拟化行）
        if !snapshot.archives.is_empty() {
            scroll_col.add_child(
                Text::new(
                    crate::t!("observatory-archives-title"),
                    appearance.ui_font_family(),
                    SMALL_FONT_SIZE,
                )
                .with_color(theme.nonactive_ui_text_color().into_solid())
                .finish(),
            );
            // 扁平行：每个 archive = 1 meta 行 + 最多 3 tail 行
            let archive_rows: Vec<String> = snapshot
                .archives
                .iter()
                .flat_map(|a| {
                    let mut rows = vec![crate::t!(
                        "observatory-archive-meta",
                        id = truncate_str(&a.dispatch_id, 22),
                        kind = a.kind.clone(),
                        time = relative_time_text(now, a.created_at),
                    )];
                    // tail 语义：只显示最后 3 行
                    rows.extend(
                        a.lines
                            .iter()
                            .rev()
                            .take(3)
                            .rev()
                            .map(|l| truncate_str(l, 120)),
                    );
                    rows
                })
                .collect();
            // 闭包按 'static 捕获：theme 克隆 + 字体参数 Copy
            let theme = theme.clone();
            let theme_for_list = theme.clone();
            let font_family = appearance.ui_font_family();
            let build = move |range: std::ops::Range<usize>, _app: &AppContext| {
                (range.start..range.end)
                    .filter_map(|i| archive_rows.get(i).cloned())
                    .map(|text| {
                        Text::new(text, font_family, SMALL_FONT_SIZE)
                            .with_color(theme.sub_text_color(theme.background()).into())
                            .soft_wrap(false)
                            .finish()
                    })
                    .collect::<Vec<_>>()
                    .into_iter()
            };
            scroll_col.add_child(
                ConstrainedBox::new(self.wrap_virtual_list(
                    self.archives_scroll_state.clone(),
                    self.archives_list.clone(),
                    archive_rows_len(&snapshot.archives),
                    build,
                    &theme_for_list,
                ))
                .with_max_height(120.)
                .finish(),
            );
        }

        // runs/gates 滚动口（ClippedScrollable 可裁剪任意元素树；messages/
        // archives 的 UniformList 有各自独立滚动口与有限高度约束）。
        col.add_child(
            Expanded::new(
                1.,
                ClippedScrollable::vertical(
                    self.orchestration_clipped_scroll.clone(),
                    scroll_col.finish(),
                    ScrollbarWidth::Auto,
                    theme.disabled_text_color(theme.background()).into(),
                    theme.main_text_color(theme.background()).into(),
                    ElementFill::None,
                )
                .finish(),
            )
            .finish(),
        );

        // ── 固定详情区（不随列表滚动） ──
        // 选中 task 详情 + 派发
        col.add_child(self.render_task_panel(app));

        // 选中消息详情卡片
        if let Some(detail) = model.message_detail() {
            col.add_child(self.render_message_detail(detail, appearance, theme));
        }

        // Composer
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
        let created_at =
            relative_time_text(chrono::Utc::now().timestamp(), run.created_at);
        let is_run_selected = selected_run.is_some_and(|s| s == run.id);
        let run_id = run.id.clone();

        let mut col = Flex::column()
            .with_main_axis_size(MainAxisSize::Min)
            .with_spacing(SPACING / 2.);

        // Run header（可点击选中 → composer 目标）
        let mut run_row = Flex::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(SPACING);
        run_row.add_child(
            Text::new(
                objective,
                appearance.ui_font_family(),
                appearance.ui_font_size(),
            )
            .with_color(theme.main_text_color(theme.background()).into())
            .soft_wrap(false)
            .finish(),
        );
        run_row.add_child(Expanded::new(1., Empty::new().finish()).finish());
        run_row.add_child(
            Text::new(
                created_at.clone(),
                appearance.ui_font_family(),
                appearance.ui_font_size(),
            )
            .with_color(theme.disabled_ui_text_color().into_solid())
            .finish(),
        );
        let run_header_row = run_row.finish();
        let run_header = Hoverable::new(run_handle, move |state| {
            let mut container = Container::new(run_header_row)
                .with_horizontal_padding(PANEL_PADDING)
                .with_vertical_padding(ROW_H_PADDING);
            if is_run_selected {
                container = container.with_background(internal_colors::fg_overlay_1(theme));
            } else if state.is_hovered() {
                container =
                    container.with_background(Fill::Solid(internal_colors::neutral_3(theme)));
            }
            container.finish()
        })
        .on_click(move |ctx, _, _| {
            ctx.dispatch_typed_action(ObservatoryPanelAction::SelectRun(Some(run_id.clone())));
        })
        .finish();
        col.add_child(run_header);

        // 嵌套 tasks（可点击选中；状态点 Icon + 语义色）
        for (handle_idx, task) in run_tasks {
            let handle = handles[*handle_idx].clone();
            let is_selected = selected_task.is_some_and(|s| s == task.id);
            let task_id = task.id.clone();
            let title = truncate_str(&task.title, 36);

            let mut task_row = Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(SPACING);
            // 状态点（P0-2：Icon + 颜色映射）
            task_row.add_child(status_dot_element(&task.status, theme));
            task_row.add_child(
                Text::new(
                    title,
                    appearance.ui_font_family(),
                    appearance.ui_font_size(),
                )
                .with_color(theme.sub_text_color(theme.background()).into())
                .soft_wrap(false)
                .finish(),
            );
            task_row.add_child(Expanded::new(1., Empty::new().finish()).finish());
            task_row.add_child(
                Text::new(
                    task.status.clone(),
                    appearance.ui_font_family(),
                    appearance.ui_font_size(),
                )
                .with_color(super::row::status_dot(&task.status, theme).1)
                .finish(),
            );
            // 耗时（DV11/DV18 列表行档）：完成 → 冻结终值；运行中 → 实时累计；
            // 其余未起算 → "—"。
            let now = chrono::Utc::now().timestamp();
            let duration = match task.completed_at {
                Some(c) => {
                    format_duration_row_ms(Some((c - task.created_at).max(0) as u64 * 1000))
                }
                None if matches!(
                    task.status.as_str(),
                    "running" | "claimed" | "dispatched" | "dispatching"
                ) =>
                {
                    format_duration_row_ms(Some((now - task.created_at).max(0) as u64 * 1000))
                }
                None => super::format::UNKNOWN_DASH.to_string(),
            };
            task_row.add_child(
                Text::new(
                    duration,
                    appearance.ui_font_family(),
                    appearance.ui_font_size(),
                )
                .with_color(theme.disabled_ui_text_color().into_solid())
                .soft_wrap(false)
                .finish(),
            );
            let task_row = task_row.finish();

            let theme = theme.clone();
            let hoverable = Hoverable::new(handle, move |state| {
                let mut container = Container::new(task_row)
                    .with_margin_left(PANEL_PADDING + 8.)
                    .with_horizontal_padding(4.)
                    .with_vertical_padding(2.);
                if is_selected {
                    container = container.with_background(internal_colors::fg_overlay_1(&theme));
                } else if state.is_hovered() {
                    container =
                        container.with_background(Fill::Solid(internal_colors::neutral_3(&theme)));
                }
                container.finish()
            })
            .on_click(move |ctx, _, _| {
                ctx.dispatch_typed_action(ObservatoryPanelAction::SelectTask(Some(
                    task_id.clone(),
                )));
            })
            .finish();
            col.add_child(hoverable);
        }

        Container::new(col.finish()).finish()
    }

    /// 选中 task 的详情面板：id/status + 派发按钮 + 最近 dispatch 反馈。
    fn render_task_panel(&self, app: &AppContext) -> Box<dyn Element> {
        let now = chrono::Utc::now().timestamp();
        let appearance = Appearance::as_ref(app);
        let theme = appearance.theme();
        let model = self.model.as_ref(app);
        let snapshot = model.snapshot();

        let mut col = Flex::column()
            .with_main_axis_size(MainAxisSize::Min)
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
                    Text::new(
                        truncate_str(&task.spec, 4000),
                        appearance.ui_font_family(),
                        SMALL_FONT_SIZE,
                    )
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
                    Text::new(
                        truncate_str(result, 16000),
                        appearance.ui_font_family(),
                        SMALL_FONT_SIZE,
                    )
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
                let status_color = Fill::Solid(super::row::status_dot(&d.status, theme).1);
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
                        format!(
                            "{}/{}",
                            d.status,
                            if d.state.is_empty() { "-" } else { &d.state }
                        ),
                        appearance.ui_font_family(),
                        SMALL_FONT_SIZE,
                    )
                    .with_color(status_color.into_solid())
                    .finish(),
                );
                row.add_child(Expanded::new(1., Empty::new().finish()).finish());
                row.add_child(
                    Text::new(
                        relative_time_text(now, d.created_at),
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
        let now = chrono::Utc::now().timestamp();

        let mut col = Flex::column()
            .with_main_axis_size(MainAxisSize::Min)
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
            Self::ensure_handles(&mut self.gate_option_handles.borrow_mut(), total_options);
            let row_handles = self.gate_row_handles.borrow();
            let option_handles = self.gate_option_handles.borrow();
            let mut option_idx = 0usize;

            for (i, gate) in snapshot.gates.iter().enumerate() {
                let is_selected = model.selected_gate().is_some_and(|g| g == gate.id);
                let gate_id = gate.id.clone();
                let question = truncate_str(&gate.question, 80);
                let created =
                    relative_time_text(chrono::Utc::now().timestamp(), gate.created_at);

                let mut gate_col = Flex::column().with_spacing(SPACING / 2.);
                let mut header = Flex::row()
                    .with_main_axis_size(MainAxisSize::Max)
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_spacing(SPACING);
                header.add_child(
                    Text::new(
                        question.clone(),
                        appearance.ui_font_family(),
                        appearance.ui_font_size(),
                    )
                    .with_color(theme.main_text_color(theme.background()).into())
                    .soft_wrap(false)
                    .finish(),
                );
                header.add_child(Expanded::new(1., Empty::new().finish()).finish());
                header.add_child(
                    Text::new(
                        created.clone(),
                        appearance.ui_font_family(),
                        SMALL_FONT_SIZE,
                    )
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
                            container = container
                                .with_background(Fill::Solid(internal_colors::neutral_3(theme)));
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
                        .with_vertical_padding(ROW_H_PADDING);
                    if is_selected {
                        container = container.with_background(internal_colors::fg_overlay_1(theme));
                    } else if state.is_hovered() {
                        container = container
                            .with_background(Fill::Solid(internal_colors::neutral_3(theme)));
                    }
                    container.finish()
                })
                .on_click(move |ctx, _, _| {
                    ctx.dispatch_typed_action(ObservatoryPanelAction::SelectGate(Some(
                        gate_id.clone(),
                    )));
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
                let resolution = gate.resolution.clone().unwrap_or_else(|| "-".to_string());
                let time = gate
                    .resolved_at
                    .map(|t| relative_time_text(now, t))
                    .unwrap_or_else(|| super::format::UNKNOWN_DASH.to_string());
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
            .with_main_axis_size(MainAxisSize::Min)
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
                    container =
                        container.with_border(Border::all(1.).with_border_fill(theme.accent()));
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

        // ── 外部捕获（T3: pane 级 harness 嗅探登记） ──
        let snapshot_reg = self.model.as_ref(app).snapshot();
        let now_secs = chrono::Utc::now().timestamp();
        let external_enabled = intercept.external_capture_enabled();
        let ext_chip = Hoverable::new(
            self.external_capture_chip_handle.clone(),
            move |state| {
                let text_color = if external_enabled {
                    theme.active_ui_text_color().into()
                } else if state.is_hovered() {
                    theme.nonactive_ui_text_color().into()
                } else {
                    theme.disabled_ui_text_color().into_solid()
                };
                let mut container = Container::new(
                    Text::new(
                        crate::t!("observatory-external-capture-toggle"),
                        appearance.ui_font_family(),
                        SMALL_FONT_SIZE,
                    )
                    .with_color(text_color)
                    .finish(),
                )
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(BADGE_RADIUS)))
                .with_horizontal_padding(8.)
                .with_vertical_padding(3.);
                if external_enabled {
                    container =
                        container.with_border(Border::all(1.).with_border_fill(theme.accent()));
                }
                container.finish()
            },
        )
        .on_click(move |ctx, _, _| {
            ctx.dispatch_typed_action(ObservatoryPanelAction::ToggleExternalCapture);
        })
        .finish();
        let mut ext_col = Flex::column().with_spacing(SPACING);
        ext_col.add_child(
            Text::new(
                crate::t!("observatory-external-capture-title"),
                appearance.ui_font_family(),
                appearance.ui_font_size(),
            )
            .with_color(theme.active_ui_text_color().into())
            .finish(),
        );
        ext_col.add_child(Container::new(ext_chip).finish());
        if snapshot_reg.external_registrations.is_empty() {
            ext_col.add_child(
                Text::new(
                    crate::t!("observatory-external-capture-empty"),
                    appearance.ui_font_family(),
                    SMALL_FONT_SIZE,
                )
                .with_color(theme.disabled_ui_text_color().into_solid())
                .finish(),
            );
        } else {
            for reg in &snapshot_reg.external_registrations {
                ext_col.add_child(
                    Text::new(
                        crate::t!(
                            "observatory-external-capture-row",
                            session = truncate_str(&reg.session_id, 8),
                            harness = reg.harness.clone(),
                            port = reg.proxy_port,
                            age = relative_time_text(now_secs, reg.last_activity_secs),
                        ),
                        appearance.ui_font_family(),
                        SMALL_FONT_SIZE,
                    )
                    .with_color(theme.sub_text_color(theme.background()).into())
                    .soft_wrap(false)
                    .finish(),
                );
            }
        }
        ext_col.add_child(
            Text::new(
                crate::t!("observatory-external-capture-hint"),
                appearance.ui_font_family(),
                SMALL_FONT_SIZE,
            )
            .with_color(theme.disabled_ui_text_color().into_solid())
            .soft_wrap(false)
            .finish(),
        );
        col.add_child(
            Container::new(ext_col.finish())
                .with_vertical_padding(SPACING)
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
                    format!(
                        "{} · {}",
                        label,
                        crate::t!("observatory-proxy-resolve-failed")
                    ),
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
            if auth_env.is_empty() {
                "(default)"
            } else {
                auth_env
            },
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
                        crate::t!("observatory-proxy-saved", time = absolute_time_millis(Some(ts))),
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
            .with_main_axis_size(MainAxisSize::Min)
            .with_spacing(COMPOSER_SPACING);

        // 发送目标 run（选中 run 优先，否则最新；无 run 时提示）
        let target_text = match (model.selected_run(), model.snapshot().runs.first()) {
            (Some(sel), _) => crate::t!("observatory-send-target", run = truncate_str(sel, 16),),
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

    /// 聚焦搜索框（pane focus_contents 入口）。
    pub(crate) fn focus_search(&mut self, ctx: &mut ViewContext<Self>) {
        ctx.focus(&self.search_input);
    }

    /// 截断提示行（DV24）：列表达到硬上限时显示，禁止静默丢弃。
    fn truncated_hint(
        len: usize,
        cap: usize,
        appearance: &Appearance,
        theme: &WarpTheme,
    ) -> Option<Box<dyn Element>> {
        if len < cap {
            return None;
        }
        Some(
            Container::new(
                Text::new(
                    crate::t!("observatory-list-truncated", shown = len, cap = cap),
                    appearance.ui_font_family(),
                    SMALL_FONT_SIZE,
                )
                .with_color(theme.nonactive_ui_text_color().into_solid())
                .soft_wrap(false)
                .finish(),
            )
            .with_horizontal_padding(PANEL_PADDING)
            .finish(),
        )
    }
}

impl Entity for ObservatoryPanelView {
    /// pane 体系关闭通道（header X 按钮 → PaneEvent::Close）。
    type Event = crate::pane_group::PaneEvent;
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
            ObservatoryPanelAction::SetSystemPromptMode(mode) => {
                let mode = *mode;
                self.system_prompt_view.borrow_mut().mode = mode;
                ctx.notify();
            }
            ObservatoryPanelAction::ToggleSystemPromptSegment(idx) => {
                let idx = *idx;
                let mut view = self.system_prompt_view.borrow_mut();
                if idx < view.segments.len() {
                    if !view.expanded.insert(idx) {
                        view.expanded.remove(&idx);
                    }
                    ctx.notify();
                }
            }
            ObservatoryPanelAction::ToggleAllSystemPromptSegments => {
                let mut view = self.system_prompt_view.borrow_mut();
                let total = view.segments.len();
                if view.expanded.len() == total {
                    view.expanded.clear();
                } else {
                    view.expanded = (0..total).collect();
                }
                ctx.notify();
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
            ObservatoryPanelAction::ToggleExternalCapture => {
                InterceptSessionsModel::handle(ctx).update(ctx, |model, ctx| {
                    let next = !model.external_capture_enabled();
                    model.set_external_capture_enabled(next, ctx);
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

        // Tab 内容（Expanded：root col(Max) 的有限主轴空间分配给内容，
        // 杜绝非-flex child 通道的 ∞ 约束传给内部虚拟列表）。
        match active_tab {
            ObservatoryTab::Sessions => col.add_child(
                Expanded::new(1., self.render_sessions_tab(app)).finish(),
            ),
            ObservatoryTab::Orchestration => col.add_child(
                Expanded::new(1., self.render_orchestration_tab(app)).finish(),
            ),
            ObservatoryTab::Proxy => {
                col.add_child(Expanded::new(1., self.render_proxy_tab(app)).finish())
            }
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
        // tab 模式下 pane 槽位给有限宽度约束，无需旧 panels-row 时代的
        // max_width 兜底；直接交给父级布局。
        Shrinkable::new(1., col.finish()).finish()
    }
}

// ── 辅助函数 ────────────────────────────────────────────────────────────────

/// 字符串截断，超过 max_len 字符加 "…"（row.rs 同款，保留旧调用点兼容）。
fn truncate_str(s: &str, max_len: usize) -> String {
    super::row::truncate_str(s, max_len)
}


/// 归档扁平化行数（1 meta + 最多 3 tail 行 / archive）。
fn archive_rows_len(archives: &[super::model::ArchiveRowGui]) -> usize {
    archives.iter().map(|a| 1 + a.lines.len().min(3)).sum()
}

/// DSH TrajectoryCell 对齐：block 类型 → kind 标签 + 语义色。
/// 色值全部走主题语义色（TK7：禁裸 RGB）；语义映射：
/// system 灰（spawn/system_prompt/exit）、user 绿（user_prompt）、
/// context 绿弱化 68%（prompt_segment）、message brand×error 混合
/// （response/response_chunk，chunk 弱化+缩进）、tool amber（tool_call/
/// pty_raw）、subtool amber 弱化+缩进 28px（tool_result）。
struct BlockKindStyle {
    label: String,
    color: warpui::color::ColorU,
    indent: bool,
    dim: bool,
}

fn block_kind_style(block_type: &str, theme: &WarpTheme) -> BlockKindStyle {
    let success = warp_core::ui::theme::Fill::success().into_solid();
    let warn = warp_core::ui::theme::Fill::warn().into_solid();
    let gray = internal_colors::neutral_5(theme);
    // message = brand × error 各半混合
    let accent = theme.accent().into_solid();
    let error = theme.ui_error_color();
    let message = warpui::color::ColorU::new(
        (accent.r / 2).saturating_add(error.r / 2),
        (accent.g / 2).saturating_add(error.g / 2),
        (accent.b / 2).saturating_add(error.b / 2),
        255,
    );
    let dimmed = |c: warpui::color::ColorU| warpui::color::ColorU::new(c.r, c.g, c.b, 173); // 68% (TK7 弱化)
    match block_type {
        "spawn" | "system_prompt" | "exit" => BlockKindStyle {
            label: match block_type {
                "spawn" => "SPAWN".to_string(),
                "system_prompt" => "SYSTEM".to_string(),
                _ => "EXIT".to_string(),
            },
            color: gray,
            indent: false,
            dim: false,
        },
        "prompt_segment" => BlockKindStyle {
            label: "CONTEXT".to_string(),
            color: dimmed(success),
            indent: false,
            dim: false,
        },
        "user_prompt" => BlockKindStyle {
            label: "USER".to_string(),
            color: success,
            indent: false,
            dim: false,
        },
        "response" => BlockKindStyle {
            label: "MESSAGE".to_string(),
            color: message,
            indent: false,
            dim: false,
        },
        "response_chunk" => BlockKindStyle {
            label: "CHUNK".to_string(),
            color: dimmed(message),
            indent: true,
            dim: true,
        },
        "tool_call" => BlockKindStyle {
            label: "TOOL".to_string(),
            color: warn,
            indent: false,
            dim: false,
        },
        "pty_raw" => BlockKindStyle {
            label: "PTY".to_string(),
            color: warn,
            indent: false,
            dim: false,
        },
        "tool_result" => BlockKindStyle {
            label: "RESULT".to_string(),
            color: dimmed(warn),
            indent: true,
            dim: true,
        },
        other => BlockKindStyle {
            label: "BLOCK".to_string(),
            color: gray,
            indent: false,
            dim: false,
        }
        .with_label(other),
    }
}

impl BlockKindStyle {
    fn with_label(mut self, raw: &str) -> Self {
        // 未知类型退回原始短标（≤8 字符大写，适配 80px tag 槽）。
        self.label = raw.chars().take(8).flat_map(|c| c.to_uppercase()).collect();
        self
    }
}

/// Block 时间线单行：DSH TrajectoryCell 式（80px tag 槽 + kind 色标签 +
/// 单行 preview + 右侧尺寸/时间）。行高 38px 圆角 8 左边 2px kind 色，
/// bg-layer-3；子单元（chunk/result）缩进 28px。
fn render_block_list_row(
    block: &BlockRowGui,
    font_family: warpui::fonts::FamilyId,
    font_size: f32,
    theme: &WarpTheme,
) -> Box<dyn Element> {
    let style = block_kind_style(&block.block_type, theme);
    let seq_text = crate::t!("observatory-block-seq", seq = block.sequence);
    let now = chrono::Utc::now().timestamp();

    let mut row = Flex::row()
        .with_main_axis_size(MainAxisSize::Max)
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_spacing(SPACING);
    if style.indent {
        row.add_child(ConstrainedBox::new(Empty::new().finish()).with_width(28.).finish());
    }
    // 80px 固定 tag 槽：22px 高圆角 6 的 kind 色标签（底 25% alpha，
    // 文字 kind 主色）。注意垂直 padding 必须 ≤2：text.rs 对
    // soft_wrap(false) 的 Text 有 don't-render-if-not-fit 丢弃逻辑，
    // 13px×1.2 行高=15.6，需保证 22-2×padding ≥ 15.6，否则文字不画。
    let chip = Container::new(
        Text::new(style.label, font_family, 13.)
            .with_color(style.color)
            .soft_wrap(false)
            .finish(),
    )
    .with_horizontal_padding(6.)
    .with_vertical_padding(2.)
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.)))
    .with_background(Fill::Solid(warpui::color::ColorU::new(
        style.color.r,
        style.color.g,
        style.color.b,
        64,
    )))
    .finish();
    row.add_child(
        ConstrainedBox::new(chip)
            .with_width(80.)
            .with_height(22.)
            .finish(),
    );

    row.add_child(
        Text::new(
            truncate_str(&block.preview, 40),
            font_family,
            font_size,
        )
        .with_color(
            if style.dim {
                theme.disabled_ui_text_color().into_solid()
            } else {
                theme.sub_text_color(theme.background()).into_solid()
            },
        )
        .soft_wrap(false)
        .finish(),
    );
    row.add_child(Expanded::new(1., Empty::new().finish()).finish());
    row.add_child(
        Text::new(
            format!("{} · {}", seq_text, compact_bytes(block.content_len)),
            font_family,
            font_size - 1.,
        )
        .with_color(theme.disabled_ui_text_color().into_solid())
        .soft_wrap(false)
        .finish(),
    );
    row.add_child(
        Text::new(
            relative_time_text(now, block.timestamp),
            font_family,
            font_size - 1.,
        )
        .with_color(theme.disabled_ui_text_color().into_solid())
        .soft_wrap(false)
        .finish(),
    );

    ConstrainedBox::new(
        Container::new(
            Container::new(row.finish())
                .with_horizontal_padding(8.)
                .finish(),
        )
        .with_vertical_padding((38. - font_size - 10.).max(2.) / 2.)
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.)))
        .with_background(Fill::Solid(internal_colors::neutral_2(theme)))
        .with_border(Border::left(2.).with_border_fill(Fill::Solid(style.color)))
        .finish(),
    )
    .with_height(38.)
    .finish()
}

/// Raw 流量单行（等高行 + direction 状态点 + preview + 尺寸）。
fn render_raw_list_row(
    entry: &RawRowGui,
    font_family: warpui::fonts::FamilyId,
    font_size: f32,
    theme: &WarpTheme,
) -> Box<dyn Element> {
    list_row(
        theme,
        font_family,
        font_size,
        Some(&entry.direction),
        truncate_str(&entry.preview, 48),
        Some(compact_bytes(entry.content_len)),
        Some(relative_time_text(
            chrono::Utc::now().timestamp(),
            entry.timestamp,
        )),
    )
}

/// block 类型 → badge 颜色（P0-2 起列表行走 row.rs 状态点；详情卡保留）。
fn block_type_color(block_type: &str, theme: &WarpTheme) -> warpui::color::ColorU {
    match block_type {
        "request" => theme.accent().into_solid(),
        "response" => internal_colors::neutral_6(theme),
        "event" => internal_colors::neutral_4(theme),
        "error" => theme.ui_error_color(),
        _ => theme.nonactive_ui_text_color().into_solid(),
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

#[cfg(test)]
mod system_prompt_view_tests {
    use super::*;

    #[test]
    fn fold_view_resets_on_block_or_content_change() {
        let mut v = SystemPromptFoldView::default();
        v.sync("b1", "intro\n# A\nbody\n");
        assert_eq!(v.segments.len(), 2);
        assert_eq!(v.segments[1].marker.as_deref(), Some("A"));

        // 用户操作后：切原文 + 展开一段。
        v.mode = SystemPromptViewMode::Raw;
        v.expanded.insert(0);

        // 同 block 同内容（5s 轮询重渲）：折叠状态必须保留。
        v.sync("b1", "intro\n# A\nbody\n");
        assert_eq!(v.mode, SystemPromptViewMode::Raw);
        assert!(v.expanded.contains(&0));

        // 切换 block：模式回默认折叠、展开集清空、重解析。
        v.sync("b2", "intro2\n# B\ny\n");
        assert_eq!(v.mode, SystemPromptViewMode::Folded);
        assert!(v.expanded.is_empty());
        assert_eq!(v.segments.len(), 2);
        assert_eq!(v.segments[1].marker.as_deref(), Some("B"));

        // 同 block 但内容变化（截断/更新）：同样重置。
        v.expanded.insert(1);
        v.sync("b2", "intro2\n# B\nmuch longer body\n");
        assert!(v.expanded.is_empty());
    }

    #[test]
    fn fold_view_handles_empty_content() {
        let mut v = SystemPromptFoldView::default();
        v.sync("b1", "   \n\n");
        assert!(v.segments.is_empty());
    }

    /// 端到端冒烟：真实 View 上走一遍 折叠渲染 → 切原文 → 段展开 →
    /// 全部展开 → block 切换重置（元素构建路径 + action 接线不 panic）。
    #[test]
    fn system_prompt_content_render_smoke() {
        struct TestView;
        impl Entity for TestView {
            type Event = ();
        }
        impl View for TestView {
            fn ui_name() -> &'static str {
                "ObservatoryTestView"
            }
            fn render(&self, _app: &AppContext) -> Box<dyn Element> {
                Empty::new().finish()
            }
        }
        impl TypedActionView for TestView {
            type Action = ();
        }

        let content = "You are Claude Code, Anthropic's official CLI for Claude.

# Harness
 - Tools run behind a user-selected permission mode.

<env>
Working directory: /home/yy/warpdotdev/zap
</env>
";
        let detail = BlockDetailGui {
            id: "b1".to_string(),
            session_id: "s1".to_string(),
            parent_id: None,
            harness_type: "claude-code".to_string(),
            block_type: "system_prompt".to_string(),
            sequence: 1,
            content_len: content.len(),
            content: content.to_string(),
            metadata: "{}".to_string(),
            timestamp: 0,
        };

        warpui::App::test((), |mut app| async move {
            crate::test_util::settings::initialize_settings_for_tests(&mut app);
            app.add_singleton_model(|_| crate::appearance::Appearance::mock());
            app.add_singleton_model(|_| {
                crate::settings_view::keybindings::KeybindingChangedNotifier::new()
            });
            app.add_singleton_model(
                crate::terminal::intercept_sessions::InterceptSessionsModel::new,
            );
            let model = app.add_singleton_model(ObservatoryModel::new);
            let (window_id, _) = app.add_window(
                warpui::platform::WindowStyle::NotStealFocus,
                |_| TestView,
            );
            let view =
                app.add_view(window_id, |ctx| ObservatoryPanelView::new(model.clone(), ctx));

            view.update(&mut app, |v, ctx| {
                // 默认折叠模式：渲染成功 + 解析出 preamble/Harness/env 3 段。
                {
                    let appearance = Appearance::as_ref(ctx);
                    let theme = appearance.theme();
                    let _el = v.render_system_prompt_content(&detail, appearance, theme);
                    assert_eq!(v.system_prompt_view.borrow().segments.len(), 3);
                    assert_eq!(
                        v.system_prompt_view.borrow().mode,
                        SystemPromptViewMode::Folded
                    );
                    assert!(v.system_prompt_view.borrow().expanded.is_empty());
                }

                // 切原文模式：渲染走 raw 分支，模式生效。
                v.handle_action(
                    &ObservatoryPanelAction::SetSystemPromptMode(SystemPromptViewMode::Raw),
                    ctx,
                );
                {
                    let appearance = Appearance::as_ref(ctx);
                    let theme = appearance.theme();
                    let _el = v.render_system_prompt_content(&detail, appearance, theme);
                    assert_eq!(v.system_prompt_view.borrow().mode, SystemPromptViewMode::Raw);
                }

                // 段展开 toggle：0 展开 → 再点收起。
                v.handle_action(&ObservatoryPanelAction::ToggleSystemPromptSegment(0), ctx);
                assert!(v.system_prompt_view.borrow().expanded.contains(&0));
                v.handle_action(&ObservatoryPanelAction::ToggleSystemPromptSegment(0), ctx);
                assert!(!v.system_prompt_view.borrow().expanded.contains(&0));

                // 全部展开 → 全部收起。
                v.handle_action(&ObservatoryPanelAction::ToggleAllSystemPromptSegments, ctx);
                assert_eq!(v.system_prompt_view.borrow().expanded.len(), 3);
                {
                    let appearance = Appearance::as_ref(ctx);
                    let theme = appearance.theme();
                    let _el = v.render_system_prompt_content(&detail, appearance, theme);
                }
                v.handle_action(&ObservatoryPanelAction::ToggleAllSystemPromptSegments, ctx);
                assert!(v.system_prompt_view.borrow().expanded.is_empty());

                // 越界段索引：静默 no-op（不 panic）。
                v.handle_action(&ObservatoryPanelAction::ToggleSystemPromptSegment(99), ctx);
            });
        });
    }

}