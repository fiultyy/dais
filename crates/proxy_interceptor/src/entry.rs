//! T5 单端口入口: loopback 明文 HTTP, 路径前缀分流到各自出口。
//!
//! 与 [`crate::server::ProxyServer`](TLS 反代, GUI 拦截路径用) 平行:
//! 入口是常驻、单端口(默认 8787)、明文 — 别名 CLI(`cc-zap`/`omp-zap`/
//! `pi-zap`)的 base URL 指到这里。前缀即 harness 标识:
//! - `/cc/*`  → ClaudeCode 出口(显式覆盖 ZAP_UPSTREAM_BASE > 用户
//!   `~/.claude/settings.json` 的 env.ANTHROPIC_BASE_URL > 官方默认)
//! - `/omp/*`、`/pi/*` → `UpstreamConfig::from_omp_config()`
//!   (`~/.config/zap/omp-upstream.json`, 编排侧写, 每请求热读)
//!
//! 透明管道(T5 口径): auth 头(`authorization`/`x-api-key`)原样转发,
//! 不剥不注 — 客户端凭据自带, zap 只改目的地 + 旁观捕获。每前缀独立
//! raw channel, 上层(harness_integration 的 entry gateway)归并各前缀
//! 流量到一个观测 session。

use std::net::SocketAddr;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::State;
use axum::http::Request;
use axum::response::IntoResponse;
use axum::Router;
use tokio::sync::mpsc;

use crate::handler::{proxy_handler, SharedState};
use crate::upstream::{HarnessType, UpstreamConfig};
use crate::{RawEvent, RAW_CHANNEL_CAPACITY, Result};

/// 一条前缀路由: 前缀 → 出口解析(每请求调用, 支持配置文件热更)。
struct EntryRoute {
    prefix: &'static str,
    resolve: fn() -> Result<UpstreamConfig>,
    raw_tx: mpsc::Sender<RawEvent>,
}

struct EntryState {
    routes: Vec<EntryRoute>,
    client: reqwest::Client,
}

/// `/cc` 出口解析: 显式覆盖(env `ZAP_UPSTREAM_BASE`) > 用户 settings
/// base > 官方默认。现有 [`UpstreamConfig::resolve`] 三级结构不动, 在
/// 入口侧组合(用户 settings base 作为 explicit 传入, env 级提前短路)。
fn resolve_cc() -> Result<UpstreamConfig> {
    if std::env::var("ZAP_UPSTREAM_BASE").is_ok() {
        // resolve 内部: explicit(None) → env ZAP_UPSTREAM_BASE → 默认。
        return UpstreamConfig::resolve(HarnessType::ClaudeCode, None);
    }
    UpstreamConfig::resolve(HarnessType::ClaudeCode, user_claude_base_url().as_deref())
}

/// 用户 `~/.claude/settings.json` 的 `env.ANTHROPIC_BASE_URL`(如有)。
fn user_claude_base_url() -> Option<String> {
    let home = std::env::var("HOME").ok()?;
    let text =
        std::fs::read_to_string(std::path::Path::new(&home).join(".claude/settings.json")).ok()?;
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    v.get("env")?
        .get("ANTHROPIC_BASE_URL")?
        .as_str()
        .map(str::to_string)
}

