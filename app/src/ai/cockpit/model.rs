//! Cockpit 数据模型 — 终端/agent 卡片快照 + 视图参数(筛选/排序/分组)+ 批量注入状态。
//!
//! 所有状态集中于本 singleton model,视图纯渲染 + 派发意图(MVU 单一数据源,
//! observatory 同款)。`refresh` 是唯一的数据入口:全链路函数调用
//! (WorkspaceRegistry → TerminalView → CLIAgentSessionsModel),零子进程、零文件 IO
//! — hub-tui 的三路外置轮询在此整层蒸发(spec §1)。
//!
//! P1(spec §4.2):
//! - 视图参数(筛选/排序/分组)入 model,typed Action 可恢复(pane 重开后状态仍在);
//! - recap 四级回退 `response > query > summary > preview_tail`(preview 接
//!   TerminalModel,FairMutex 快照拷贝,锁外处理);
//! - branch/connected/writable 入卡(进程内直取,零额外查询);
//! - multi-select + 批量注入状态机(确认 → `try_send_text_to_cli_agent_or_rich_input`)。

use std::collections::HashSet;

use warpui::{AppContext, Entity, EntityId, ModelContext, SingletonEntity, ViewHandle};

use crate::terminal::cli_agent_sessions::{
    CLIAgentSession, CLIAgentSessionContext, CLIAgentSessionStatus, CLIAgentSessionsModel,
};
use crate::terminal::TerminalView;
use crate::workspace::WorkspaceRegistry;

/// preview 尾行从 active block 提取的行数上限(只取尾部,成本有界)。
const PREVIEW_TAIL_ROWS: usize = 4;

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

    /// 状态判别键(供 `CockpitStatusFilter` 匹配;Blocked 不比 message 文本)。
    pub fn kind(&self) -> CockpitStatusFilter {
        match self {
            CockpitCardStatus::Working => CockpitStatusFilter::Working,
            CockpitCardStatus::Done => CockpitStatusFilter::Done,
            CockpitCardStatus::Blocked(_) => CockpitStatusFilter::Blocked,
            CockpitCardStatus::Busy => CockpitStatusFilter::Busy,
            CockpitCardStatus::Idle => CockpitStatusFilter::Idle,
        }
    }
}

/// 状态筛选(kind 级;`None` = 不过滤)。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CockpitStatusFilter {
    Working,
    Done,
    Blocked,
    Busy,
    Idle,
}

impl CockpitStatusFilter {
    pub fn label(&self) -> &'static str {
        match self {
            CockpitStatusFilter::Working => "working",
            CockpitStatusFilter::Done => "done",
            CockpitStatusFilter::Blocked => "blocked",
            CockpitStatusFilter::Busy => "busy",
            CockpitStatusFilter::Idle => "shell",
        }
    }

    /// 状态筛选按钮循环:All → Working → Blocked → Done → Busy → Idle → All。
    pub fn cycle(current: Option<Self>) -> Option<Self> {
        use CockpitStatusFilter::*;
        match current {
            None => Some(Working),
            Some(Working) => Some(Blocked),
            Some(Blocked) => Some(Done),
            Some(Done) => Some(Busy),
            Some(Busy) => Some(Idle),
            Some(Idle) => None,
        }
    }
}

/// 排序模式。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CockpitSort {
    /// 活跃度(阻塞 > working > busy > done > idle),默认。
    #[default]
    Activity,
    /// 标题字典序。
    Title,
    /// cwd 字典序。
    Cwd,
}

impl CockpitSort {
    pub fn label(&self) -> &'static str {
        match self {
            CockpitSort::Activity => "activity",
            CockpitSort::Title => "title",
            CockpitSort::Cwd => "cwd",
        }
    }

    /// 排序按钮循环:Activity → Title → Cwd → Activity。
    pub fn cycle(self) -> Self {
        match self {
            CockpitSort::Activity => CockpitSort::Title,
            CockpitSort::Title => CockpitSort::Cwd,
            CockpitSort::Cwd => CockpitSort::Activity,
        }
    }
}

/// 分组模式。`CwdProject` = 按 cwd 的项目目录名聚合(hub-tui worktree 分组的等价物;
/// spec 表述"cwd 首段",但绝对路径首段恒为 `/` 无区分度,取末段目录名才对齐
/// worktree 粒度,见 §2.2 表)。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CockpitGroupBy {
    #[default]
    None,
    CwdProject,
}

