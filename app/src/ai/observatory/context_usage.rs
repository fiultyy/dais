//! 外部 session 上下文占用派生（T10）。
//!
//! 透明管道（T5-T8）不改协议字节，dais 侧只从旁路捕获产物**读**：
//! - 占用量 = 最近一次带非零 usage 的响应块的
//!   `usage.input_tokens + output_tokens`（T6 双向解析落进块 metadata）。
//!   聊天形 API 每个请求都携带全部历史，故最后一次响应的 prompt+completion
//!   即会话当前上下文占用 — 与 app 自有会话的 chat_stream
//!   `context_window_usage` 折算同语义。
//! - 窗口 = 分层只读映射：① harness 自己的模型配置（omp `models.yml` /
//!   pi `models.json` 的 `contextWindow`，即该 harness UI 自身使用的分母，
//!   编排侧文件，dais 只读不写）② models.dev catalog（同名模型在多个
//!   provider 下窗口一致才采信，歧义/未命中 = 未知）。dais/* 别名模型通常(zap/* 兼容期同)
//!   无注册表条目，未知时 UI 只显示 tokens 不显示百分比。

use crate::ai::agent_providers::models_dev;

/// 选中 session 的上下文占用摘要（观测台 Blocks 侧栏展示）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionContextInfo {
    /// 当前上下文占用量（tokens）。
    pub used_tokens: u64,
    /// 模型上下文窗口；None = 未知（UI 降级为只显示 tokens）。
    pub window_tokens: Option<u64>,
    /// 展示用模型名（响应侧上游上报优先，回落请求侧声明）。
    pub model: String,
}

/// 供窗口映射的 harness 归类：外部捕获 session 前缀（T8 lane 名兼容）优先，
/// 回落块上的 harness_type（GUI 拦截路径 spawn 的 omp/pi 同样读各自配置）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ExternalHarness {
    Omp,
    Pi,
    Cc,
}

fn harness_from_session(session_id: &str, harness_type: &str) -> Option<ExternalHarness> {
    if session_id.starts_with("external-omp") || harness_type == "omp" {
        Some(ExternalHarness::Omp)
    } else if session_id.starts_with("external-pi") || harness_type == "pi" {
        Some(ExternalHarness::Pi)
    } else if session_id.starts_with("external-cc") || harness_type == "claude-code" {
        Some(ExternalHarness::Cc)
    } else {
        None
    }
}

/// 从选中 session 的 blocks 派生上下文占用（DB 只读查询 + metadata 解析）。
///
/// 扫描最近 200 个带模型信息的块（新→旧）：
/// - `used_tokens`：第一个 `usage` 非零的响应块；
/// - `model`：响应块上游上报的 `model` 优先（如 bigmodel 上报 `glm-5.3`），
///   缺失时回落最近的请求侧 `model`（如 `glm-5.2`）。
pub fn derive_session_context(
    conn: &rusqlite::Connection,
    session_id: &str,
    catalog: Option<&models_dev::Catalog>,
) -> Option<SessionContextInfo> {
    let mut stmt = conn
        .prepare(
            "SELECT block_type, harness_type, metadata FROM harness_blocks \
             WHERE session_id = ?1 \
               AND block_type IN ('response', 'response_chunk', 'system_prompt', \
                                  'user_prompt', 'prompt_segment') \
             ORDER BY timestamp DESC, sequence DESC LIMIT 200",
        )
        .ok()?;
    let rows = stmt
        .query_map(rusqlite::params![session_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
            ))
        })
        .ok()?;

    let mut harness_type = String::new();
    let mut used: Option<(u64, Option<String>)> = None; // (tokens, 响应侧 model)
    let mut request_model: Option<String> = None;
    for row in rows {
        let Ok((block_type, row_harness, metadata)) = row else {
            continue;
        };
        if harness_type.is_empty() {
            harness_type = row_harness;
        }
        let Some(meta) = metadata
            .as_deref()
            .and_then(|m| serde_json::from_str::<serde_json::Value>(m).ok())
        else {
            continue;
        };
        let is_response = block_type.starts_with("response");
        if used.is_none() && is_response {
            let usage = meta.get("usage");
            let input = usage
                .and_then(|u| u.get("input_tokens"))
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0);
            let output = usage
                .and_then(|u| u.get("output_tokens"))
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0);
            if input + output > 0 {
                used = Some((
                    input + output,
                    meta.get("model")
                        .and_then(serde_json::Value::as_str)
                        .filter(|m| !m.is_empty())
                        .map(str::to_string),
                ));
            }
        }
        if request_model.is_none() {
            request_model = meta
                .get("model")
                .and_then(serde_json::Value::as_str)
                .filter(|m| !m.is_empty())
                .map(str::to_string);
        }
        if used.is_some() && request_model.is_some() && !harness_type.is_empty() {
            break;
        }
    }

    let (tokens, response_model) = used?;
    let model = response_model
        .clone()
        .or_else(|| request_model.clone())
        .unwrap_or_default();
    let harness = harness_from_session(session_id, &harness_type);
    if model.is_empty() {
        // 无模型名无法做窗口映射，但 tokens 仍有意义。
        return Some(SessionContextInfo {
            used_tokens: tokens,
            window_tokens: None,
            model,
        });
    }
    let models = [model.as_str(), request_model.as_deref().unwrap_or("")];
    let window_tokens = resolve_context_window(harness, &models, catalog);
    Some(SessionContextInfo {
        used_tokens: tokens,
        window_tokens,
        model,
    })
}

