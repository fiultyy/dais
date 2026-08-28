//! Cockpit 纯壳模型 — 终端/agent 卡片快照 + 视图参数(筛选/排序/分组)+ 批量注入状态。
//!
//! 自 app/src/ai/cockpit/model.rs 拆出 (2026-08-28, 布局拆分 v1 步骤4):
//! - **本 crate = 纯壳**: POD 类型 (Card/Group/Status/Sort/GroupBy/PendingInjection)
//!   + Model 纯字段状态机 + 纯函数派生 (apply_view_params/compute_groups/resolve_targets)。
//!   依赖仅 warpui — nav crate 可依赖它而不拉进 app。
//! - **留在 app**: `refresh` (绑死 WorkspaceRegistry→TerminalView→CLIAgentSessionsModel
//!   采集链) / `confirm_injection` (TerminalView PTY 写) / `set_panel_open` 的
//!   TerminalActivityModel hub 同步。app 经 `ModelHandle::update` 推入快照
//!   (`replace_snapshot` / `retain_live` / `confirm_injection_with`), 事件契约
//!   `CockpitEvent::SnapshotUpdated` 不变。
//!
//! MVU 单一数据源约定不变 (observatory 同款): 视图纯渲染 + 派发意图。
//!
//! 幂等性 (本 crate 测试锁定): set_* 幂等 (同值不 emit) / refresh 派生纯函数
//! (同输入同输出) / selection toggle 对合 (×2 = 未选) / 注入状态机安全 no-op。

use std::collections::HashSet;

use warpui::{Entity, EntityId, ModelContext, SingletonEntity};

pub mod pure;

pub use pure::{
    apply_view_params, card_matches_filter, compute_groups, cwd_group_key,
    preview_tail_from_output, recap_chain, resolve_targets, sort_cards, sort_cards_cmp,
};


/// 卡片状态(agent 会话状态优先,无 agent 时按终端活跃度回退)。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CockpitCardStatus {
    /// agent 会话进行中(CLI agent session InProgress)。
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
            CockpitCardStatus::Working => Some("working"),
            CockpitCardStatus::Done => Some("done"),
            CockpitCardStatus::Blocked(_) => Some("blocked"),
            CockpitCardStatus::Busy => Some("busy"),
            CockpitCardStatus::Idle => None,
        }
    }

    /// 排序权重:阻塞 > working > busy > done > idle。
    pub fn sort_rank(&self) -> u8 {
        match self {
            CockpitCardStatus::Blocked(_) => 0,
            CockpitCardStatus::Working => 1,
            CockpitCardStatus::Busy => 2,
            CockpitCardStatus::Done => 3,
            CockpitCardStatus::Idle => 4,
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
    /// 持久化项目目录名快照(app 侧从 ProjectManagementModel 推入;
    /// nav 空项目组渲染消费,不 import app 类型)。
    empty_project_names: Vec<String>,
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
    /// 快照推入计数(app 侧 refresh 推入观测;非 UI 契约)。
    refresh_count: u64,
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
            empty_project_names: Vec::new(),
            last_window_count: 0,
            filter: String::new(),
            status_filter: None,
            sort: CockpitSort::default(),
            group_by: CockpitGroupBy::default(),
            pending_injection: None,
            refresh_count: 0,
        }
    }

    // ── 读 API(nav 渲染消费的完整面) ─────────────────────────────────

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

    /// 持久化项目目录名快照(空项目组渲染;app 推入)。
    pub fn empty_project_names(&self) -> &[String] {
        &self.empty_project_names
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

    pub fn read_view_params(&self) -> (CockpitSort, CockpitGroupBy, Option<CockpitStatusFilter>) {
        (self.sort, self.group_by, self.status_filter)
    }

    pub fn all_card_count(&self) -> usize {
        self.all_cards.len()
    }

    /// 快照推入计数(app refresh 推入路径的测试观测点)。
    pub fn refresh_count(&self) -> u64 {
        self.refresh_count
    }

    // ── app 侧数据推入入口 (原 refresh 的纯状态半边) ────────────────────

    /// 整体替换全量快照并重算视图 (app `refresh` 采集完成后推入)。
    /// 选中/注入目标对快照的清理规则不变: 依据是全量快照而非筛选结果。
    pub fn replace_snapshot(
        &mut self,
        all_cards: Vec<CockpitCard>,
        window_count: usize,
        ctx: &mut ModelContext<Self>,
    ) {
        self.refresh_count += 1;
        // 快照稳定排序(卡片顺序与 workspace/tab 遍历序无关)。
        let mut all_cards = all_cards;
        all_cards.sort_by(|a, b| a.terminal_view_id.cmp(&b.terminal_view_id));

        // 选中态清理(warpui 陷阱#4):终端已关闭 → 选中失配即清。
        if let Some(selected) = self.selected {
            if !all_cards.iter().any(|card| card.terminal_view_id == selected) {
                self.selected = None;
            }
        }
        self.selected_set
            .retain(|id| all_cards.iter().any(|card| card.terminal_view_id == *id));
        // 目标终端已消失 → 待确认注入作废。
        if let Some(pending) = &mut self.pending_injection {
            pending
                .target_ids
                .retain(|id| all_cards.iter().any(|card| card.terminal_view_id == *id));
            if pending.target_ids.is_empty() {
                self.pending_injection = None;
            }
        }

        self.last_window_count = window_count;
        self.all_cards = all_cards;
        self.recompute_view();
        ctx.emit(CockpitEvent::SnapshotUpdated);
    }

    /// 推入持久化项目名快照(空项目组渲染数据)。
    /// 幂等:同快照重复推入不 emit。
    pub fn set_empty_project_names(
        &mut self,
        names: Vec<String>,
        ctx: &mut ModelContext<Self>,
    ) {
        if self.empty_project_names != names {
            self.empty_project_names = names;
            ctx.emit(CockpitEvent::SnapshotUpdated);
        }
    }

    // ── 状态机(原样迁移) ──────────────────────────────────────────────

    pub fn set_panel_open(&mut self, open: bool, ctx: &mut ModelContext<Self>) {
        self.panel_open = open;
        // hub 同步(app 侧 TerminalActivityModel)由 app 的 wrapper 处理;
        // 本 crate 不知道 hub 存在。
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

    /// 依当前视图参数重算 `cards`/`groups`(replace_snapshot 与 set_* 共用)。
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

    /// 取出待注入状态(无 pending 时安全 no-op → None)。
    /// 实际注入(TerminalView PTY 写)在 app 侧执行;执行完毕清空选中。
    pub fn take_pending_injection(&mut self, ctx: &mut ModelContext<Self>) -> Option<CockpitPendingInjection> {
        let pending = self.pending_injection.take()?;
        self.selected_set.clear();
        self.selected = None;
        ctx.emit(CockpitEvent::SnapshotUpdated);
        Some(pending)
    }

    /// 无 pending 时为安全 no-op(保留旧 API 语义;nav 旧调用点)。
    pub fn cancel_injection(&mut self, ctx: &mut ModelContext<Self>) {
        if self.pending_injection.take().is_some() {
            ctx.emit(CockpitEvent::SnapshotUpdated);
        }
    }
}

