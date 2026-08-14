//! Observatory 面板数据模型 — 拦截会话块流 + 编排状态 + 消息发送闭环。
//!
//! 所有状态（快照、选中 session、tab、composer 草稿、busy/error）集中于本 model，
//! 视图纯渲染 + 派发意图（MVU 单一数据源）。singleton 注册，cfg(not(wasm)) 门控。

use std::path::PathBuf;
use std::process::Stdio;
use warpui::{AppContext, Entity, ModelContext, SingletonEntity};

use crate::terminal::intercept_sessions::InterceptSessionsModel;

// ---------------------------------------------------------------------------
// GUI 行类型
// ---------------------------------------------------------------------------

/// 会话列表行。
#[derive(Clone, Debug)]
pub struct SessionRowGui {
    pub session_id: String,
    pub block_count: u64,
    pub first_ts: i64,
    pub last_ts: i64,
}

/// Block 时间线行（sequence 升序，上限 500）。
#[derive(Clone, Debug)]
pub struct BlockRowGui {
    pub id: String,
    pub sequence: u32,
    pub block_type: String,
    pub content_len: usize,
    pub preview: String,
    pub timestamp: i64,
}

/// Run 列表行（最新 50）。
#[derive(Clone, Debug)]
pub struct RunRowGui {
    pub id: String,
    pub objective: String,
    pub created_at: String,
}

/// Task 列表行（最新 200）。
#[derive(Clone, Debug)]
pub struct TaskRowGui {
    pub id: String,
    pub run_id: String,
    pub title: String,
    pub status: String,
}

/// 最近消息行（最新 30）。
#[derive(Clone, Debug)]
pub struct MessageRowGui {
    pub seq: i64,
    pub from_handle: String,
    pub to_handle: String,
    pub subject: String,
    pub created_at: String,
}

/// 消息详情（点击消息行后加载，body 截断至 64 KiB）。
#[derive(Clone, Debug)]
pub struct MessageDetailGui {
    pub seq: i64,
    pub id: String,
    pub run_id: String,
    pub from_handle: String,
    pub to_handle: String,
    pub subject: String,
    pub body: String,
    pub message_type: String,
    pub priority: String,
    pub created_at: String,
}

/// Block 详情（点击时间线行后加载，content 截断至 64 KiB）。
#[derive(Clone, Debug)]
pub struct BlockDetailGui {
    pub id: String,
    pub session_id: String,
    pub parent_id: Option<String>,
    pub harness_type: String,
    pub block_type: String,
    pub sequence: u32,
    pub content_len: usize,
    pub content: String,
    pub metadata: String,
    pub timestamp: i64,
}

/// Pending gate 行。
#[derive(Clone, Debug)]
pub struct GateRowGui {
    pub id: String,
    pub task_id: String,
    pub question: String,
    pub options: Vec<String>,
    pub status: String,
    pub created_at: String,
}

// ---------------------------------------------------------------------------
// Tab 枚举
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ObservatoryTab {
    Sessions,
    Orchestration,
    Proxy,
}

/// Raw 代理流量行（选中 session 的 raw_cache 条目，时间升序，上限 200）。
#[derive(Clone, Debug)]
pub struct RawRowGui {
    pub id: String,
    pub direction: String,
    pub content_len: usize,
    pub preview: String,
    pub timestamp: i64,
}

/// Raw 载荷详情（content 截断至 64 KiB）。
#[derive(Clone, Debug)]
pub struct RawDetailGui {
    pub id: String,
    pub session_id: String,
    pub direction: String,
    pub content_len: usize,
    pub content: String,
    pub timestamp: i64,
}

/// 选中 task 的 dispatch 行（dispatch_contexts JOIN worker_dispatches）。
#[derive(Clone, Debug)]
pub struct DispatchRowGui {
    pub dispatch_id: String,
    pub status: String,
    pub state: String,
    pub start_options: String,
    pub created_at: String,
}

/// 活跃拦截会话行（GUI 交互 CC tab 的 proxy 运行态）。
#[derive(Clone, Debug)]
pub struct ActiveInterceptRowGui {
    pub session_id: String,
    pub proxy_port: Option<u16>,
    pub hook_url: Option<String>,
}
/// refresh() 后的完整数据快照。
#[derive(Clone, Default, Debug)]
pub struct ObservatorySnapshot {
    pub sessions: Vec<SessionRowGui>,
    /// 选中 session 的 blocks（sequence 升序，上限 500）。
    pub blocks: Vec<BlockRowGui>,
    /// 选中 session 的 raw 代理流量（时间升序，上限 200）。
    pub raw_entries: Vec<RawRowGui>,
    /// 最新 50 runs。
    pub runs: Vec<RunRowGui>,
    /// 最新 200 tasks。
    pub tasks: Vec<TaskRowGui>,
    /// Pending gates（最新 50）。
    pub gates: Vec<GateRowGui>,
    /// 选中 task 的 dispatches（最新 20）。
    pub dispatches: Vec<DispatchRowGui>,
    /// 活跃 GUI 交互拦截会话（Proxy tab 展示）。
    pub active_intercepts: Vec<ActiveInterceptRowGui>,
    pub recent_messages: Vec<MessageRowGui>,
}

// ---------------------------------------------------------------------------
// 事件
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ObservatoryEvent {
    SnapshotUpdated,
    DraftChanged,
    BusyChanged,
}

// ---------------------------------------------------------------------------
// Composer 草稿字段
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DraftField {
    To,
    Subject,
    Body,
}

// ---------------------------------------------------------------------------
// Model
// ---------------------------------------------------------------------------

pub struct ObservatoryModel {
    snapshot: ObservatorySnapshot,
    selected_session: Option<String>,
    active_tab: ObservatoryTab,
    busy: bool,
    last_error: Option<String>,
    draft_to: String,
    draft_subject: String,
    draft_body: String,
    /// Sessions/blocks 搜索过滤（子串匹配，空 = 不过滤）。
    search_filter: String,
    selected_block: Option<String>,
    block_detail: Option<BlockDetailGui>,
    /// 观测台面板是否打开（gate 5s 轮询：关闭时跳过 DB 查询）。
    panel_open: bool,
    /// 选中的 raw 流量条目 id。
    selected_raw: Option<String>,
    raw_detail: Option<RawDetailGui>,
    selected_task: Option<String>,
    /// 最近一次 dispatch 的 id（反馈展示用）。
    last_dispatch: Option<String>,
    /// 选中的 pending gate id。
    selected_gate: Option<String>,
    /// gate 自定义 resolution 草稿。
    gate_draft: String,
    /// 选中的消息 sequence（messages 表 PK）。
    selected_message: Option<i64>,
    message_detail: Option<MessageDetailGui>,
}

impl ObservatoryModel {

    pub fn new(_ctx: &mut ModelContext<Self>) -> Self {
        Self {
            snapshot: ObservatorySnapshot::default(),
            selected_session: None,
            panel_open: false,
            active_tab: ObservatoryTab::Sessions,
            busy: false,
            last_error: None,
            draft_to: String::new(),
            draft_subject: String::new(),
            draft_body: String::new(),
            search_filter: String::new(),
            selected_block: None,
            block_detail: None,
            selected_raw: None,
            raw_detail: None,
            selected_task: None,
            last_dispatch: None,
            selected_gate: None,
            gate_draft: String::new(),
            selected_message: None,
            message_detail: None,
        }
    }

    // ── 只读访问 ───────────────────────────────────────────────────────

    pub fn snapshot(&self) -> &ObservatorySnapshot {
        &self.snapshot
    }

    pub fn selected_session(&self) -> Option<&str> {
        self.selected_session.as_deref()
    }

    pub fn active_tab(&self) -> ObservatoryTab {
        self.active_tab
    }

