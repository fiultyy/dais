//! search_ui — 搜索域纯引擎/纯模型层 crate(P-ARC R5,依 R1-R4 同刀切模式)。
//!
//! 自 `app/src/search` 同刀切出的零 app 依赖部分(目录结构镜像宿主路径,
//! 宿主 `app/src/search` 各 mod 以 re-export shim 保持旧路径全部可解析):
//!
//! - [`searcher`]:tantivy 全文检索引擎(SimpleFullTextSearcher /
//!   FullTextSearchSchema / tokenizer / schema trait 面,wasm 目标 cfg 摘除);
//! - [`command_search::settings`]:CommandSearchSettings 设置组(settings crate DSL);
//! - [`env_var_collections::fuzzy_match`]:EnvVarCollection 模糊匹配
//!   (依赖 env_vars_ui::EnvVarCollection,R1 已就绪);
//! - [`slash_command_menu::fuzzy_match`]:slash 命令名/描述模糊匹配;
//! - [`slash_command_menu::static_commands`]:StaticCommand / Availability /
//!   Argument 纯模型(bitflags 需求位 + 过滤判定);
//! - [`external_secrets::external_secret_fuzzy_match`]:外部密钥名模糊匹配;
//! - [`files::search_item`]:FileSearchResult / FileSearchItem POD;
//! - [`ai_context_menu`]:is_valid_search_query 纯启发式 + styles 常量 +
//!   safe_truncate / floor_char_boundary UTF-8 安全截断工具。
//!
//! 只依赖既有 crate 面(warpui / fuzzy_match / env_vars_ui /
//! settings / tantivy 等),禁止依赖任何 app 类型(无 `dais::` / `crate::`
//! 反向引用),零 `ObjectStoreModel` 引用。
//!
//! ## 留守 app 的部分(物理依赖约束,对应 R2-a/R2-b/R4 留守层)
//!
//! - `macros.rs`(define_search_schema! DSL):`$crate::search::searcher::*`
//!   在宿主调用点展开 + 三个根级 helper 宏同文件定义,宏体锚定 app 根路径
//!   → 留宿主(经 shim 引用本 crate searcher 类型,调用点零改动);
//! - `searcher_test.rs`:经宿主 define_search_schema! 宏建 schema → 留宿主;
//! - `data_source.rs`(QueryFilter/FilterAtom 框架):~50 处 `crate::t!`
//!   i18n 宏直连宿主(R4 panel.rs 同型裁决);
//! - `mixer.rs` / `async_snapshot_data_source.rs`:debounce /
//!   send_telemetry_from_ctx / server::telemetry 宿主单例;
//! - `item.rs` / `result_renderer.rs` / `palette_styles.rs` /
//!   `filter_chip_renderer.rs`:与留守 data_source::QueryFilter/
//!   QueryResult 互耦或依赖宿主 themes;
//! - `search_bar.rs` / `command_palette/*` / `command_search` 视图与
//!   `ai_context_menu/view.rs` / `external_secrets` 视图:appearance themes /
//!   ui_components / editor / drive / workspace / session_management 等
//!   宿主视图依赖;
//! - `files/{model,icon}.rs`:local_fs feature 下订阅
//!   workspace::ActiveSession + RepoMetadataModel 宿主单例;
//! - `slash_command_menu/static_commands/{commands,bindings}.rs`:
//!   `crate::t!` / `t_static!` / `crate::i18n` i18n 绑定;
//! - `notebooks/` / `ai_queries/`:未被 search/mod.rs 声明的死模块,原样留守。
//!
//! ## 宿主契约
//!
//! app 侧 `crate::search::*` 全部既有路径经宿主 mod shim 指回本 crate,
//! 域内外调用点零改动;本 crate 无 ObjectStoreModel 访问,无需数据桥。

pub mod ai_context_menu;
pub mod command_search;
pub mod env_var_collections;
pub mod external_secrets;
pub mod files;
pub mod searcher;
pub mod slash_command_menu;

/// P-ARC R8-B: workspace 全局搜索模型(原 app/src/workspace/view/global_search)。
/// SearchConfig/GlobalSearchEvent 随迁;宿主 mod.rs/view.rs re-export 兼容。
pub mod global_search;

pub use global_search::{GlobalSearchEvent, SearchConfig};