impl CockpitGroupBy {
    pub fn label(&self) -> &'static str {
        match self {
            CockpitGroupBy::None => "none",
            CockpitGroupBy::CwdProject => "project",
        }
    }

    /// 分组按钮循环:None → CwdProject → None。
    pub fn cycle(self) -> Self {
        match self {
            CockpitGroupBy::None => CockpitGroupBy::CwdProject,
            CockpitGroupBy::CwdProject => CockpitGroupBy::None,
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
    /// recap 行:`response > query > summary > preview_tail` 四级回退(P1 补全)。
    pub recap: Option<String>,
    /// 当前工具名(L2 上下文;权限请求时上报)。
    pub tool_name: Option<String>,
    /// 状态。
    pub status: CockpitCardStatus,
    /// git 分支(active block OSC 回报,零子进程;None = 未上报)。
    pub branch: Option<String>,
    /// 是否为共享会话查看端(`is_shared_session_viewer`)。
    pub connected: bool,
    /// 是否可写(`!is_read_only`;只读 = SSH 查看端等)。
    pub writable: bool,
}

/// 分组后的卡片区间(对 `cards()` 的连续切片;分组关闭时恒为单个全量组)。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CockpitCardGroup {
    /// 分组键(项目目录名;未分组时为空串)。
    pub key: String,
    /// 组内卡片在 `cards()` 中的下标区间。
    pub range: std::ops::Range<usize>,
}

/// 批量注入待确认状态(确认对话框数据)。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CockpitPendingInjection {
    /// 待注入文本。
    pub text: String,
    /// 目标终端(选中集 ∩ 当前快照,按卡片顺序稳定排序)。
    pub target_ids: Vec<EntityId>,
}

/// Cockpit 事件。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CockpitEvent {
    /// 快照、视图参数或选中态更新,订阅方应 rerender。
    SnapshotUpdated,
}

pub struct CockpitModel {
    /// 筛选+排序+分组后的卡片(渲染直接消费;`all_cards` 是全量快照)。
    cards: Vec<CockpitCard>,
    /// 全量快照(筛选前;选中集清理依据)。
    all_cards: Vec<CockpitCard>,
    groups: Vec<CockpitCardGroup>,
    selected: Option<EntityId>,
    /// multi-select 选中集(批量注入目标)。
    selected_set: HashSet<EntityId>,
    /// 面板开合状态(toggle_cockpit / pane attach/detach 写;刷新 gate 读取)。
    panel_open: bool,
    /// 最近一次 refresh 收集到的终端数(诊断/标头计数)。
    last_window_count: usize,
    /// 文本筛选(标题/cwd/agent/recap/tool 不区分大小写子串)。
    filter: String,
    /// 状态筛选(None = 不过滤)。
    status_filter: Option<CockpitStatusFilter>,
    /// 排序模式。
    sort: CockpitSort,
    /// 分组模式。
    group_by: CockpitGroupBy,
    /// 批量注入待确认状态(Some = 确认对话框打开)。
    pending_injection: Option<CockpitPendingInjection>,
}

impl Entity for CockpitModel {
    type Event = CockpitEvent;
}

impl SingletonEntity for CockpitModel {}

impl CockpitModel {
    pub fn new(_ctx: &mut ModelContext<Self>) -> Self {
        Self {
            cards: Vec::new(),
            all_cards: Vec::new(),
            groups: Vec::new(),
            selected: None,
            selected_set: HashSet::new(),
            panel_open: false,
            last_window_count: 0,
            filter: String::new(),
            status_filter: None,
            sort: CockpitSort::default(),
            group_by: CockpitGroupBy::default(),
            pending_injection: None,
        }
    }

    // ── 只读访问 ───────────────────────────────────────────────────────

    /// 渲染用卡片序列(已按 filter/sort/group_by 处理)。
    pub fn cards(&self) -> &[CockpitCard] {
        &self.cards
    }

    pub fn groups(&self) -> &[CockpitCardGroup] {
        &self.groups
    }

    pub fn selected(&self) -> Option<EntityId> {
        self.selected
    }

    pub fn selected_set(&self) -> &HashSet<EntityId> {
        &self.selected_set
    }

    pub fn panel_open(&self) -> bool {
        self.panel_open
    }

    pub fn last_window_count(&self) -> usize {
        self.last_window_count
    }

    pub fn filter(&self) -> &str {
        &self.filter
    }

    pub fn status_filter(&self) -> Option<CockpitStatusFilter> {
        self.status_filter
    }

    pub fn sort(&self) -> CockpitSort {
        self.sort
    }

    pub fn group_by(&self) -> CockpitGroupBy {
        self.group_by
    }

    pub fn pending_injection(&self) -> Option<&CockpitPendingInjection> {
        self.pending_injection.as_ref()
    }

    /// 视图参数三元组(控制按钮标签同步用)。
    pub fn read_view_params(&self) -> (CockpitSort, CockpitGroupBy, Option<CockpitStatusFilter>) {
        (self.sort, self.group_by, self.status_filter)
    }

    /// 全量快照终端数(区分"无终端"与"被筛选排除")。
    pub fn all_card_count(&self) -> usize {
        self.all_cards.len()
    }

