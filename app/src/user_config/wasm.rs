use std::path::Path;

use warpui::ModelContext;

use crate::launch_configs::launch_config::LaunchConfig;
use crate::themes::theme::WarpThemeConfig;

impl super::WarpConfig {
    pub fn new(_ctx: &mut ModelContext<Self>) -> Self {
        Self {
            launch_configs: Default::default(),
            tab_configs: Default::default(),
            tab_config_errors: Default::default(),
            theme_config: WarpThemeConfig::new(),
        }
    }
}

/// Loads all themes relative to the `theme_path`.
pub fn load_theme_configs(_theme_path: &Path) -> WarpThemeConfig {
    // There's no local filesystem for wasm, so we'll never be able to retrieve
    // themes from any path.
    Default::default()
}


/// Loads all launch configs relative to the `launch_config_path`.
pub fn load_launch_configs(_launch_config_path: &Path) -> Vec<LaunchConfig> {
    // There's no local filesystem for wasm, so we'll never be able to retrieve
    // launch configs from any path.
    Default::default()
}
