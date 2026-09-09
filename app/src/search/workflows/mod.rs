// R5 路径兼容 shim:Workflow 模糊匹配已下沉 crates/search_ui
// (workflows_ui::Workflow 依赖面,R2-b 已就绪)。
pub mod fuzzy_match {
    pub use search_ui::workflows::fuzzy_match::*;
}
