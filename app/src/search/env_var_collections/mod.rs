// R5 路径兼容 shim:EnvVarCollection 模糊匹配已下沉 crates/search_ui
// (env_vars_ui::EnvVarCollection 依赖面,R1 已就绪)。
pub mod fuzzy_match {
    pub use search_ui::env_var_collections::fuzzy_match::*;
}
