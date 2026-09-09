//! SSH 管理器 UI(左侧 Tool Panel):树状文件夹/服务器列表 + 右侧详情表单。
//!
//! 数据层在独立 crate `warp_ssh_manager`(`crates/warp_ssh_manager/`)。
//!
//! # P-ARC R4 后的本模块(留守层)
//!
//! 纯模型/纯函数层已下沉 `crates/ssh_manager_ui`(SshTreeChangedNotifier、
//! OneKeyCredential 体系、password_prompt / shell_prompt 检测)。
//! 本模块保留(物理依赖约束,依 R2-a/R2-b 留守层裁决):
//! - `panel.rs` / `server_view.rs`:`crate::t!` i18n、`crate::editor`、
//!   `crate::ui_components`、`crate::view_components::dropdown`、
//!   `crate::workspace::WorkspaceAction`、`crate::pane_group`(R3 主线
//!   互斥区)等宿主视图依赖;
//! - `candidates.rs`:`refresh()` 读 `crate::settings::SshSettings`
//!   (R2-b WorkflowViewMode 同型裁决:方法读宿主状态 → 留宿主);
//! - `secret_injector` / `startup_command_injector` / `su_password_injector`:
//!   持有 `crate::terminal::TerminalView`,随 terminal 域 crate 化再迁;
//! - 路径兼容 shim(`notifier`/`onekey`/`password_prompt`/`shell_prompt`
//!   模块),保证 `crate::ssh_manager::*` 全部既有路径不变(含
//!   terminal/view.rs、lib.rs、pane_group 测试等域外消费点)。

pub mod candidates;
pub mod panel;
pub mod secret_injector;
pub mod server_view;
pub mod startup_command_injector;
pub mod su_password_injector;

// R4 路径兼容 shim:主体已下沉 crates/ssh_manager_ui,保持
// `crate::ssh_manager::{notifier,onekey,password_prompt,shell_prompt}::*`
// 旧路径不变。
pub mod notifier {
    pub use ssh_manager_ui::notifier::*;
}
pub mod onekey {
    pub use ssh_manager_ui::onekey::*;
}
pub mod password_prompt {
    pub use ssh_manager_ui::password_prompt::*;
}
pub mod shell_prompt {
    pub use ssh_manager_ui::shell_prompt::*;
}

// `CandidatesViewModel` 暂时只被 `panel.rs` 引用;`CandidateRow` 仅是 panel
// 内部布局用的中间表示,不需要导出。需要被外部消费时再加 re-export。
#[allow(unused_imports)]
pub use candidates::CandidatesViewModel;
pub use notifier::{SshTreeChangedEvent, SshTreeChangedNotifier};
pub use panel::SshManagerPanel;
// Re-exports for downstream UI consumers (Commit 2b).
#[allow(unused_imports)]
pub use panel::{SshManagerPanelAction, SshManagerPanelEvent};
