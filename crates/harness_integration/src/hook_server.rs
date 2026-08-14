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
//!
//! ## 鉴权
//!
//! 所有 `/hooks/*` 端点要求 token 校验，支持三种方式（任一匹配即通过）：
//! - `Authorization: Bearer <token>` 头
//! - `x-zap-hook-token: <token>` 头
//! - `?token=<token>` query 参数
//!
//! `/health` 端点不鉴权。

use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::middleware::Next;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use harness_blocks::{BlockStore, BlockType, HarnessBlock};
use parking_lot::Mutex;
use serde::Deserialize;
use serde_json::Value;
use tokio::net::TcpListener;
use tokio::task::JoinHandle;
use uuid::Uuid;

use crate::session::SessionContext;

/// 共享状态，注入到每个 axum handler 中。
#[derive(Clone)]
struct HookState {
    store: Arc<Mutex<BlockStore>>,
    ctx: Arc<SessionContext>,
    /// 鉴权 token，生成时为 uuid v4。
    token: String,
}

/// Running hook server. Drop to shut down.
pub struct HookServer {
    port: u16,
    shutdown: Option<JoinHandle<()>>,
    /// 鉴权 token。
    token: String,
}

/// Query 参数：`?token=<value>`
#[derive(Deserialize)]
struct TokenQuery {
    token: Option<String>,
}

/// 鉴权中间件：校验 token，不匹配则返回 401。
async fn auth_middleware(
    State(state): State<HookState>,
    headers: HeaderMap,
    query: Query<TokenQuery>,
    request: axum::extract::Request,
    next: Next,
) -> impl IntoResponse {
    let expected = &state.token;
    let mut found = false;

    // 1. Authorization: Bearer <token>
    if let Some(auth) = headers.get("authorization") {
        if let Ok(s) = auth.to_str() {
            if let Some(t) = s.strip_prefix("Bearer ") {
                if t == expected {
                    found = true;
                }
            }
        }
    }

    // 2. x-zap-hook-token: <token>
    if !found {
        if let Some(h) = headers.get("x-zap-hook-token") {
            if let Ok(t) = h.to_str() {
                if t == expected {
                    found = true;
                }
            }
        }
    }

    // 3. ?token=<token>
    if !found {
        if let Some(ref t) = query.0.token {
            if t == expected {
                found = true;
            }
        }
    }

    if found {
        next.run(request).await
    } else {
        StatusCode::UNAUTHORIZED.into_response()
    }
}

impl HookServer {
    /// Bind on `127.0.0.1:0` (ephemeral port) and start serving.
    ///
    /// 生成随机 token (uuid v4) 用于鉴权。
    pub async fn start(
        store: Arc<Mutex<BlockStore>>,
        ctx: Arc<SessionContext>,
    ) -> anyhow::Result<Self> {
        let token = Uuid::new_v4().to_string();
        let state = HookState {
            store,
            ctx,
            token: token.clone(),
        };

        // 鉴权路由组：所有 /hooks/* 端点需要 token 校验
        let hook_routes = Router::new()
            .route("/hooks/user_prompt_submit", post(user_prompt_submit))
            .route("/hooks/pre_tool_use", post(pre_tool_use))
            .route("/hooks/post_tool_use", post(post_tool_use))
            .route("/hooks/stop", post(stop))
            .layer(axum::middleware::from_fn_with_state(
                state.clone(),
                auth_middleware,
            ));

        let app = Router::new()
            .route("/health", get(health))
            .merge(hook_routes)
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
            token,
        })
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// Base URL for hook callbacks, e.g. `http://127.0.0.1:34567`.
    pub fn base_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    /// 鉴权 token。
    pub fn token(&self) -> &str {
        &self.token
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
    insert_hook_block(
        &st,
        BlockType::UserPrompt,
        prompt.into_bytes(),
        body,
        "user_prompt_submit",
    );
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
    insert_hook_block(
        &st,
        BlockType::ToolResult,
        tool.into_bytes(),
        body,
        "post_tool_use",
    );
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
