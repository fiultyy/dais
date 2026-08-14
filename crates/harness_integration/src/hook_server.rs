//! HookServer — plain-HTTP (localhost) server that receives agent harness hook
//! callbacks and converts them into [`HarnessBlock`]s stored in the
//! [`BlockStore`].
//!
//! Endpoints:
//! | Method | Path                         | Block produced |
//! |--------|------------------------------|----------------|
//! | POST   | /hooks/user_prompt_submit    | `UserPrompt`   |
//! | POST   | /hooks/pre_tool_use          | `ToolCall`     |
//! | POST   | /hooks/post_tool_use         | `ToolResult`   |
//! | POST   | /hooks/stop                  | `Exit`         |
//!
//! Each handler accepts a JSON body (fields are best-effort extracted), creates
//! a block with metadata `{ source: "hook", event: <name> }`, and returns 200.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::Json;
use axum::Router;
use harness_blocks::{BlockStore, BlockType, HarnessBlock};
use parking_lot::Mutex;
use serde_json::Value;
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

use crate::session::SessionContext;

/// Shared state injected into every axum handler.
#[derive(Clone)]
struct HookState {
    store: Arc<Mutex<BlockStore>>,
    ctx: Arc<SessionContext>,
}

/// Running hook server. Drop to shut down.
pub struct HookServer {
    port: u16,
    shutdown: Option<JoinHandle<()>>,
}

impl HookServer {
    /// Bind on `127.0.0.1:0` (ephemeral port) and start serving.
    pub async fn start(
        store: Arc<Mutex<BlockStore>>,
        ctx: Arc<SessionContext>,
    ) -> anyhow::Result<Self> {
        let state = HookState { store, ctx };
        let app = Router::new()
            .route("/health", get(health))
            .route("/hooks/user_prompt_submit", post(user_prompt_submit))
            .route("/hooks/pre_tool_use", post(pre_tool_use))
            .route("/hooks/post_tool_use", post(post_tool_use))
            .route("/hooks/stop", post(stop))
            .with_state(state);

        let listener = TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0))).await?;
        let port = listener.local_addr()?.port();

        let handle = tokio::spawn(async move {
            if let Err(e) = axum::serve(listener, app).await {
                tracing::warn!("hook server stopped: {e}");
            }
        });

        Ok(Self {
            port,
            shutdown: Some(handle),
        })
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// Base URL for hook callbacks, e.g. `http://127.0.0.1:34567`.
    pub fn base_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }
}

impl Drop for HookServer {
    fn drop(&mut self) {
        if let Some(handle) = self.shutdown.take() {
            handle.abort();
        }
    }
}

// ── handlers ─────────────────────────────────────────────────────────────

async fn health() -> impl IntoResponse {
    StatusCode::OK
}

async fn user_prompt_submit(
    State(st): State<HookState>,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    let prompt = body
        .get("prompt")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    insert_hook_block(&st, BlockType::UserPrompt, prompt.into_bytes(), body, "user_prompt_submit");
    StatusCode::OK
}

async fn pre_tool_use(
    State(st): State<HookState>,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    let tool = body
        .get("tool_name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    insert_hook_block(&st, BlockType::ToolCall, tool.into_bytes(), body, "pre_tool_use");
    StatusCode::OK
}

async fn post_tool_use(
    State(st): State<HookState>,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    let tool = body
        .get("tool_name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    insert_hook_block(&st, BlockType::ToolResult, tool.into_bytes(), body, "post_tool_use");
    StatusCode::OK
}

async fn stop(State(st): State<HookState>, Json(body): Json<Value>) -> impl IntoResponse {
    let exit_code = body
        .get("exit_code")
        .and_then(|v| v.as_i64())
        .unwrap_or(0)
        .to_string();
    insert_hook_block(&st, BlockType::Exit, exit_code.into_bytes(), body, "stop");
    StatusCode::OK
}

// ── helpers ──────────────────────────────────────────────────────────────

fn insert_hook_block(
    st: &HookState,
    block_type: BlockType,
    content: Vec<u8>,
    raw_body: Value,
    event: &str,
) {
    let metadata = serde_json::json!({
        "source": "hook",
        "event": event,
        "raw": raw_body,
    });
    let block = {
        let mut b = HarnessBlock::new(
            &st.ctx.session_id,
            &st.ctx.harness_type,
            block_type,
            st.ctx.next_seq(),
            content,
            st.ctx.now_ms(),
        );
        b.metadata = metadata;
        b
    };
    let store = st.store.lock();
    if let Err(e) = store.insert_block(&block) {
        tracing::warn!("hook_server insert_block failed: {e}");
    }
}