    pub fn set_panel_open(&mut self, open: bool, ctx: &mut ModelContext<Self>) {
        self.panel_open = open;
        ctx.emit(CockpitEvent::SnapshotUpdated);
    }

    /// 选中/取消选中卡片(单选高亮;multi-select 走 `toggle_card_selection`)。
    pub fn select_card(&mut self, id: Option<EntityId>, ctx: &mut ModelContext<Self>) {
        if self.selected != id {
            self.selected = id;
            ctx.emit(CockpitEvent::SnapshotUpdated);
        }
    }

    /// multi-select 切换(批量注入目标集)。
    pub fn toggle_card_selection(&mut self, id: EntityId, ctx: &mut ModelContext<Self>) {
        if !self.selected_set.insert(id) {
            self.selected_set.remove(&id);
        }
        ctx.emit(CockpitEvent::SnapshotUpdated);
    }

    /// 清空全部选中(单选 + multi-select)。
    pub fn clear_selection(&mut self, ctx: &mut ModelContext<Self>) {
        self.selected = None;
        self.selected_set.clear();
        ctx.emit(CockpitEvent::SnapshotUpdated);
    }

    // ── 视图参数(spec §2.2 Action 表;pane 重开后状态保留 = 可恢复) ──

    pub fn set_filter(&mut self, filter: String, ctx: &mut ModelContext<Self>) {
        if self.filter != filter {
            self.filter = filter;
            self.recompute_view();
            ctx.emit(CockpitEvent::SnapshotUpdated);
        }
    }

    pub fn set_status_filter(
        &mut self,
        filter: Option<CockpitStatusFilter>,
        ctx: &mut ModelContext<Self>,
    ) {
        if self.status_filter != filter {
            self.status_filter = filter;
            self.recompute_view();
            ctx.emit(CockpitEvent::SnapshotUpdated);
        }
    }

    pub fn set_sort(&mut self, sort: CockpitSort, ctx: &mut ModelContext<Self>) {
        if self.sort != sort {
            self.sort = sort;
            self.recompute_view();
            ctx.emit(CockpitEvent::SnapshotUpdated);
        }
    }

    pub fn set_group_by(&mut self, group_by: CockpitGroupBy, ctx: &mut ModelContext<Self>) {
        if self.group_by != group_by {
            self.group_by = group_by;
            self.recompute_view();
            ctx.emit(CockpitEvent::SnapshotUpdated);
        }
    }

    /// 依当前视图参数重算 `cards`/`groups`(refresh 与 set_* 共用)。
    fn recompute_view(&mut self) {
        let mut cards = self.all_cards.clone();
        let filter = self.filter.clone();
        let status_filter = self.status_filter;
        let sort = self.sort;
        let group_by = self.group_by;
        apply_view_params(&mut cards, &filter, status_filter, sort, group_by);
        self.groups = compute_groups(&cards, group_by);
        self.cards = cards;
    }

    // ── 批量注入状态机 ─────────────────────────────────────────────────

    /// 进入注入确认态:目标 = 选中集 ∩ 当前全量快照(按卡片顺序)。
    /// 空文本或目标为空时不进入确认对话框。
    pub fn begin_injection(&mut self, text: String, ctx: &mut ModelContext<Self>) {
        let text = text.trim().to_string();
        if text.is_empty() || self.selected_set.is_empty() {
            return;
        }
        let target_ids = resolve_targets(&self.all_cards, &self.selected_set);
        if target_ids.is_empty() {
            return;
        }
        self.pending_injection = Some(CockpitPendingInjection { text, target_ids });
        ctx.emit(CockpitEvent::SnapshotUpdated);
    }

    /// 取消注入(保留选中集,便于改文本重试)。
    pub fn cancel_injection(&mut self, ctx: &mut ModelContext<Self>) {
        if self.pending_injection.take().is_some() {
            ctx.emit(CockpitEvent::SnapshotUpdated);
        }
    }

    /// 确认注入:逐目标 `try_send_text_to_cli_agent_or_rich_input`
    /// (agent 终端富输入打开 → 进 composer 不炸 TUI;关闭或纯 shell → PTY 补换行执行,
    /// hub-tui multi-inject 语义)。注入完成清空选中集。
    ///
    /// 返回成功发送的目标数(目标终端中途关闭的跳过)。
    pub fn confirm_injection(&mut self, ctx: &mut ModelContext<Self>) -> usize {
        let Some(pending) = self.pending_injection.take() else {
            return 0;
        };
        let targets: Vec<(EntityId, ViewHandle<TerminalView>)> = terminal_views(ctx)
            .into_iter()
            .filter(|(id, _)| pending.target_ids.contains(id))
            .collect();
        let mut sent = 0usize;
        for (_, terminal_view) in targets {
            let text = pending.text.clone();
            terminal_view.update(ctx, |view, ctx| {
                let execute = format!("{text}\n");
                let payload = if view.is_cli_agent_rich_input_open(ctx) {
                    text.clone()
                } else {
                    execute.clone()
                };
                if view
                    .try_send_text_to_cli_agent_or_rich_input(payload, ctx)
                    .is_none()
                {
                    // 无活跃 CLI agent(纯 shell 卡):直接落 PTY 执行。
                    view.write_to_pty(execute.into_bytes(), ctx);
                }
            });
            sent += 1;
        }
        self.selected_set.clear();
        self.selected = None;
        ctx.emit(CockpitEvent::SnapshotUpdated);
        sent
    }

