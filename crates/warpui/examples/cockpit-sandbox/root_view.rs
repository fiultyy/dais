//! 沙盒根视图: 多页预览壳。
//! 左侧 = 页面切换条(垂直 tab: cockpit-nav / observatory / cockpit);
//! 右侧 = 当前页画布。
//! 层级照抄 app/src/workspace/view.rs render_panels() 的双栏骨架。

use warpui::fonts::FamilyId;
use warpui::SingletonEntity as _;
use warpui::{
    elements::{
        ChildView, ConstrainedBox, Container, CornerRadius, CrossAxisAlignment, Empty, Expanded,
        Fill as ElementFill, Flex, Hoverable, MainAxisSize, MouseStateHandle, ParentElement,
        Shrinkable, Text,
    },
    scene::Radius,
    AppContext, Element, Entity, TypedActionView, View, ViewContext, ViewHandle,
};

use warp_core::ui::appearance::Appearance;
use warp_core::ui::theme::WarpTheme;

use nav::cockpit_nav::CockpitNavView;

// ── 页面切换条规格(Orca 化轻量 tab,同 nav 组头语言) ──

/// 切换条宽度。
const TAB_BAR_WIDTH: f32 = 44.;
/// 切换图标尺寸。
const TAB_ICON_SIZE: f32 = 16.;
/// 页面清单(垂直从上到下)。
const PAGES: &[Page] = &[
    Page {
        id: PageId::CockpitNav,
        label: "Nav",
        icon: warp_core::ui::icons::Icon::Rocket,
    },
    Page {
        id: PageId::Observatory,
        label: "Obs",
        icon: warp_core::ui::icons::Icon::Eye,
    },
    Page {
        id: PageId::Cockpit,
        label: "Cp",
        icon: warp_core::ui::icons::Icon::Terminal,
    },
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PageId {
    /// 左栏导航(本次 Orca 化重设计主角)。
    CockpitNav,
    /// 观测台面板真身(app crate)。
    Observatory,
    /// 驾驶舱面板真身(app crate;与 nav 共享 CockpitModel 单例)。
    Cockpit,
}

struct Page {
    id: PageId,
    label: &'static str,
    icon: warp_core::ui::icons::Icon,
}

pub struct RootView {
    nav: ViewHandle<CockpitNavView>,
    observatory: Option<ViewHandle<warp::ObservatoryPanelView>>,
    cockpit_panel: Option<ViewHandle<warp::CockpitPanelView>>,
    current: PageId,
    tab_handles: Vec<MouseStateHandle>,
    font_family: FamilyId,
}

impl RootView {
    pub fn new(ctx: &mut ViewContext<Self>) -> Self {
        // 1) 主题单例: mock 底座 + app Dark 主题字面值(同 app/src/themes/default_themes.rs)
        // 先注册单例再改主题(沙盒自建,无 app 侧初始化链)
        let appearance = ctx.add_singleton_model(|_ctx| Appearance::mock());
        appearance.update(ctx, |appearance, ctx| {
            appearance.set_theme(dark_theme_literal(), ctx);
        });
        // 2) 字体: Linux 用 Noto Sans CJK SC,兜底 DejaVu
        let font_family = warpui::fonts::Cache::handle(ctx)
            .update(ctx, |cache, _| {
                cache
                    .load_system_font("Noto Sans CJK SC")
                    .or_else(|_| cache.load_system_font("DejaVu Sans"))
                    .expect("no usable system font")
            });
        // 3) 把真实字体族写回 Appearance(mock 的 FamilyId(0)/(1) 不在 Cache 注册,
        //    渲染时 select_font 会 panic "Font family must exist")
        let appearance = Appearance::handle(ctx);
        appearance.update(ctx, |appearance, ctx| {
            appearance.set_ui_font_family(font_family, ctx);
            appearance.set_ai_font_family(font_family, ctx);
            appearance.set_monospace_font_family(font_family, ctx);
        });
        // 4) 假快照: 两组项目 × 多状态卡片, 覆盖 nav 三态渲染路径(nav 与
        //    cockpit 面板共享同一 CockpitModel 单例,面板页同享这份假数据)
        inject_mock_snapshot(ctx);
        let nav = CockpitNavView::init(ctx);
        // 5) app 面板真身按需挂载(构造即注册 action handler,成本高,首次切到再建)
        let me = Self {
            nav,
            observatory: None,
            cockpit_panel: None,
            current: PageId::CockpitNav,
            tab_handles: (0..PAGES.len()).map(|_| MouseStateHandle::default()).collect(),
            font_family,
        };
        me
    }

    /// 惰性挂载 app 面板真身。
    fn ensure_panels(&mut self, ctx: &mut ViewContext<Self>) {
        if self.observatory.is_none() {
            let model = warp::ObservatoryModel::handle(ctx);
            model.update(ctx, |m, ctx| {
                m.set_panel_open(true, ctx);
                m.refresh(ctx);
            });
            let view = ctx.add_typed_action_view(|ctx| {
                warp::ObservatoryPanelView::new(model.clone(), ctx)
            });
            self.observatory = Some(view);
        }
        if self.cockpit_panel.is_none() {
            let model = cockpit_model::CockpitModel::handle(ctx);
            let view = ctx.add_typed_action_view(|ctx| {
                warp::CockpitPanelView::new(model.clone(), ctx)
            });
            self.cockpit_panel = Some(view);
        }
    }
}

impl Entity for RootView {
    type Event = ();
}
impl warpui::TypedActionView for RootView {
    type Action = PageId;

    fn handle_action(&mut self, action: &Self::Action, ctx: &mut ViewContext<Self>) {
        self.current = *action;
        // app 面板真身首次切到时才挂载(构造即注册 action handler,有成本)。
        self.ensure_panels(ctx);
        ctx.notify();
    }
}

impl View for RootView {
    fn ui_name() -> &'static str {
        "cockpit-sandbox-root"
    }

    fn render(&self, app: &AppContext) -> Box<dyn Element> {
        let theme = Appearance::as_ref(app).theme();

        // 页面切换条(垂直图标 tab,Orca rail 语言: 无边框+hover wash+选中 wash)。
        let mut tab_col = Flex::column()
            .with_main_axis_alignment(warpui::elements::MainAxisAlignment::Start)
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(4.);
        tab_col.add_child(
            Container::new(Empty::new().finish())
                .with_vertical_padding(6.)
                .finish(),
        );
        for (idx, page) in PAGES.iter().enumerate() {
            let handle = self
                .tab_handles
                .get(idx)
                .cloned()
                .unwrap_or_default();
            let selected = self.current == page.id;
            let icon = page.icon;
            let label = page.label;
            let text_color = if selected {
                theme.main_text_color(theme.background()).into_solid()
            } else {
                theme.sub_text_color(theme.background()).into_solid()
            };
            let icon_fill = warp_core::ui::theme::Fill::Solid(text_color);
            let page_id = page.id;
            let content = move |hovered: bool| {
                let mut col = Flex::column()
                    .with_main_axis_size(MainAxisSize::Max)
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_spacing(2.);
                col.add_child(
                    ConstrainedBox::new(
                        icon.to_warpui_icon(icon_fill).finish(),
                    )
                    .with_width(TAB_ICON_SIZE)
                    .with_height(TAB_ICON_SIZE)
                    .finish(),
                );
                col.add_child(
                    Text::new(label, self.font_family, 9.)
                        .with_color(text_color)
                        .finish(),
                );
                let mut container = Container::new(col.finish())
                    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.)));
                if selected {
                    let a = theme.accent().into_solid();
                    container = container.with_background_color(warpui::color::ColorU::new(a.r, a.g, a.b, 38));
                } else if hovered {
                    container = container.with_background(theme.surface_overlay_1());
                }
                container.finish()
            };
            tab_col.add_child(
                Hoverable::new(handle, move |state| content(state.is_hovered()))
                    .on_click(move |ctx, _app, _pos| {
                        ctx.dispatch_typed_action(page_id);
                    })
                    .with_cursor(warpui::platform::Cursor::PointingHand)
                    .finish(),
            );
        }
        let tab_bar = Container::new(
            ConstrainedBox::new(tab_col.finish())
                .with_width(TAB_BAR_WIDTH)
                .finish(),
        )
        .with_background_color(theme.background().into())
        .finish();

        // 当前页画布。
        let content_pane: Box<dyn Element> = match self.current {
            PageId::CockpitNav => ConstrainedBox::new(ChildView::new(&self.nav).finish())
                .with_width(260.)
                .finish(),
            PageId::Observatory => match &self.observatory {
                Some(view) => ChildView::new(view).finish(),
                None => placeholder("Observatory(首次切换后挂载)".to_string(), self.font_family, &theme),
            },
            PageId::Cockpit => match &self.cockpit_panel {
                Some(view) => ChildView::new(view).finish(),
                None => placeholder("Cockpit 面板(首次切换后挂载)".to_string(), self.font_family, &theme),
            },
        };

        let mut canvas_col = Flex::column()
            .with_main_axis_size(MainAxisSize::Max)
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch);
        canvas_col.add_child(content_pane);
        let canvas = Container::new(canvas_col.finish())
            .with_background_color(theme.surface_2().into())
            .finish();

        Flex::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_child(tab_bar)
            .with_child(canvas)
            .finish()
    }
}


