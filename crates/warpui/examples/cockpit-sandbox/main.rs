//! cockpit-sandbox — 布局设计迭代沙盒(秒级编译,不碰 app crate)
//!
//! 用途: cockpit 驾驶舱/nav 双栏布局的 1:1 迭代。直接 use crate 层本体:
//!   - `nav::CockpitNavView`(与 app 同一份代码,非复刻)
//!   - `cockpit_model::CockpitModel`(注入假快照)
//!   - `Appearance::mock()` + app Dark 主题字面值(与 app/src/themes/default_themes.rs 同源)
//!
//! 迭代循环: 改布局 → `cargo run -p warpui --example cockpit-sandbox`
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
