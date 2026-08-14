//! proxy_interceptor — TLS 透明代理核心。
//!
//! 本地 HTTPS 代理拦截 harness (Claude Code / Codex / ...) 的 LLM 流量:
//! 请求/响应旁路捕获到 mpsc channel (RawEvent), 业务流量 SSE 流式透传到上游。

mod ca;
mod handler;
mod manager;
mod server;
mod upstream;

pub use ca::LocalCA;
pub use manager::{ProxyHandle, ProxyManager};
pub use server::ProxyServer;
pub use upstream::{HarnessType, ResponseFormat, UpstreamConfig};

use bytes::Bytes;
use uuid::Uuid;

pub type Error = Box<dyn std::error::Error + Send + Sync + 'static>;
pub type Result<T> = std::result::Result<T, Error>;

/// 旁路捕获的原始事件。通过 mpsc channel 发送, 不阻塞业务流量。
#[derive(Debug, Clone)]
pub enum RawEvent {
    Request {
        id: Uuid,
        method: String,
        path: String,
        headers: serde_json::Value,
        body: Bytes,
    },
    ResponseChunk {
        id: Uuid,
        seq: u64,
        chunk: Bytes,
    },
    ResponseDone {
        id: Uuid,
        status: u16,
    },
}

/// raw 捕获 channel 容量; 满时丢 chunk 并告警 (捕获是旁路, 不阻塞透传)。
pub const RAW_CHANNEL_CAPACITY: usize = 1024;