    pub fn busy(&self) -> bool {
        self.busy
    }

    pub fn last_error(&self) -> Option<&str> {
        self.last_error.as_deref()
    }

    pub fn draft_to(&self) -> &str {
        &self.draft_to
    }

    pub fn draft_subject(&self) -> &str {
        &self.draft_subject
    }

    pub fn draft_body(&self) -> &str {
        &self.draft_body
    }

    pub fn search_filter(&self) -> &str {
        &self.search_filter
    }

    pub fn block_detail(&self) -> Option<&BlockDetailGui> {
        self.block_detail.as_ref()
    }

    pub fn selected_block(&self) -> Option<&str> {
        self.selected_block.as_deref()
    }

    pub fn selected_raw(&self) -> Option<&str> {
        self.selected_raw.as_deref()
    }

    pub fn raw_detail(&self) -> Option<&RawDetailGui> {
        self.raw_detail.as_ref()
    }

    pub fn selected_task(&self) -> Option<&str> {
        self.selected_task.as_deref()
    }

    pub fn last_dispatch(&self) -> Option<&str> {
        self.last_dispatch.as_deref()
    }

    pub fn selected_gate(&self) -> Option<&str> {
        self.selected_gate.as_deref()
    }

    pub fn gate_draft(&self) -> &str {
        &self.gate_draft
    }

    pub fn selected_message(&self) -> Option<i64> {
        self.selected_message
    }

    pub fn message_detail(&self) -> Option<&MessageDetailGui> {
        self.message_detail.as_ref()
    }

    /// 面板开合状态（toggle_observatory 写入；timer 读取 gate 轮询）。
    pub fn panel_open(&self) -> bool {
        self.panel_open
    }

    pub fn set_panel_open(&mut self, open: bool, ctx: &mut ModelContext<Self>) {
        self.panel_open = open;
        ctx.emit(ObservatoryEvent::SnapshotUpdated);
    }

    /// 读 InterceptSessionsModel 的拦截模式。
    pub fn mode(&self, ctx: &AppContext) -> harness_integration::InterceptMode {
        InterceptSessionsModel::as_ref(ctx).mode()
    }

    /// 读 InterceptSessionsModel 的 block 总数。
    pub fn block_count_total(&self, ctx: &AppContext) -> u64 {
        InterceptSessionsModel::as_ref(ctx).block_count()
    }

    // ── 写操作（全部经 model.update，MVU） ──────────────────────────────

    pub fn set_active_tab(&mut self, tab: ObservatoryTab, ctx: &mut ModelContext<Self>) {
        if self.active_tab == tab {
            return;
        }
        self.active_tab = tab;
        ctx.emit(ObservatoryEvent::SnapshotUpdated);
    }

    /// 选中/取消选中 session。None → 清空 blocks。
    pub fn select_session(
        &mut self,
        id: Option<String>,
        ctx: &mut ModelContext<Self>,
    ) {
        self.selected_session = id.clone();
        // 若选中了 session，立即加载其 blocks/raw；否则清空
        self.snapshot.blocks = match &self.selected_session {
            Some(sid) => self.load_blocks(sid),
            None => Vec::new(),
        };
        self.snapshot.raw_entries = match &id {
            Some(sid) => self.load_raw(sid),
            None => Vec::new(),
        };
        ctx.emit(ObservatoryEvent::SnapshotUpdated);
    }

    pub fn set_draft(
        &mut self,
        field: DraftField,
        value: String,
        ctx: &mut ModelContext<Self>,
    ) {
        match field {
            DraftField::To => self.draft_to = value,
            DraftField::Subject => self.draft_subject = value,
            DraftField::Body => self.draft_body = value,
        }
        ctx.emit(ObservatoryEvent::DraftChanged);
    }

    /// 设置搜索过滤并立即重载 sessions/blocks。
    pub fn set_search_filter(&mut self, filter: String, ctx: &mut ModelContext<Self>) {
        let filter = filter.trim().to_string();
        if self.search_filter == filter {
            return;
        }
        self.search_filter = filter;
        self.snapshot.sessions = self.load_sessions();
        self.snapshot.blocks = match &self.selected_session {
            Some(sid) => self.load_blocks(sid),
            None => Vec::new(),
        };
        self.snapshot.raw_entries = match &self.selected_session {
            Some(sid) => self.load_raw(sid),
            None => Vec::new(),
        };
        ctx.emit(ObservatoryEvent::SnapshotUpdated);
    }

    /// 选中/取消选中 block（加载详情）。None → 清空详情。
    pub fn select_block(&mut self, id: Option<String>, ctx: &mut ModelContext<Self>) {
        self.last_error = None;
        self.selected_block = id.clone();
        self.block_detail = match &id {
            Some(bid) => {
                match self.load_block_detail(bid) {
                    Some(d) => Some(d),
                    None => {
                        self.last_error = Some(format!("block {bid} not found"));
                        None
                    }
                }
            }
            None => None,
        };
        ctx.emit(ObservatoryEvent::SnapshotUpdated);
    }

    /// 选中/取消选中 raw 流量条目（加载详情）。None → 清空详情。
    pub fn select_raw(&mut self, id: Option<String>, ctx: &mut ModelContext<Self>) {
        self.last_error = None;
        self.selected_raw = id.clone();
        self.raw_detail = match &id {
            Some(rid) => {
                match self.load_raw_detail(rid) {
                    Some(d) => Some(d),
                    None => {
                        self.last_error = Some(format!("raw entry {rid} not found"));
                        None
                    }
                }
            }
            None => None,
        };
        ctx.emit(ObservatoryEvent::SnapshotUpdated);
    }

    /// 选中/取消选中消息（加载详情）。None → 清空详情。
    pub fn select_message(&mut self, seq: Option<i64>, ctx: &mut ModelContext<Self>) {
        self.last_error = None;
        self.selected_message = seq;
        self.message_detail = match &seq {
            Some(s) => match self.load_message_detail(*s) {
                Some(d) => Some(d),
                None => {
                    self.last_error = Some(format!("message {s} not found"));
                    None
                }
            },
            None => None,
        };
        ctx.emit(ObservatoryEvent::SnapshotUpdated);
    }

    /// 选中/取消选中编排 task（同时加载该 task 的 dispatches）。
    pub fn select_task(&mut self, id: Option<String>, ctx: &mut ModelContext<Self>) {
        self.selected_task = id.clone();
        self.snapshot.dispatches = match &id {
            Some(tid) => self.load_dispatches(tid),
            None => Vec::new(),
        };
        ctx.emit(ObservatoryEvent::SnapshotUpdated);
    }

    /// 选中/取消选中 pending gate。
    pub fn select_gate(&mut self, id: Option<String>, ctx: &mut ModelContext<Self>) {
        self.selected_gate = id;
        ctx.emit(ObservatoryEvent::SnapshotUpdated);
    }

    pub fn set_gate_draft(&mut self, value: String, ctx: &mut ModelContext<Self>) {
        self.gate_draft = value;
        ctx.emit(ObservatoryEvent::DraftChanged);
    }

    /// 为 task 创建 dispatch（store 同步调用）。
    pub fn dispatch_task(&mut self, task_id: &str, ctx: &mut ModelContext<Self>) {
        #[cfg(feature = "orchestration")]
        {
            use ::ai::agent::orchestration::connection::store;

            let run_id = self
                .snapshot
                .tasks
                .iter()
                .find(|t| t.id == task_id)
                .map(|t| t.run_id.clone());
            let Some(run_id) = run_id else {
                self.last_error = Some(format!("task {task_id} not in snapshot"));
                ctx.emit(ObservatoryEvent::SnapshotUpdated);
                return;
            };
            match store().create_dispatch(&run_id, task_id, "{}") {
                Ok(id) => {
                    self.last_dispatch = Some(id);
                    self.last_error = None;
                }
                Err(e) => {
                    self.last_error = Some(format!("create_dispatch failed: {e:?}"));
                }
            }
            self.load_orchestration_data();
        }
        #[cfg(not(feature = "orchestration"))]
        {
            self.last_error = Some("orchestration feature disabled".to_string());
        }
        ctx.emit(ObservatoryEvent::SnapshotUpdated);
    }

