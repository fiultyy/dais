//! ProxyManager: CA 生命周期 + proxy 分配 + harness 环境变量注入。

use std::path::PathBuf;

use tokio::sync::mpsc;

use crate::ca::{ensure_local_ca, LocalCA};
use crate::server::ProxyServer;
use crate::upstream::{HarnessType, UpstreamConfig};
use crate::{RawEvent, Result};

pub struct ProxyManager {
    ca: LocalCA,
    client: reqwest::Client,
}

pub struct ProxyHandle {
    pub port: u16,
    pub ca_cert_path: PathBuf,
    pub raw_rx: mpsc::Receiver<RawEvent>,
    shutdown: Option<axum_server::Handle>,
}

impl ProxyManager {
    pub fn new() -> Result<Self> {
        Ok(Self {
            ca: ensure_local_ca()?,
            client: reqwest::Client::new(),
        })
    }

    pub async fn allocate(&self, upstream: UpstreamConfig) -> Result<ProxyHandle> {
        let server = ProxyServer::start(upstream, &self.ca, self.client.clone()).await?;
        Ok(ProxyHandle {
            port: server.port,
            ca_cert_path: self.ca.ca_cert_path.clone(),
            raw_rx: server.raw_rx,
            shutdown: Some(server.handle),
        })
    }

    /// 注入给 harness 进程的环境变量, 让其流量走本地 proxy。
    pub fn env_injection(handle: &ProxyHandle, harness: HarnessType) -> Vec<(&'static str, String)> {
        let base = format!("https://127.0.0.1:{}", handle.port);
        let ca = handle.ca_cert_path.display().to_string();
        match harness {
            HarnessType::ClaudeCode => vec![
                ("ANTHROPIC_BASE_URL", base),
                ("NODE_EXTRA_CA_CERTS", ca),
            ],
            HarnessType::Codex => vec![("OPENAI_BASE_URL", base)],
            HarnessType::Omp | HarnessType::Generic => vec![
                ("HTTPS_PROXY", base),
                ("SSL_CERT_FILE", ca),
            ],
        }
    }
}

impl Drop for ProxyHandle {
    fn drop(&mut self) {
        if let Some(handle) = self.shutdown.take() {
            handle.shutdown();
        }
    }
}
