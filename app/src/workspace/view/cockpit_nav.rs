//! cockpit_nav re-export 壳 (2026-08-28 布局拆分 v1 步骤5)。
//!
//! 实现下沉 crates/nav; app 内全部 `crate::workspace::view::cockpit_nav::*`
//! 调用点 (view.rs 挂载/订阅 + vertical_tabs 桥) 路径不变。
//!
//! v1 新增契约: `CockpitNavEvent::AddProjectRequested` (原 OpenAddProjectPicker
//! dispatch 事件化), app 订阅点见 view.rs。

pub use nav::{CockpitNavEvent, CockpitNavView};
