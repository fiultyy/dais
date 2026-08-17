//! Cockpit 数据模型 — 终端/agent 卡片快照(进程内直取)。
//!
//! 所有状态(快照、选中卡)集中于本 singleton model,视图纯渲染 + 派发意图
//! (MVU 单一数据源,observatory 同款)。`refresh` 是唯一的数据入口:
//! 全链路函数调用(WorkspaceRegistry → TerminalView → CLIAgentSessionsModel),
//! 零子进程、零文件 IO — hub-tui 的三路外置轮询在此整层蒸发(spec §1)。

use warpui::{Entity, EntityId, ModelContext, SingletonEntity};

use crate::terminal::cli_agent_sessions::{
    CLIAgentSession, CLIAgentSessionStatus, CLIAgentSessionsModel,
};
use crate::workspace::WorkspaceRegistry;

/// 卡片状态(agent 会话状态优先,无 agent 时按终端活跃度回退)。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CockpitCardStatus {
    /// agent 会话进行中(CLA gentSessionStatus::InProgress)。
    Working,
    /// agent 会话完成(Success)。
    Done,
    /// agent 阻塞(权限请求/提问),携带插件上报的 message 文本。
    Blocked(Option<String>),
    /// 无 agent,但终端有活跃长命令。
    Busy,
    /// 普通空闲 shell。
    Idle,
}

impl CockpitCardStatus {
    /// 映射到 observatory `status_dot` 的字符串键(复用既有 Icon+语义色表)。
    pub fn dot_key(&self) -> Option<&'static str> {
        match self {
            CockpitCardStatus::Working => Some("running"),
            CockpitCardStatus::Done => Some("done"),
            CockpitCardStatus::Blocked(_) => Some("blocked"),
            CockpitCardStatus::Busy => Some("ready"),
            CockpitCardStatus::Idle => None,
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            CockpitCardStatus::Working => "working",
            CockpitCardStatus::Done => "done",
            CockpitCardStatus::Blocked(_) => "blocked",
            CockpitCardStatus::Busy => "busy",
            CockpitCardStatus::Idle => "shell",
        }
    }

    /// 排序权重:阻塞/工作中在前,空闲 shell 在后。
    fn sort_rank(&self) -> u8 {
        match self {
            CockpitCardStatus::Blocked(_) => 0,
            CockpitCardStatus::Working => 1,
            CockpitCardStatus::Busy => 2,
            CockpitCardStatus::Done => 3,
            CockpitCardStatus::Idle => 4,
        }
    }
}

/// 终端卡片快照。字段全部来自进程内直取(spec §1.1/§1.2 映射表)。
#[derive(Clone, Debug)]
pub struct CockpitCard {
    /// `TerminalView::id()`(EntityId)— 进程内稳定 key,选中态用它。
    pub terminal_view_id: EntityId,
    /// pane 标题(`PaneConfiguration::title`,terminal view 自更新;
    /// agent 会话时自动为 agent 标题)。
    pub title: String,
    /// cwd(OSC 回报;None = 未上报)。
    pub cwd: Option<String>,
    /// 活跃 CLI agent 展示名(如 "Claude Code");None = 普通 shell。
    pub agent_name: Option<&'static str>,
    /// recap 行:`query > summary` 回退链(P1 补 response/preview 尾行)。
    pub recap: Option<String>,
    /// 当前工具名(L2 上下文;权限请求时上报)。
    pub tool_name: Option<String>,
    /// 状态。
    pub status: CockpitCardStatus,
}

/// Cockpit 事件。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CockpitEvent {
    /// 快照或选中态更新,订阅方应 rerender。
    SnapshotUpdated,
}

pub struct CockpitModel {
    cards: Vec<CockpitCard>,
    selected: Option<EntityId>,
    /// 面板开合状态(toggle_cockpit / pane attach/detach 写;timer 读取 gate 刷新)。
    panel_open: bool,
    /// 最近一次 refresh 收集到的终端数(诊断/标头计数)。
    last_window_count: usize,
}

impl Entity for CockpitModel {
    type Event = CockpitEvent;
}

impl SingletonEntity for CockpitModel {}

impl CockpitModel {
    pub fn new(_ctx: &mut ModelContext<Self>) -> Self {
        Self {
            cards: Vec::new(),
            selected: None,
            panel_open: false,
            last_window_count: 0,
        }
    }

    // ── 只读访问 ───────────────────────────────────────────────────────

    pub fn cards(&self) -> &[CockpitCard] {
        &self.cards
    }

    pub fn selected(&self) -> Option<EntityId> {
        self.selected
    }

    pub fn panel_open(&self) -> bool {
        self.panel_open
    }

    pub fn last_window_count(&self) -> usize {
        self.last_window_count
    }

    pub fn set_panel_open(&mut self, open: bool, ctx: &mut ModelContext<Self>) {
        self.panel_open = open;
        ctx.emit(CockpitEvent::SnapshotUpdated);
    }

