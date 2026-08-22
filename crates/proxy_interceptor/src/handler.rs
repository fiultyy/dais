//! 拦截 + 透传核心: 读 body → 旁路捕获 → 注入 auth → 上游流式透传。

use std::sync::Arc;

use axum::body::{to_bytes, Body};
use axum::extract::State;
use axum::http::{Request, Response, StatusCode};
use axum::response::IntoResponse;

use tokio::sync::mpsc;
use uuid::Uuid;

use crate::upstream::UpstreamConfig;
use crate::{RawEvent, RAW_CHANNEL_CAPACITY};

/// 单请求 body 上限 64MB (信任边界: 防止 harness 异常撑爆内存)。
const MAX_BODY_BYTES: usize = 64 * 1024 * 1024;

/// 转发时跳过的请求头: host 指向入口会错、content-length 由 reqwest 重算、
/// connection 是 hop-by-hop。**auth 头(authorization/x-api-key)透明转发**
/// (T5 透明管道: 客户端凭据原样到出口, dais 只改目的地+旁观捕获)。
/// `x-dais-instance` 是 dais 别名铸造的实例标记(T8) — 网关数据面读取做
/// session 键控后必须在转发前剥掉(dais 内部信号, 不进上游字节)。
/// `x-zap-instance` 是改名前铸的同一标记(D4 兼容): 只剥不读 — 升级后
/// 未重 bootstrap 的旧 shell 旧别名仍发旧头, 剥掉保证内部信号不进上游,
/// 键控回落默认 session(T5 行为)。
// zap-purge: legacy wire headers, kept for compat
const SKIPPED_REQUEST_HEADERS: [&str; 5] = [
    "host",
    "content-length",
    "connection",
    "x-dais-instance",
    "x-zap-instance", // zap-purge: legacy header from pre-rename shell aliases
];
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
            (StatusCode::BAD_GATEWAY, format!("dais proxy error: {e}")).into_response()
        }
    }
}
pub(crate) async fn proxy_inner(
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

    // 3. 构造上游请求(透明管道: auth 头原样透传, 不注不剥 — T5)
    let url = format!("{}{}", state.upstream.api_base, path);
    let mut rb = state
        .client
        .request(parts.method.clone(), &url)
        .body(body_bytes);

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

    // 6. 边透传边旁路捕获 chunk; 7. Body::from_stream — SSE 不缓冲。
    // ResponseDone 在流结束时发出 (所有 chunk 之后), 语义 = "响应已完整捕获",
    // 下游 raw processor 据此把累计 chunk 拼装成完整响应体。
    let tx = state.raw_tx.clone();
    let status_u16 = status.as_u16();
    let inner = resp.bytes_stream();
    let stream = futures_util::stream::unfold(
        (inner, Some((tx, id, status_u16)), 0u64),
        |(mut inner, done, mut seq)| async move {
            use futures_util::StreamExt;
            match inner.next().await {
                Some(Ok(bytes)) => {
                    if let Some((tx, id, _)) = &done {
                        drop_capture(tx, RawEvent::ResponseChunk {
                            id: *id,
                            seq,
                            chunk: bytes.clone(),
                        });
                        seq += 1;
                    }
                    Some((
                        Ok::<bytes::Bytes, std::io::Error>(bytes),
                        (inner, done, seq),
                    ))
                }
                Some(Err(e)) => Some((
                    Err(std::io::Error::new(std::io::ErrorKind::Other, e)),
                    (inner, None, seq),
                )),
                None => {
                    // 流正常结束: 发 ResponseDone。
                    if let Some((tx, id, status)) = done {
                        drop_capture(&tx, RawEvent::ResponseDone { id, status });
                    }
                    None
                }
            }
        },
    );

    Ok(builder.body(Body::from_stream(stream))?)
}

fn drop_capture(tx: &mpsc::Sender<RawEvent>, event: RawEvent) {
    if let Err(e) = tx.try_send(event) {
        tracing::warn!("raw capture channel full, dropping event: {e}");
    }
}
