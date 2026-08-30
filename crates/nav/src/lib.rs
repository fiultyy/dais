//! nav — 左栏导航 (cockpit_nav + left_rail_status + 状态点渲染)。
//!
//! 布局拆分 v1 步骤5 (split-plan §3): 视图自持 (订阅 CockpitModel 快照 +
//! 低频对账 timer), 对 app 只有两条事件契约:
//! 1. `CockpitNavEvent::CardActivated{terminal_view_id}` → app 聚焦目标 pane
//!    (dispatch FocusTerminalViewInWorkspace);
//! 2. `CockpitNavEvent::AddProjectRequested` → app 打开加项目 picker
//!    (原 OpenAddProjectPicker dispatch, 拆分事件化)。
//!
//! 数据面: CockpitModel 纯壳 (crates/cockpit_model) 单例; 快照由 app refresh
//! 推入 (`refresh_model`), nav 不依赖任何 app 类型。

pub mod cockpit_nav;
pub mod left_rail_status;
pub mod data_source;
pub mod panel_api;
pub mod status_row;

pub use cockpit_nav::{CockpitNavEvent, CockpitNavView, set_cockpit_refresh_hook};
pub use left_rail_status::{
    HarnessRunState, LeftRailStatusEvent, LeftRailStatusModel, ProjectAggregate,
    SessionProgressChanged, SessionRunStatus, SessionStateChanged, UnreadChanged,
};
pub use panel_api::{HeaderToolbarItemKind, PanelDescriptor, PanelHost, PanelRegistry};
pub use status_row::status_dot_element;