    // ── 刷新 ───────────────────────────────────────────────────────────

    /// 全量刷新:遍历所有 workspace 的 terminal pane,进程内直取卡片数据。
    ///
    /// 数据通路证明(spec §4.1):本函数是卡片数据唯一入口,链路为
    /// `WorkspaceRegistry::all_workspaces` → `workspace.tabs` →
    /// `PaneGroup::terminal_pane_ids` → `TerminalView::{pane_configuration,
    /// pwd,id}` + `TerminalModel`(FairMutex 短锁快照) +
    /// `CLIAgentSessionsModel::session` — 全部内存读,零 `std::process`/CLI
    /// 子进程、零文件 stat。
    ///
    /// P1:事件驱动(`CLIAgentSessionsModelEvent` 订阅,见 view.rs)+ 10s
    /// 低频 timer 对账(终端开合无会话事件,靠对账兜底)。
    pub fn refresh(&mut self, ctx: &mut ModelContext<Self>) {
        let mut window_count = 0usize;
        let mut all_cards = Vec::new();

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
                        all_cards.push(snapshot_card(view, ctx));
                    }
                }
            });
        }

        // 快照稳定排序(卡片顺序与 workspace/tab 遍历序无关)。
        all_cards.sort_by(|a, b| a.terminal_view_id.cmp(&b.terminal_view_id));

        // 选中态清理(warpui 陷阱#4):终端已关闭 → 选中失配即清。
        // 注意依据是全量快照(而非筛选结果),筛选不丢选中。
        if let Some(selected) = self.selected {
            if !all_cards
                .iter()
                .any(|card| card.terminal_view_id == selected)
            {
                self.selected = None;
            }
        }
        self.selected_set.retain(|id| {
            all_cards
                .iter()
                .any(|card| card.terminal_view_id == *id)
        });
        // 目标终端已消失 → 待确认注入作废。
        if let Some(pending) = &mut self.pending_injection {
            pending.target_ids.retain(|id| {
                all_cards
                    .iter()
                    .any(|card| card.terminal_view_id == *id)
            });
            if pending.target_ids.is_empty() {
                self.pending_injection = None;
            }
        }

        self.last_window_count = window_count;
        self.all_cards = all_cards;
        self.recompute_view();
        ctx.emit(CockpitEvent::SnapshotUpdated);
    }
}

/// 收集全部 workspace 的 terminal view(id → handle),refresh 与注入共用。
fn terminal_views(ctx: &AppContext) -> Vec<(EntityId, ViewHandle<TerminalView>)> {
    let mut out = Vec::new();
    for (_window_id, workspace) in WorkspaceRegistry::as_ref(ctx).all_workspaces(ctx) {
        workspace.read(ctx, |workspace, ctx| {
            for tab in &workspace.tabs {
                let pane_group = tab.pane_group.as_ref(ctx);
                for pane_id in pane_group.terminal_pane_ids() {
                    if let Some(terminal_view) =
                        pane_group.terminal_view_from_pane_id(pane_id, ctx)
                    {
                        let id = terminal_view.as_ref(ctx).id();
                        out.push((id, terminal_view));
                    }
                }
            }
        });
    }
    out
}

/// 单个终端 → 卡片快照(全部进程内直取)。
///
/// TerminalModel 访问收敛到单次 FairMutex 短锁:锁内只做字符串拷贝
/// (long_running 判定 + branch + preview 尾行),渲染在锁外(spec §5-2)。
fn snapshot_card(view: &TerminalView, ctx: &AppContext) -> CockpitCard {
    let terminal_view_id = view.id();
    let title = view.pane_configuration().as_ref(ctx).title().to_string();
    let cwd = view.pwd();
    let connected = view.is_shared_session_viewer();
    let writable = !view.is_read_only();

    // 单锁快照:活跃块状态 + git 分支 + preview 尾行。
    let (long_running, branch, preview_tail) = {
        let model = view.model.lock();
        let active_block = model.block_list().active_block();
        (
            active_block.is_active_and_long_running(),
            active_block.git_branch_name().cloned(),
            preview_tail_from_output(
                &active_block
                    .output_grid()
                    .contents_to_string(false, Some(PREVIEW_TAIL_ROWS)),
            ),
        )
    };

    let (agent_name, recap, tool_name, status) =
        match CLIAgentSessionsModel::as_ref(ctx).session(terminal_view_id) {
            Some(session) => agent_card_fields(session, preview_tail),
            None => (
                None,
                preview_tail,
                None,
                if long_running {
                    CockpitCardStatus::Busy
                } else {
                    CockpitCardStatus::Idle
                },
            ),
        };

    CockpitCard {
        terminal_view_id,
        title,
        cwd,
        agent_name,
        recap,
        tool_name,
        status,
        branch,
        connected,
        writable,
    }
}

