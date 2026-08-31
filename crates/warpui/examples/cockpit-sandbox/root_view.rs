//! 沙盒根视图: 复刻 workspace 双栏骨架(左 nav 面板 + 右内容区占位)。
//! 层级照抄 app/src/workspace/view.rs render_panels():
//!   Flex::row [ 左栏(宽 260, bg=theme.background) + Shrinkable(1.0, 内容占位) ]

use warpui::fonts::FamilyId;
use warpui::SingletonEntity as _;
use warpui::{
    elements::{
        ConstrainedBox, Container, Flex, MainAxisSize, ParentElement, Shrinkable, Text,
    },
    AppContext, Element, Entity, View, ViewContext, ViewHandle,
};

use warp_core::ui::appearance::Appearance;
use warp_core::ui::theme::WarpTheme;

use nav::cockpit_nav::CockpitNavView;

pub struct RootView {
    nav: ViewHandle<CockpitNavView>,
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
        // 2) 字体: Linux 用 Noto Sans(macOS 侧换 "Arial")
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
        // 4) 假快照: 两组项目 × 多状态卡片, 覆盖 nav 三态渲染路径
        inject_mock_snapshot(ctx);
        let nav = CockpitNavView::init(ctx);
        Self { nav, font_family }
    }
}

impl Entity for RootView {
    type Event = ();
}
impl warpui::TypedActionView for RootView {
    type Action = ();
}

impl View for RootView {
    fn ui_name() -> &'static str {
        "cockpit-sandbox-root"
    }

    fn render(&self, app: &AppContext) -> Box<dyn Element> {
        let theme = Appearance::as_ref(app).theme();

        let nav_pane = Container::new(
            ConstrainedBox::new(self.nav.read(app, |nav, app| nav.render(app)))
                .with_width(260.)
                .finish(),
        )
        .with_background_color(theme.background().into_solid())
        .finish();

        let content_pane = Container::new(
            Shrinkable::new(
                1.,
                Flex::column()
                    .with_main_axis_size(MainAxisSize::Max)
                    .with_child(
                        Text::new_inline(
                            "pane 内容区占位 — 布局定稿后在 app 侧挂真实 PaneGroup",
                            self.font_family,
                            13.,
                        )
                        .finish(),
                    )
                    .finish(),
            )
            .finish(),
        )
        .with_background_color(theme.surface_2().into_solid())
        .finish();

        Flex::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_child(nav_pane)
            .with_child(content_pane)
            .finish()
    }
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