    /// 解决 pending gate（store 同步调用），成功后刷新编排数据。
    pub fn resolve_gate(
        &mut self,
        gate_id: &str,
        resolution: &str,
        ctx: &mut ModelContext<Self>,
    ) {
        #[cfg(feature = "orchestration")]
        {
            use ::ai::agent::orchestration::connection::store;

            match store().resolve_gate(gate_id, resolution) {
                Ok(()) => self.last_error = None,
                Err(e) => self.last_error = Some(format!("resolve_gate failed: {e:?}")),
            }
            self.load_orchestration_data();
        }
        #[cfg(not(feature = "orchestration"))]
        {
            let _ = (gate_id, resolution);
            self.last_error = Some("orchestration feature disabled".to_string());
        }
        ctx.emit(ObservatoryEvent::SnapshotUpdated);
    }

    /// 全量刷新快照（手动：同时清除 last_error）。
    pub fn refresh(&mut self, ctx: &mut ModelContext<Self>) {
        self.last_error = None;
        self.refresh_auto(ctx);
    }

    /// 定时自动刷新：与 [`Self::refresh`] 相同的数据面，
    /// 但保留 last_error（否则 5s 轮询会把错误信息瞬间冲掉）。
    pub fn refresh_auto(&mut self, ctx: &mut ModelContext<Self>) {
        // 1. Sessions + blocks + raw
        self.snapshot.sessions = self.load_sessions();
        // 若当前选中 session 仍存在则刷新 blocks/raw，否则清空选中
        self.snapshot.blocks = match &self.selected_session {
            Some(sid) if self.snapshot.sessions.iter().any(|s| &s.session_id == sid) => {
                self.load_blocks(sid)
            }
            _ => {
                self.selected_session = None;
                Vec::new()
            }
        };
        self.snapshot.raw_entries = match &self.selected_session {
            Some(sid) => self.load_raw(sid),
            None => Vec::new(),
        };

        // 2. Orchestration（cfg 门控）
        self.load_orchestration_data();
        // 选中 task 的 dispatches 跟随刷新
        self.snapshot.dispatches = match &self.selected_task {
            Some(tid) if self.snapshot.tasks.iter().any(|t| &t.id == tid) => {
                self.load_dispatches(tid)
            }
            _ => Vec::new(),
        };

        // 3. 活跃 GUI 交互拦截会话（Proxy tab）
        self.snapshot.active_intercepts =
            crate::ai::harness_intercept::active_gui_intercepts()
                .into_iter()
                .map(|a| ActiveInterceptRowGui {
                    session_id: a.session_id,
                    proxy_port: a.proxy_port,
                    hook_url: a.hook_url,
                })
                .collect();

        ctx.emit(ObservatoryEvent::SnapshotUpdated);
    }

    /// 发送消息: draft → spawn 异步 future（spawn_blocking 跑子进程）→ 回主线程刷新。
    pub fn send_message(&mut self, ctx: &mut ModelContext<Self>) {
        if self.busy {
            return;
        }
        let to = self.draft_to.clone();
        let subject = self.draft_subject.clone();
        let body = self.draft_body.clone();

        // 取最新 run_id 或 "gui"
        let run_id = self
            .snapshot
            .runs
            .first()
            .map(|r| r.id.clone())
            .unwrap_or_else(|| "gui".to_string());

        self.busy = true;
        self.last_error = None;
        ctx.emit(ObservatoryEvent::BusyChanged);

        // spawn_blocking 在线程池执行子进程，完成后回调主线程
        let future = async move {
            tokio::task::spawn_blocking(move || {
                Self::run_send_message(&run_id, &to, &subject, &body)
            })
            .await
            .unwrap_or_else(|e| Err(format!("send-message task panicked: {e}")))
        };

        ctx.spawn(future, |model, result, ctx| {
            model.busy = false;
            match result {
                Ok(()) => {
                    model.draft_body.clear();
                    // 子进程退出后刷新数据
                    model.refresh(ctx);
                }
                Err(e) => {
                    model.last_error = Some(e);
                }
            }
            ctx.emit(ObservatoryEvent::BusyChanged);
        });
    }

    // ── 内部方法 ─────────────────────────────────────────────────────────

    fn blocks_db_path() -> Option<PathBuf> {
        let dir = warp_core::paths::state_dir();
        if dir.as_os_str().is_empty() {
            return None;
        }
        Some(dir.join("harness_blocks.db"))
    }

