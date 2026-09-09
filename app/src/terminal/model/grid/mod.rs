//! R10 shim: grid 模型层已下沉 crates/terminal_grid_model,
//! 本模块 re-export 保持 `terminal_grid_model::grid::*` 旧路径全兼容。
//! ansi_handler(app ansi::Handler 实现,依赖 app Event/telemetry)与
//! Handler 驱动的测试留守在此。

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
#[cfg(test)]
#[path = "grid_test.rs"]
mod grid_test_mod;
#[cfg(test)]
#[path = "grid_handler_test.rs"]
mod grid_handler_test_mod;
#[cfg(test)]
#[path = "filtering_tests.rs"]
mod filtering_tests_mod;
#[cfg(test)]
#[path = "secrets_tests.rs"]
mod secrets_tests_mod;