pub struct EntryServer {
    pub port: u16,
    /// `(prefix, receiver)`: 每前缀一条旁路捕获流, 上层归并到各自 session。
    pub raw_rxs: Vec<(&'static str, mpsc::Receiver<RawEvent>)>,
    handle: axum_server::Handle,
}

impl EntryServer {
    /// 绑定 `127.0.0.1:port`(明文)。端口被占等绑定失败 → Err(调用方降级)。
    pub async fn start(port: u16) -> Result<Self> {
        let handle = axum_server::Handle::new();

        let spec: [(&'static str, fn() -> Result<UpstreamConfig>); 3] = [
            ("/cc", resolve_cc),
            ("/omp", UpstreamConfig::from_omp_config),
            ("/pi", UpstreamConfig::from_omp_config),
        ];
        let mut raw_rxs = Vec::new();
        let mut routes = Vec::new();
        for (prefix, resolve) in spec {
            let (tx, rx) = mpsc::channel(RAW_CHANNEL_CAPACITY);
            routes.push(EntryRoute {
                prefix,
                resolve,
                raw_tx: tx,
            });
            raw_rxs.push((prefix, rx));
        }

        let app = Router::new()
            .fallback(entry_handler)
            .with_state(Arc::new(EntryState {
                routes,
                client: reqwest::Client::new(),
            }));

        let server_handle = handle.clone();
        let addr = SocketAddr::from(([127, 0, 0, 1], port));
        tokio::spawn(async move {
            if let Err(e) = axum_server::bind(addr)
                .handle(server_handle)
                .serve(app.into_make_service())
                .await
            {
                tracing::error!("entry server error: {e}");
            }
        });

        let bound = handle
            .listening()
            .await
            .ok_or("entry server failed to bind")?;

        Ok(Self {
            port: bound.port(),
            raw_rxs,
            handle,
        })
    }

    pub fn stop(self) {
        self.handle.shutdown();
    }
}

/// 前缀分流: 匹配最长前缀 → strip 前缀(保留 query) → 逐请求解析出口 →
/// 复用 TLS 路径的转发核心([`proxy_handler`], 透明管道语义)。
async fn entry_handler(
    State(state): State<Arc<EntryState>>,
    req: Request<Body>,
) -> axum::response::Response {
    let (mut parts, body) = req.into_parts();
    let path = parts.uri.path().to_string();

    let route = state
        .routes
        .iter()
        .find(|r| path == r.prefix || path.starts_with(&format!("{}/", r.prefix)));
    let Some(route) = route else {
        return (axum::http::StatusCode::NOT_FOUND, "no entry prefix match\n").into_response();
    };

    // strip 前缀: /cc/v1/messages → /v1/messages (query 保留)。
    let stripped = path.strip_prefix(route.prefix).unwrap_or("");
    let stripped = if stripped.is_empty() { "/" } else { stripped };
    let query = parts.uri.path_and_query().and_then(|pq| pq.query());
    let new_pq = match query {
        Some(q) => format!("{stripped}?{q}"),
        None => stripped.to_string(),
    };
    let uri = match axum::http::Uri::builder()
        .path_and_query(new_pq)
        .build()
    {
        Ok(u) => u,
        Err(e) => {
            return (axum::http::StatusCode::BAD_REQUEST, format!("bad path: {e}\n"))
                .into_response()
        }
    };
    parts.uri = uri;
    let req = Request::from_parts(parts, body);

    // 每请求解析出口(配置热更); 解析失败 → 502(不猜测目的地)。
    let upstream = match (route.resolve)() {
        Ok(u) => u,
        Err(e) => {
            return (axum::http::StatusCode::BAD_GATEWAY, format!("upstream resolve: {e}\n"))
                .into_response()
        }
    };
    let shared = Arc::new(SharedState {
        upstream,
        client: state.client.clone(),
        raw_tx: route.raw_tx.clone(),
    });
    proxy_handler(State(shared), req).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_claude_base_url_reads_settings_env_block() {
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("HOME", dir.path());
        let r = (|| {
            let none = user_claude_base_url();
            assert_eq!(none, None, "no settings.json → None");

            std::fs::create_dir_all(dir.path().join(".claude")).unwrap();
            std::fs::write(
                dir.path().join(".claude/settings.json"),
                r#"{"env":{"ANTHROPIC_BASE_URL":"https://relay.example.com","OTHER":"x"}}"#,
            )
            .unwrap();
            assert_eq!(
                user_claude_base_url().as_deref(),
                Some("https://relay.example.com")
            );

            // env 块缺失/损坏 → None(不 panic)。
            std::fs::write(dir.path().join(".claude/settings.json"), "{bad json").unwrap();
            assert_eq!(user_claude_base_url(), None);
        })();
        std::env::set_var("HOME", dir.path().parent().unwrap());
        r
    }
}