    /// 选中/取消选中卡片(单选;P1 扩展 multi-select)。
    pub fn select_card(&mut self, id: Option<EntityId>, ctx: &mut ModelContext<Self>) {
        if self.selected != id {
            self.selected = id;
            ctx.emit(CockpitEvent::SnapshotUpdated);
        }
    }

    /// 全量刷新:遍历所有 workspace 的 terminal pane,进程内直取卡片数据。
    ///
    /// 数据通路证明(spec §4.1):本函数是卡片数据唯一入口,链路为
    /// `WorkspaceRegistry::all_workspaces` → `workspace.tabs` →
    /// `PaneGroup::terminal_pane_ids` → `TerminalView::{pane_configuration,
    /// pwd,is_long_running,id}` + `CLIAgentSessionsModel::session` —
    /// 全部内存读,零 `std::process`/CLI 子进程、零文件 stat。
    pub fn refresh(&mut self, ctx: &mut ModelContext<Self>) {
        let mut cards = Vec::new();
        let mut window_count = 0usize;

        for (_window_id, workspace) in WorkspaceRegistry::as_ref(ctx).all_workspaces(ctx) {
            window_count += 1;
            workspace.read(ctx, |workspace, ctx| {
                for tab in &workspace.tabs {
                    let pane_group = tab.pane_group.as_ref(ctx);
                    for pane_id in pane_group.terminal_pane_ids() {
                        let Some(terminal_view) =
                            pane_group.terminal_view_from_pane_id(pane_id, ctx)
                        else {
                            continue;
                        };
                        let view = terminal_view.as_ref(ctx);
                        let terminal_view_id = view.id();
                        let title = view.pane_configuration().as_ref(ctx).title().to_string();
                        let cwd = view.pwd();
                        let long_running = view.is_long_running();

                        let (agent_name, recap, tool_name, status) =
                            match CLIAgentSessionsModel::as_ref(ctx).session(terminal_view_id) {
                                Some(session) => agent_card_fields(session),
                                None => (
                                    None,
                                    None,
                                    None,
                                    if long_running {
                                        CockpitCardStatus::Busy
                                    } else {
                                        CockpitCardStatus::Idle
                                    },
                                ),
                            };

                        cards.push(CockpitCard {
                            terminal_view_id,
                            title,
                            cwd,
                            agent_name,
                            recap,
                            tool_name,
                            status,
                        });
                    }
                }
            });
        }

        // 排序:活跃(阻塞>工作中>busy>done)在前,空闲 shell 在后;
        // 同权重按标题稳定排序(hub-tui 活跃排序的等价物,P0 简化版)。
        cards.sort_by(|a, b| {
            a.status
                .sort_rank()
                .cmp(&b.status.sort_rank())
                .then_with(|| a.title.cmp(&b.title))
        });

        // 选中态清理(warpui 陷阱#4):终端已关闭 → 选中失配即清。
        if let Some(selected) = self.selected {
            if !cards
                .iter()
                .any(|card| card.terminal_view_id == selected)
            {
                self.selected = None;
            }
        }

        self.last_window_count = window_count;
        self.cards = cards;
        ctx.emit(CockpitEvent::SnapshotUpdated);
    }
}

/// agent 会话 → 卡片字段(status 三态 + L2 上下文回退链)。
fn agent_card_fields(
    session: &CLIAgentSession,
) -> (
    Option<&'static str>,
    Option<String>,
    Option<String>,
    CockpitCardStatus,
) {
    let context = &session.session_context;
    // recap 回退链:query(用户最新 prompt) > summary(插件摘要)。
    let recap = context
        .query
        .clone()
        .or_else(|| context.summary.clone());
    let status = match &session.status {
        CLIAgentSessionStatus::InProgress => CockpitCardStatus::Working,
        CLIAgentSessionStatus::Success => CockpitCardStatus::Done,
        CLIAgentSessionStatus::Blocked { message } => CockpitCardStatus::Blocked(message.clone()),
    };
    (
        Some(session.agent.display_name()),
        recap,
        context.tool_name.clone(),
        status,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_dot_key_and_rank_consistency() {
        // 有 agent 活动的前三态必须有 dot,空闲 shell 无 dot。
        assert!(CockpitCardStatus::Working.dot_key().is_some());
        assert!(CockpitCardStatus::Done.dot_key().is_some());
        assert!(CockpitCardStatus::Blocked(None).dot_key().is_some());
        assert!(CockpitCardStatus::Busy.dot_key().is_some());
        assert!(CockpitCardStatus::Idle.dot_key().is_none());
        // 排序权重:blocked < working < busy < done < idle。
        assert!(
            CockpitCardStatus::Blocked(None).sort_rank()
                < CockpitCardStatus::Working.sort_rank()
        );
        assert!(
            CockpitCardStatus::Working.sort_rank() < CockpitCardStatus::Busy.sort_rank()
        );
        assert!(CockpitCardStatus::Busy.sort_rank() < CockpitCardStatus::Done.sort_rank());
        assert!(CockpitCardStatus::Done.sort_rank() < CockpitCardStatus::Idle.sort_rank());
    }
}
