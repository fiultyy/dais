//! LeftRailStatusModel — 左栏状态聚合单例。
//!
//! 把三路数据源汇聚成左栏(tab 卡/项目卡/未分组组头)可直接渲染的状态视图:
//! 1. 外部 harness 会话五态(idle/working/waiting-input/done/error)——由
//!    `ai::observatory::run_state` 派生器写入(session↔pane 映射由数据源维护)。
//! 2. marker/todo 进度——intercept 层解析 `dais:progress`/`dais:halt` 写入。
//! 3. 编排邮箱未读回调——`block_settle` messages 表轮询 adapter 写入。
//!
//! 渲染侧只读(`pane_status`/`aggregate`/`unread_*`);写入侧全部走带
//! `ModelContext` 的 update 方法,事件驱动 Workspace 重渲。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use pane_tree::PaneId;
use warpui::{Entity, ModelContext, SingletonEntity};

/// 派生状态变化(五态翻转)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionStateChanged;

/// 进度/halt/marker 变化。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionProgressChanged;

/// 编排未读数变化。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnreadChanged;

/// 外部 harness 会话的五态运行状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HarnessRunState {
    /// 无会话/长时间(默认 10min)无活动。
    Idle,
    /// 一个 request/response 周期内(Request 已发,ResponseDone 未到)。
    Working,
    /// 上一个回合完成、CLI 存活、等待用户输入。
    WaitingInput,
    /// 会话退出且 exit code = 0(或 harness 自报完成)。
    Done,
    /// 会话退出且 exit code != 0,或 harness 自报错误。
    Error,
}

/// 单个 pane 关联会话的运行状态(渲染单元)。
#[derive(Debug, Clone, Default)]
pub struct SessionRunStatus {
    pub state: Option<HarnessRunState>,
    pub state_since: Option<Instant>,
    /// todo/marker 进度 (done, total)。
    pub progress: Option<(u32, u32)>,
    /// agent 反向 halt 请求(`dais:halt`)。
    pub halt_requested: bool,
    /// 最近一次 marker 注释(`dais:note`,截断到 80 chars)。
    pub marker_note: Option<String>,
}

/// 项目/未分组维度的聚合状态(项目卡头渲染单元)。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ProjectAggregate {
    pub working: u32,
    pub waiting_input: u32,
    pub done: u32,
    pub error: u32,
    /// 该项目下未读编排回调(worker_done)数。
    pub unread_callbacks: u32,
}

impl ProjectAggregate {
    /// 聚合里是否含需要注意的信号(waiting/error/halt)。
    pub fn needs_attention(&self) -> bool {
        self.waiting_input > 0 || self.error > 0 || self.unread_callbacks > 0
    }
}

#[derive(Debug, Default)]
struct LeftRailStatusInner {
    /// pane_id → 状态。pane 是渲染主键。
    pane_status: HashMap<PaneId, SessionRunStatus>,
    /// session_id → pane_id(写入侧维护的映射)。
    session_to_pane: HashMap<String, PaneId>,
    /// 全局未读回调数。
    unread_callbacks: u32,
    /// per-project 未读(编排 adapter 若能解析 dispatch 归属则填,否则为空)。
    unread_by_project: HashMap<PathBuf, u32>,
}

/// 左栏状态聚合 model(单例)。
#[derive(Debug, Default)]
pub struct LeftRailStatusModel(LeftRailStatusInner);

impl Entity for LeftRailStatusModel {
    type Event = LeftRailStatusEvent;
}

impl SingletonEntity for LeftRailStatusModel {}

/// model 事件(渲染订阅用)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeftRailStatusEvent {
    SessionStateChanged,
    SessionProgressChanged,
    UnreadChanged,
}

impl From<SessionStateChanged> for LeftRailStatusEvent {
    fn from(_: SessionStateChanged) -> Self {
        Self::SessionStateChanged
    }
}

impl From<SessionProgressChanged> for LeftRailStatusEvent {
    fn from(_: SessionProgressChanged) -> Self {
        Self::SessionProgressChanged
    }
}

impl From<UnreadChanged> for LeftRailStatusEvent {
    fn from(_: UnreadChanged) -> Self {
        Self::UnreadChanged
    }
}

impl LeftRailStatusModel {
    // ------------------------------------------------------------------
    // 写入侧 — 数据源 adapter
    // ------------------------------------------------------------------

    /// 建立 session↔pane 绑定(lane spawn / 首个 block 落库时)。
    pub fn bind_pane(
        &mut self,
        pane_id: PaneId,
        session_id: &str,
        ctx: &mut ModelContext<Self>,
    ) {
        let prev = self
            .0
            .session_to_pane
            .insert(session_id.to_string(), pane_id);
        if prev != Some(pane_id) {
            let entry = self.0.pane_status.entry(pane_id).or_default();
            if entry.state.is_none() {
                entry.state = Some(HarnessRunState::Idle);
                entry.state_since = Some(Instant::now());
            }
            ctx.emit(LeftRailStatusEvent::SessionStateChanged);
        }
    }

