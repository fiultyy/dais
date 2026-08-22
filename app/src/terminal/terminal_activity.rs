//! TerminalActivityModel — per-view 终端事件 → 全局可订阅聚合(cockpit-instant)。
//!
//! 背景:TerminalModel 的事件是 per-view 的(async channel,由各 view 自己的
//! pump 消费),外部模型无法订阅;cockpit 的非 agent 终端状态(Busy/Idle/
//! preview 尾行/git branch/cwd)此前只有 2s 对账轮询一个刷新源。
//!
//! 本 singleton 做"推"式聚合:TerminalView 在既有事件处理点(块完成、长运行
//! 翻转、wakeup 输出泵、prompt 更新)调用 [`Self::publish`],全局面板(cockpit
//! 等)订阅 [`TerminalActivityEvent`] 即时刷新。
//!
//! 事件分级:
//! - `StateChanged` — Busy/Idle 状态转移(块完成 / 长运行检测翻转),零延迟,
//!   消费者必须即时处理(任务要求:状态转移不允许进合并窗)。
//! - `OutputChanged` — 输出增长类(wakeup 泵,PTR 高吞吐时每秒可达数百次),
//!   消费者侧自行合并(cockpit 用 150ms 窗)。
//!
//! 开关(生产端热路径保护):`enabled`。wakeup 泵在每批输出上都会跑,关态时
//! 生产端只做一次 atomic load 即返回 — 无消费者时零开销。cockpit 面板
//! 开合时置位(`CockpitModel::set_panel_open`)。
//!
//! 生命周期:生产端是"推"而非"订阅"——无 per-view 订阅可泄漏;消费端订阅随
//! 消费者 view 生命周期(warpui Drop 自动退订,同 cockpit 对
//! CLIAgentSessionsModel 的订阅模式)。

use std::sync::atomic::{AtomicBool, Ordering};

use warpui::{Entity, EntityId, ModelContext, SingletonEntity};

/// 聚合事件。`StateChanged` 零延迟;`OutputChanged` 高频,消费侧合并。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalActivityEvent {
    /// 状态转移:块完成(Busy→Idle)或长运行检测翻转(Idle→Busy)。
    StateChanged { terminal_view_id: EntityId },
    /// 输出增长(wakeup 泵)/prompt·cwd 更新。高频,消费侧合并窗处理。
    OutputChanged { terminal_view_id: EntityId },
}

pub struct TerminalActivityModel {
    enabled: AtomicBool,
    /// 诊断计数(事件量级观察;非契约)。
    state_event_count: u64,
    output_event_count: u64,
}

impl Entity for TerminalActivityModel {
    type Event = TerminalActivityEvent;
}

impl SingletonEntity for TerminalActivityModel {}

impl TerminalActivityModel {
    pub fn new(_ctx: &mut ModelContext<Self>) -> Self {
        Self {
            enabled: AtomicBool::new(false),
            state_event_count: 0,
            output_event_count: 0,
        }
    }

    /// 消费者开合时置位(cockpit 面板 open/close)。关态生产端短路。
    pub fn set_enabled(&mut self, enabled: bool, _ctx: &mut ModelContext<Self>) {
        self.enabled.store(enabled, Ordering::Release);
    }

    /// 热路径安全的一次 load 检查(生产端在 publish 前调用)。
    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Acquire)
    }

    /// 生产端入口。调用方(TerminalView)已持 view ctx;先查
    /// [`Self::is_enabled`],关态短路——本方法只在开态才应被走到。
    pub fn publish(
        &mut self,
        terminal_view_id: EntityId,
        event: TerminalActivityEvent,
        ctx: &mut ModelContext<Self>,
    ) {
        match event {
            TerminalActivityEvent::StateChanged { .. } => self.state_event_count += 1,
            TerminalActivityEvent::OutputChanged { .. } => self.output_event_count += 1,
        }
        let _ = terminal_view_id;
        ctx.emit(event);
    }

    pub fn state_event_count(&self) -> u64 {
        self.state_event_count
    }

    pub fn output_event_count(&self) -> u64 {
        self.output_event_count
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use warpui::App;

    /// 开关短路:关态 publish 前置检查由调用方做;开态事件必达订阅者。
    #[test]
    fn events_reach_subscribers_when_enabled() {
        App::test((), |mut app| async move {
            app.add_singleton_model(TerminalActivityModel::new);
            let hub = TerminalActivityModel::handle(&mut app);
            let (tx, rx) = async_channel::unbounded();
            let tx2 = tx.clone();

            hub.update(&mut app, |_, ctx| {
                ctx.subscribe_to_model(&hub, move |_, event, _| {
                    let _ = tx2.try_send(*event);
                });
            });

            // 关态:调用方约定不 publish(短路),这里直接验证 flag 语义。
            assert!(!warpui::ReadModel::read_model(
                &app,
                &hub,
                |m: &TerminalActivityModel, _| m.is_enabled()
            ));
            hub.update(&mut app, |m, ctx| m.set_enabled(true, ctx));
            assert!(warpui::ReadModel::read_model(
                &app,
                &hub,
                |m: &TerminalActivityModel, _| m.is_enabled()
            ));

            let id = EntityId::from_usize(7);
            hub.update(&mut app, |m, ctx| {
                m.publish(id, TerminalActivityEvent::StateChanged { terminal_view_id: id }, ctx);
                m.publish(id, TerminalActivityEvent::OutputChanged { terminal_view_id: id }, ctx);
            });

            let first = rx.try_recv().expect("StateChanged should arrive");
            let second = rx.try_recv().expect("OutputChanged should arrive");
            assert_eq!(
                first,
                TerminalActivityEvent::StateChanged { terminal_view_id: id }
            );
            assert_eq!(
                second,
                TerminalActivityEvent::OutputChanged { terminal_view_id: id }
            );
            assert!(rx.try_recv().is_err(), "no further events");
        });
    }
}
