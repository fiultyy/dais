// R5 留守层:static_commands 纯模型(Availability/Argument/StaticCommand)
// 已下沉 crates/search_ui;SlashCommandId 定义与 i18n 命令表(commands.rs,
// t_static!/crate::i18n 绑定)及键位绑定(bindings.rs,crate::t!)留守本目录。
pub mod bindings;
pub mod commands;

pub use commands::SlashCommandId;
pub use search_ui::slash_command_menu::static_commands::{
    Availability, Argument, StaticCommand,
};
