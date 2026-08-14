//! TLS Proxy Server: axum-server + rustls, HTTPS 监听 127.0.0.1:0 (动态端口)。

use std::net::SocketAddr;
use std::sync::Arc;

use axum::Router;
use axum_server::tls_rustls::RustlsConfig;
use tokio::sync::mpsc;

use crate::ca::LocalCA;
use crate::handler::{proxy_handler, SharedState};
use crate::upstream::UpstreamConfig;
use crate::{RawEvent, Result};

pub struct ProxyServer {
    pub port: u16,
    pub raw_rx: mpsc::Receiver<RawEvent>,
    pub(crate) handle: axum_server::Handle,
}

impl ProxyServer {
    pub async fn start(
        upstream: UpstreamConfig,
        ca: &LocalCA,
        client: reqwest::Client,
    ) -> Result<Self> {
        install_crypto_provider();
        let tls = RustlsConfig::from_pem_file(&ca.server_cert_path, &ca.server_key_path).await?;

        let handle = axum_server::Handle::new();
        let (state, raw_rx) = SharedState::new(upstream, client);
        // 任何 method+path → proxy_handler
        let app = Router::new()
            .fallback(proxy_handler)
            .with_state(Arc::new(state));

        let server_handle = handle.clone();
        let addr = SocketAddr::from(([127, 0, 0, 1], 0));
        tokio::spawn(async move {
            if let Err(e) = axum_server::bind_rustls(addr, tls)
                .handle(server_handle)
                .serve(app.into_make_service())
                .await
            {
                tracing::error!("proxy server error: {e}");
            }
        });

        // port 0: 等绑定完成取真实端口
        let bound = handle
            .listening()
            .await
            .ok_or("proxy server failed to bind")?;

        Ok(Self {
            port: bound.port(),
            raw_rx,
            handle,
        })
    }

    pub fn stop(self) {
        self.handle.shutdown();
    }
}

/// rustls 0.23 需要进程级 CryptoProvider; workspace 特性并集同时引入 ring/aws-lc-rs,
/// 无法自动选择 — 显式装 ring, 已装则忽略。
fn install_crypto_provider() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}
