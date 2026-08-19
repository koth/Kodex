//! Generate/merge the dsh `settings.yaml` document from Kodex's BYOK provider
//! catalog before spawning `dsh web`.
//!
//! Kodex owns two settings sections — `llm-pi-ai` (the LLM provider routes)
//! and `agent-default-model` (the default route+model for new sessions) —
//! and rewrites them on each bring-up from its current provider catalog.
//! Other sections (`ui-onboarding`, `agent-presets`, anything the user
//! hand-edited under other namespaces) are preserved by a YAML round-trip
//! that replaces only those two top-level keys.
//!
//! See `design-dsh-settings.md` for the full rationale.

use anyhow::{Context, anyhow};
use serde::Serialize;
use serde_json::Value;
use std::path::{Path, PathBuf};

/// The env-var name Kodex injects for a provider's API key. dsh reads it via
/// the `apiKeyEnv` credential ref in `settings.yaml`.
pub fn key_env_for_provider(provider_id: &str) -> String {
    format!(
        "KODEX_DSH_{}_KEY",
        provider_id.to_ascii_uppercase().replace('-', "_")
    )
}

/// One LLM provider route for dsh's `llm-pi-ai.providers` dict.
#[derive(Debug, Clone, Serialize)]
pub struct DshProviderRoute {
    /// Route key (also the dsh provider id). Kodex provider id, e.g.
    /// `deepseek`, `kimi`, `mimo`, `commandcode`, `timiai`, custom ids.
    pub id: String,
    /// `apiKeyEnv` — env var name Kodex injects at spawn. Never the secret.
    pub api_key_env: String,
    /// Wire protocol: `openai-completions` | `openai-responses` |
    /// `anthropic-messages`.
    pub api: String,
    /// Upstream provider endpoint.
    pub base_url: String,
    /// Models this route serves.
    pub models: Vec<DshModelEntry>,
    /// Optional display name.
    pub display_name: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DshModelEntry {
    pub id: String,
    pub name: String,
    pub context_window: i64,
    pub max_tokens: i64,
}

/// The default model selection for new dsh sessions.
#[derive(Debug, Clone, Serialize)]
pub struct DshDefaultModel {
    pub provider: String,
    pub model: String,
}

/// The full settings document Kodex writes/merges. Both sections are owned by
/// Kodex and replaced wholesale on each bring-up.
#[derive(Debug, Clone)]
pub struct DshSettingsConfig {
    pub providers: Vec<DshProviderRoute>,
    pub default_model: DshDefaultModel,
}

/// Read the existing `settings.yaml` (if any) as a JSON value, so we can
/// round-trip it while replacing only the two Kodex-owned sections. Returns
/// an empty mapping when the file does not exist.
fn read_existing(path: &Path) -> anyhow::Result<Value> {
    if !path.exists() {
        return Ok(Value::Object(serde_json::Map::new()));
    }
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read dsh settings {}", path.display()))?;
    if text.trim().is_empty() {
        return Ok(Value::Object(serde_json::Map::new()));
    }
    let yaml: Value = serde_yaml::from_str(&text).context("failed to parse dsh settings.yaml")?;
    Ok(yaml)
}

/// Build the `llm-pi-ai` section value from the provider routes.
fn build_llm_section(providers: &[DshProviderRoute]) -> Value {
    let mut routes = serde_json::Map::new();
    for route in providers {
        let mut entry = serde_json::Map::new();
        entry.insert("apiKeyEnv".into(), Value::String(route.api_key_env.clone()));
        entry.insert("api".into(), Value::String(route.api.clone()));
        entry.insert("baseURL".into(), Value::String(route.base_url.clone()));
        if let Some(name) = &route.display_name {
            entry.insert("displayName".into(), Value::String(name.clone()));
        }
        let mut models = Vec::with_capacity(route.models.len());
        for model in &route.models {
            let mut m = serde_json::Map::new();
            m.insert("id".into(), Value::String(model.id.clone()));
            m.insert("name".into(), Value::String(model.name.clone()));
            m.insert("contextWindow".into(), Value::Number(model.context_window.into()));
            m.insert("maxTokens".into(), Value::Number(model.max_tokens.into()));
            models.push(Value::Object(m));
        }
        entry.insert("models".into(), Value::Array(models));
        routes.insert(route.id.clone(), Value::Object(entry));
    }
    let mut llm = serde_json::Map::new();
    llm.insert("providers".into(), Value::Object(routes));
    Value::Object(llm)
}

/// Build the `agent-default-model` section value.
fn build_default_model_section(default: &DshDefaultModel) -> Value {
    let mut section = serde_json::Map::new();
    section.insert("provider".into(), Value::String(default.provider.clone()));
    section.insert("model".into(), Value::String(default.model.clone()));
    Value::Object(section)
}

/// Write the merged `settings.yaml` to `path`, replacing the `llm-pi-ai` and
/// `agent-default-model` sections and preserving all other top-level keys.
/// The parent directory is created if missing.
pub fn write_settings(path: &Path, config: &DshSettingsConfig) -> anyhow::Result<()> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create dsh settings dir {}", parent.display()))?;
    }
    let mut doc = read_existing(path)?;
    if !doc.is_object() {
        // A non-object root (e.g. a stray scalar) is replaced wholesale; the
        // document is expected to be a mapping.
        doc = Value::Object(serde_json::Map::new());
    }
    let obj = doc
        .as_object_mut()
        .ok_or_else(|| anyhow!("dsh settings.yaml root is not a mapping"))?;
    obj.insert("llm-pi-ai".into(), build_llm_section(&config.providers));
    obj.insert(
        "agent-default-model".into(),
        build_default_model_section(&config.default_model),
    );

    let yaml = serde_yaml::to_string(&doc).context("failed to serialize dsh settings.yaml")?;
    std::fs::write(path, yaml)
        .with_context(|| format!("failed to write dsh settings {}", path.display()))?;
    Ok(())
}