    /// 打开 harness_blocks.db 只读 rusqlite 连接。
    fn open_blocks_db() -> Option<rusqlite::Connection> {
        let path = Self::blocks_db_path()?;
        if !path.exists() {
            return None;
        }
        rusqlite::Connection::open_with_flags(
            &path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(|e| {
            log::warn!("observatory: cannot open block store {}: {e}", path.display());
            e
        })
        .ok()
    }

    /// 加载 session 列表（按 last_ts 降序，上限 100；search_filter 子串过滤）。
    fn load_sessions(&self) -> Vec<SessionRowGui> {
        let conn = match Self::open_blocks_db() {
            Some(c) => c,
            None => return Vec::new(),
        };
        let pattern = like_pattern(&self.search_filter);
        let mut stmt = match conn.prepare(
            "SELECT session_id, COUNT(*), MIN(timestamp), MAX(timestamp) \
             FROM harness_blocks \
             WHERE (?1 = '' OR session_id LIKE ?1 ESCAPE '\\') \
             GROUP BY session_id ORDER BY MAX(timestamp) DESC LIMIT 100",
        ) {
            Ok(s) => s,
            Err(e) => {
                log::warn!("observatory: load_sessions prepare error: {e}");
                return Vec::new();
            }
        };
        let rows = match stmt.query_map(rusqlite::params![pattern], |row| {
            Ok(SessionRowGui {
                session_id: row.get(0)?,
                block_count: row.get(1)?,
                first_ts: row.get(2)?,
                last_ts: row.get(3)?,
            })
        }) {
            Ok(r) => r,
            Err(e) => {
                log::warn!("observatory: load_sessions query error: {e}");
                return Vec::new();
            }
        };
        rows.filter_map(|r| r.ok()).collect()
    }

    /// 加载选中 session 的 blocks（sequence 升序，上限 500；
    /// search_filter 对 block_type 与 content 做子串过滤）。
    fn load_blocks(&self, session_id: &str) -> Vec<BlockRowGui> {
        let conn = match Self::open_blocks_db() {
            Some(c) => c,
            None => return Vec::new(),
        };
        let pattern = like_pattern(&self.search_filter);
        let mut stmt = match conn.prepare(
            "SELECT id, sequence, block_type, LENGTH(content), substr(content, 1, 80), timestamp \
             FROM harness_blocks \
             WHERE session_id = ?1 \
               AND (?2 = '' OR block_type LIKE ?2 ESCAPE '\\' \
                    OR CAST(content AS TEXT) LIKE ?2 ESCAPE '\\') \
             ORDER BY sequence ASC LIMIT 500",
        ) {
            Ok(s) => s,
            Err(e) => {
                log::warn!("observatory: load_blocks prepare error: {e}");
                return Vec::new();
            }
        };
        let rows = match stmt.query_map(rusqlite::params![session_id, pattern], |row| {
            let raw_preview: Vec<u8> = row.get(4)?;
            let preview = String::from_utf8_lossy(&raw_preview)
                .chars()
                .take(80)
                .collect::<String>();
            Ok(BlockRowGui {
                id: row.get(0)?,
                sequence: row.get(1)?,
                block_type: row.get(2)?,
                content_len: row.get(3)?,
                preview,
                timestamp: row.get(5)?,
            })
        }) {
            Ok(r) => r,
            Err(e) => {
                log::warn!("observatory: load_blocks query error: {e}");
                return Vec::new();
            }
        };
        rows.filter_map(|r| r.ok()).collect()
    }

    /// 加载单个 block 的完整详情（content 截断至 64 KiB）。
    fn load_block_detail(&self, block_id: &str) -> Option<BlockDetailGui> {
        let conn = Self::open_blocks_db()?;
        let mut stmt = conn
            .prepare(
                "SELECT id, session_id, parent_id, harness_type, block_type, sequence, \
                        LENGTH(content), substr(content, 1, 65536), metadata, timestamp \
                 FROM harness_blocks WHERE id = ?1 LIMIT 1",
            )
            .map_err(|e| log::warn!("observatory: load_block_detail prepare error: {e}"))
            .ok()?;
        let (id, session_id, parent_id, harness_type, block_type, sequence, content_len, content, metadata, timestamp) = stmt
            .query_row(rusqlite::params![block_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, i64>(5)? as u32,
                    row.get::<_, Option<i64>>(6)?.unwrap_or(0) as usize,
                    row.get::<_, Vec<u8>>(7)?,
                    row.get::<_, Option<String>>(8)?,
                    row.get::<_, i64>(9)?,
                ))
            })
            .map_err(|e| log::warn!("observatory: load_block_detail query error: {e}"))
            .ok()?;
        Some(BlockDetailGui {
            id,
            session_id,
            parent_id,
            harness_type,
            block_type,
            sequence,
            content_len,
            content: String::from_utf8_lossy(&content).to_string(),
            metadata: metadata.unwrap_or_else(|| "null".to_string()),
            timestamp,
        })
    }

    /// 打开 harness_raw_cache.db 只读连接（不存在返回 None）。
    fn open_raw_db() -> Option<rusqlite::Connection> {
        let dir = warp_core::paths::state_dir();
        if dir.as_os_str().is_empty() {
            return None;
        }
        let path = dir.join("harness_raw_cache.db");
        if !path.exists() {
            return None;
        }
        rusqlite::Connection::open_with_flags(
            &path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(|e| {
            log::warn!("observatory: cannot open raw cache {}: {e}", path.display());
            e
        })
        .ok()
    }

    /// 加载选中 session 的 raw 代理流量（时间升序，上限 200；
    /// search_filter 对 direction 与 content 做子串过滤）。
    fn load_raw(&self, session_id: &str) -> Vec<RawRowGui> {
        let conn = match Self::open_raw_db() {
            Some(c) => c,
            None => return Vec::new(),
        };
        let pattern = like_pattern(&self.search_filter);
        let mut stmt = match conn.prepare(
            "SELECT id, direction, LENGTH(content), substr(content, 1, 80), timestamp \
             FROM raw_cache \
             WHERE session_id = ?1 \
               AND (?2 = '' OR direction LIKE ?2 ESCAPE '\\' \
                    OR CAST(content AS TEXT) LIKE ?2 ESCAPE '\\') \
             ORDER BY timestamp ASC LIMIT 200",
        ) {
            Ok(s) => s,
            Err(e) => {
                log::warn!("observatory: load_raw prepare error: {e}");
                return Vec::new();
            }
        };
        let rows = match stmt.query_map(rusqlite::params![session_id, pattern], |row| {
            let raw_preview: Vec<u8> = row.get(3)?;
            let preview = String::from_utf8_lossy(&raw_preview)
                .chars()
                .take(80)
                .collect::<String>();
            Ok(RawRowGui {
                id: row.get(0)?,
                direction: row.get(1)?,
                content_len: row.get::<_, Option<i64>>(2)?.unwrap_or(0) as usize,
                preview,
                timestamp: row.get(4)?,
            })
        }) {
            Ok(r) => r,
            Err(e) => {
                log::warn!("observatory: load_raw query error: {e}");
                return Vec::new();
            }
        };
        rows.filter_map(|r| r.ok()).collect()
    }

    /// 加载单个 raw 载荷详情（content 截断至 64 KiB）。
    fn load_raw_detail(&self, raw_id: &str) -> Option<RawDetailGui> {
        let conn = Self::open_raw_db()?;
        let (id, session_id, direction, content_len, content, timestamp) = conn
            .query_row(
                "SELECT id, session_id, direction, LENGTH(content), substr(content, 1, 65536), timestamp \
                 FROM raw_cache WHERE id = ?1 LIMIT 1",
                rusqlite::params![raw_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, Option<i64>>(3)?.unwrap_or(0) as usize,
                        row.get::<_, Vec<u8>>(4)?,
                        row.get::<_, i64>(5)?,
                    ))
                },
            )
            .map_err(|e| log::warn!("observatory: load_raw_detail query error: {e}"))
            .ok()?;
        Some(RawDetailGui {
            id,
            session_id,
            direction,
            content_len,
            content: String::from_utf8_lossy(&content).to_string(),
            timestamp,
        })
    }

    /// 加载选中 task 的 dispatches（dispatch_contexts LEFT JOIN worker_dispatches，
    /// 最新 20；rusqlite 直查 warp.sqlite，messages 同模式）。
    #[cfg(all(feature = "orchestration", feature = "local_fs"))]
    fn load_dispatches(&self, task_id: &str) -> Vec<DispatchRowGui> {
        let db_path = warp_core::paths::state_dir().join("warp.sqlite");
        if !db_path.exists() {
            return Vec::new();
        }
        let conn = match rusqlite::Connection::open_with_flags(
            &db_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        ) {
            Ok(c) => c,
            Err(e) => {
                log::warn!("observatory: cannot open warp.sqlite for dispatches: {e}");
                return Vec::new();
            }
        };
        let mut stmt = match conn.prepare(
            "SELECT dc.id, dc.status, \
                    COALESCE(wd.state, ''), COALESCE(wd.start_options, ''), \
                    dc.created_at \
             FROM dispatch_contexts dc \
             LEFT JOIN worker_dispatches wd ON wd.dispatch_id = dc.id \
             WHERE dc.task_id = ?1 \
             ORDER BY dc.rowid DESC LIMIT 20",
        ) {
            Ok(s) => s,
            Err(e) => {
                log::warn!("observatory: load_dispatches prepare error: {e}");
                return Vec::new();
            }
        };
        let rows = match stmt.query_map(rusqlite::params![task_id], |row| {
            let created: String = row.get(4)?;
            Ok(DispatchRowGui {
                dispatch_id: row.get(0)?,
                status: row.get(1)?,
                state: row.get(2)?,
                start_options: row.get(3)?,
                created_at: format_datetime_sqlite(&created),
            })
        }) {
            Ok(r) => r,
            Err(e) => {
                log::warn!("observatory: load_dispatches query error: {e}");
                return Vec::new();
            }
        };
        rows.filter_map(|r| r.ok()).collect()
    }

    /// local_fs 未开（或 orchestration 关）时 dispatches 留空。
    #[cfg(not(all(feature = "orchestration", feature = "local_fs")))]
    fn load_dispatches(&self, _task_id: &str) -> Vec<DispatchRowGui> {
        Vec::new()
    }

    /// 加载编排数据（runs / tasks / messages）。
    #[cfg(feature = "orchestration")]
    fn load_orchestration_data(&mut self) {
        use ::ai::agent::orchestration::connection::store;

        let s = store();

        // runs（最新 50）
        self.snapshot.runs = s
            .list_runs()
            .map(|runs| {
                runs.into_iter()
                    .take(50)
                    .map(|r| RunRowGui {
                        id: r.id,
                        objective: r.objective,
                        created_at: r.created_at.format("%Y-%m-%d %H:%M:%S").to_string(),
                    })
                    .collect()
            })
            .unwrap_or_default();

        // tasks（最新 200，无 status 过滤）
        self.snapshot.tasks = s
            .list_tasks(None, None)
            .map(|tasks| {
                tasks
                    .into_iter()
                    .take(200)
                    .map(|t| TaskRowGui {
                        id: t.id,
                        run_id: t.run_id,
                        title: t
                            .task_title
                            .or(t.display_name)
                            .unwrap_or_default(),
                        status: t.status,
                    })
                    .collect()
            })
            .unwrap_or_default();

        // pending gates（最新 50）
        self.snapshot.gates = s
            .list_gates(None, Some("pending"))
            .map(|gates| {
                gates
                    .into_iter()
                    .take(50)
                    .map(|g| GateRowGui {
                        id: g.id,
                        task_id: g.task_id,
                        question: g.question,
                        options: serde_json::from_str::<Vec<String>>(&g.options)
                            .unwrap_or_default(),
                        status: g.status,
                        created_at: g.created_at.format("%Y-%m-%d %H:%M:%S").to_string(),
                    })
                    .collect()
            })
            .unwrap_or_default();

        // 选中的 gate 已不再是 pending → 清除选中
        if let Some(gid) = self.selected_gate.clone() {
            if !self.snapshot.gates.iter().any(|g| g.id == gid) {
                self.selected_gate = None;
            }
        }

        // messages — rusqlite 直查 warp.sqlite messages 表
        self.load_recent_messages();
    }

    #[cfg(not(feature = "orchestration"))]
    fn load_orchestration_data(&mut self) {
        // orchestration feature 关闭时四项全空
        self.snapshot.runs = Vec::new();
        self.snapshot.tasks = Vec::new();
        self.snapshot.gates = Vec::new();
        self.snapshot.recent_messages = Vec::new();
    }

    /// rusqlite 直查 messages 表（最新 30 条）。
    ///
    /// 需要 `local_fs` feature 才能访问 sqlite 文件路径。
    #[cfg(all(feature = "orchestration", feature = "local_fs"))]
    fn load_recent_messages(&mut self) {
        let conn = match Self::open_warp_sqlite() {
            Some(c) => c,
            None => return,
        };
        let mut stmt = match conn.prepare(
            "SELECT sequence, from_handle, to_handle, subject, created_at \
             FROM messages ORDER BY sequence DESC LIMIT 30",
        ) {
            Ok(s) => s,
            Err(e) => {
                log::warn!("observatory: messages prepare error: {e}");
                return;
            }
        };
        let rows = match stmt.query_map([], |row| {
            Ok(MessageRowGui {
                seq: row.get(0)?,
                from_handle: row.get(1)?,
                to_handle: row.get(2)?,
                subject: row.get(3)?,
                created_at: row.get(4)?,
            })
        }) {
            Ok(r) => r,
            Err(e) => {
                log::warn!("observatory: messages query error: {e}");
                return;
            }
        };
        let mut msgs: Vec<MessageRowGui> = rows.filter_map(|r| r.ok()).collect();
        // DESC 取回 → 翻转为时间正序
        msgs.reverse();
        self.snapshot.recent_messages = msgs;
    }

    /// orchestration 开但 local_fs 未开时，messages 留空。
    #[cfg(all(feature = "orchestration", not(feature = "local_fs")))]
    fn load_recent_messages(&mut self) {
        self.snapshot.recent_messages = Vec::new();
    }

    /// 打开 warp.sqlite 只读连接（不存在/不可开返回 None）。
    /// 与 orchestration store 使用同一个库文件。
    fn open_warp_sqlite() -> Option<rusqlite::Connection> {
        let dir = warp_core::paths::state_dir();
        if dir.as_os_str().is_empty() {
            return None;
        }
        let path = dir.join("warp.sqlite");
        if !path.exists() {
            return None;
        }
        rusqlite::Connection::open_with_flags(
            &path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(|e| log::warn!("observatory: cannot open warp.sqlite: {e}"))
        .ok()
    }

    /// 加载单条消息完整详情（body 截断至 64 KiB）。无库/未命中返回 None。
    fn load_message_detail(&self, seq: i64) -> Option<MessageDetailGui> {
        let conn = Self::open_warp_sqlite()?;
        let mut stmt = conn
            .prepare(
                "SELECT sequence, id, run_id, from_handle, to_handle, subject, \
                        substr(body, 1, 65536), type, priority, created_at \
                 FROM messages WHERE sequence = ?1 LIMIT 1",
            )
            .map_err(|e| log::warn!("observatory: load_message_detail prepare error: {e}"))
            .ok()?;
        stmt.query_row(rusqlite::params![seq], |row| {
            Ok(MessageDetailGui {
                seq: row.get(0)?,
                id: row.get(1)?,
                run_id: row.get(2)?,
                from_handle: row.get(3)?,
                to_handle: row.get(4)?,
                subject: row.get(5)?,
                body: row.get(6)?,
                message_type: row.get(7)?,
                priority: row.get(8)?,
                created_at: row.get(9)?,
            })
        })
        .map_err(|e| log::warn!("observatory: load_message_detail query error: {e}"))
        .ok()
    }

    /// 在子线程中执行 `current_exe orchestration send-message`。
    fn run_send_message(run_id: &str, to: &str, subject: &str, body: &str) -> Result<(), String> {
        let exe = match std::env::current_exe() {
            Ok(e) => e,
            Err(e) => return Err(format!("cannot find current exe: {e}")),
        };

        let mut cmd = std::process::Command::new(exe);
        cmd.arg("orchestration")
            .arg("send-message")
            .arg(run_id)
            .arg("--from")
            .arg("observatory")
            .arg(to)
            .arg("--message-type")
            .arg("status")
            .arg("--subject")
            .arg(subject)
            .arg("--body")
            .arg(body)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let output = cmd.output().map_err(|e| format!("spawn failed: {e}"))?;
        if output.status.success() {
            Ok(())
        } else {
            Err(String::from_utf8_lossy(&output.stderr).to_string())
        }
    }
}

