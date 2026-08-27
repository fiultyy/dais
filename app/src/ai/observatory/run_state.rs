/**
 * 五态派生器 — 从观测台 DB 块流推导左栏 HarnessRunState。
 *
 * 数据流:
 * 1. 每次 `ObservatoryModel::refresh_auto`（5s 轮询）末尾调用 `derive_run_states(ctx)`。
 * 2. 派生器打开 harness_blocks.db 只读连接，扫描每个活跃拦截 session 的
 *    最新 blocks（spawn/user_prompt/response/exit），按规则翻转五态。
 * 3. 同时扫描 blocks content 中的 marker 行（`dais:progress`/`dais:halt`/
 *    `dais:note`），写入 LeftRailStatusModel 的进度/halt/note 字段。
 *
 * pane↔session 映射:
 * - GUI 交互拦截（CC/Codex 等 tab 内 CLI）: `GUI_INTERCEPT` 注册表
 *   `terminal_view_id → (session, …)`，再通过 PaneGroup::find_pane_id_for_terminal_view
 *   反查 PaneId。映射在进程内存，**重启丢失**（已知限制）。
 * - 外部捕获（proxy 端口入口）: 无 pane 归属，不绑定——仅 GUI 交互式
 *   tab 才有 pane，外部捕获 session 在 Proxy tab 只展示列表，不映射到左栏。
 * - agent run CLI 会话（AgentDriver 持有）: 生命周期短暂，无稳定 pane 绑定，
 *   同样不映射。
 *
 * 已知限制:
 * - 映射依赖 GUI_INTERCEPT 进程内注册表 + PaneGroup 遍历；app 重启后
 *   已有 session 无 pane 绑定，直到该 tab 重新启动 CLI agent。
 * - 5s 轮询延迟：状态翻转最慢 5s 感知。
 * - Idle 去抖 1.5s 最短驻留 + 10min 超时在 DB 层面判断;
 *   Working→WaitingInput 依赖 Response block 落库时机（proxy 转发后即落库）。
 * - 外部 harness 的 TodoWrite（结构化 tool_use JSON）从 blocks content
 *   做启发式解析不太可靠，仅走 marker 通道（`dais:progress N/M`）。
 */

use std::collections::HashMap;

use warpui::{AppContext, SingletonEntity};

use crate::pane_group::pane::PaneId;
use crate::workspace::view::left_rail_status::{HarnessRunState, LeftRailStatusModel};
use crate::workspace::WorkspaceRegistry;

// ── 常量 ─────────────────────────────────────────────────────────────

/// 无活动超过此时长视为 Idle（10 分钟）。
const IDLE_TIMEOUT_SECS: i64 = 600;

/// 每次轮询扫描的 blocks 上限（per session），防止大 session 拖慢 5s 周期。
const SCAN_BLOCK_LIMIT: u32 = 50;

// ── 派生入口 ─────────────────────────────────────────────────────────

