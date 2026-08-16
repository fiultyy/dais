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

    /// Pure env-var core: build the harness-specific proxy environment from
    /// explicit values (no live [`ProxyHandle`] needed — external captures
    /// assemble env from recorded values; see harness_integration's
    /// `external_capture` module).
    pub fn env_injection_for(
        port: u16,
        ca_cert_path: &std::path::Path,
        harness: HarnessType,
    ) -> Vec<(&'static str, String)> {
        let base = format!("https://127.0.0.1:{}", port);
        let ca = ca_cert_path.display().to_string();
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

    /// 注入给 harness 进程的环境变量, 让其流量走本地 proxy。
    pub fn env_injection(handle: &ProxyHandle, harness: HarnessType) -> Vec<(&'static str, String)> {
        Self::env_injection_for(handle.port, &handle.ca_cert_path, harness)
    }

    /// Path of the persisted CA certificate (CA is owned for the manager's
    /// lifetime; regenerated only if missing on disk).
    pub fn ca_cert_path(&self) -> &std::path::Path {
        &self.ca.ca_cert_path
    }
}

impl Drop for ProxyHandle {
    fn drop(&mut self) {
        if let Some(handle) = self.shutdown.take() {
            handle.shutdown();
        }
    }
}
