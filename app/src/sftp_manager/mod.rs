//! SFTP 文件浏览器模块(留守层)
//!
//! 提供 SFTP 连接管理、远程文件浏览、上传下载等功能。
//! author: logic
//! date: 2026-05-26
//!
//! # P-ARC R4 后的本模块(留守层)
//!
//! 纯模型层已下沉 `crates/sftp_manager_ui`(types 展示类型 / sftp_ops
//! 协议操作封装 / sftp_backend trait + Live/InMemory 实现)。
//! 本模块保留(物理依赖约束,依 R2-a/R2-b 留守层裁决):
//! - `browser.rs`:`SftpBrowserView` / `SftpBrowserAction` 主体,依赖
//!   `crate::editor` / `crate::pane_group`(R3 主线互斥区)/
//!   `crate::settings_view::keybindings`;
//! - `breadcrumb` / `context_menu` / `dialogs` / `drop_target` /
//!   `file_list` / `transfer_panel`:视图渲染件,均 dispatch 宿主侧
//!   `SftpBrowserAction` 或依赖 `crate::editor::EditorView`;
//! - 路径兼容 shim(`types`/`sftp_ops`/`sftp_backend` 模块),保证
//!   `crate::sftp_manager::*` 全部既有路径不变(含
//!   integration_testing/sftp.rs 等域外消费点)。

pub mod breadcrumb;
pub mod browser;
pub mod context_menu;
pub mod dialogs;
pub mod drop_target;
pub mod file_list;
pub mod transfer_panel;

// R4 路径兼容 shim:主体已下沉 crates/sftp_manager_ui,保持
// `crate::sftp_manager::{types,sftp_ops,sftp_backend}::*` 旧路径不变。
pub mod sftp_backend {
    pub use sftp_manager_ui::sftp_backend::*;
}
pub mod sftp_ops {
    pub use sftp_manager_ui::sftp_ops::*;
}
pub mod types {
    pub use sftp_manager_ui::types::*;
}

#[cfg(test)]
#[path = "browser_tests.rs"]
mod browser_tests;

#[cfg(test)]
#[path = "browser_integration_tests.rs"]
mod browser_integration_tests;

#[allow(unused_imports)]
pub use browser::{SftpBrowserAction, SftpBrowserView};
#[allow(unused_imports)]
pub use types::*;