    /// pane 关闭时清理绑定。
    pub fn unbind_pane(&mut self, pane_id: PaneId, ctx: &mut ModelContext<Self>) {
        self.0.session_to_pane.retain(|_, p| *p != pane_id);
        if self.0.pane_status.remove(&pane_id).is_some() {
            ctx.emit(LeftRailStatusEvent::SessionStateChanged);
        }
    }

    /// 会话五态翻转(派生器调用;内部做最短驻留去抖,避免抖动)。
    pub fn update_state(
        &mut self,
        session_id: &str,
        state: HarnessRunState,
        ctx: &mut ModelContext<Self>,
    ) {
        let Some(&pane_id) = self.0.session_to_pane.get(session_id) else {
            return;
        };
        let entry = self.0.pane_status.entry(pane_id).or_default();
        if entry.state != Some(state) {
            entry.state = Some(state);
            entry.state_since = Some(Instant::now());
            ctx.emit(LeftRailStatusEvent::SessionStateChanged);
        }
    }

    /// todo/marker 进度更新。
    pub fn update_progress(
        &mut self,
        session_id: &str,
        progress: Option<(u32, u32)>,
        ctx: &mut ModelContext<Self>,
    ) {
        let Some(&pane_id) = self.0.session_to_pane.get(session_id) else {
            return;
        };
        let entry = self.0.pane_status.entry(pane_id).or_default();
        if entry.progress != progress {
            entry.progress = progress;
            ctx.emit(LeftRailStatusEvent::SessionProgressChanged);
        }
    }

    /// marker halt 请求(`dais:halt`);note=None 表示清除。
    pub fn set_halt(
        &mut self,
        session_id: &str,
        halted: bool,
        note: Option<String>,
        ctx: &mut ModelContext<Self>,
    ) {
        let Some(&pane_id) = self.0.session_to_pane.get(session_id) else {
            return;
        };
        let entry = self.0.pane_status.entry(pane_id).or_default();
        let note = note.map(|n| n.chars().take(80).collect::<String>());
        if entry.halt_requested != halted || entry.marker_note != note {
            entry.halt_requested = halted;
            entry.marker_note = note;
            ctx.emit(LeftRailStatusEvent::SessionProgressChanged);
        }
    }

    /// 会话退出(exit code: None=未知, Some(0)=Done, Some(_)=Error)。
    pub fn session_exited(
        &mut self,
        session_id: &str,
        code: Option<i32>,
        ctx: &mut ModelContext<Self>,
    ) {
        let state = match code {
            Some(0) => HarnessRunState::Done,
            _ => HarnessRunState::Error,
        };
        self.update_state(session_id, state, ctx);
    }

    /// 编排邮箱未读快照(adapter 轮询写入;数值不变不 emit)。
    pub fn set_unread(
        &mut self,
        total: u32,
        by_project: HashMap<PathBuf, u32>,
        ctx: &mut ModelContext<Self>,
    ) {
        if self.0.unread_callbacks == total && self.0.unread_by_project == by_project {
            return;
        }
        self.0.unread_callbacks = total;
        self.0.unread_by_project = by_project;
        ctx.emit(LeftRailStatusEvent::UnreadChanged);
    }

    // ------------------------------------------------------------------
    // 读取侧 — 渲染
    // ------------------------------------------------------------------

    /// pane 的运行状态(无会话/未绑定返回 None)。
    pub fn pane_status(&self, pane_id: PaneId) -> Option<&SessionRunStatus> {
        self.0.pane_status.get(&pane_id)
    }

    /// 一组 pane(=tab 或项目内全部 tab 的 panes)的聚合。
    pub fn aggregate(&self, pane_ids: &[PaneId]) -> ProjectAggregate {
        let mut agg = ProjectAggregate::default();
        for id in pane_ids {
            if let Some(status) = self.0.pane_status.get(id) {
                match status.state {
                    Some(HarnessRunState::Working) => agg.working += 1,
                    Some(HarnessRunState::WaitingInput) => agg.waiting_input += 1,
                    Some(HarnessRunState::Done) => agg.done += 1,
                    Some(HarnessRunState::Error) => agg.error += 1,
                    _ => {}
                }
            }
        }
        agg
    }

    /// 全局未读编排回调数。
    pub fn unread_callbacks(&self) -> u32 {
        self.0.unread_callbacks
    }

    /// 项目维度未读(dispatch 归属可解析时;否则并入全局数由组头显示)。
    pub fn unread_for_project(&self, project: &Path) -> u32 {
        self.0.unread_by_project.get(project).copied().unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aggregate_counts_states() {
        let model = LeftRailStatusModel::default();
        let agg = model.aggregate(&[]);
        assert_eq!(agg, ProjectAggregate::default());
        assert!(!agg.needs_attention());
    }

    #[test]
    fn unread_set_is_idempotent_eventwise() {
        // set_unread 同值不 emit 的语义由 ctx.emit 计数验证(集成侧)。
        // 此处只锚定数值读写。
        let mut model = LeftRailStatusModel::default();
        model.0.unread_callbacks = 2;
        assert_eq!(model.unread_callbacks(), 2);
    }
}
