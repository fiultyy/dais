//! Combo button 内层图标按钮 (2026-08-28 自 app/src/ui_components/buttons.rs
//! 下沉, 布局拆分 v1 步骤2)。
//!
//! 下沉原因: cockpit_nav (v1 迁往 crates/nav) 是它的消费端, 留在 app 会让
//! nav crate 拉不干净。依赖闭包 = warp_core(WarpTheme/Fill/blended colors)
//! + warpui(Button/UiComponentStyles/MouseStateHandle) + Icon (warp_core::
//! ui::icons) — 恰为本 crate 已有依赖, 零新增。
//!
//! 幂等性约定: 纯函数 — 相同 (theme, icon, active) 输入产出等价 Button
//! 样式, 无隐藏状态; 测试锁定样式参数的确定性。

use warp_core::ui::icons::{Icon, ICON_DIMENSIONS};
use warp_core::ui::theme::color::internal_colors;
use warp_core::ui::theme::{Fill, WarpTheme};
use warpui::elements::Radius;
use warpui::elements::{CornerRadius, MouseStateHandle};
use warpui::ui_components::button::Button;
use warpui::ui_components::components::{Coords, UiComponent, UiComponentStyles};

const ICON_BUTTON_PADDING: f32 = 4.;
const COMBO_BORDER_RADIUS: f32 = 4.;

#[derive(Copy, Clone)]
enum ButtonState {
    Default,
    Disabled,
    Pressed,
    Hover,
}

fn combo_inner_button_styles(warp_theme: &WarpTheme, state: ButtonState) -> UiComponentStyles {
    let background = match state {
        ButtonState::Default => None,
        ButtonState::Hover => Some(internal_colors::neutral_2(warp_theme)),
        ButtonState::Pressed => Some(internal_colors::neutral_4(warp_theme)),
        ButtonState::Disabled => Some(warp_theme.background().into()),
    };

    UiComponentStyles {
        width: Some(ICON_DIMENSIONS),
        height: Some(ICON_DIMENSIONS),
        border_width: None,
        padding: Some(Coords::uniform(ICON_BUTTON_PADDING - 1.)),
        border_radius: None,
        font_color: Some(warp_theme.foreground().into()),
        border_color: None,
        background: background.map(Into::into),
        ..Default::default()
    }
}

/// This creates an inner icon_button for the purpose of adding it into a
/// combo button. In these cases, the icon_button should not have a border
/// as the combo button will provide these. Note that b/c
/// it is not needed at this time, disabled is not implemented.
///
/// TODO(CORE-2300): Evaluate whether or not this helper makes sense in this
/// location, as it is only used in workspace/view.rs right now (it is here
/// b/c of access to non-pub fields).
pub fn combo_inner_button(
    theme: &WarpTheme,
    icon: Icon,
    active: bool,
    mouse_state_handle: MouseStateHandle,
) -> Button {
    let button = Button::new(
        mouse_state_handle,
        combo_inner_button_styles(theme, ButtonState::Default),
        Some(combo_inner_button_styles(theme, ButtonState::Hover)),
        Some(combo_inner_button_styles(theme, ButtonState::Pressed)),
        Some(combo_inner_button_styles(theme, ButtonState::Disabled)),
    )
    .with_icon_label(icon.to_warpui_icon(theme.foreground()));

    if active {
        return button.active();
    }
    button
}

#[cfg(test)]
mod tests {
    use super::*;
    use warp_core::ui::appearance::Appearance;

    /// 幂等验收: 同一 (theme, icon, active) 输入 → 样式参数逐项相等。
    /// 防止下沉过程或后续重构引入隐藏可变状态。
    fn assert_styles_equivalent(a: &UiComponentStyles, b: &UiComponentStyles) {
        assert_eq!(a.width, b.width);
        assert_eq!(a.height, b.height);
        assert_eq!(a.border_width, b.border_width);
        assert_eq!(a.padding, b.padding);
        assert_eq!(a.font_color, b.font_color);
        assert_eq!(a.background, b.background);
        assert_eq!(a.border_color, b.border_color);
    }

    #[test]
    fn combo_inner_button_styles_are_pure() {
        let appearance = Appearance::mock();
        let theme = appearance.theme();
        let s1 = combo_inner_button_styles(theme, ButtonState::Hover);
        let s2 = combo_inner_button_styles(theme, ButtonState::Hover);
        assert_styles_equivalent(&s1, &s2);
    }

    #[test]
    fn combo_inner_button_hover_differs_from_default() {
        let appearance = Appearance::mock();
        let theme = appearance.theme();
        let d = combo_inner_button_styles(theme, ButtonState::Default);
        let h = combo_inner_button_styles(theme, ButtonState::Hover);
        assert!(d.background.is_none());
        assert!(h.background.is_some());
    }

    #[test]
    fn combo_inner_button_padding_is_icon_padding_minus_one() {
        let appearance = Appearance::mock();
        let theme = appearance.theme();
        let s = combo_inner_button_styles(theme, ButtonState::Default);
        assert_eq!(s.padding, Some(Coords::uniform(ICON_BUTTON_PADDING - 1.)));
        assert_eq!(s.width, Some(ICON_DIMENSIONS));
    }

    #[test]
    fn button_construction_is_repeatable() {
        // 完整构造路径跑两遍, 无 panic 无状态泄漏 (MouseStateHandle 独立)。
        let appearance = Appearance::mock();
        let theme = appearance.theme();
        let _b1 = combo_inner_button(theme, Icon::Plus, false, MouseStateHandle::default());
        let _b2 = combo_inner_button(theme, Icon::Plus, false, MouseStateHandle::default());
    }
}