/// 从观测台数据推导五态，写入 LeftRailStatusModel。
///
/// 由 `ObservatoryModel::refresh_auto` 在 5s 轮询中调用。
/// 不返回值；所有副作用通过 LeftRailStatusModel 事件传播。
pub fn derive_run_states(ctx: &mut AppContext) {
    let conn = match open_blocks_db() {
        Some(c) => c,
        None => return,
    };

    // 1. 收集 GUI_INTERCEPT 的 terminal_view_id → session_id 映射。
    //    ⚠ 接口缺口: harness_intercept 模块需新增 `gui_intercept_session_map()` 函数，
    //    返回 HashMap<String, String>（terminal_view_id → session_id）。
    //    当前调用会在 harness_intercept 补丁合入前编译失败。
    let intercept_map = crate::ai::harness_intercept::gui_intercept_session_map();
    if intercept_map.is_empty() {
        return;
    }

    // 2. terminal_view_id → PaneId，通过 WorkspaceRegistry 遍历所有 PaneGroup。
    let session_to_pane = resolve_session_pane_map(&intercept_map, ctx);
    if session_to_pane.is_empty() {
        return;
    }

    let session_ids: Vec<&str> = session_to_pane.keys().map(|s| s.as_str()).collect();
    let now_ts = chrono::Utc::now().timestamp();

    // 3. 批量扫描每个 session 的最新 blocks，推导状态 + marker。
    //    收集所有变更后一次性写入 LeftRailStatusModel（减少 emit 次数）。
    let mut pending_binds: Vec<(PaneId, String)> = Vec::new();
    let mut pending_states: Vec<(String, HarnessRunState)> = Vec::new();
    let mut pending_progress: Vec<(String, Option<(u32, u32)>)> = Vec::new();
    let mut pending_halts: Vec<(String, bool, Option<String>)> = Vec::new();
    let mut pending_exits: Vec<(String, Option<i32>)> = Vec::new();

    for session_id in &session_ids {
        let blocks = match scan_session_blocks(&conn, session_id) {
            Some(b) => b,
            None => continue,
        };

        let derived = derive_session_state(&blocks, now_ts);
        let markers = parse_markers(&blocks);

        // 确保绑定存在
        let pane_id = session_to_pane[*session_id];
        pending_binds.push((pane_id, session_id.to_string()));

        // 五态
        match derived {
            SessionDerivation::State(s) => {
                pending_states.push((session_id.to_string(), s));
            }
            SessionDerivation::Exited(code) => {
                pending_exits.push((session_id.to_string(), code));
            }
            SessionDerivation::Idle => {
                pending_states.push((session_id.to_string(), HarnessRunState::Idle));
            }
        }

        // markers
        if let Some(prog) = markers.progress {
            pending_progress.push((session_id.to_string(), Some(prog)));
        }
        if markers.halt {
            pending_halts.push((
                session_id.to_string(),
                true,
                markers.halt_note.clone(),
            ));
        } else if let Some(note) = markers.note_only {
            // dais:note 不设 halt，只更新 note
            pending_halts.push((session_id.to_string(), false, Some(note)));
        }
    }

    // 4. 批量写入 LeftRailStatusModel
    LeftRailStatusModel::handle(ctx).update(ctx, |model, ctx| {
        for (pane_id, sid) in pending_binds {
            model.bind_pane(pane_id, &sid, ctx);
        }
        for (sid, state) in pending_states {
            model.update_state(&sid, state, ctx);
        }
        for (sid, code) in pending_exits {
            model.session_exited(&sid, code, ctx);
        }
        for (sid, prog) in pending_progress {
            model.update_progress(&sid, prog, ctx);
        }
        for (sid, halted, note) in pending_halts {
            model.set_halt(&sid, halted, note, ctx);
        }
    });
}

// ── 内部类型 ─────────────────────────────────────────────────────────

/// 会话级别状态推导结果。
enum SessionDerivation {
    /// 活跃五态（Working / WaitingInput）。
    State(HarnessRunState),
    /// 检测到 Exit block。
    Exited(Option<i32>),
    /// 无活动超时 / 初绑。
    Idle,
}

/// Marker 解析结果。
struct Markers {
    /// `dais:progress N/M`
    progress: Option<(u32, u32)>,
    /// `dais:halt`
    halt: bool,
    /// `dais:note <text>` （无 halt 时单独的 note）。
    note_only: Option<String>,
    /// `dais:halt` 附带的 note。
    halt_note: Option<String>,
}

/// 扫描得到的 block 摘要（足够推导状态，不持全量 content）。
struct BlockSummary {
    block_type: String,
    sequence: u32,
    timestamp: i64,
    /// content 前缀（用于 marker 扫描；截断到 4 KiB）。
    content_head: String,
    /// metadata JSON（exit_code 等）。
    metadata: serde_json::Value,
}

// ── DB 辅助 ──────────────────────────────────────────────────────────

/// 打开 harness_blocks.db 只读连接（与 ObservatoryModel 同路径/标志）。
fn open_blocks_db() -> Option<rusqlite::Connection> {
    let dir = warp_core::paths::state_dir();
    if dir.as_os_str().is_empty() {
        return None;
    }
    let path = dir.join("harness_blocks.db");
    if !path.exists() {
        return None;
    }
    rusqlite::Connection::open_with_flags(
        &path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|e| {
        log::warn!("run_state: cannot open block store {}: {e}", path.display());
        e
    })
    .ok()
}