/// agent 会话 → 卡片字段(status 三态 + 四级 recap 回退链)。
fn agent_card_fields(
    session: &CLIAgentSession,
    preview_tail: Option<String>,
) -> (
    Option<&'static str>,
    Option<String>,
    Option<String>,
    CockpitCardStatus,
) {
    let context = &session.session_context;
    let status = match &session.status {
        CLIAgentSessionStatus::InProgress => CockpitCardStatus::Working,
        CLIAgentSessionStatus::Success => CockpitCardStatus::Done,
        CLIAgentSessionStatus::Blocked { message } => CockpitCardStatus::Blocked(message.clone()),
    };
    (
        Some(session.agent.display_name()),
        recap_chain(context, preview_tail),
        context.tool_name.clone(),
        status,
    )
}

/// recap 四级回退(spec §4.2):`response > query > summary > preview_tail`。
fn recap_chain(
    context: &CLIAgentSessionContext,
    preview_tail: Option<String>,
) -> Option<String> {
    context
        .response
        .clone()
        .or_else(|| context.query.clone())
        .or_else(|| context.summary.clone())
        .or(preview_tail)
}

/// preview 尾行:active block 输出的最后一条非空行(单行,已 trim)。
fn preview_tail_from_output(output: &str) -> Option<String> {
    output
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .map(|line| line.trim().to_string())
}

/// 文本筛选:标题/cwd/agent 名/recap/tool 不区分大小写子串匹配。
fn card_matches_filter(card: &CockpitCard, filter: &str) -> bool {
    if filter.is_empty() {
        return true;
    }
    let needle = filter.to_lowercase();
    let matches = |haystack: Option<&str>| {
        haystack.is_some_and(|h| h.to_lowercase().contains(&needle))
    };
    card.title.to_lowercase().contains(&needle)
        || matches(card.cwd.as_deref())
        || matches(card.agent_name)
        || matches(card.recap.as_deref())
        || matches(card.tool_name.as_deref())
}

/// 分组键:cwd 末段目录名(项目/worktree 粒度;未上报 = "?")。
fn cwd_group_key(cwd: &Option<String>) -> String {
    cwd.as_deref()
        .and_then(|c| c.rsplit('/').find(|segment| !segment.is_empty()))
        .unwrap_or("?")
        .to_string()
}

/// 就地应用视图参数:筛选 → 排序(分组开启时分组键为主键,保证组连续)。
fn apply_view_params(
    cards: &mut Vec<CockpitCard>,
    filter: &str,
    status_filter: Option<CockpitStatusFilter>,
    sort: CockpitSort,
    group_by: CockpitGroupBy,
) {
    cards.retain(|card| {
        card_matches_filter(card, filter)
            && status_filter.is_none_or(|kind| card.status.kind() == kind)
    });
    match group_by {
        CockpitGroupBy::None => sort_cards(cards, sort),
        CockpitGroupBy::CwdProject => {
            let mut keyed: Vec<(String, CockpitCard)> = cards
                .drain(..)
                .map(|c| (cwd_group_key(&c.cwd), c))
                .collect();
            keyed.sort_by(|(ka, a), (kb, b)| ka.cmp(kb).then_with(|| sort_cards_cmp(a, b, sort)));
            cards.extend(keyed.into_iter().map(|(_, c)| c));
        }
    }
}

/// 排序比较(不含分组键;末级用 EntityId 断平,保证稳定序)。
fn sort_cards_cmp(a: &CockpitCard, b: &CockpitCard, sort: CockpitSort) -> std::cmp::Ordering {
    match sort {
        CockpitSort::Activity => a
            .status
            .sort_rank()
            .cmp(&b.status.sort_rank())
            .then_with(|| a.title.cmp(&b.title)),
        CockpitSort::Title => a.title.cmp(&b.title),
        CockpitSort::Cwd => a.cwd.cmp(&b.cwd).then_with(|| a.title.cmp(&b.title)),
    }
    .then_with(|| a.terminal_view_id.cmp(&b.terminal_view_id))
}

fn sort_cards(cards: &mut [CockpitCard], sort: CockpitSort) {
    cards.sort_by(|a, b| sort_cards_cmp(a, b, sort));
}

