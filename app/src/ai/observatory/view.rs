//! 观测台面板视图 — ObservatoryPanelView
//!
//! 全部用户交互经 `ModelHandle<ObservatoryModel>` 派发，视图不持有业务状态，
//! 仅维护渲染缓存（鼠标悬停句柄、子输入框句柄等纯 UI 状态）。

use std::cell::RefCell;

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


use harness_integration::InterceptMode;

use crate::view_components::action_button::{ActionButton, ButtonSize};
use crate::view_components::{SubmittableTextInput, SubmittableTextInputEvent};
use crate::ai::blocklist::agent_view::agent_input_footer::AgentInputButtonTheme;

use super::model::{
    DraftField, ObservatoryModel, ObservatoryTab, BlockRowGui,
    RunRowGui, SessionRowGui, TaskRowGui,
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


// ── Action ────────────────────────────────────────────────────────────────────

/// 面板视图的 typed action，由 on_click 分发、handle_action 处理。
#[derive(Clone, Debug)]
pub enum ObservatoryPanelAction {
    Refresh,
    SendMessage,
    SetTab(ObservatoryTab),
    SelectSession(Option<String>),
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
    /// Tab 切换行的鼠标句柄：[Sessions, Orchestration]。
    tab_handles: [MouseStateHandle; 2],
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
}

impl ObservatoryPanelView {
    pub fn new(model: ModelHandle<ObservatoryModel>, ctx: &mut ViewContext<Self>) -> Self {
        // 刷新按钮
        let refresh_button = ctx.add_typed_action_view(|ctx| {
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
                to_input.update(ctx, |input, ctx| {
                    let editor = input.editor().clone();
                    editor.update(ctx, |ed, ctx| ed.set_buffer_text("", ctx));
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
                    editor.update(ctx, |ed, ctx| ed.set_buffer_text("", ctx));
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
                    editor.update(ctx, |ed, ctx| ed.set_buffer_text("", ctx));
                });
            }
        });

        // 发送按钮
        let send_button = ctx.add_typed_action_view(|ctx| {
            ActionButton::new(crate::t!("observatory-send"), AgentInputButtonTheme)
                .with_size(ButtonSize::AgentInputButton)
                .on_click(|ctx| {
                    ctx.dispatch_typed_action(ObservatoryPanelAction::SendMessage);
                })
        });

        // 订阅 model 事件 → 重绘
        ctx.subscribe_to_model(&model, |_me, _handle, _event, ctx| {
            ctx.notify();
        });

        Self {
            model,
            refresh_button,
            session_row_handles: RefCell::new(Vec::new()),
            block_row_handles: RefCell::new(Vec::new()),
            tab_handles: [MouseStateHandle::default(), MouseStateHandle::default()],
            draft_to_input,
            draft_subject_input,
            draft_body_input,
            send_button,
            message_row_handles: RefCell::new(Vec::new()),
        }
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

    /// Tab 切换行: Sessions / Orchestration（可点击文字标签）。
    fn render_tab_bar(&self, app: &AppContext) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        let theme = appearance.theme();
        let active_tab = self.model.as_ref(app).active_tab();

        let sessions_active = active_tab == ObservatoryTab::Sessions;
        let orchestration_active = active_tab == ObservatoryTab::Orchestration;

        // Sessions tab
        let sessions_text = crate::t!("observatory-tab-sessions");
        let sessions_handle = self.tab_handles[0].clone();
        let sessions_hoverable = Hoverable::new(sessions_handle, move |state| {
            let text_color = if sessions_active {
                theme.active_ui_text_color().into()
            } else if state.is_hovered() {
                theme.nonactive_ui_text_color().into()
            } else {
                theme.disabled_ui_text_color().into_solid()
            };
            let mut container = Container::new(
                Text::new(sessions_text.clone(), appearance.ui_font_family(), appearance.ui_font_size())
                    .with_color(text_color)
                    .finish(),
            )
            .with_horizontal_padding(TAB_H_PADDING)
            .with_vertical_padding(TAB_V_PADDING);
            if sessions_active {
                container = container.with_border(Border::bottom(2.).with_border_fill(theme.accent()));
            }
            container.finish()
        })
        .on_click(|ctx, _, _| {
            ctx.dispatch_typed_action(ObservatoryPanelAction::SetTab(ObservatoryTab::Sessions));
        })
        .finish();

        // Orchestration tab
        let orch_text = crate::t!("observatory-tab-orchestration");
        let orch_handle = self.tab_handles[1].clone();
        let orch_hoverable = Hoverable::new(orch_handle, move |state| {
            let text_color = if orchestration_active {
                theme.active_ui_text_color().into()
            } else if state.is_hovered() {
                theme.nonactive_ui_text_color().into()
            } else {
                theme.disabled_ui_text_color().into_solid()
            };
            let mut container = Container::new(
                Text::new(orch_text.clone(), appearance.ui_font_family(), appearance.ui_font_size())
                    .with_color(text_color)
                    .finish(),
            )
            .with_horizontal_padding(TAB_H_PADDING)
            .with_vertical_padding(TAB_V_PADDING);
            if orchestration_active {
                container = container.with_border(Border::bottom(2.).with_border_fill(theme.accent()));
            }
            container.finish()
        })
        .on_click(|ctx, _, _| {
            ctx.dispatch_typed_action(ObservatoryPanelAction::SetTab(ObservatoryTab::Orchestration));
        })
        .finish();

        let mut row = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(SPACING);
        row.add_child(sessions_hoverable);
        row.add_child(orch_hoverable);

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
                    let block_el = self.render_block_row(block, appearance, theme);
                    let hoverable = Hoverable::new(handle, move |state| {
                        let mut container = Container::new(block_el)
                            .with_horizontal_padding(PANEL_PADDING)
                            .with_vertical_padding(ROW_V_PADDING);
                        if state.is_hovered() {
                            container = container.with_background(Fill::Solid(internal_colors::neutral_3(theme)));
                        }
                        container.finish()
                    })
                    .finish();
                    col.add_child(hoverable);
                }
            }
        }

        Shrinkable::new(1., col.finish()).finish()
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

        row.finish()
    }

    /// Orchestration tab 内容: runs + tasks + messages + composer。
    fn render_orchestration_tab(&self, app: &AppContext) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        let theme = appearance.theme();
        let model = self.model.as_ref(app);
        let snapshot = model.snapshot();

        let mut col = Flex::column()
            .with_main_axis_size(MainAxisSize::Max)
            .with_spacing(SECTION_SPACING);

        // ── Runs + Tasks ──
        if snapshot.runs.is_empty() {
            col.add_child(self.render_empty_state(
                &crate::t!("observatory-runs-empty"),
                appearance,
                theme,
            ));
        } else {
            for run in &snapshot.runs {
                col.add_child(self.render_run_entry(run, &snapshot.tasks, appearance, theme));
            }
        }

        // ── 最近 Messages ──
        if !snapshot.recent_messages.is_empty() {
            Self::ensure_handles(
                &mut self.message_row_handles.borrow_mut(),
                snapshot.recent_messages.len(),
            );
            let handles = self.message_row_handles.borrow();
            for (i, msg) in snapshot.recent_messages.iter().enumerate() {
                let handle = handles[i].clone();
                let msg_text = format!("{} → {}: {}", msg.from_handle, msg.to_handle, msg.subject);
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
                    if state.is_hovered() {
                        container = container.with_background(Fill::Solid(internal_colors::neutral_3(theme)));
                    }
                    container.finish()
                })
                .finish();
                col.add_child(hoverable);
            }
        }

        // ── Composer ──
        col.add_child(self.render_composer(app));

        // ── 错误态 ──
        if let Some(err) = model.last_error() {
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

    /// 单个 run 及其下属 tasks 渲染。
    fn render_run_entry(
        &self,
        run: &RunRowGui,
        tasks: &[TaskRowGui],
        appearance: &Appearance,
        theme: &WarpTheme,
    ) -> Box<dyn Element> {
        let objective = truncate_str(&run.objective, 40);
        let created_at = &run.created_at;

        let mut col = Flex::column()
            .with_main_axis_size(MainAxisSize::Max)
            .with_spacing(SPACING / 2.);

        // Run header
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
        col.add_child(
            Container::new(run_row.finish())
                .with_horizontal_padding(PANEL_PADDING)
                .finish(),
        );

        // 嵌套 tasks
        let run_tasks: Vec<&TaskRowGui> = tasks
            .iter()
            .filter(|t| t.run_id == run.id)
            .collect();
        for task in run_tasks {
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
            col.add_child(
                Container::new(task_row.finish())
                    .with_margin_left(PANEL_PADDING + 8.)
                    .finish(),
            );
        }

        Container::new(col.finish()).finish()
    }

    /// Composer 区域: draft_to / subject / body 输入框 + 发送按钮。
    fn render_composer(&self, app: &AppContext) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        let model = self.model.as_ref(app);
        let busy = model.busy();

        let mut col = Flex::column()
            .with_main_axis_size(MainAxisSize::Max)
            .with_spacing(COMPOSER_SPACING);

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

// ── Entity / View ────────────────────────────────────────────────────────────

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
                let id = id.clone();
                ObservatoryModel::handle(ctx).update(ctx, |model, ctx| {
                    model.select_session(id, ctx);
                });
            }
        }
    }
}

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
