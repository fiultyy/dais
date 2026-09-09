mod env_var_collections;
mod history;
pub mod projects;
pub mod searcher;
// R5 路径兼容 shim:CommandSearchSettings 设置组(settings crate DSL,
// 零 app 依赖)已下沉 crates/search_ui。
pub mod settings {
    pub use search_ui::command_search::settings::*;
}
pub mod view;
mod workflows;
mod zero_state;
