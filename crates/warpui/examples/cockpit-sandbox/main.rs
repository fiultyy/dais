//! cockpit-sandbox — 布局设计迭代沙盒(秒级编译,复用 crate 层本体)
//!
//! 用途: 各界面布局的 1:1 迭代预览。直接 use crate 层本体(与 app 同一份代码,
//! 非复刻),多页切换:
//!   - cockpit-nav: `nav::CockpitNavView`(左栏导航,注入假快照)
//!   - observatory: `warp::ai::observatory::view::ObservatoryPanelView`(真身,读真实 harness_blocks.db)
//!   - cockpit: `warp::ai::cockpit::view::CockpitPanelView`(真身,与 nav 共享 CockpitModel 快照)
//!   - `Appearance::mock()` + app Dark 主题字面值(与 app/src/themes/default_themes.rs 同源)
//!
//! 迭代循环: 改布局 → `dais-sandbox`(或 `cargo run -p warpui --example cockpit-sandbox`)
//! 定稿: 把改动搬回 crates/nav 或 app/src 对应文件。

use anyhow::{anyhow, Result};
use std::borrow::Cow;
pub mod root_view;

extern crate warpui;
use rust_embed::RustEmbed;
use warpui::{platform, AssetProvider};

#[derive(Clone, Copy, RustEmbed)]
#[folder = "examples/assets"]
pub struct Assets;

pub static ASSETS: Assets = Assets;

impl AssetProvider for Assets {
    fn get(&self, path: &str) -> Result<Cow<'_, [u8]>> {
        <Assets as RustEmbed>::get(path)
            .map(|f| f.data)
            .ok_or_else(|| anyhow!("no asset exists at path {}", path))
    }
}

fn main() -> Result<()> {
    env_logger::init();
    // sandbox 挂载 app 面板真身,其依赖路径(ActionButton/InterceptSessionsModel
    // 等)会查询 FeatureFlag;debug 构建下未初始化即 assert。先按 app 启动链
    // 同款初始化(全开 enabled_features + mark_initialized)。
    warp::init_feature_flags();
    let app_builder =
        platform::AppBuilder::new(platform::AppCallbacks::default(), Box::new(ASSETS), None);
    let _ = app_builder.run(move |ctx| {
        ctx.add_window(
            warpui::AddWindowOptions {
                title: Some("cockpit-sandbox".to_string()),
                ..Default::default()
            },
            root_view::RootView::new,
        );
    });

    Ok(())
}
