pub mod default_themes;
pub mod theme;
pub mod theme_chooser;

use warp_core::ui::theme::WarpTheme;

pub fn onboarding_theme_picker_themes() -> [WarpTheme; 2] {
    [default_themes::dark_theme(), default_themes::light_theme()]
}
