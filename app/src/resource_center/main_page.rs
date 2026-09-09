use crate::{
    auth::AuthStateProvider,
    changelog_model::ChangelogModel,
    channel::ChannelState,
    features::FeatureFlag,
    resource_center::skip_tips_and_write_to_user_defaults,
    send_telemetry_from_ctx,
    server::telemetry::TelemetryEvent,
    themes::theme::{Blend, Fill as FillTheme},
};
use pathfinder_geometry::vector::vec2f;
use warpui::{
    elements::{
        Align, ClippedScrollStateHandle, ClippedScrollable, Container, CornerRadius, Element,
        Empty, Fill, Flex, Hoverable, Icon, MainAxisAlignment, MainAxisSize, MouseStateHandle,
        ParentElement, Radius, ScrollbarWidth, Shrinkable,
    },
    platform::Cursor,
    ui_components::{
        button::{ButtonVariant, TextAndIcon, TextAndIconAlignment},
        components::{Coords, UiComponent, UiComponentStyles},
    },
    AppContext, Entity, EntityId, ModelHandle, SingletonEntity, TypedActionView, View, ViewContext,
    WindowId,
};

use crate::{
    appearance::Appearance,
    workspace::WorkspaceAction,
};

use super::TipsCompleted;

// 原 section_views 常量,随模块删除后本地化(仅保留本文件用到的)。
const SCROLLBAR_OFFSET: f32 = 7.;
const SCROLLBAR_WIDTH: ScrollbarWidth = ScrollbarWidth::Auto;
const SECTION_SPACING_BOTTOM: f32 = 24.;
const SECTION_SPACING: f32 = 12.;
const BUTTON_PADDING: f32 = 10.;
const DETAIL_FONT_SIZE: f32 = 12.;
const FOOTER_ICON_SIZE: f32 = 15.;

const SEND_SVG_PATH: &str = "bundled/svg/send.svg";

#[derive(Default)]
struct MouseStateHandles {
    copy_version: MouseStateHandle,
    invite_people: MouseStateHandle,
    skip_tips: MouseStateHandle,
}

pub enum ResourceCenterMainEvent {
    Close,
}

pub struct ResourceCenterMainView {
    button_mouse_states: MouseStateHandles,
    clipped_scroll_state: ClippedScrollStateHandle,
    tips_completed: ModelHandle<TipsCompleted>,
}

#[derive(Debug, Clone)]
pub enum ResourceCenterMainAction {
    Close,
    SkipTips,
}

impl ResourceCenterMainView {
    pub fn new(
        ctx: &mut ViewContext<Self>,
        tips_completed: ModelHandle<TipsCompleted>,
        changelog_model_handle: ModelHandle<ChangelogModel>,
    ) -> Self {
        let _ = changelog_model_handle; // 章节视图已删除; 保留签名兼容现有调用方。
        Self {
            button_mouse_states: Default::default(),
            clipped_scroll_state: Default::default(),
            tips_completed,
        }
    }

    pub fn set_action_target(
        &mut self,
        _window_id: WindowId,
        _input_id: Option<EntityId>,
        _ctx: &mut ViewContext<Self>,
    ) {
        // 章节视图已删除, 无需转发 action target。
    }

    fn render_body(&self, appearance: &Appearance) -> Box<dyn Element> {
        let theme = appearance.theme();
        // 章节视图已删除: 主页面仅保留空滚动容器占位。
        let body = Flex::column().finish();

        ClippedScrollable::vertical(
            self.clipped_scroll_state.clone(),
            body,
            SCROLLBAR_WIDTH,
            theme.disabled_text_color(theme.background()).into(),
            theme.main_text_color(theme.background()).into(),
            Fill::None,
        )
        .finish()
    }

    fn render_current_version(&self, appearance: &Appearance) -> Box<dyn Element> {
        // Use a dummy string for git release tag which is not available on local env
        let version = ChannelState::app_version().unwrap_or("v0.local.testing.string_00");

        let style = UiComponentStyles {
            font_color: Some(appearance.theme().nonactive_ui_text_color().into()),
            ..Default::default()
        };

        let text = appearance
            .ui_builder()
            .wrappable_text(version, true)
            .with_style(style)
            .build()
            .finish();

        let copy_icon = appearance
            .ui_builder()
            .copy_button(
                FOOTER_ICON_SIZE,
                self.button_mouse_states.copy_version.clone(),
            )
            .build()
            .on_click(move |ctx, _, _| {
                ctx.dispatch_typed_action(WorkspaceAction::CopyVersion(version))
            })
            .finish();

        Container::new(
            Flex::row()
                .with_child(Shrinkable::new(1., Align::new(text).left().finish()).finish())
                .with_child(Shrinkable::new(0.2, Align::new(copy_icon).finish()).finish())
                .with_main_axis_size(MainAxisSize::Max)
                .finish(),
        )
        .with_margin_left(SECTION_SPACING)
        .with_margin_bottom(BUTTON_PADDING)
        .with_uniform_padding(BUTTON_PADDING)
        .finish()
    }

