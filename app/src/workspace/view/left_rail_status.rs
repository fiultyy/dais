//! left_rail_status re-export 壳 (2026-08-28 布局拆分 v1 步骤5)。
//! 实现下沉 crates/nav; app 内调用点 (view.rs 订阅 + vertical_tabs 消费 +
//! ai 写入端 left_rail_unread/run_state) 路径不变。

pub use nav::{
    HarnessRunState, LeftRailStatusEvent, LeftRailStatusModel, ProjectAggregate,
    SessionProgressChanged, SessionRunStatus, SessionStateChanged, UnreadChanged,
};
