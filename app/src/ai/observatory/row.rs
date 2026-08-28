//! observatory 状态行渲染 re-export 壳 (2026-08-28 布局拆分 v1 步骤5)。
//! 实现下沉 crates/nav (status_row); observatory/view.rs 与 cockpit/view.rs
//! 的 `use ...row::{status_dot, status_dot_element, list_row, truncate_str}`
//! 调用点路径不变。

pub use nav::status_row::{list_row, status_dot, status_dot_element, truncate_str};
