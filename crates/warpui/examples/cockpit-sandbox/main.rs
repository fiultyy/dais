//! cockpit-sandbox — nav 布局设计迭代沙盒(秒级编译,复用 crates/nav 本体)
//!
//! 用途: `nav::CockpitNavView` 布局 1:1 迭代预览,注入假快照。
//! observatory/cockpit 面板预览已移交 `dais --ui-preview <name>`(真数据真交互)。
//!
//! 迭代循环: 改布局 → `dais-sandbox`(或 `cargo run -p warpui --example cockpit-sandbox`)
//! 定稿: 把改动搬回 crates/nav。

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