/// 分层窗口映射：harness 配置 → models.dev（一致才采信）。
fn resolve_context_window(
    harness: Option<ExternalHarness>,
    models: &[&str],
    catalog: Option<&models_dev::Catalog>,
) -> Option<u64> {
    for m in models.iter().filter(|m| !m.is_empty()) {
        match harness {
            Some(ExternalHarness::Omp) => {
                if let Some(w) = omp_config_path().and_then(|p| window_from_omp_config(&p, m)) {
                    return Some(w);
                }
            }
            Some(ExternalHarness::Pi) => {
                if let Some(w) = pi_config_path().and_then(|p| window_from_pi_config(&p, m)) {
                    return Some(w);
                }
            }
            _ => {}
        }
        if let Some(w) = catalog.and_then(|c| window_from_catalog(c, m)) {
            return Some(w);
        }
    }
    None
}

fn omp_config_path() -> Option<std::path::PathBuf> {
    let home = std::env::var("HOME").ok()?;
    Some(std::path::PathBuf::from(home).join(".omp/agent/models.yml"))
}

fn pi_config_path() -> Option<std::path::PathBuf> {
    let home = std::env::var("HOME").ok()?;
    Some(std::path::PathBuf::from(home).join(".pi/agent/models.json"))
}

/// `dais/glm-5.2` / `glm-5.2` → 匹配配置里的裸模型 id `glm-5.2`。
fn model_id_matches(declared: &str, queried: &str) -> bool {
    let q = queried.rsplit('/').next().unwrap_or(queried);
    let d = declared.rsplit('/').next().unwrap_or(declared);
    !q.is_empty() && q == d
}

/// omp `~/.omp/agent/models.yml`：`providers.<p>.models[].{id, contextWindow}`。
fn window_from_omp_config(path: &std::path::Path, model: &str) -> Option<u64> {
    let raw = std::fs::read_to_string(path).ok()?;
    let v: serde_yaml::Value = serde_yaml::from_str(&raw).ok()?;
    for (_pid, provider) in v.get("providers")?.as_mapping()? {
        let Some(models) = provider.get("models").and_then(|m| m.as_sequence()) else {
            continue;
        };
        for m in models {
            let Some(id) = m.get("id").and_then(serde_yaml::Value::as_str) else {
                continue;
            };
            if model_id_matches(id, model) {
                return m
                    .get("contextWindow")
                    .and_then(serde_yaml::Value::as_u64)
                    .filter(|w| *w > 0);
            }
        }
    }
    None
}

/// pi `~/.pi/agent/models.json`：`providers.<p>.models[].{id, contextWindow}`。
fn window_from_pi_config(path: &std::path::Path, model: &str) -> Option<u64> {
    let raw = std::fs::read_to_string(path).ok()?;
    let v: serde_json::Value = serde_json::from_str(&raw).ok()?;
    let providers = v.get("providers").and_then(serde_json::Value::as_object)?;
    for (_pid, provider) in providers {
        let Some(models) = provider.get("models").and_then(serde_json::Value::as_array) else {
            continue;
        };
        for m in models {
            let Some(id) = m.get("id").and_then(serde_json::Value::as_str) else {
                continue;
            };
            if model_id_matches(id, model) {
                return m
                    .get("contextWindow")
                    .and_then(serde_json::Value::as_u64)
                    .filter(|w| *w > 0);
            }
        }
    }
    None
}