fn placeholder(text: String, font: FamilyId, theme: &WarpTheme) -> Box<dyn Element> {
    Flex::column()
        .with_main_axis_alignment(warpui::elements::MainAxisAlignment::Center)
        .with_main_axis_size(MainAxisSize::Max)
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_child(
            Text::new(text, font, 13.)
                .with_color(theme.disabled_ui_text_color().into_solid())
                .finish(),
        )
        .finish()
}

// ── app Dark 主题字面值(app/src/themes/default_themes.rs dark_theme() 同源拷贝) ──

fn dark_theme_literal() -> WarpTheme {
    use warp_core::ui::theme::{AnsiColor, AnsiColors, Details, Fill, TerminalColors};
    use warpui::color::ColorU;

    const DARK_NORMAL: AnsiColors = AnsiColors::new(
        AnsiColor::from_u32(0x616161FF),
        AnsiColor::from_u32(0xFF8272FF),
        AnsiColor::from_u32(0xB4FA72FF),
        AnsiColor::from_u32(0xFEFDC2FF),
        AnsiColor::from_u32(0xA5D5FEFF),
        AnsiColor::from_u32(0xFF8FFDFF),
        AnsiColor::from_u32(0xD0D1FEFF),
        AnsiColor::from_u32(0xF1F1F1FF),
    );
    const DARK_BRIGHT: AnsiColors = AnsiColors::new(
        AnsiColor::from_u32(0x8E8E8EFF),
        AnsiColor::from_u32(0xFFC4BDFF),
        AnsiColor::from_u32(0xD6FCB9FF),
        AnsiColor::from_u32(0xFEFDD5FF),
        AnsiColor::from_u32(0xC1E3FEFF),
        AnsiColor::from_u32(0xFFB1FEFF),
        AnsiColor::from_u32(0xE5E6FEFF),
        AnsiColor::from_u32(0xFEFFFFFF),
    );

    WarpTheme::new(
        Fill::Solid(ColorU::from_u32(0x000000FF)),
        ColorU::from_u32(0xffffffff),
        Fill::Solid(ColorU::from_u32(0x19AAD8FF)),
        None,
        Some(Details::Darker),
        TerminalColors::new(DARK_NORMAL, DARK_BRIGHT),
        None,
        Some("Dark".to_string()),
        None,
    )
}