    fn render_invite_button(&self, appearance: &Appearance) -> Box<dyn Element> {
        let default_styles = UiComponentStyles {
            font_size: Some(DETAIL_FONT_SIZE),
            font_family_id: Some(appearance.ui_font_family()),
            font_color: Some(appearance.theme().accent().into()),
            border_radius: Some(CornerRadius::with_all(Radius::Percentage(20.))),
            border_width: Some(1.),
            border_color: Some(appearance.theme().accent().into()),
            padding: Some(Coords {
                top: BUTTON_PADDING,
                bottom: BUTTON_PADDING,
                ..Default::default()
            }),
            ..Default::default()
        };

        let hovered_styles = UiComponentStyles {
            background: Some(appearance.theme().accent().into()),
            font_color: Some(
                appearance
                    .theme()
                    .main_text_color(appearance.theme().accent())
                    .into_solid(),
            ),
            ..default_styles
        };

        let clicked_color = appearance.theme().accent().blend(
            &FillTheme::black().with_opacity(*appearance.theme().details().button_click_opacity()),
        );
        let clicked_styles = UiComponentStyles {
            background: Some(clicked_color.into()),
            border_color: Some(clicked_color.into()),
            ..hovered_styles
        };

        Container::new(
            appearance
                .ui_builder()
                .button_with_custom_styles(
                    ButtonVariant::Outlined,
                    self.button_mouse_states.invite_people.clone(),
                    default_styles,
                    Some(hovered_styles),
                    Some(clicked_styles),
                    None,
                )
                .with_text_and_icon_label(
                    TextAndIcon::new(
                        TextAndIconAlignment::IconFirst,
                        "Invite a friend to Dais",
                        Icon::new(SEND_SVG_PATH, appearance.theme().accent()),
                        MainAxisSize::Max,
                        MainAxisAlignment::Center,
                        vec2f(FOOTER_ICON_SIZE, FOOTER_ICON_SIZE),
                    )
                    .with_inner_padding(BUTTON_PADDING),
                )
                .build()
                .on_click(|_ctx, _, _| {
                    // 去中心化分支:Refer-a-friend 入口已移除。
                })
                .finish(),
        )
        .with_margin_top(SECTION_SPACING)
        .with_margin_bottom(SECTION_SPACING_BOTTOM)
        .with_margin_left(SECTION_SPACING + SCROLLBAR_OFFSET)
        .with_margin_right(SECTION_SPACING + SCROLLBAR_OFFSET)
        .finish()
    }

    fn render_skip_tips_button(&self, appearance: &Appearance) -> Box<dyn Element> {
        Container::new(
            Align::new(
                Hoverable::new(self.button_mouse_states.skip_tips.clone(), |state| {
                    let text_color = if state.is_hovered() {
                        appearance.theme().active_ui_text_color().into_solid()
                    } else {
                        appearance.theme().nonactive_ui_text_color().into_solid()
                    };

                    let style = UiComponentStyles {
                        font_size: Some(DETAIL_FONT_SIZE),
                        font_color: Some(text_color),
                        ..Default::default()
                    };

                    appearance
                        .ui_builder()
                        .wrappable_text("Mark all as read", false)
                        .with_style(style)
                        .build()
                        .finish()
                })
                .on_click(move |ctx, _, _| {
                    ctx.dispatch_typed_action(ResourceCenterMainAction::SkipTips)
                })
                .with_cursor(Cursor::PointingHand)
                .finish(),
            )
            .right()
            .finish(),
        )
        .with_margin_bottom(SECTION_SPACING)
        .with_margin_right(SCROLLBAR_OFFSET + SECTION_SPACING)
        .finish()
    }
}

impl Entity for ResourceCenterMainView {
    type Event = ResourceCenterMainEvent;
}

impl TypedActionView for ResourceCenterMainView {
    type Action = ResourceCenterMainAction;

    fn handle_action(&mut self, action: &ResourceCenterMainAction, ctx: &mut ViewContext<Self>) {
        use ResourceCenterMainAction::*;
        match action {
            Close => {
                ctx.notify();
                ctx.emit(ResourceCenterMainEvent::Close);
            }
            SkipTips => {
                send_telemetry_from_ctx!(TelemetryEvent::ResourceCenterTipsSkipped, ctx);
                self.tips_completed.update(ctx, |tips_completed, ctx| {
                    skip_tips_and_write_to_user_defaults(tips_completed, ctx);
                    ctx.notify();
                });
            }
        }
    }
}

impl View for ResourceCenterMainView {
    fn ui_name() -> &'static str {
        "ResourceCenterMain"
    }

    fn render(&self, app: &AppContext) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        let body = self.render_body(appearance);
        let invite_button = self.render_invite_button(appearance);
        let skip_tips = self.render_skip_tips_button(appearance);

        let mut main_page = Flex::column();

        if !AuthStateProvider::as_ref(app)
            .get()
            .is_anonymous_or_logged_out()
            && !FeatureFlag::AvatarInTabBar.is_enabled()
        {
            main_page = main_page.with_child(invite_button);
        }

        if !self.tips_completed.as_ref(app).skipped_or_completed
            && !FeatureFlag::AvatarInTabBar.is_enabled()
        {
            main_page.add_child(skip_tips);
        }

        main_page = main_page
            .with_child(Shrinkable::new(20., body).finish())
            .with_child(Shrinkable::new(0.1, Empty::new().finish()).finish()); // placeholder to ensure pane extends to bottom of the window

        if FeatureFlag::Autoupdate.is_enabled() && ChannelState::show_autoupdate_menu_items() {
            let current_version = self.render_current_version(appearance);
            main_page.add_child(current_version);
        }

        main_page.finish()
    }
}