/// models.dev catalog：所有 provider 下同名模型的 `limit.context` 一致才采信
/// （同名模型散布在多个聚合 provider 且窗口各异时视为不可判定）。
fn window_from_catalog(catalog: &models_dev::Catalog, model: &str) -> Option<u64> {
    let mut found: Option<u64> = None;
    for provider in catalog.values() {
        for (mid, m) in &provider.models {
            if model_id_matches(mid, model) || format!("{}/{}", provider.id, mid) == model {
                let w = u64::from(m.limit.context);
                if w == 0 {
                    continue;
                }
                match found {
                    Some(prev) if prev != w => return None, // 歧义
                    _ => found = Some(w),
                }
            }
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    struct Db {
        _tmp: tempfile::TempDir,
        conn: rusqlite::Connection,
        store: harness_integration::BlockStore,
    }

    fn temp_db() -> Db {
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("harness_blocks.db");
        let store =
            harness_integration::BlockStore::open(db_path.to_string_lossy().to_string()).unwrap();
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        Db {
            _tmp: tmp,
            conn,
            store,
        }
    }

    fn insert(
        db: &Db,
        session: &str,
        block_type: &str,
        seq: u32,
        ts: i64,
        metadata: serde_json::Value,
    ) {
        let mut b = harness_integration::HarnessBlock::new(
            session,
            "omp",
            block_type.parse().unwrap(),
            seq,
            Vec::new(),
            ts,
        );
        b.metadata = metadata;
        db.store.insert_block(&b).unwrap();
    }

    #[test]
    fn derives_last_nonzero_usage_with_response_model() {
        let db = temp_db();
        let base: i64 = 1_700_000_000_000;
        insert(
            &db,
            "external-omp",
            "system_prompt",
            1,
            base,
            json!({"model": "glm-5.2", "source": "openai_request"}),
        );
        insert(
            &db,
            "external-omp",
            "user_prompt",
            2,
            base + 10,
            json!({"model": "glm-5.2", "source": "openai_request"}),
        );
        insert(
            &db,
            "external-omp",
            "response",
            3,
            base + 20,
            json!({
                "model": "glm-5.3", "source": "openai_response",
                "usage": {"input_tokens": 22180, "output_tokens": 27}
            }),
        );
        // 第二轮响应（最新，非零 usage）→ 应取这条
        insert(
            &db,
            "external-omp",
            "response",
            7,
            base + 60,
            json!({
                "model": "glm-5.3", "source": "openai_response",
                "usage": {"input_tokens": 22221, "output_tokens": 7}
            }),
        );

        let info = derive_session_context(&db.conn, "external-omp", None).unwrap();
        assert_eq!(info.used_tokens, 22228);
        assert_eq!(info.model, "glm-5.3");
        assert_eq!(info.window_tokens, None); // 无 catalog，HOME 配置不保证存在
    }

    #[test]
    fn skips_zero_usage_legacy_rows_and_falls_back_to_request_model() {
        let db = temp_db();
        let base: i64 = 1_700_000_000_000;
        // 旧 anthropic 形 0 usage 行（T6 前遗留，最新但无 usage）
        insert(
            &db,
            "external-omp",
            "response",
            9,
            base + 90,
            json!({
                "model": "", "source": "anthropic_response",
                "usage": {"input_tokens": 0, "output_tokens": 0}
            }),
        );
        // 更早的真实 usage 响应（响应侧 model 为空）
        insert(
            &db,
            "external-omp",
            "response",
            5,
            base + 50,
            json!({
                "model": "", "source": "openai_response",
                "usage": {"input_tokens": 100, "output_tokens": 5}
            }),
        );
        insert(
            &db,
            "external-omp",
            "system_prompt",
            1,
            base,
            json!({"model": "glm-5.2"}),
        );

        let info = derive_session_context(&db.conn, "external-omp", None).unwrap();
        assert_eq!(info.used_tokens, 105);
        assert_eq!(info.model, "glm-5.2"); // 响应侧空 → 请求侧回落
    }

    #[test]
    fn no_usage_rows_yields_none() {
        let db = temp_db();
        insert(
            &db,
            "external-omp",
            "user_prompt",
            1,
            1_700_000_000_000,
            json!({"model": "glm-5.2"}),
        );
        assert!(derive_session_context(&db.conn, "external-omp", None).is_none());
    }

    #[test]
    fn omp_yaml_window_lookup() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = tmp.path().join("models.yml");
        std::fs::write(
            &cfg,
            "providers:\n  zap:\n    models:\n      - id: glm-5.2\n        contextWindow: 131072\n      - id: glm-5-turbo\n        contextWindow: 131072\n",
        )
        .unwrap();
        assert_eq!(window_from_omp_config(&cfg, "glm-5.2"), Some(131072));
        assert_eq!(window_from_omp_config(&cfg, "dais/glm-5.2"), Some(131072));
        assert_eq!(window_from_omp_config(&cfg, "nope"), None);
        // 未声明 contextWindow 的条目 → None
        let cfg2 = tmp.path().join("models2.yml");
        std::fs::write(
            &cfg2,
            "providers:\n  zap:\n    models:\n      - id: glm-5.2\n",
        )
        .unwrap();
        assert_eq!(window_from_omp_config(&cfg2, "glm-5.2"), None);
    }

    #[test]
    fn pi_json_window_lookup() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = tmp.path().join("models.json");
        std::fs::write(
            &cfg,
            r#"{"providers": {"zap": {"models": [{"id": "glm-5.2", "contextWindow": 1000000}]}}}"#,
        )
        .unwrap();
        assert_eq!(window_from_pi_config(&cfg, "glm-5.2"), Some(1_000_000));
        assert_eq!(window_from_pi_config(&cfg, "dais/glm-5.2"), Some(1_000_000));
        assert_eq!(window_from_pi_config(&cfg, "nope"), None);
    }

    #[test]
    fn catalog_window_requires_unanimity() {
        let mut catalog = models_dev::Catalog::new();
        let mut provider_a = models_dev::Provider::default();
        provider_a.id = "alpha".into();
        provider_a.models.insert(
            "glm-5.2".into(),
            models_dev::Model {
                limit: models_dev::ModelLimit {
                    context: 131072,
                    output: 0,
                },
                ..Default::default()
            },
        );
        catalog.insert("alpha".into(), provider_a);
        // 一致 → 采信
        assert_eq!(window_from_catalog(&catalog, "glm-5.2"), Some(131072));
        // provider/model 全名匹配
        assert_eq!(window_from_catalog(&catalog, "alpha/glm-5.2"), Some(131072));
        // 第二个 provider 同名不同窗口 → 歧义 None
        let mut provider_b = models_dev::Provider::default();
        provider_b.id = "beta".into();
        provider_b.models.insert(
            "glm-5.2".into(),
            models_dev::Model {
                limit: models_dev::ModelLimit {
                    context: 1_000_000,
                    output: 0,
                },
                ..Default::default()
            },
        );
        catalog.insert("beta".into(), provider_b);
        assert_eq!(window_from_catalog(&catalog, "glm-5.2"), None);
        // 未命中 → None
        assert_eq!(window_from_catalog(&catalog, "glm-9.9"), None);
    }

    #[test]
    fn harness_prefix_classification() {
        assert_eq!(
            harness_from_session("external-omp", ""),
            Some(ExternalHarness::Omp)
        );
        assert_eq!(
            harness_from_session("external-omp-1755439103-1234", ""),
            Some(ExternalHarness::Omp)
        );
        assert_eq!(
            harness_from_session("external-pi", ""),
            Some(ExternalHarness::Pi)
        );
        assert_eq!(
            harness_from_session("external-cc", ""),
            Some(ExternalHarness::Cc)
        );
        // GUI 拦截路径: session 名无前缀,靠 harness_type
        assert_eq!(
            harness_from_session("harness-abc", "omp"),
            Some(ExternalHarness::Omp)
        );
        assert_eq!(
            harness_from_session("harness-abc", "pi"),
            Some(ExternalHarness::Pi)
        );
        assert_eq!(
            harness_from_session("harness-abc", "claude-code"),
            Some(ExternalHarness::Cc)
        );
        assert_eq!(harness_from_session("harness-abc", "claude"), None);
        assert_eq!(harness_from_session("", ""), None);
    }
}
