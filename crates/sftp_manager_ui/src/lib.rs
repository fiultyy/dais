//! sftp_manager_ui — SFTP 文件浏览器域纯模型层 crate(P-ARC R4,依 R1/R2 同刀切模式)。
//!
//! 自 `app/src/sftp_manager` 同刀切出的零 app 依赖部分:
//! - [`types`]:UI 层展示类型(FileEntry / TransferTask / Dialog /
//!   ConnectionState / format_size 等纯数据 + 纯函数,内联单测);
//! - [`sftp_ops`]:dais_sftp 协议层到 UI 层的高级操作封装
//!   (connect_from_server / list_dir / 流式上传下载的临时文件模式等,
//!   认证经 warp_ssh_manager 持久层解析);
//! - [`sftp_backend`]:`SftpBackend` trait(Live/InMemory 双实现,
//!   InMemory 基于本地文件系统供测试与 sandbox 预览)。
//!
//! 只依赖既有 crate 面(dais_sftp / warp_ssh_manager / chrono / dirs /
//! dunce),禁止依赖任何 app 类型(无 `dais::` / `crate::` 反向引用),
//! 零 `ObjectStoreModel` 引用。
//!
//! ## 留守 app 的部分(物理依赖约束,对应 R2-a/R2-b 留守层)
//!
//! - `browser.rs`:`SftpBrowserView` / `SftpBrowserAction` 主体,
//!   依赖 `crate::editor` / `crate::pane_group`(R3 主线互斥区)/
//!   `crate::settings_view::keybindings`;
//! - `breadcrumb` / `context_menu` / `dialogs` / `drop_target` /
//!   `file_list` / `transfer_panel`:视图渲染件,均 dispatch 宿主侧
//!   `SftpBrowserAction` 或依赖 `crate::editor::EditorView`。
//!
//! app 侧 `sftp_manager::{types,sftp_ops,sftp_backend}` 均为到本 crate
//! 的 re-export shim,旧路径继续可解析。

pub mod sftp_backend;
pub mod sftp_ops;
pub mod types;

pub use sftp_backend::{InMemorySftpBackend, LiveSftpBackend, SftpBackend};
pub use sftp_ops::{ProgressCallback, SftpOpsError, connect_from_server, list_dir};
pub use types::{
    ConnectionState, Dialog, FileEntry, FileEntryType, TransferDirection, TransferState,
    TransferTask, format_size,
};
