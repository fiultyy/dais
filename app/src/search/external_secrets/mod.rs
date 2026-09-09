pub mod external_secret_data_source;
// R5 路径兼容 shim:外部密钥名模糊匹配已下沉 crates/search_ui
// (原为私有 mod,shim 保持私有)。
mod external_secret_fuzzy_match {
    pub use search_ui::external_secrets::external_secret_fuzzy_match::*;
}
pub mod external_secret_search_item;
pub mod searcher;
pub mod view;