/// 归一化毫秒/秒时间戳到秒。
fn ts_to_secs(ts: i64) -> i64 {
    if ts > 1_000_000_000_000 {
        ts / 1000
    } else {
        ts
    }
}

/// 扫描单个 session 的最新 blocks（sequence 降序，上限 SCAN_BLOCK_LIMIT）。
fn scan_session_blocks(
    conn: &rusqlite::Connection,
    session_id: &str,
) -> Option<Vec<BlockSummary>> {
    let mut stmt = conn
        .prepare(
            "SELECT block_type, sequence, timestamp, \
             substr(content, 1, 4096), COALESCE(metadata, '{}') \
             FROM harness_blocks \
             WHERE session_id = ?1 \
             ORDER BY sequence DESC LIMIT ?2",
        )
        .ok()?;

    let rows = stmt
        .query_map(rusqlite::params![session_id, SCAN_BLOCK_LIMIT], |row| {
            let content_bytes: Vec<u8> = row.get(3)?;
            let content_str = String::from_utf8_lossy(&content_bytes).to_string();
            let meta_str: String = row.get(4)?;
            let metadata: serde_json::Value =
                serde_json::from_str(&meta_str).unwrap_or(serde_json::Value::Null);
            Ok(BlockSummary {
                block_type: row.get(0)?,
                sequence: row.get(1)?,
                timestamp: ts_to_secs(row.get(2)?),
                content_head: content_str,
                metadata,
            })
        })
        .ok()?;

    Some(rows.filter_map(|r| r.ok()).collect())
}

// ── 状态推导 ─────────────────────────────────────────────────────────

/// 从 block 摘要推导会话五态。
///
/// 规则（按优先级）:
/// 1. 存在 Exit block → Done(0) / Error(非0)。
/// 2. 最新 activity 距今 >10min → Idle。
/// 3. 最新 block 为 UserPrompt/PromptSegment（请求侧）→ Working。
/// 4. 最新 block 为 Response/ToolCall/ToolResult（响应侧）→ WaitingInput。
/// 5. 仅 Spawn 无后续 → Working（刚启动，首个 Request 还没到）。
/// 6. 仅 Spawn + Response/ToolResult → WaitingInput。
/// 7. 其他 → Idle。
fn derive_session_state(blocks: &[BlockSummary], now_ts: i64) -> SessionDerivation {
    if blocks.is_empty() {
        return SessionDerivation::Idle;
    }

    // blocks 降序，blocks[0] 是 sequence 最大的（最新）
    let mut has_exit = false;
    let mut exit_code: Option<i32> = None;
    let mut has_user_prompt = false;
    let mut has_response = false;
    let mut latest_ts: i64 = 0;

    for b in blocks {
        if b.timestamp > latest_ts {
            latest_ts = b.timestamp;
        }

        match b.block_type.as_str() {
            "exit" => {
                has_exit = true;
                // metadata: {"exit_code": 0, "reason": "..."}
                if exit_code.is_none() {
                    exit_code = b
                        .metadata
                        .get("exit_code")
                        .and_then(|v| v.as_i64())
                        .map(|c| c as i32);
                }
            }
            "user_prompt" | "prompt_segment" => {
                has_user_prompt = true;
            }
            "response" | "response_chunk" | "tool_call" | "tool_result" => {
                has_response = true;
            }
            _ => {}
        }
    }

    // 1. Exit → 终态
    if has_exit {
        return SessionDerivation::Exited(exit_code);
    }

    // 2. 超时 → Idle
    if now_ts - latest_ts > IDLE_TIMEOUT_SECS {
        return SessionDerivation::Idle;
    }

    // 3-7. 按最新 block 类型判断
    //    blocks 降序，blocks[0] 是最新
    let latest_type = blocks[0].block_type.as_str();

    match latest_type {
        "user_prompt" | "prompt_segment" => {
            SessionDerivation::State(HarnessRunState::Working)
        }
        "response" | "response_chunk" | "tool_call" | "tool_result" | "system_prompt" => {
            SessionDerivation::State(HarnessRunState::WaitingInput)
        }
        "spawn" => {
            // 刚启动，看有没有后续 response
            if has_response {
                SessionDerivation::State(HarnessRunState::WaitingInput)
            } else {
                // Spawn 后还在等首个 Request → 短暂 Working
                SessionDerivation::State(HarnessRunState::Working)
            }
        }
        "pty_raw" => {
            // PtyRaw 通常伴随其他 block；如果有 response 则 WaitingInput
            if has_response {
                SessionDerivation::State(HarnessRunState::WaitingInput)
            } else if has_user_prompt {
                SessionDerivation::State(HarnessRunState::Working)
            } else {
                SessionDerivation::Idle
            }
        }
        _ => SessionDerivation::Idle,
    }
}

