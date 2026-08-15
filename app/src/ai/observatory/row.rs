//! 观测台行构建 helper — 等高行 + 状态点 + 主文本 + 右对齐辅助列（P0-2）。
//!
//! UniformList 要求等高行：所有行高固定为 [`LIST_ROW_HEIGHT`]，行内文本单行
//! ellipsis 截断。状态点映射复用 `AgentRunDisplayStatus::status_icon_and_color`
//! 的「Icon + 语义色」思想（见 agent_conversations_model.rs），但按观测台
//! 的字符串 status/state 枚举建独立映射表（单测覆盖）。

use warp_core::ui::icons::Icon;
use warp_core::ui::theme::color::internal_colors;
use warp_core::ui::theme::{Fill, WarpTheme};
use warpui::color::ColorU;
use warpui::elements::{
    ConstrainedBox, Container, CrossAxisAlignment, Element, Empty, Expanded, Flex, MainAxisSize,
    ParentElement, Text,
};
use warpui::fonts::FamilyId;

/// 列表行高（UniformList 等高行；28-32px 体系取 30）。
pub const LIST_ROW_HEIGHT: f32 = 30.;
/// 详情区头行高（36px 体系）。
pub const DETAIL_HEADER_HEIGHT: f32 = 36.;
/// 状态点图标尺寸。
pub const STATUS_ICON_SIZE: f32 = 12.;
/// 状态点与主文本间距。
pub const STATUS_TEXT_SPACING: f32 = 6.;
/// 行右对齐辅助列与主文本的间距。
pub const AUX_COL_SPACING: f32 = 8.;

/// 观测台行状态点：Icon + 语义色（theme 语义 accessor / ansi 语义色）。
pub fn status_dot(status: &str, theme: &WarpTheme) -> (Icon, ColorU) {
    match status {
        // task.status
        "completed" => (Icon::Check, theme.ansi_fg_green()),
        "failed" => (Icon::Triangle, theme.ansi_fg_red()),
        "ready" => (Icon::ClockLoader, theme.ansi_fg_yellow()),
        "dispatched" | "dispatching" => (Icon::ClockLoader, theme.ansi_fg_blue()),
        "blocked" => (Icon::StopFilled, theme.ansi_fg_yellow()),
        "pending" => (Icon::ClockLoader, theme.ansi_fg_magenta()),
        "running" | "claimed" => (Icon::ClockLoader, theme.ansi_fg_magenta()),
        "cancelled" => (
            Icon::Cancelled,
            theme.disabled_text_color(theme.background()).into_solid(),
        ),
        // gate.status
        "resolved" => (Icon::Check, theme.ansi_fg_green()),
        "timeout" => (Icon::Triangle, theme.ansi_fg_yellow()),
        // worker dispatch state（worker.rs 状态机）
        "starting" => (Icon::ClockLoader, theme.ansi_fg_yellow()),
        "done" | "succeeded" => (Icon::Check, theme.ansi_fg_green()),
        "error" | "failed_setup" => (Icon::Triangle, theme.ansi_fg_red()),
        _ => (Icon::Circle, internal_colors::neutral_5(theme)),
    }
}

/// 状态点元素（固定尺寸小图标）。
pub fn status_dot_element(status: &str, theme: &WarpTheme) -> Box<dyn warpui::elements::Element> {
    let (icon, color) = status_dot(status, theme);
    ConstrainedBox::new(icon.to_warpui_icon(Fill::Solid(color)).finish())
        .with_width(STATUS_ICON_SIZE)
        .with_height(STATUS_ICON_SIZE)
        .finish()
}

/// 通用观测台列表行：状态点（可选）+ 主文本 + 右对齐辅助列 + 辅助文本。
///
/// 主文本经 `soft_wrap(false)` 单行截断；辅助列右对齐（aux_color 较弱）。
/// 参数取 owned 值（theme 克隆 + 字体族/字号），便于 UniformList 的
/// build_items 闭包按 'static 捕获。
#[allow(clippy::too_many_arguments)]
pub fn list_row(
    theme: &WarpTheme,
    font_family: FamilyId,
    font_size: f32,
    status: Option<&str>,
    main_text: String,
    aux_text: Option<String>,
    trailing_text: Option<String>,
) -> Box<dyn warpui::elements::Element> {
    let mut row = Flex::row()
        .with_main_axis_size(MainAxisSize::Max)
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_spacing(STATUS_TEXT_SPACING);

    if let Some(status) = status {
        row.add_child(status_dot_element(status, theme));
    }
    row.add_child(
        Text::new(main_text, font_family, font_size)
            .with_color(theme.main_text_color(theme.background()).into())
            .soft_wrap(false)
            .finish(),
    );
    row.add_child(Expanded::new(1., Empty::new().finish()).finish());
    if let Some(aux) = aux_text {
        row.add_child(
            Text::new(aux, font_family, font_size)
                .with_color(theme.sub_text_color(theme.background()).into())
                .soft_wrap(false)
                .finish(),
        );
    }
    if let Some(trailing) = trailing_text {
        row.add_child(
            Text::new(trailing, font_family, font_size - 1.)
                .with_color(theme.disabled_ui_text_color().into_solid())
                .soft_wrap(false)
                .finish(),
        );
    }

    // 等高行：行内容垂直居中，整行钳到 LIST_ROW_HEIGHT。
    ConstrainedBox::new(
        Container::new(row.finish())
            .with_vertical_padding((LIST_ROW_HEIGHT - font_size - 6.).max(2.) / 2.)
            .finish(),
    )
    .with_height(LIST_ROW_HEIGHT)
    .finish()
}

