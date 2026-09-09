pub mod action;
pub mod ai_context_menu;
pub(crate) mod async_snapshot_data_source;
pub mod binding_source;
pub mod command_palette;
pub mod command_search;
pub mod data_source;
mod env_var_collections;
pub mod external_secrets;
pub mod files;
mod filter_chip_renderer;
pub mod item;
pub mod macros;
pub mod mixer;
mod palette_styles;
pub mod result_renderer;
pub mod search_bar;
pub mod slash_command_menu;

// R5 路径兼容 shim:tantivy 全文检索引擎已下沉 crates/search_ui,保持
// `crate::search::searcher::*` 旧路径不变。wasm 目标下模块为空(与原
// searcher.rs 顶部 #![cfg(not(target_family = "wasm"))] 语义一致)。
pub mod searcher {
    #[cfg(not(target_family = "wasm"))]
    pub use search_ui::searcher::*;
}

// searcher_test 留宿主:经 define_search_schema! 宏(宏 $crate 展开锚定
// app 根,见 macros.rs 留守说明)建 schema,searcher 类型经上方 shim
// 引自 crates/search_ui。
#[cfg(all(test, not(target_family = "wasm")))]
#[path = "searcher_test.rs"]
mod searcher_test;

pub use item::SearchItem;
pub use mixer::SyncDataSource;
pub use result_renderer::ItemHighlightState;

pub use data_source::QueryFilter;
use filter_chip_renderer::FilterChipRenderer;