impl Entity for ObservatoryModel {
    type Event = ObservatoryEvent;
}

impl SingletonEntity for ObservatoryModel {}

/// 构造 SQL LIKE 子串匹配 pattern：空过滤 → 空串（查询侧以 `? = ''` 短路），
/// 否则转义 `%`/`_`/`\` 并两侧包 `%`。
fn like_pattern(filter: &str) -> String {
    if filter.is_empty() {
        return String::new();
    }
    let escaped: String = filter
        .chars()
        .flat_map(|c| match c {
            '%' | '_' | '\\' => vec!['\\', c],
            _ => vec![c],
        })
        .collect();
    format!("%{escaped}%")
}

/// SQLite NaiveDateTime 文本（"YYYY-MM-DD HH:MM:SS"）→ "MM-DD HH:MM"。
/// 非预期形状原样返回。
pub(crate) fn format_datetime_sqlite(s: &str) -> String {
    let parts: Vec<&str> = s.split(' ').collect();
    if parts.len() == 2 && parts[0].len() >= 10 && parts[1].len() >= 5 {
        format!("{} {}", &parts[0][5..], &parts[1][..5])
    } else {
        s.to_string()
    }
}

// ---------------------------------------------------------------------------
// 测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use warpui::ReadModel;

    use super::*;

    /// 在 temp dir 建 harness_blocks.db 并写入样例数据，
    /// 然后用 rusqlite 直接验证 SQL 查询正确性（model 内部查询同源）。
    #[test]
    fn test_session_and_block_queries_correctness() {
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("harness_blocks.db");

        // 用 BlockStore 写入两个 session 的样例 blocks
        let store = harness_integration::BlockStore::open(db_path.to_string_lossy().to_string()).unwrap();

        let ts_base: i64 = 1700000000000;
        let all_blocks = vec![
            // session-a: 3 个 prompt blocks
            harness_integration::HarnessBlock::new(
                "session-a", "claude",
                harness_integration::BlockType::UserPrompt, 0,
                b"prompt content 0".to_vec(), ts_base,
            ),
            harness_integration::HarnessBlock::new(
                "session-a", "claude",
                harness_integration::BlockType::UserPrompt, 1,
                b"prompt content 1".to_vec(), ts_base + 1000,
            ),
            harness_integration::HarnessBlock::new(
                "session-a", "claude",
                harness_integration::BlockType::UserPrompt, 2,
                b"prompt content 2".to_vec(), ts_base + 2000,
            ),
            // session-b: 1 个 response block，时间戳更大
            harness_integration::HarnessBlock::new(
                "session-b", "claude",
                harness_integration::BlockType::Response, 0,
                b"response data".to_vec(), ts_base + 5000,
            ),
        ];

        for b in &all_blocks {
            store.insert_block(b).unwrap();
        }

        // 用 rusqlite 直查验证 model 的 SQL 查询逻辑
        let conn = rusqlite::Connection::open(db_path).unwrap();

        // 1. 测试 session list 查询
        let mut stmt = conn
            .prepare(
                "SELECT session_id, COUNT(*), MIN(timestamp), MAX(timestamp) \
                 FROM harness_blocks GROUP BY session_id ORDER BY MAX(timestamp) DESC LIMIT 100",
            )
            .unwrap();

        let sessions: Vec<SessionRowGui> = stmt
            .query_map([], |row| {
                Ok(SessionRowGui {
                    session_id: row.get(0)?,
                    block_count: row.get(1)?,
                    first_ts: row.get(2)?,
                    last_ts: row.get(3)?,
                })
            })
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();

        assert_eq!(sessions.len(), 2);
        // session-b 的 MAX(timestamp) 更大（ts_base+5000 > ts_base+2000），排第一
        assert_eq!(sessions[0].session_id, "session-b");
        assert_eq!(sessions[0].block_count, 1);
        assert_eq!(sessions[1].session_id, "session-a");
        assert_eq!(sessions[1].block_count, 3);

        // 2. 测试 blocks 查询（session-a）
        let mut stmt = conn
            .prepare(
                "SELECT id, sequence, block_type, LENGTH(content), substr(content, 1, 80), timestamp \
                 FROM harness_blocks WHERE session_id = ?1 ORDER BY sequence ASC LIMIT 500",
            )
            .unwrap();

        let blocks: Vec<BlockRowGui> = stmt
            .query_map(rusqlite::params!["session-a"], |row| {
                let raw: Vec<u8> = row.get(4)?;
                let preview = String::from_utf8_lossy(&raw).chars().take(80).collect();
                Ok(BlockRowGui {
                    id: row.get(0)?,
                    sequence: row.get(1)?,
                    block_type: row.get(2)?,
                    content_len: row.get(3)?,
                    preview,
                    timestamp: row.get(5)?,
                })
            })
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();

        assert_eq!(blocks.len(), 3);
        assert_eq!(blocks[0].sequence, 0);
        assert_eq!(blocks[1].sequence, 1);
        assert_eq!(blocks[2].sequence, 2);
        assert_eq!(blocks[0].block_type, "user_prompt");
        assert!(blocks[0].preview.contains("prompt content 0"));
    }

    /// 测试模型状态转移: tab 切换、draft 设置、session 选中/取消。
    #[test]
    fn test_model_state_transitions() {
        warpui::App::test((), |mut app| async move {
            // 需要先注册 InterceptSessionsModel 单例（ObservatoryModel.mode() 读取它）
            app.add_singleton_model(
                crate::terminal::intercept_sessions::InterceptSessionsModel::new,
            );
            let model = app.add_singleton_model(ObservatoryModel::new);

            // 初始状态
            assert!(app.read_model(&model, |m, _| m.snapshot().sessions.is_empty()));
            assert!(app.read_model(&model, |m, _| m.snapshot().blocks.is_empty()));
            assert!(app.read_model(&model, |m, _| m.selected_session().is_none()));
            assert_eq!(app.read_model(&model, |m, _| m.active_tab()), ObservatoryTab::Sessions);
            assert!(!app.read_model(&model, |m, _| m.busy()));
            assert!(app.read_model(&model, |m, _| m.last_error().is_none()));
            assert!(app.read_model(&model, |m, _| m.draft_to().is_empty()));
            assert!(app.read_model(&model, |m, _| m.draft_subject().is_empty()));
            assert!(app.read_model(&model, |m, _| m.draft_body().is_empty()));

            // set_active_tab
            model.update(&mut app, |m, ctx| {
                m.set_active_tab(ObservatoryTab::Orchestration, ctx);
            });
            assert_eq!(app.read_model(&model, |m, _| m.active_tab()), ObservatoryTab::Orchestration);

            // 切回 Sessions（重复设同值应无变化）
            model.update(&mut app, |m, ctx| {
                m.set_active_tab(ObservatoryTab::Orchestration, ctx);
            });
            assert_eq!(app.read_model(&model, |m, _| m.active_tab()), ObservatoryTab::Orchestration);

            // set_draft
            model.update(&mut app, |m, ctx| {
                m.set_draft(DraftField::To, "agent-1".to_string(), ctx);
                m.set_draft(DraftField::Subject, "hello".to_string(), ctx);
                m.set_draft(DraftField::Body, "world".to_string(), ctx);
            });
            assert_eq!(app.read_model(&model, |m, _| m.draft_to().to_string()), "agent-1");
            assert_eq!(app.read_model(&model, |m, _| m.draft_subject().to_string()), "hello");
            assert_eq!(app.read_model(&model, |m, _| m.draft_body().to_string()), "world");

            // select_block 不存在的 block → 选中态保留 + last_error 置位
            model.update(&mut app, |m, ctx| {
                m.select_block(Some("missing-block".to_string()), ctx);
            });
            assert_eq!(app.read_model(&model, |m, _| m.selected_block().map(str::to_string)), Some("missing-block".to_string()));
            assert!(app.read_model(&model, |m, _| m.block_detail().is_none()));
            assert!(app.read_model(&model, |m, _| m.last_error().is_some()));

            // select_block None → 清空
            model.update(&mut app, |m, ctx| {
                m.select_block(None, ctx);
            });
            assert!(app.read_model(&model, |m, _| m.block_detail().is_none()));
            assert!(app.read_model(&model, |m, _| m.last_error().is_none()));

            // select_task 状态转移
            model.update(&mut app, |m, ctx| {
                m.select_task(Some("task-1".to_string()), ctx);
            });
            assert_eq!(app.read_model(&model, |m, _| m.selected_task().map(str::to_string)), Some("task-1".to_string()));
            model.update(&mut app, |m, ctx| {
                m.select_task(None, ctx);
            });
            assert!(app.read_model(&model, |m, _| m.selected_task().is_none()));

            // set_search_filter 状态转移（DB 无数据仍应正常）
            model.update(&mut app, |m, ctx| {
                m.set_search_filter("claude".to_string(), ctx);
            });
            assert_eq!(app.read_model(&model, |m, _| m.search_filter().to_string()), "claude");
            // select_gate / set_gate_draft 状态转移
            model.update(&mut app, |m, ctx| {
                m.select_gate(Some("gate-1".to_string()), ctx);
                m.set_gate_draft("proceed".to_string(), ctx);
            });
            assert_eq!(app.read_model(&model, |m, _| m.selected_gate().map(str::to_string)), Some("gate-1".to_string()));
            assert_eq!(app.read_model(&model, |m, _| m.gate_draft().to_string()), "proceed");
            model.update(&mut app, |m, ctx| {
                m.select_gate(None, ctx);
            });
            assert!(app.read_model(&model, |m, _| m.selected_gate().is_none()));

            // select session（DB 里无数据，但状态转移应正确）
            model.update(&mut app, |m, ctx| {
                m.select_session(Some("nonexistent".to_string()), ctx);
            });
            assert_eq!(app.read_model(&model, |m, _| m.selected_session().map(str::to_string)), Some("nonexistent".to_string()));
            // blocks 应为空（session 不存在于 DB）
            assert!(app.read_model(&model, |m, _| m.snapshot().blocks.is_empty()));

            // select None → 清空选中
            model.update(&mut app, |m, ctx| {
                m.select_session(None, ctx);
            });
            assert!(app.read_model(&model, |m, _| m.selected_session().is_none()));
            assert!(app.read_model(&model, |m, _| m.snapshot().blocks.is_empty()));
            // raw 选中状态转移（DB 无数据：missing → last_error；None → 清空）
            model.update(&mut app, |m, ctx| {
                m.select_raw(Some("missing-raw".to_string()), ctx);
            });
            assert_eq!(app.read_model(&model, |m, _| m.selected_raw().map(str::to_string)), Some("missing-raw".to_string()));
            assert!(app.read_model(&model, |m, _| m.raw_detail().is_none()));
            assert!(app.read_model(&model, |m, _| m.last_error().is_some()));
            model.update(&mut app, |m, ctx| {
                m.select_raw(None, ctx);
            });
            assert!(app.read_model(&model, |m, _| m.raw_detail().is_none()));
            assert!(app.read_model(&model, |m, _| m.last_error().is_none()));
            // message 选中状态转移（seq 极大必不存在：missing → last_error；None → 清空）
            model.update(&mut app, |m, ctx| {
                m.select_message(Some(i64::MAX), ctx);
            });
            assert_eq!(app.read_model(&model, |m, _| m.selected_message()), Some(i64::MAX));
            assert!(app.read_model(&model, |m, _| m.message_detail().is_none()));
            assert!(app.read_model(&model, |m, _| m.last_error().is_some()));
            model.update(&mut app, |m, ctx| {
                m.select_message(None, ctx);
            });
            assert!(app.read_model(&model, |m, _| m.message_detail().is_none()));
            assert!(app.read_model(&model, |m, _| m.last_error().is_none()));
        });
    }

    /// 测试 draft field 枚举
    #[test]
    fn test_draft_field_enum() {
        let fields = [DraftField::To, DraftField::Subject, DraftField::Body];
        assert_eq!(fields.len(), 3);
    }

    /// 测试 ObservatoryTab 枚举（含新 Proxy variant）
    #[test]
    fn test_tab_enum() {
        assert_ne!(ObservatoryTab::Sessions, ObservatoryTab::Orchestration);
        assert_ne!(ObservatoryTab::Orchestration, ObservatoryTab::Proxy);
        assert_ne!(ObservatoryTab::Sessions, ObservatoryTab::Proxy);
    }

    /// like_pattern: 空短路、子串包裹、通配符转义。
    #[test]
    fn test_like_pattern() {
        assert_eq!(like_pattern(""), "");
        assert_eq!(like_pattern("abc"), "%abc%");
        assert_eq!(like_pattern("a%b_c\\"), "%a\\%b\\_c\\\\%");
    }

    /// 搜索过滤 SQL 与 block 详情 SQL 的正确性（temp db 直查，与 model 内部查询同源）。
    #[test]
    fn test_search_filter_and_block_detail_queries() {
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("harness_blocks.db");
        let store = harness_integration::BlockStore::open(db_path.to_string_lossy().to_string()).unwrap();

        let ts: i64 = 1_700_000_000_000;
        let mut a0 = harness_integration::HarnessBlock::new(
            "alpha", "claude", harness_integration::BlockType::UserPrompt, 0,
            b"fix the login bug".to_vec(), ts,
        );
        a0.metadata = serde_json::json!({"meta": true});
        let a1 = harness_integration::HarnessBlock::new(
            "alpha", "claude", harness_integration::BlockType::Response, 1,
            b"all good".to_vec(), ts + 1000,
        );
        let b0 = harness_integration::HarnessBlock::new(
            "beta", "claude", harness_integration::BlockType::UserPrompt, 0,
            b"other prompt".to_vec(), ts + 2000,
        );
        for blk in [&a0, &a1, &b0] {
            store.insert_block(blk).unwrap();
        }

        let conn = rusqlite::Connection::open(&db_path).unwrap();

        // 1. session 过滤（model load_sessions 同源 SQL）
        let pattern = like_pattern("alph");
        let mut stmt = conn.prepare(
            "SELECT session_id, COUNT(*), MIN(timestamp), MAX(timestamp) \
             FROM harness_blocks \
             WHERE (?1 = '' OR session_id LIKE ?1 ESCAPE '\\') \
             GROUP BY session_id ORDER BY MAX(timestamp) DESC LIMIT 100",
        ).unwrap();
        let sessions: Vec<String> = stmt
            .query_map(rusqlite::params![pattern], |row| row.get::<_, String>(0))
            .unwrap().filter_map(|r| r.ok()).collect();
        assert_eq!(sessions, vec!["alpha"]);

        // 空过滤 → 全量
        let mut stmt = conn.prepare(
            "SELECT session_id, COUNT(*), MIN(timestamp), MAX(timestamp) \
             FROM harness_blocks \
             WHERE (?1 = '' OR session_id LIKE ?1 ESCAPE '\\') \
             GROUP BY session_id ORDER BY MAX(timestamp) DESC LIMIT 100",
        ).unwrap();
        let all: Vec<String> = stmt
            .query_map(rusqlite::params![like_pattern("")], |row| row.get::<_, String>(0))
            .unwrap().filter_map(|r| r.ok()).collect();
        assert_eq!(all.len(), 2);

        // 2. block 内容过滤（model load_blocks 同源 SQL）："login" 只命中 alpha seq0
        let pattern = like_pattern("login");
        let mut stmt = conn.prepare(
            "SELECT id, sequence, block_type, LENGTH(content), substr(content, 1, 80), timestamp \
             FROM harness_blocks \
             WHERE session_id = ?1 \
               AND (?2 = '' OR block_type LIKE ?2 ESCAPE '\\' \
                    OR CAST(content AS TEXT) LIKE ?2 ESCAPE '\\') \
             ORDER BY sequence ASC LIMIT 500",
        ).unwrap();
        let hits: Vec<(String, u32)> = stmt
            .query_map(rusqlite::params!["alpha", pattern], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)? as u32))
 })
            .unwrap().filter_map(|r| r.ok()).collect();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].0, a0.id);
        assert_eq!(hits[0].1, 0);

        // 3. block 详情（model load_block_detail 同源 SQL）
        let detail: (String, Option<String>, String, usize, String, Option<String>) = conn
            .query_row(
                "SELECT id, parent_id, block_type, LENGTH(content), substr(content, 1, 65536), metadata \
                 FROM harness_blocks WHERE id = ?1 LIMIT 1",
                rusqlite::params![a0.id],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get::<_, Option<i64>>(3)?.unwrap_or(0) as usize,
                        String::from_utf8_lossy(&row.get::<_, Vec<u8>>(4)?).to_string(),
                        row.get(5)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(detail.0, a0.id);
        assert_eq!(detail.2, "user_prompt");
        assert_eq!(detail.3, b"fix the login bug".len());
        assert_eq!(detail.4, "fix the login bug");
        assert!(detail.5.unwrap().contains("\"meta\":true"));
    }

    /// raw_cache 查询（列表过滤 + 详情）与 dispatch JOIN 查询正确性。
    #[test]
    fn test_raw_and_dispatch_queries() {
        // ── raw_cache：与 model load_raw/load_raw_detail 同源 SQL ──
        let tmp = tempfile::tempdir().unwrap();
        let raw_path = tmp.path().join("harness_raw_cache.db");
        let conn = rusqlite::Connection::open(&raw_path).unwrap();
        conn.execute(
            "CREATE TABLE raw_cache (id TEXT PRIMARY KEY, session_id TEXT NOT NULL, \
             direction TEXT NOT NULL, content BLOB, timestamp INTEGER NOT NULL)",
            [],
        )
        .unwrap();
        let ts: i64 = 1_700_000_000_000;
        for (id, dir, body, t) in [
            ("r1", "request", b"{\"model\":\"glm\"}".as_slice(), ts),
            ("r2", "response", b"hello world", ts + 1),
            ("r3", "request", b"{\"model\":\"other\"}", ts + 2),
        ] {
            conn.execute(
                "INSERT INTO raw_cache (id, session_id, direction, content, timestamp) \
                 VALUES (?1, 'alpha', ?2, ?3, ?4)",
                rusqlite::params![id, dir, body, t],
            )
            .unwrap();
        }

        // 过滤 "glm" → 只命中 r1
        let pattern = like_pattern("glm");
        let mut stmt = conn
            .prepare(
                "SELECT id, direction, LENGTH(content), substr(content, 1, 80), timestamp \
                 FROM raw_cache \
                 WHERE session_id = ?1 \
                   AND (?2 = '' OR direction LIKE ?2 ESCAPE '\\' \
                        OR CAST(content AS TEXT) LIKE ?2 ESCAPE '\\') \
                 ORDER BY timestamp ASC LIMIT 200",
            )
            .unwrap();
        let hits: Vec<String> = stmt
            .query_map(rusqlite::params!["alpha", pattern], |row| row.get::<_, String>(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        assert_eq!(hits, vec!["r1"]);

        // 过滤 "response" → r2（direction 命中）
        let pattern = like_pattern("response");
        let mut stmt = conn
            .prepare(
                "SELECT id FROM raw_cache \
                 WHERE session_id = ?1 \
                   AND (?2 = '' OR direction LIKE ?2 ESCAPE '\\' \
                        OR CAST(content AS TEXT) LIKE ?2 ESCAPE '\\') \
                 ORDER BY timestamp ASC LIMIT 200",
            )
            .unwrap();
        let hits: Vec<String> = stmt
            .query_map(rusqlite::params!["alpha", pattern], |row| row.get::<_, String>(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        assert_eq!(hits, vec!["r2"]);

        // 详情
        let (dir, len, content): (String, usize, String) = conn
            .query_row(
                "SELECT direction, LENGTH(content), substr(content, 1, 65536) \
                 FROM raw_cache WHERE id = 'r2' LIMIT 1",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get::<_, Option<i64>>(1)?.unwrap_or(0) as usize,
                        String::from_utf8_lossy(&row.get::<_, Vec<u8>>(2)?).to_string(),
                    ))
                },
            )
            .unwrap();
        assert_eq!(dir, "response");
        assert_eq!(len, 11);
        assert_eq!(content, "hello world");

        // ── dispatches：与 model load_dispatches 同源 JOIN SQL（内存库） ──
        conn.execute_batch(
            "CREATE TABLE dispatch_contexts (id TEXT PRIMARY KEY, run_id TEXT, task_id TEXT, \
             status TEXT, failure_count INTEGER, created_at TEXT, completed_at TEXT); \
             CREATE TABLE worker_dispatches (dispatch_id TEXT PRIMARY KEY, runtime_epoch TEXT, \
             state TEXT, start_options TEXT, last_error TEXT, created_at TEXT, updated_at TEXT);",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO dispatch_contexts VALUES ('ctx-1', 'run-1', 'task-1', 'dispatched', 0, '2026-08-15 03:54:47', NULL)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO worker_dispatches VALUES ('ctx-1', NULL, 'ready', '{\"cwd\":\"/tmp\"}', NULL, '2026-08-15 03:54:47', '2026-08-15 03:54:48')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO dispatch_contexts VALUES ('ctx-2', 'run-1', 'task-1', 'completed', 0, '2026-08-15 03:00:00', '2026-08-15 03:01:00')",
            [],
        )
        .unwrap();

        let mut stmt = conn
            .prepare(
                "SELECT dc.id, dc.status, COALESCE(wd.state, ''), COALESCE(wd.start_options, ''), dc.created_at \
                 FROM dispatch_contexts dc \
                 LEFT JOIN worker_dispatches wd ON wd.dispatch_id = dc.id \
                 WHERE dc.task_id = ?1 \
                 ORDER BY dc.rowid DESC LIMIT 20",
            )
            .unwrap();
        let rows: Vec<(String, String, String, String, String)> = stmt
            .query_map(rusqlite::params!["task-1"], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?))
            })
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        assert_eq!(rows.len(), 2);
        // rowid DESC → ctx-2 在前
        assert_eq!(rows[0].0, "ctx-2");
        assert_eq!(rows[0].2, ""); // 无 worker → state 空
        assert_eq!(rows[1].0, "ctx-1");
        assert_eq!(rows[1].2, "ready");
        assert!(rows[1].3.contains("cwd"));
        assert_eq!(rows[1].4, "2026-08-15 03:54:47");
        assert_eq!(format_datetime_sqlite(&rows[1].4), "08-15 03:54");

        // ── messages：与 model load_message_detail 同源 SQL ──
        conn.execute_batch(
            "CREATE TABLE messages (sequence INTEGER PRIMARY KEY AUTOINCREMENT, id TEXT UNIQUE, \
             run_id TEXT, delivery_contract TEXT, from_handle TEXT, to_handle TEXT, \
             subject TEXT, body TEXT, type TEXT, priority TEXT, created_at TEXT, \
             delivered_at TEXT);",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO messages (id, run_id, from_handle, to_handle, subject, body, type, priority, created_at) \
             VALUES ('m1', 'run-1', 'orchestrator', 'worker-1', 'task update', 'all good', 'status', 'normal', '2026-08-15 04:00:00')",
            [],
        )
        .unwrap();
        let detail = conn
            .query_row(
                "SELECT sequence, id, run_id, from_handle, to_handle, subject, \
                        substr(body, 1, 65536), type, priority, created_at \
                 FROM messages WHERE sequence = ?1 LIMIT 1",
                rusqlite::params![1i64],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, String>(5)?,
                        row.get::<_, String>(6)?,
                        row.get::<_, String>(7)?,
                        row.get::<_, String>(8)?,
                        row.get::<_, String>(9)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(detail.1, "m1");
        assert_eq!(detail.2, "run-1");
        assert_eq!(detail.6, "all good");
        assert_eq!(detail.7, "status");
        assert_eq!(detail.8, "normal");
        // 未命中 sequence → query_row Err（model 侧映射为 None → last_error）
        let missing: rusqlite::Result<i64> = conn.query_row(
            "SELECT sequence FROM messages WHERE sequence = 999 LIMIT 1",
            [],
            |row| row.get(0),
        );
        assert!(missing.is_err());
    }
}