// ── Marker 解析 ──────────────────────────────────────────────────────

/// 扫描 blocks content 中的 dais: marker 行。
///
/// 支持格式:
/// - `dais:progress <done>/<total>` → 进度
/// - `dais:halt` → halt 请求
/// - `dais:halt <note>` → halt + note
/// - `dais:note <text>` → 仅 note
///
/// 从最新到最旧扫描，取第一个匹配（最新 marker 优先）。
fn parse_markers(blocks: &[BlockSummary]) -> Markers {
    let mut result = Markers {
        progress: None,
        halt: false,
        note_only: None,
        halt_note: None,
    };

    for b in blocks {
        for line in b.content_head.lines() {
            let trimmed = line.trim();
            if let Some(rest) = trimmed.strip_prefix("dais:progress ") {
                if result.progress.is_none() {
                    result.progress = parse_progress(rest);
                }
            } else if let Some(rest) = trimmed.strip_prefix("dais:halt") {
                if !result.halt {
                    result.halt = true;
                    // `dais:halt` 或 `dais:halt some reason`
                    let note = rest.trim();
                    if !note.is_empty() {
                        result.halt_note = Some(note.to_string());
                    }
                }
            } else if let Some(rest) = trimmed.strip_prefix("dais:note ") {
                // note 只在非 halt 时单独记录（halt 自带 note）
                if !result.halt && result.note_only.is_none() {
                    result.note_only = Some(rest.trim().to_string());
                }
            }
        }
    }

    result
}

/// 解析 `<done>/<total>` 进度。
fn parse_progress(s: &str) -> Option<(u32, u32)> {
    let mut parts = s.trim().splitn(2, '/');
    let done: u32 = parts.next()?.parse().ok()?;
    let total: u32 = parts.next()?.parse().ok()?;
    if total == 0 || done > total {
        return None;
    }
    Some((done, total))
}

// ── pane↔session 映射 ────────────────────────────────────────────────

/// 从 GUI_INTERCEPT 注册表 + WorkspaceRegistry + PaneGroup 反查，
/// 建立 session_id → PaneId 映射。
///
/// 已知限制: GUI_INTERCEPT 是进程内 HashMap，重启后清空；外部捕获 session
/// 无 terminal_view_id，不在此映射。
fn resolve_session_pane_map(
    intercept_map: &HashMap<String, String>,
    ctx: &AppContext,
) -> HashMap<String, PaneId> {
    let mut result = HashMap::new();

    let registry = WorkspaceRegistry::handle(ctx);
    let workspaces = registry.read(ctx, |reg, ctx| reg.all_workspaces(ctx));

    for (view_id_str, session_id) in intercept_map {
        if result.contains_key(session_id) {
            continue;
        }
        let entity_id = match view_id_str.parse::<usize>() {
            Ok(v) => warpui::EntityId::from_usize(v),
            Err(_) => continue,
        };

        // 遍历所有 window 的 Workspace，在其所有 tab 的 PaneGroup 中查找
        for (_, workspace) in &workspaces {
            let found = workspace.read(ctx, |ws, ctx| {
                for tab_handle in ws.tab_views() {
                    if let Some(pane_id) =
                        tab_handle.read(ctx, |pg, ctx| {
                            pg.find_pane_id_for_terminal_view(entity_id, ctx)
                        })
                    {
                        return Some(pane_id);
                    }
                }
                None
            });
            if let Some(pane_id) = found {
                result.insert(session_id.clone(), pane_id);
                break;
            }
        }
    }

    result
}
