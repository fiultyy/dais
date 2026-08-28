//! cockpit 纯壳模型 — 自 2026-08-28 起实现下沉 crates/cockpit_model
//! (布局拆分 v1 步骤4, 见 split-plan §3)。
//!
//! 本文件保留**业务半边**(不能下沉的部分, 全部绑 app 类型):
//! - `refresh`: 采集链 WorkspaceRegistry → TerminalView → CLIAgentSessionsModel
//!   (model.rs:505 原实现), 结果经 `replace_snapshot` 推入纯壳;
//! - `set_panel_open` 的 TerminalActivityModel hub 同步 (cockpit-instant);
//! - `snapshot_card`/`agent_card_fields`/`terminal_views` (TerminalView 直读);
//! - `confirm_injection` (TerminalView PTY 写), 经 `take_pending_injection`
//!   取出纯壳状态机裁决的目标后执行。
//!
//! `empty_project_names` (nav 空项目组) 由 nav 消费前由 app 推入纯壳,
//! cockpit_nav.rs 不再直读 ProjectManagementModel (拔钉 F)。
//!
//! 幂等契约见 crates/cockpit_model (17 测试); 本壳测试锁定 app 半边的
//! 推入路径 (refresh→snapshot→事件)。

pub use cockpit_model::{
    apply_view_params, card_matches_filter, compute_groups, cwd_group_key, preview_tail_from_output,
    recap_chain, resolve_targets, sort_cards, sort_cards_cmp, CockpitCard, CockpitCardGroup,
    CockpitCardStatus, CockpitEvent, CockpitGroupBy, CockpitModel, CockpitPendingInjection,
    CockpitSort, CockpitStatusFilter,
};

use std::collections::HashSet;

use warpui::{AppContext, Entity, EntityId, ModelContext, SingletonEntity, ViewHandle};

use crate::terminal::cli_agent_sessions::{
    CLIAgentSession, CLIAgentSessionContext, CLIAgentSessionStatus, CLIAgentSessionsModel,
};
use crate::terminal::TerminalView;
use crate::workspace::WorkspaceRegistry;

/// preview 尾行从 active block 提取的行数上限(只取尾部,成本有界)。
const PREVIEW_TAIL_ROWS: usize = 4;

/// 全量刷新 (app 半边):遍历所有 workspace 的 terminal pane 进程内直取,
    /// 推入纯壳 `replace_snapshot`。
    ///
    /// 数据通路证明(spec §4.1):本函数是卡片数据唯一入口,链路为
    /// `WorkspaceRegistry::all_workspaces` → `workspace.tabs` →
    /// `PaneGroup::terminal_pane_ids` → `TerminalView::{pane_configuration,
    /// pwd,id}` + `TerminalModel`(FairMutex 短锁快照) +
    /// `CLIAgentSessionsModel::session` — 全部内存读,零 `std::process`/CLI
    /// 子进程、零文件 stat。
pub fn refresh_model(model: &mut CockpitModel, ctx: &mut ModelContext<CockpitModel>) {
    let mut window_count = 0usize;
    let mut all_cards = Vec::new();

    for (_window_id, workspace) in WorkspaceRegistry::as_ref(ctx).all_workspaces(ctx) {
        window_count += 1;
        workspace.read(ctx, |workspace, ctx| {
            for tab in &workspace.tabs {
                let pane_group = tab.pane_group.as_ref(ctx);
                for pane_id in pane_group.terminal_pane_ids() {
                    let Some(terminal_view) = pane_group.terminal_view_from_pane_id(pane_id, ctx)
                    else {
                        continue;
                    };
                    let view = terminal_view.as_ref(ctx);
                    all_cards.push(snapshot_card(view, ctx));
                }
            }
        });
    }

    model.replace_snapshot(all_cards, window_count, ctx);
}

/// 批量注入执行半边:取出纯壳状态机裁决的目标,对 TerminalView 落 PTY/富输入。
/// 自由函数理由同 `refresh_model` (E0116 orphan rule)。
pub fn confirm_injection_model(
    model: &mut CockpitModel,
    ctx: &mut ModelContext<CockpitModel>,
) -> usize {
    let Some(pending) = model.take_pending_injection(ctx) else {
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
    sent
}

/// 收集全部 workspace 的 terminal view(id → handle),refresh 与注入共用。
fn terminal_views(ctx: &AppContext) -> Vec<(EntityId, ViewHandle<TerminalView>)> {
    let mut views = Vec::new();
    for (_window_id, workspace) in WorkspaceRegistry::as_ref(ctx).all_workspaces(ctx) {
        workspace.read(ctx, |workspace, ctx| {
            for tab in &workspace.tabs {
                let pane_group = tab.pane_group.as_ref(ctx);
                for pane_id in pane_group.terminal_pane_ids() {
                    if let Some(view) = pane_group.terminal_view_from_pane_id(pane_id, ctx) {
                        views.push((view.as_ref(ctx).id(), view));
                    }
                }
            }
        });
    }
    views
}

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
        recap_chain(
            context.response.as_ref(),
            context.query.as_ref(),
            context.summary.as_ref(),
            preview_tail,
        ),
        context.tool_name.clone(),
        status,
    )
}
