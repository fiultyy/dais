//! ssh_manager_ui — SSH 管理器域纯模型层 crate(P-ARC R4,依 R1/R2 同刀切模式)。
//!
//! 自 `app/src/ssh_manager` 同刀切出的零 app 依赖部分:
//! - [`notifier`]:`SshTreeChangedNotifier` / `SshTreeChangedEvent`
//!   (Empty struct + SingletonEntity + 单 Event 变体的全局树变更广播);
//! - [`onekey`]:`OneKeyCredential` POD 形态 + `load_saved_ssh_credentials`
//!   (warp_ssh_manager 持久层 + Keychain 读取,同步 API,调用方走
//!   `spawn_blocking`);
//! - [`password_prompt`] / [`shell_prompt`]:PTY 字节流 prompt 检测纯函数
//!   (regex::bytes / 尾部字节匹配),注入器们的共享判定核心。
//!
//! 只依赖既有 crate 面(warpui / warp_ssh_manager),禁止依赖任何 app 类型
//! (无 `dais::` / `crate::` 反向引用),零 `ObjectStoreModel` 引用。
//!
//! ## 留守 app 的部分(物理依赖约束,对应 R2-a/R2-b 留守层)
//!
//! - `panel.rs` / `server_view.rs`:`crate::t!` i18n、`crate::editor`、
//!   `crate::ui_components`、`crate::view_components::dropdown`、
//!   `crate::workspace::WorkspaceAction`、`crate::pane_group`(R3 主线
//!   互斥区)等宿主视图依赖;
//! - `candidates.rs`:`refresh()` 读 `crate::settings::SshSettings`
//!   (R2-b WorkflowViewMode 同型裁决:方法读宿主状态 → 留宿主);
//! - `secret_injector` / `startup_command_injector` / `su_password_injector`:
//!   持有 `crate::terminal::TerminalView`,随 terminal 域 crate 化再迁。
//!
//! app 侧 `ssh_manager::{notifier,onekey,password_prompt,shell_prompt}`
//! 均为到本 crate 的 re-export shim,旧路径继续可解析。

pub mod notifier;
pub mod onekey;
pub mod password_prompt;
pub mod shell_prompt;

pub use notifier::{SshTreeChangedEvent, SshTreeChangedNotifier};
pub use onekey::{OneKeyCredential, OneKeyCredentialKind, load_saved_ssh_credentials};
pub use password_prompt::bytes_look_like_password_prompt;
pub use shell_prompt::bytes_look_like_shell_prompt;
