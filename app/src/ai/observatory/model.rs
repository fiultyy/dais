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

// ---------------------------------------------------------------------------
// Tab 枚举
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ObservatoryTab {
    Sessions,
    Orchestration,
}

// ---------------------------------------------------------------------------
// 快照
// ---------------------------------------------------------------------------

/// refresh() 后的完整数据快照。
#[derive(Clone, Default, Debug)]
pub struct ObservatorySnapshot {
    pub sessions: Vec<SessionRowGui>,
    /// 选中 session 的 blocks（sequence 升序，上限 500）。
    pub blocks: Vec<BlockRowGui>,
    /// 最新 50 runs。
    pub runs: Vec<RunRowGui>,
    /// 最新 200 tasks。
    pub tasks: Vec<TaskRowGui>,
    /// 最新 30 messages。
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
}

impl ObservatoryModel {
    pub fn new(_ctx: &mut ModelContext<Self>) -> Self {
        Self {
            snapshot: ObservatorySnapshot::default(),
            selected_session: None,
            active_tab: ObservatoryTab::Sessions,
            busy: false,
            last_error: None,
            draft_to: String::new(),
            draft_subject: String::new(),
            draft_body: String::new(),
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
        self.selected_session = id;
        // 若选中了 session，立即加载其 blocks；否则清空
        self.snapshot.blocks = match &self.selected_session {
            Some(sid) => self.load_blocks(sid),
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

    /// 全量刷新快照。
    pub fn refresh(&mut self, ctx: &mut ModelContext<Self>) {
        // 1. Sessions + blocks
        self.snapshot.sessions = self.load_sessions();
        // 若当前选中 session 仍存在则刷新 blocks，否则清空选中
        self.snapshot.blocks = match &self.selected_session {
            Some(sid) if self.snapshot.sessions.iter().any(|s| &s.session_id == sid) => {
                self.load_blocks(sid)
            }
            _ => {
                self.selected_session = None;
                Vec::new()
            }
        };

        // 2. Orchestration（cfg 门控）
        self.load_orchestration_data();

        self.last_error = None;
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

    /// 加载 session 列表（按 last_ts 降序，上限 100）。
    fn load_sessions(&self) -> Vec<SessionRowGui> {
        let conn = match Self::open_blocks_db() {
            Some(c) => c,
            None => return Vec::new(),
        };
        let mut stmt = match conn.prepare(
            "SELECT session_id, COUNT(*), MIN(timestamp), MAX(timestamp) \
             FROM harness_blocks GROUP BY session_id ORDER BY MAX(timestamp) DESC LIMIT 100",
        ) {
            Ok(s) => s,
            Err(e) => {
                log::warn!("observatory: load_sessions prepare error: {e}");
                return Vec::new();
            }
        };
        let rows = match stmt.query_map([], |row| {
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

    /// 加载选中 session 的 blocks（sequence 升序，上限 500）。
    fn load_blocks(&self, session_id: &str) -> Vec<BlockRowGui> {
        let conn = match Self::open_blocks_db() {
            Some(c) => c,
            None => return Vec::new(),
        };
        let mut stmt = match conn.prepare(
            "SELECT id, sequence, block_type, LENGTH(content), substr(content, 1, 80), timestamp \
             FROM harness_blocks WHERE session_id = ?1 ORDER BY sequence ASC LIMIT 500",
        ) {
            Ok(s) => s,
            Err(e) => {
                log::warn!("observatory: load_blocks prepare error: {e}");
                return Vec::new();
            }
        };
        let rows = match stmt.query_map(rusqlite::params![session_id], |row| {
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

        // messages — rusqlite 直查 warp.sqlite messages 表
        self.load_recent_messages();
    }

    #[cfg(not(feature = "orchestration"))]
    fn load_orchestration_data(&mut self) {
        // orchestration feature 关闭时三项全空
        self.snapshot.runs = Vec::new();
        self.snapshot.tasks = Vec::new();
        self.snapshot.recent_messages = Vec::new();
    }

    /// rusqlite 直查 messages 表（最新 30 条）。
    ///
    /// 需要 `local_fs` feature 才能访问 sqlite 文件路径。
    #[cfg(all(feature = "orchestration", feature = "local_fs"))]
    fn load_recent_messages(&mut self) {
        // store 底层用的 diesel sqlite connection，我们直接用 rusqlite
        // 打开同一个 warp.sqlite 来查 messages 表（避免 diesel 依赖传递到 model）。
        // 通过 warp_core::paths 取路径，避免依赖 crate::persistence（需 local_fs feature）。
        let db_path = warp_core::paths::state_dir().join("warp.sqlite");
        if !db_path.exists() {
            return;
        }
        let conn = match rusqlite::Connection::open_with_flags(
            &db_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        ) {
            Ok(c) => c,
            Err(e) => {
                log::warn!("observatory: cannot open warp.sqlite for messages: {e}");
                return;
            }
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
        });
    }

    /// 测试 draft field 枚举
    #[test]
    fn test_draft_field_enum() {
        let fields = [DraftField::To, DraftField::Subject, DraftField::Body];
        assert_eq!(fields.len(), 3);
    }

    /// 测试 ObservatoryTab 枚举
    #[test]
    fn test_tab_enum() {
        assert_ne!(ObservatoryTab::Sessions, ObservatoryTab::Orchestration);
    }
}