/// 由连续分组键切出分组区间(分组关闭 → 单个全量组)。
fn compute_groups(cards: &[CockpitCard], group_by: CockpitGroupBy) -> Vec<CockpitCardGroup> {
    match group_by {
        CockpitGroupBy::None => vec![CockpitCardGroup {
            key: String::new(),
            range: 0..cards.len(),
        }],
        CockpitGroupBy::CwdProject => {
            let mut groups = Vec::new();
            let mut start = 0usize;
            let mut current_key: Option<String> = None;
            for (idx, card) in cards.iter().enumerate() {
                let key = cwd_group_key(&card.cwd);
                match &current_key {
                    Some(k) if *k == key => {}
                    Some(_) => {
                        groups.push(CockpitCardGroup {
                            key: current_key.clone().unwrap_or_default(),
                            range: start..idx,
                        });
                        start = idx;
                        current_key = Some(key);
                    }
                    None => current_key = Some(key),
                }
            }
            if let Some(key) = current_key {
                groups.push(CockpitCardGroup {
                    key,
                    range: start..cards.len(),
                });
            }
            groups
        }
    }
}


/// 注入目标解析:选中集 ∩ 全量快照,按卡片顺序稳定输出(选中集本身无序)。
fn resolve_targets(
    all_cards: &[CockpitCard],
    selected_set: &HashSet<EntityId>,
) -> Vec<EntityId> {
    all_cards
        .iter()
        .filter(|card| selected_set.contains(&card.terminal_view_id))
        .map(|card| card.terminal_view_id)
        .collect()
}
#[cfg(test)]
mod tests {
    use super::*;
    use warpui::ReadModel;

    fn card(id: usize, title: &str, cwd: Option<&str>, status: CockpitCardStatus) -> CockpitCard {
        CockpitCard {
            terminal_view_id: EntityId::from_usize(id),
            title: title.to_string(),
            cwd: cwd.map(str::to_string),
            agent_name: None,
            recap: None,
            tool_name: None,
            status,
            branch: None,
            connected: false,
            writable: true,
        }
    }

