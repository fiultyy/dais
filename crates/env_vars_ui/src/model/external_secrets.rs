//! External-secret POD 切片(自 `app/src/external_secrets/mod.rs` 上移)。
//!
//! 依据 env_vars 契约 G1:serde POD 类型 + 纯命令串生成沉入本 crate 模型层;
//! `verify_installed_and_fetch_secrets` / `is_installed` / `fetch_secrets`
//! (依赖 `terminal::local_shell` spawn)与 toast 文案组装留 app 侧残根。
//! app 侧 `external_secrets` 模块以 re-export 保持 `crate::external_secrets::`
//! 既有调用点路径不变。

use core::fmt;
use std::sync::LazyLock;

use serde::{Deserialize, Serialize};
use warp_util::path::ShellFamily;

// Used as a delimeter to separate metadata (such as names and references)
// in cases the cli tool doesn't display secrets in a common format (i.e. json)
static WARP_SECRET_DELIMITER: LazyLock<&'static str> = LazyLock::new(|| "/warp-secret-delimeter/");

static LASTPASS_LIST_SECRETS_COMMAND: LazyLock<Vec<String>> = LazyLock::new(|| {
    vec![
        "lpass".to_owned(),
        "ls".to_owned(),
        format!("--format=%an{}%ai", *WARP_SECRET_DELIMITER),
    ]
});

/// Represents a "completed" secret
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum ExternalSecret {
    OnePassword(OnePasswordSecret),
    LastPass(LastPassSecret),
}

impl ExternalSecret {
    pub fn get_secret_extraction_command(&self, shell_family: ShellFamily) -> String {
        let prefix = match shell_family {
            ShellFamily::Posix => "\\",
            ShellFamily::PowerShell => "",
        };
        match self {
            ExternalSecret::OnePassword(secret) => {
                format!(
                    "{}op item get --fields credential --reveal {}",
                    prefix, secret.reference
                )
            }
            ExternalSecret::LastPass(secret) => {
                format!("{}lpass show --password {}", prefix, secret.reference)
            }
        }
    }

    pub fn get_display_name(&self) -> String {
        match self {
            ExternalSecret::OnePassword(secret) => secret.name.clone(),
            ExternalSecret::LastPass(secret) => secret.name.clone(),
        }
    }
}

/// Used to identify a secret manager in menus/actions (POD)。
/// 安装检测/拉取流程留 app(依赖 local shell)。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum SecretManager {
    OnePassword,
    LastPass,
}

impl fmt::Display for SecretManager {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            SecretManager::OnePassword => write!(f, "1Password"),
            SecretManager::LastPass => write!(f, "LastPass"),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct OnePasswordSecret {
    pub name: String,
    pub reference: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct LastPassSecret {
    pub name: String,
    pub reference: String,
}

/// 解析 1Password `op item list --format=json` 输出(纯函数)。
pub fn parse_onepassword_secrets(output: &str) -> anyhow::Result<Vec<ExternalSecret>> {
    let value: serde_json::Value = serde_json::from_str(output)?;
    let parsed_output: Vec<ExternalSecret> = value
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("Expected array in JSON"))?
        .iter()
        .map(|secret: &serde_json::Value| {
            let name = secret
                .get("title")
                .and_then(|v: &serde_json::Value| v.as_str())
                .ok_or_else(|| anyhow::anyhow!("Secret is missing title"))?;
            let reference = secret
                .get("id")
                .and_then(|v: &serde_json::Value| v.as_str())
                .ok_or_else(|| anyhow::anyhow!("Secret is missing id"))?;

            Ok(ExternalSecret::OnePassword(OnePasswordSecret {
                name: name.to_string(),
                reference: reference.to_string(),
            }))
        })
        .collect::<Result<Vec<_>, anyhow::Error>>()?;

    Ok(parsed_output)
}

/// 解析 LastPass `lpass ls` 输出(纯函数)。
pub fn parse_lastpass_secrets(output: &str) -> anyhow::Result<Vec<ExternalSecret>> {
    let parsed_output: Vec<ExternalSecret> = output
        .lines()
        .filter_map(|line| {
            let parts = line.split(*WARP_SECRET_DELIMITER).collect::<Vec<_>>();
            if parts.len() == 2 && !parts[0].is_empty() && !parts[1].is_empty() {
                Some(ExternalSecret::LastPass(LastPassSecret {
                    name: parts[0].to_owned(),
                    reference: parts[1].to_owned(),
                }))
            } else {
                None
            }
        })
        .collect();

    if !parsed_output.is_empty() {
        Ok(parsed_output)
    } else {
        Err(anyhow::anyhow!("Failed to parse any secrets"))
    }
}
