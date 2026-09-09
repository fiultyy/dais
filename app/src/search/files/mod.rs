pub mod icon;
pub mod model;
// R5 路径兼容 shim:FileSearchResult / FileSearchItem POD 已下沉
// crates/search_ui;model.rs(local_fs feature 下订阅宿主
// workspace::ActiveSession + RepoMetadataModel)与 icon.rs(crate::code)
// 留守本目录。
pub mod search_item {
    pub use search_ui::files::search_item::*;
}
