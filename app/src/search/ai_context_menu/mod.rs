mod blocks;
mod commands;
mod conversations;
mod diffset;
mod files;
pub mod mixer;
mod notebooks;
mod rules;
#[cfg(not(target_family = "wasm"))]
mod skills;
pub mod view;
mod workflows;

// R5 路径兼容 shim:is_valid_search_query 纯启发式 / styles 常量 /
// safe_truncate / floor_char_boundary 工具已下沉 crates/search_ui;
// view.rs 及各 data_source/search_item(appearance/t!/terminal/
// workspace 等宿主依赖)留守本目录。
pub mod search {
    pub use search_ui::ai_context_menu::search::*;
}
mod styles {
    pub use search_ui::ai_context_menu::styles::*;
}
pub use search_ui::ai_context_menu::{floor_char_boundary, safe_truncate};
