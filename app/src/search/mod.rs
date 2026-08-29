pub mod action;
pub mod ai_context_menu;
mod ai_queries;
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
pub mod notebook_embedding;
mod notebooks;
mod palette_styles;
pub mod result_renderer;
mod search_bar;
pub mod search_results_menu;
// 全文搜索引擎依赖 tantivy (optional dep): 仅在 use_tantivy_search feature 下编译,
// 缺省时整个模块不存在, 数据源只走 fuzzy 分支
#[cfg(feature = "use_tantivy_search")]
pub mod searcher;
pub mod slash_command_menu;
pub mod welcome_palette;
mod workflows;

pub use item::SearchItem;
pub use mixer::SyncDataSource;
pub use result_renderer::ItemHighlightState;

pub use data_source::QueryFilter;
use filter_chip_renderer::FilterChipRenderer;
pub use workflows::fuzzy_match::FuzzyMatchWorkflowResult;
