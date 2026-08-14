//! 上游配置: 按 harness 类型三级优先解析 (显式 > 探测 > 默认)。

use crate::Result;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HarnessType {
    ClaudeCode,
    Codex,
    Omp,
    Generic,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResponseFormat {
    AnthropicSSE,
    OpenAISSE,
    Generic,
}

#[derive(Debug, Clone)]
pub struct UpstreamConfig {
    pub api_base: String,
    pub auth_header: String,
    pub auth_prefix: String,
    pub api_key_env: String,
    pub request_path: String,
    pub response_format: ResponseFormat,
}

impl UpstreamConfig {
    /// 三级优先: 显式 base > 探测 (ZAP_UPSTREAM_BASE env) > harness 默认。
    pub fn resolve(harness: HarnessType, explicit: Option<&str>) -> Result<Self> {
        if let Some(base) = explicit {
            return Ok(Self::with_base(harness, base));
        }
        if let Ok(base) = std::env::var("ZAP_UPSTREAM_BASE") {
            return Ok(Self::with_base(harness, &base));
        }
        Ok(match harness {
            HarnessType::ClaudeCode => Self::anthropic("https://api.anthropic.com"),
            HarnessType::Codex => Self::openai("https://api.openai.com"),
            HarnessType::Omp => Self::from_omp_config()?,
            HarnessType::Generic => Self {
                api_base: String::new(),
                auth_header: "authorization".into(),
                auth_prefix: "Bearer ".into(),
                api_key_env: "ZAP_API_KEY".into(),
                request_path: "/".into(),
                response_format: ResponseFormat::Generic,
            },
        })
    }

    fn with_base(harness: HarnessType, base: &str) -> Self {
        let mut cfg = match harness {
            HarnessType::Codex => Self::openai(""),
            HarnessType::Omp | HarnessType::ClaudeCode => Self::anthropic(""),
            HarnessType::Generic => Self {
                api_base: String::new(),
                auth_header: "authorization".into(),
                auth_prefix: "Bearer ".into(),
                api_key_env: "ZAP_API_KEY".into(),
                request_path: "/".into(),
                response_format: ResponseFormat::Generic,
            },
        };
        cfg.api_base = base.trim_end_matches('/').to_string();
        cfg
    }

    fn anthropic(base: &str) -> Self {
        Self {
            api_base: base.to_string(),
            auth_header: "x-api-key".into(),
            auth_prefix: String::new(),
            api_key_env: "ANTHROPIC_API_KEY".into(),
            request_path: "/v1/messages".into(),
            response_format: ResponseFormat::AnthropicSSE,
        }
    }

    fn openai(base: &str) -> Self {
        Self {
            api_base: base.to_string(),
            auth_header: "authorization".into(),
            auth_prefix: "Bearer ".into(),
            api_key_env: "OPENAI_API_KEY".into(),
            request_path: "/v1/chat/completions".into(),
            response_format: ResponseFormat::OpenAISSE,
        }
    }

    /// Omp: 读 ~/.config/zap/omp-upstream.json
    /// { "api_base": "...", "api_key_env": "...", "response_format": "anthropic"|"openai"|"generic" }
    fn from_omp_config() -> Result<Self> {
        #[derive(serde::Deserialize)]
        struct OmpCfg {
            api_base: String,
            #[serde(default = "default_key_env")]
            api_key_env: String,
            #[serde(default)]
            response_format: String,
        }
        fn default_key_env() -> String {
            "ZAP_API_KEY".to_string()
        }

        let path = std::env::var("HOME").map(std::path::PathBuf::from).map(|h| {
            h.join(".config")
                .join("zap")
                .join("omp-upstream.json")
        })?;
        let mut cfg = Self::anthropic("https://api.anthropic.com");
        if let Ok(text) = std::fs::read_to_string(&path) {
            let omp: OmpCfg = serde_json::from_str(&text)?;
            cfg.api_base = omp.api_base.trim_end_matches('/').to_string();
            cfg.api_key_env = omp.api_key_env;
            cfg.response_format = match omp.response_format.as_str() {
                "openai" => ResponseFormat::OpenAISSE,
                "generic" => ResponseFormat::Generic,
                _ => ResponseFormat::AnthropicSSE,
            };
        }
        Ok(cfg)
    }
}
