//! R25: BlockList 本体迁移 terminal_grid_model::block::blocks;本文件为 app 侧 mod 级 shim。
pub use terminal_grid_model::block::blocks::*;
#[cfg(test)]
#[path = "blocks_test.rs"]
pub(crate) mod tests;

#[cfg(test)]
pub use tests::{insert_block, insert_block_with_prompt};

#[cfg(test)]
#[path = "selection_tests.rs"]
mod selection_tests;
