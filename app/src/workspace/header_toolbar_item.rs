//! header_toolbar_item — 兼容壳。
//!
//! 布局拆分 v3a: `HeaderToolbarItemKind` 实体迁入 nav crate
//! (`nav::panel_api`), 本模块仅 re-export 以兼容 app 内既有的
//! `crate::workspace::header_toolbar_item::HeaderToolbarItemKind` 路径
//! (TabSettings / HeaderToolbarChipSelection 的 serde 序列化格式不变)。
//!
//! `is_supported` / `is_available` 依赖 app 侧 settings (TabSettings /
//! AISettings) 与 FeatureFlag, 留在 app (见 workspace/view.rs 内实现)。

pub use nav::panel_api::HeaderToolbarItemKind;
