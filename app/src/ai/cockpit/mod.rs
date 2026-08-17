//! Cockpit 面板 — 多 agent 终端驾驶舱(hub-tui 设计模式的 dais 原生移植)。
//!
//! 数据源(全部进程内直取,零 CLI 子进程,见仓库根 dais-cockpit-spec.md §1):
//! - 终端清单: `WorkspaceRegistry` → `Workspace.tabs` → `PaneGroup::terminal_pane_ids`
//!   → `TerminalView` 直读(标题/cwd/长命令状态)
//! - agent 结构化状态: `CLIAgentSessionsModel` 单例(L1 命令检测 + L2 插件富化上下文)
//! - 持久层: 无(P0/P1 零持久化;P2 仅 alert_rules/macros 落 dais persistence)
//!
//! 挂载: 独立 tab pane(工具条按钮 → `Workspace::toggle_cockpit`,与 observatory 同款)。

pub mod model;
pub mod view;
