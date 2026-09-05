//! R11 shim: VT 解析层(ansi 家族)已下沉 crates/terminal_grid_model,
//! 本模块 re-export 保持 `terminal_grid_model::ansi::*` 旧路径全兼容。
pub use terminal_grid_model::ansi::*;
