//! Cockpit 面板 — 多 agent 终端驾驶舱(hub-tui 设计模式的 dais 原生移植)。
//!
//! 数据源(全部进程内直取,零 CLI 子进程,见 docs/specs/cockpit.md §1):
//! - 终端清单: `WorkspaceRegistry` → `Workspace.tabs` → `PaneGroup::terminal_pane_ids`
//!   → `TerminalView` 直读(标题/cwd/长命令状态/git 分支/preview 尾行)
//! - agent 结构化状态: `CLIAgentSessionsModel` 单例(L1 命令检测 + L2 插件富化上下文)
//! - 刷新模型(P1): 视图订阅 `CLIAgentSessionsModelEvent`(事件粒度反映)+
//!   10s 低频 timer 对账(终端开合)
//! - 持久层: 无(P0/P1 零持久化;P2 仅 alert_rules/macros 落 dais persistence)
//!
//! 挂载: 独立 tab pane(工具条按钮 → `Workspace::toggle_cockpit`,与 observatory 同款)。

pub mod model;
pub mod view;
