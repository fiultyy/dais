//! 拦截 + 透传核心: 读 body → 旁路捕获 → 注入 auth → 上游流式透传。

use std::sync::Arc;

use axum::body::{to_bytes, Body};
use axum::extract::State;
use axum::http::{Request, Response, StatusCode};
use axum::response::IntoResponse;
use futures_util::StreamExt;

use tokio::sync::mpsc;
use uuid::Uuid;

use crate::upstream::UpstreamConfig;
use crate::{RawEvent, RAW_CHANNEL_CAPACITY};

/// 单请求 body 上限 64MB (信任边界: 防止 harness 异常撑爆内存)。
const MAX_BODY_BYTES: usize = 64 * 1024 * 1024;

/// 转发时跳过的请求头: host 会错、auth 由代理重注入、content-length 由 reqwest 重算。
const SKIPPED_REQUEST_HEADERS: [&str; 5] =
    ["host", "authorization", "x-api-key", "content-length", "connection"];
/// 透传响应时跳过的 hop-by-hop / 由 axum 重写的头。
const SKIPPED_RESPONSE_HEADERS: [&str; 3] =
    ["transfer-encoding", "connection", "content-length"];

#[derive(Clone)]
pub(crate) struct SharedState {
    pub upstream: UpstreamConfig,
    pub client: reqwest::Client,
    pub raw_tx: mpsc::Sender<RawEvent>,
}

impl SharedState {
    pub(crate) fn new(upstream: UpstreamConfig, client: reqwest::Client) -> (Self, mpsc::Receiver<RawEvent>) {
        let (raw_tx, raw_rx) = mpsc::channel(RAW_CHANNEL_CAPACITY);
        (
            Self {
                upstream,
                client,
                raw_tx,
            },
            raw_rx,
        )
    }
}

/// 任何 method + path 都走这里 (Router::fallback)。
pub(crate) async fn proxy_handler(
    State(state): State<Arc<SharedState>>,
    req: Request<Body>,
) -> axum::response::Response {
    match proxy_inner(state, req).await {
        Ok(resp) => resp,
        Err(e) => {
            tracing::warn!("proxy error: {e}");
            (StatusCode::BAD_GATEWAY, format!("zap proxy error: {e}")).into_response()
        }
    }
}

async fn proxy_inner(
    state: Arc<SharedState>,
    req: Request<Body>,
) -> crate::Result<axum::response::Response> {
    let (parts, body) = req.into_parts();

    // 1. 读取完整 request body
    let body_bytes = to_bytes(body, MAX_BODY_BYTES).await?;

    // 2. 旁路捕获 (try_send 不阻塞; 满即丢)
    let path = parts
        .uri
        .path_and_query()
        .map(|pq| pq.as_str().to_string())
        .unwrap_or_else(|| "/".to_string());
    let id = Uuid::new_v4();
    let headers_json: serde_json::Map<String, serde_json::Value> = parts
        .headers
        .iter()
        .map(|(k, v)| {
            (
                k.as_str().to_string(),
                serde_json::Value::String(v.to_str().unwrap_or_default().to_string()),
            )
        })
        .collect();
    drop_capture(&state.raw_tx, RawEvent::Request {
        id,
        method: parts.method.as_str().to_string(),
        path: path.clone(),
        headers: serde_json::Value::Object(headers_json),
        body: body_bytes.clone(),
    });

    // 3. 构造上游请求
    let url = format!("{}{}", state.upstream.api_base, path);
    let mut rb = state
        .client
        .request(parts.method.clone(), &url)
        .body(body_bytes);

    // 4. API key 运行时从 env 读, 不存储
    if let Ok(key) = std::env::var(&state.upstream.api_key_env) {
        rb = rb.header(
            &state.upstream.auth_header,
            format!("{}{}", state.upstream.auth_prefix, key),
        );
    }
    for (name, value) in &parts.headers {
        if SKIPPED_REQUEST_HEADERS.contains(&name.as_str()) {
            continue;
        }
        if let Ok(v) = value.to_str() {
            rb = rb.header(name.as_str(), v);
        }
    }

    // 5. 发送, 取 streaming response
    let resp = rb.send().await?;
    let status = resp.status();

    let mut builder = Response::builder().status(StatusCode::from_u16(status.as_u16())?);
    for (name, value) in resp.headers() {
        if SKIPPED_RESPONSE_HEADERS.contains(&name.as_str()) {
            continue;
        }
        if let Ok(v) = value.to_str() {
            builder = builder.header(name.as_str(), v);
        }
    }

    // 6. 边透传边旁路捕获 chunk; 7. Body::from_stream — SSE 不缓冲
    drop_capture(&state.raw_tx, RawEvent::ResponseDone {
        id,
        status: status.as_u16(),
    });
    let tx = state.raw_tx.clone();
    let mut seq = 0u64;
    let stream = resp.bytes_stream().map(move |chunk| {
        if let Ok(bytes) = &chunk {
            drop_capture(&tx, RawEvent::ResponseChunk {
                id,
                seq,
                chunk: bytes.clone(),
            });
            seq += 1;
        }
        chunk.map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))
    });

    Ok(builder.body(Body::from_stream(stream))?)
}

fn drop_capture(tx: &mpsc::Sender<RawEvent>, event: RawEvent) {
    if let Err(e) = tx.try_send(event) {
        tracing::warn!("raw capture channel full, dropping event: {e}");
    }
}