/// Resolve the dsh settings path for a Kodex data root. Convenience wrapper
/// mirroring `AppPaths::dsh_settings_path` (kept here so `dsh-bridge` tests
/// can exercise the generator without `app-core`).
pub fn settings_path_for_root(root: &Path) -> PathBuf {
    root.join("dsh").join("settings.yaml")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_route() -> DshProviderRoute {
        DshProviderRoute {
            id: "deepseek".into(),
            api_key_env: "KODEX_DSH_DEEPSEEK_KEY".into(),
            api: "openai-completions".into(),
            base_url: "https://api.deepseek.com/v1".into(),
            display_name: Some("DeepSeek".into()),
            models: vec![DshModelEntry {
                id: "deepseek-v4-pro".into(),
                name: "DeepSeek V4 Pro".into(),
                context_window: 1000000,
                max_tokens: 50000,
            }],
        }
    }

    #[test]
    fn key_env_name_is_uppercase_underscored() {
        assert_eq!(key_env_for_provider("deepseek"), "KODEX_DSH_DEEPSEEK_KEY");
        assert_eq!(key_env_for_provider("kimi-code"), "KODEX_DSH_KIMI_CODE_KEY");
    }

    #[test]
    fn write_creates_file_with_two_sections() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.yaml");
        let cfg = DshSettingsConfig {
            providers: vec![sample_route()],
            default_model: DshDefaultModel {
                provider: "deepseek".into(),
                model: "deepseek-v4-pro".into(),
            },
        };
        write_settings(&path, &cfg).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("llm-pi-ai:"));
        assert!(text.contains("apiKeyEnv: KODEX_DSH_DEEPSEEK_KEY"));
        assert!(text.contains("baseURL: https://api.deepseek.com/v1"));
        assert!(text.contains("agent-default-model:"));
        assert!(text.contains("provider: deepseek"));
    }

    #[test]
    fn write_preserves_other_sections() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.yaml");
        std::fs::write(
            &path,
            "ui-onboarding:\n  welcomeNoticeVersion: '1'\nagent-presets:\n  default: code\n",
        )
        .unwrap();
        let cfg = DshSettingsConfig {
            providers: vec![sample_route()],
            default_model: DshDefaultModel {
                provider: "deepseek".into(),
                model: "deepseek-v4-pro".into(),
            },
        };
        write_settings(&path, &cfg).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("ui-onboarding:"));
        assert!(text.contains("welcomeNoticeVersion"));
        assert!(text.contains("agent-presets:"));
        assert!(text.contains("llm-pi-ai:"));
    }

    #[test]
    fn write_replaces_owned_sections_on_second_run() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.yaml");
        let cfg = DshSettingsConfig {
            providers: vec![sample_route()],
            default_model: DshDefaultModel {
                provider: "deepseek".into(),
                model: "deepseek-v4-pro".into(),
            },
        };
        write_settings(&path, &cfg).unwrap();
        // Second run with a different provider set.
        let cfg2 = DshSettingsConfig {
            providers: vec![DshProviderRoute {
                id: "kimi".into(),
                api_key_env: "KODEX_DSH_KIMI_KEY".into(),
                api: "openai-completions".into(),
                base_url: "https://api.kimi.com/v1".into(),
                display_name: None,
                models: vec![],
            }],
            default_model: DshDefaultModel {
                provider: "kimi".into(),
                model: "kimi-k3".into(),
            },
        };
        write_settings(&path, &cfg2).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("kimi:"));
        assert!(text.contains("KODEX_DSH_KIMI_KEY"));
        // The old deepseek route is gone (the section is replaced wholesale).
        assert!(!text.contains("KODEX_DSH_DEEPSEEK_KEY"));
        assert!(text.contains("model: kimi-k3"));
    }
}
