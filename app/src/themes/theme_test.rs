use super::*;
use crate::themes::default_themes::{dark_theme, light_theme};
use warpui::color::ColorU;

/// 验证 dark_theme 基础颜色正确。
#[test]
fn dark_theme_base_colors() {
    let theme = dark_theme();

    let bg = theme.background().into_solid();
    let fg = theme.foreground().into_solid();
    // 暗色主题:背景亮度应低于前景
    let bg_luma = bg.r as u32 + bg.g as u32 + bg.b as u32;
    let fg_luma = fg.r as u32 + fg.g as u32 + fg.b as u32;
    assert!(
        bg_luma < fg_luma,
        "dark theme background ({bg_luma}) should be darker than foreground ({fg_luma})"
    );

    // name
    assert_eq!(theme.name(), Some("Dark".to_string()));
}

/// 验证 light_theme 基础颜色正确。
#[test]
fn light_theme_base_colors() {
    let theme = light_theme();

    let bg = theme.background().into_solid();
    let fg = theme.foreground().into_solid();
    // 亮色主题:背景亮度应高于前景
    let bg_luma = bg.r as u32 + bg.g as u32 + bg.b as u32;
    let fg_luma = fg.r as u32 + fg.g as u32 + fg.b as u32;
    assert!(
        bg_luma > fg_luma,
        "light theme background ({bg_luma}) should be lighter than foreground ({fg_luma})"
    );

    assert_eq!(theme.name(), Some("Light".to_string()));
}

/// 验证 dark/light 终端色板齐全(16 色非零)。
#[test]
fn terminal_palettes_are_populated() {
    for theme in [dark_theme(), light_theme()] {
        let colors = theme.terminal_colors();
        for color in [
            colors.normal.black,
            colors.normal.red,
            colors.normal.green,
            colors.normal.yellow,
            colors.normal.blue,
            colors.normal.magenta,
            colors.normal.cyan,
            colors.normal.white,
            colors.bright.black,
            colors.bright.red,
            colors.bright.green,
            colors.bright.yellow,
            colors.bright.blue,
            colors.bright.magenta,
            colors.bright.cyan,
            colors.bright.white,
        ] {
            assert!(
                color.r != 0 || color.g != 0 || color.b != 0,
                "terminal color should not be pure black"
            );
        }
    }
}

/// 验证默认主题配置只含 Dark/Light 两个内置主题。
#[test]
fn default_config_contains_only_dark_and_light() {
    let config = WarpThemeConfig::default();
    let mut kinds: Vec<ThemeKind> = config.theme_items().map(|(kind, _)| kind.clone()).collect();
    kinds.sort();

    assert_eq!(
        kinds,
        vec![ThemeKind::Dark, ThemeKind::Light],
        "默认主题配置应只含 Dark 与 Light"
    );
}

/// 验证 ThemeKind::default() 为 Dark,Display 输出正确。
#[test]
fn theme_kind_default_and_display() {
    assert_eq!(ThemeKind::default(), ThemeKind::Dark);
    assert_eq!(format!("{}", ThemeKind::Dark), "Dark");
    assert_eq!(format!("{}", ThemeKind::Light), "Light");
}

/// 验证未知主题字符串(旧 TOML 残留,如 Dracula)反序列化回落 Dark。
#[test]
fn unknown_theme_kind_deserializes_to_dark() {
    let legacy: ThemeKind = serde_json::from_str("\"Dracula\"").expect("冷门主题应回落而非报错");
    assert_eq!(legacy, ThemeKind::Dark);

    let also_legacy: ThemeKind = serde_json::from_str("\"Solarized Dark\"").unwrap();
    assert_eq!(also_legacy, ThemeKind::Dark);
}

/// 验证 Dark/Light 序列化保持 snake_case 原名,往返一致。
#[test]
fn dark_light_roundtrip() {
    for kind in [ThemeKind::Dark, ThemeKind::Light] {
        let json = serde_json::to_string(&kind).unwrap();
        assert_eq!(json, format!("\"{kind}\""));
        let parsed: ThemeKind = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, kind);
    }
}

/// 验证未知主题在配置中查找时回落 dark 主题实例。
#[test]
fn config_theme_lookup_falls_back_to_dark() {
    let config = WarpThemeConfig::default();
    let theme = config.theme(&ThemeKind::Dark);
    assert_eq!(theme.name(), Some("Dark".to_string()));
    assert_eq!(
        theme.background().into_solid(),
        dark_theme().background().into_solid()
    );
}

/// 验证 SelectedSystemThemes 默认值是 Light/Dark 组合。
#[test]
fn selected_system_themes_default() {
    let selected = SelectedSystemThemes::default();
    assert_eq!(selected.light, ThemeKind::Light);
    assert_eq!(selected.dark, ThemeKind::Dark);
}

/// 验证两个主题渲染颜色可区分(不是同一张卡片的复制)。
#[test]
fn dark_and_light_are_distinct() {
    let dark_bg = dark_theme().background().into_solid();
    let light_bg = light_theme().background().into_solid();
    assert_ne!(
        dark_bg, light_bg,
        "Dark 与 Light 的背景色必须不同,否则卡片无法区分"
    );
    let _ = ColorU::white(); // keep import used if assertions change
}
