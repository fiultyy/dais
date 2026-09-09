// R5 留守层 + 路径兼容 shim:slash 命令模糊匹配(含测试)已下沉
// crates/search_ui;static_commands 纯模型经子模块 shim 指回 crate;
// commands.rs / bindings.rs(i18n 绑定)留守。
pub mod static_commands;

// R5 路径兼容 shim:fuzzy_match 主体已下沉 crates/search_ui。
pub mod fuzzy_match {
    pub use search_ui::slash_command_menu::fuzzy_match::*;
}

pub use static_commands::{SlashCommandId, StaticCommand};