/// 字符串截断，超过 max_len 字符加 "…"（单行 ellipsis 策略，DSH 同款）。
pub fn truncate_str(s: &str, max_len: usize) -> String {
    if s.chars().count() <= max_len {
        s.to_string()
    } else {
        let truncated: String = s.chars().take(max_len).collect();
        format!("{}…", truncated)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 构造默认主题（映射表只需语义色可寻址）。
    fn test_theme() -> WarpTheme {
        warp_core::ui::appearance::Appearance::mock()
            .theme()
            .clone()
    }

    /// 状态点映射表：每个已知枚举值有确定 Icon；未知值回落 Circle。
    #[test]
    fn test_status_dot_known_and_unknown() {
        let theme = test_theme();
        for known in [
            "completed",
            "failed",
            "ready",
            "dispatched",
            "blocked",
            "pending",
            "running",
            "cancelled",
            "resolved",
            "timeout",
            "starting",
            "done",
            "error",
        ] {
            let (icon, color) = status_dot(known, &theme);
            assert_ne!(icon, Icon::Circle, "known status {known} should map");
            // 颜色确定且非全透明（可见性；mock 主题的 disabled 色带透明度，只查 >0）
            assert!(color.a > 0, "status {known} color should be visible");
        }
        let (icon, _) = status_dot("whatever-unknown", &theme);
        assert_eq!(icon, Icon::Circle);
    }

    /// 状态语义分组：完成类绿、失败类红、等待类黄/紫。
    #[test]
    fn test_status_dot_color_semantics() {
        let theme = test_theme();
        let (_, green) = status_dot("completed", &theme);
        let (_, red) = status_dot("failed", &theme);
        let (_, yellow) = status_dot("blocked", &theme);
        assert_ne!(green, red);
        assert_ne!(red, yellow);
        assert_ne!(green, yellow);
        // resolved 与 completed 同为完成语义 → 同色
        assert_eq!(status_dot("resolved", &theme).1, green);
    }

    #[test]
    fn test_truncate_str() {
        assert_eq!(truncate_str("abc", 5), "abc");
        assert_eq!(truncate_str("abcdef", 5), "abcde…");
        // 多字节安全
        assert_eq!(truncate_str("你好世界呀", 4), "你好世界…");
    }

    /// P0-1 虚拟化自证：500 行场景下 build_items 只构建可见行
    /// （≈ 视口高度 / 行高 + 1），而非全量 500 行。
    #[test]
    fn test_uniform_list_virtualizes_500_items() {
        use std::cell::Cell;
        use std::rc::Rc;
        use warpui::elements::{Empty, UniformList, UniformListState};
        use warpui::{platform::WindowStyle, App, Presenter, WindowInvalidation};

        /// 测试 root view：render 输出 500 行 UniformList（等高 30px 行）。
        struct VirtualizationTestView {
            built: Rc<Cell<usize>>,
        }
        impl warpui::Entity for VirtualizationTestView {
            type Event = ();
        }
        impl warpui::View for VirtualizationTestView {
            fn ui_name() -> &'static str {
                "VirtualizationTestView"
            }
            fn render(&self, _app: &warpui::AppContext) -> Box<dyn warpui::elements::Element> {
                let built = self.built.clone();
                let build = move |range: std::ops::Range<usize>, _app: &warpui::AppContext| {
                    built.set(built.get() + range.len());
                    (range.start..range.end)
                        .map(|_| {
                            // 30px 等高行（LIST_ROW_HEIGHT 契约）
                            warpui::elements::ConstrainedBox::new(Empty::new().finish())
                                .with_height(LIST_ROW_HEIGHT)
                                .finish()
                        })
                        .collect::<Vec<_>>()
                        .into_iter()
                };
                UniformList::new(UniformListState::new(), 500, build).finish()
            }
        }
        impl warpui::TypedActionView for VirtualizationTestView {
            type Action = ();
        }

        let built = Rc::new(Cell::new(0usize));
        let built_for_assert = built.clone();
        App::test((), move |mut app| {
            let built = built.clone();
            async move {
                let (_, view) = app.add_window(WindowStyle::NotStealFocus, move |_| {
                    VirtualizationTestView {
                        built: built.clone(),
                    }
                });
                let window_id = app.window_ids()[0];
                let mut presenter = Presenter::new(window_id);
                let mut updated = std::collections::HashSet::new();
                updated.insert(view.id());
                app.update(move |ctx| {
                    presenter.invalidate(
                        WindowInvalidation {
                            updated,
                            ..Default::default()
                        },
                        ctx,
                    );
                    // 480×600 视口（面板默认宽 × 典型面板高）
                    let _scene = presenter.build_scene(
                        pathfinder_geometry::vector::vec2f(480., 600.),
                        1.,
                        None,
                        ctx,
                    );
                });
                // 500 行 → 只构建可见行（测试窗口默认高 ~1500px / 30px 行高
                // ≈ 50 可见行），远小于 500 —— 若无虚拟化应为 500
                let built_rows = built_for_assert.get();
                assert!(
                    built_rows > 0 && built_rows < 500,
                    "virtualization broken: built {} rows for 500-item list \
                     (expected roughly visible-window-rows ≈ 50, not all 500)",
                    built_rows
                );
                log::info!("observatory P0-1 virtualization check: {built_rows}/500 rows built");
            }
        });
    }
}