    fn ids(cards: &[CockpitCard]) -> Vec<EntityId> {
        cards.iter().map(|c| c.terminal_view_id).collect()
    }

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
        assert!(CockpitCardStatus::Working.sort_rank() < CockpitCardStatus::Busy.sort_rank());
        assert!(CockpitCardStatus::Busy.sort_rank() < CockpitCardStatus::Done.sort_rank());
        assert!(CockpitCardStatus::Done.sort_rank() < CockpitCardStatus::Idle.sort_rank());
    }

    #[test]
    fn status_kind_maps_without_payload() {
        // Blocked 携带 message,kind 匹配仍只看判别键。
        assert_eq!(
            CockpitCardStatus::Blocked(Some("msg".into())).kind(),
            CockpitStatusFilter::Blocked
        );
        assert_eq!(CockpitCardStatus::Idle.kind(), CockpitStatusFilter::Idle);
    }

    #[test]
    fn status_filter_cycle_visits_all_then_none() {
        use CockpitStatusFilter::*;
        let seq = [
            None,
            Some(Working),
            Some(Blocked),
            Some(Done),
            Some(Busy),
            Some(Idle),
            None,
        ];
        let mut current = None;
        for expected in seq {
            assert_eq!(current, expected);
            current = CockpitStatusFilter::cycle(current);
        }
    }

    #[test]
    fn sort_and_group_cycle_round_trip() {
        assert_eq!(
            CockpitSort::Activity.cycle().cycle().cycle(),
            CockpitSort::Activity
        );
        assert_eq!(CockpitGroupBy::None.cycle().cycle(), CockpitGroupBy::None);
    }

    #[test]
    fn preview_tail_takes_last_nonempty_line() {
        assert_eq!(preview_tail_from_output(""), None);
        assert_eq!(preview_tail_from_output("\n\n  \n"), None);
        assert_eq!(
            preview_tail_from_output("first\nmid\n  last line  \n"),
            Some("last line".to_string())
        );
        // CJK 行不受影响。
        assert_eq!(
            preview_tail_from_output("out\n输出完成"),
            Some("输出完成".to_string())
        );
    }

    #[test]
    fn recap_chain_four_level_fallback() {
        let mut context = CLIAgentSessionContext::default();
        let preview = Some("tail".to_string());
        // 全空 → preview。
        assert_eq!(recap_chain(&context, preview.clone()), preview.clone());
        // summary → 第三级。
        context.summary = Some("sum".into());
        assert_eq!(recap_chain(&context, preview.clone()), Some("sum".into()));
        // query 压过 summary。
        context.query = Some("q".into());
        assert_eq!(recap_chain(&context, preview.clone()), Some("q".into()));
        // response 最高。
        context.response = Some("r".into());
        assert_eq!(recap_chain(&context, preview.clone()), Some("r".into()));
    }

    #[test]
    fn filter_matches_case_insensitive_across_fields() {
        let mut c = card(1, "Build Server", Some("/repo/a"), CockpitCardStatus::Idle);
        assert!(card_matches_filter(&c, "build"));
        assert!(card_matches_filter(&c, "REPO/A"));
        c.agent_name = Some("Claude Code");
        c.recap = Some("优化数据库".into());
        c.tool_name = Some("Edit".into());
        assert!(card_matches_filter(&c, "claude"));
        assert!(card_matches_filter(&c, "数据库"));
        assert!(card_matches_filter(&c, "edit"));
        assert!(!card_matches_filter(&c, "nope"));
        // 空筛选恒匹配。
        assert!(card_matches_filter(&c, ""));
    }

    #[test]
    fn cwd_group_key_uses_last_segment() {
        assert_eq!(
            cwd_group_key(&Some("/home/yy/orca/workspaces/dais/hub-cockpit".into())),
            "hub-cockpit"
        );
        assert_eq!(cwd_group_key(&Some("/".into())), "?");
        assert_eq!(cwd_group_key(&Some("/repo/".into())), "repo");
        assert_eq!(cwd_group_key(&None), "?");
    }

    #[test]
    fn apply_view_params_activity_sort_default() {
        let mut cards = vec![
            card(1, "idle-b", Some("/w/shell2"), CockpitCardStatus::Idle),
            card(2, "agent-a", Some("/w/alpha/repo"), CockpitCardStatus::Working),
            card(
                3,
                "agent-b",
                Some("/w/beta/repo"),
                CockpitCardStatus::Blocked(None),
            ),
            card(4, "idle-a", Some("/w/alpha/repo"), CockpitCardStatus::Idle),
            card(5, "done-c", None, CockpitCardStatus::Done),
        ];
        apply_view_params(
            &mut cards,
            "",
            None,
            CockpitSort::Activity,
            CockpitGroupBy::None,
        );
        // 排序:blocked(id3) > working(id2) > done(id5) > idle(标题序 idle-a=4, idle-b=1)。
        assert_eq!(
            ids(&cards),
            vec![
                EntityId::from_usize(3),
                EntityId::from_usize(2),
                EntityId::from_usize(5),
                EntityId::from_usize(4),
                EntityId::from_usize(1),
            ]
        );
    }

    #[test]
    fn apply_view_params_status_filter() {
        let mut cards = vec![
            card(1, "x", None, CockpitCardStatus::Idle),
            card(2, "y", None, CockpitCardStatus::Working),
            card(3, "z", None, CockpitCardStatus::Blocked(Some("m".into()))),
        ];
        apply_view_params(
            &mut cards,
            "",
            Some(CockpitStatusFilter::Working),
            CockpitSort::Activity,
            CockpitGroupBy::None,
        );
        assert_eq!(ids(&cards), vec![EntityId::from_usize(2)]);
        // Blocked 按判别键命中(payload 不参与匹配)。
        let mut cards = vec![
            card(1, "x", None, CockpitCardStatus::Idle),
            card(3, "z", None, CockpitCardStatus::Blocked(Some("m".into()))),
        ];
        apply_view_params(
            &mut cards,
            "",
            Some(CockpitStatusFilter::Blocked),
            CockpitSort::Activity,
            CockpitGroupBy::None,
        );
        assert_eq!(ids(&cards), vec![EntityId::from_usize(3)]);
    }

    #[test]
    fn apply_view_params_text_filter_and_group() {
        // 文本筛选:命中 title/cwd。
        let mut cards = vec![
            card(1, "build-server", Some("/w/alpha"), CockpitCardStatus::Idle),
            card(2, "agent", Some("/w/beta"), CockpitCardStatus::Idle),
        ];
        apply_view_params(&mut cards, "build", None, CockpitSort::Title, CockpitGroupBy::None);
        assert_eq!(ids(&cards), vec![EntityId::from_usize(1)]);

        // 分组:按项目名连续分段,组内按标题排。
        let mut cards3 = vec![
            card(1, "b", Some("/w/alpha"), CockpitCardStatus::Idle),
            card(2, "a", Some("/w/beta"), CockpitCardStatus::Idle),
            card(3, "a", Some("/w/alpha"), CockpitCardStatus::Idle),
        ];
        apply_view_params(
            &mut cards3,
            "",
            None,
            CockpitSort::Title,
            CockpitGroupBy::CwdProject,
        );

        assert_eq!(
            ids(&cards3),
            vec![
                EntityId::from_usize(3),
                EntityId::from_usize(1),
                EntityId::from_usize(2),
            ]
        ); // alpha 组(标题序 a,b)然后 beta 组。
        let groups = compute_groups(&cards3, CockpitGroupBy::CwdProject);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].key, "alpha");
        assert_eq!(groups[0].range, 0..2);
        assert_eq!(groups[1].key, "beta");
        assert_eq!(groups[1].range, 2..3);

        // 未分组:单个全量组。
        let groups = compute_groups(&cards3, CockpitGroupBy::None);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].range, 0..3);
        assert_eq!(groups[0].key, "");
    }

    #[test]
    fn compute_groups_unknown_cwd_buckets_together() {
        let cards = vec![
            card(1, "a", None, CockpitCardStatus::Idle),
            card(2, "b", None, CockpitCardStatus::Idle),
        ];
        let groups = compute_groups(&cards, CockpitGroupBy::CwdProject);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].key, "?");
        assert_eq!(groups[0].range, 0..2);
    }

    #[test]
    fn resolve_targets_follows_card_order() {
        let all_cards = vec![
            card(1, "a", None, CockpitCardStatus::Idle),
            card(2, "b", None, CockpitCardStatus::Working),
            card(3, "c", None, CockpitCardStatus::Idle),
        ];
        // HashSet 插入序与卡片序不同,验证输出仍按卡片顺序。
        let selected_set: HashSet<EntityId> = [
            EntityId::from_usize(3),
            EntityId::from_usize(1),
        ]
        .into_iter()
        .collect();
        assert_eq!(
            resolve_targets(&all_cards, &selected_set),
            vec![EntityId::from_usize(1), EntityId::from_usize(3)]
        );
        // 已关闭的终端(不在快照内)被自然排除。
        let stale: HashSet<EntityId> = [EntityId::from_usize(99)].into_iter().collect();
        assert!(resolve_targets(&all_cards, &stale).is_empty());
    }

    /// 状态机走查(warpui App 测试桩):multi-select / 视图参数 / 注入状态机
    /// 的可达分支;快照为空(无 workspace)时 refresh 安全。
    #[test]
    fn cockpit_model_state_machine() {
        warpui::App::test((), |mut app| async move {
            // refresh 链路依赖 WorkspaceRegistry 单例(空注册表 = 零 workspace)。
            app.add_singleton_model(|_| WorkspaceRegistry::new());
            let model = app.add_singleton_model(CockpitModel::new);

            // 初始态。
            assert!(app.read_model(&model, |m, _| m.selected().is_none()));
            assert!(app.read_model(&model, |m, _| m.selected_set().is_empty()));
            assert!(app.read_model(&model, |m, _| m.pending_injection().is_none()));

            // refresh(零 workspace):不 panic,快照为空。
            model.update(&mut app, |m, ctx| m.refresh(ctx));
            assert_eq!(app.read_model(&model, |m, _| m.all_card_count()), 0);
            assert_eq!(app.read_model(&model, |m, _| m.last_window_count()), 0);

            // multi-select toggle ×2 = 回到未选。
            let id_a = EntityId::from_usize(11);
            let id_b = EntityId::from_usize(22);
            model.update(&mut app, |m, ctx| m.toggle_card_selection(id_a, ctx));
            model.update(&mut app, |m, ctx| m.toggle_card_selection(id_b, ctx));
            assert_eq!(app.read_model(&model, |m, _| m.selected_set().len()), 2);
            model.update(&mut app, |m, ctx| m.toggle_card_selection(id_a, ctx));
            assert_eq!(app.read_model(&model, |m, _| m.selected_set().len()), 1);

            // 视图参数:set + 幂等。
            model.update(&mut app, |m, ctx| {
                m.set_filter("alpha".into(), ctx);
            });
            model.update(&mut app, |m, ctx| {
                m.set_sort(CockpitSort::Title, ctx);
            });
            model.update(&mut app, |m, ctx| {
                m.set_group_by(CockpitGroupBy::CwdProject, ctx);
            });
            model.update(&mut app, |m, ctx| {
                m.set_status_filter(Some(CockpitStatusFilter::Blocked), ctx);
            });
            assert_eq!(
                app.read_model(&model, |m, _| m.read_view_params()),
                (CockpitSort::Title, CockpitGroupBy::CwdProject, Some(CockpitStatusFilter::Blocked))
            );

            // begin_injection:选中非空但快照为空 → 目标为空 → 不进入确认态。
            model.update(&mut app, |m, ctx| {
                m.begin_injection("git status".into(), ctx);
            });
            assert!(app.read_model(&model, |m, _| m.pending_injection().is_none()));

            // cancel/confirm 无 pending 时为安全 no-op。
            model.update(&mut app, |m, ctx| m.cancel_injection(ctx));
            let sent = model.update(&mut app, |m, ctx| m.confirm_injection(ctx));
            assert_eq!(sent, 0);

            // clear_selection 清空单选+multi-select。
            model.update(&mut app, |m, ctx| {
                m.select_card(Some(id_b), ctx);
            });
            model.update(&mut app, |m, ctx| m.clear_selection(ctx));
            assert!(app.read_model(&model, |m, _| m.selected().is_none()));
            assert!(app.read_model(&model, |m, _| m.selected_set().is_empty()));
        });
    }
}