// ── 假快照(覆盖 nav 渲染三态: agent 运行 / 普通 shell / 等待输入) ──

fn inject_mock_snapshot(ctx: &mut AppContext) {
    use cockpit_model::{CockpitCard, CockpitCardStatus, CockpitModel};

    let model = ctx.add_singleton_model(CockpitModel::new);
    model.update(ctx, |m, ctx| {
        let mk = |id: u64,
                  title: &str,
                  cwd: &str,
                  agent: Option<&'static str>,
                  recap: Option<&str>,
                  status: CockpitCardStatus,
                  branch: Option<&str>| CockpitCard {
            terminal_view_id: { let _ = id; warpui::EntityId::new() },
            title: title.to_string(),
            cwd: Some(cwd.to_string()),
            agent_name: agent,
            recap: recap.map(|s| s.to_string()),
            tool_name: agent.map(|_| "Edit".to_string()),
            status,
            branch: branch.map(|s| s.to_string()),
            connected: false,
            writable: true,
        };
        let cards = vec![
            mk(1, "agent 主力", "~/projects/dais", Some("Claude Code"),
               Some("重构 pane 布局: 拆 render_panels"),
               CockpitCardStatus::Working, Some("feat/gui-cockpit-nav")),
            mk(2, "shell", "~/projects/dais", None, None,
               CockpitCardStatus::Busy, Some("main")),
            mk(3, "agent-2 调研", "~/projects/memory-service", Some("Codex"),
               Some("调研 tantivy 0.26 schema API"),
               CockpitCardStatus::Blocked(None), None),
            mk(4, "scratch", "~/tmp", None, Some("cargo test -p warp --lib"),
               CockpitCardStatus::Working, None),
            mk(5, "只读查看端", "~/projects/warp", None, None,
               CockpitCardStatus::Busy, Some("master")),
        ];
        m.replace_snapshot(cards, 1, ctx);
        m.set_group_by(cockpit_model::CockpitGroupBy::CwdProject, ctx);
    });
}
