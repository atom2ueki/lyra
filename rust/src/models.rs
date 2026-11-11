use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use serde_yaml::Value as YamlValue;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::config::lyra_home;

pub const DEFAULT_MODEL_POLICY: &str = r#"classes:
  architecture:
    - astralform/deepseek-v4.1-flash:max
    - zai/glm-5.3:high
  implementation:
    - astralform/deepseek-v4.1-flash:high
    - xiaomi-token-plan-cn/mimo-v2.6-pro:high
  review:
    - zai/glm-5.3:high
    - astralform/deepseek-v4.1-flash:max
  search:
    - astralform/ornith-1.5-35b-a3b:auto
    - minimax-code-cn/MiniMax-M3.1-Flash:max
roles:
  smol:
    - astralform/ornith-1.5-35b-a3b:auto
    - minimax-code-cn/MiniMax-M3.1-Flash:max
  slow:
    - zai/glm-5.3:high
    - astralform/deepseek-v4.1-flash:max
  plan:
    - zai/glm-5.3:high
    - xiaomi-token-plan-cn/mimo-v2.6-pro:high
limits:
  providers:
    astralform: 2
"#;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelPolicy {
    pub classes: HashMap<String, Vec<String>>,
    pub roles: HashMap<String, Vec<String>>,
    pub limits: ModelLimits,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelLimits {
    pub providers: HashMap<String, u32>,
}

impl Default for ModelPolicy {
    fn default() -> Self {
        parse_policy(
            serde_yaml::from_str(DEFAULT_MODEL_POLICY).expect("default model policy"),
        )
        .expect("default model policy")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelInfo {
    pub provider: String,
    pub id: String,
    pub selector: String,
    #[serde(default)]
    pub context_window: Option<u64>,
    #[serde(default)]
    pub max_tokens: Option<u64>,
    #[serde(default)]
    pub reasoning: Option<bool>,
    #[serde(default)]
    pub input: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct UsageReport {
    pub provider: String,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub limits: Vec<UsageLimit>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct UsageLimit {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub amount: Option<UsageAmount>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct UsageAmount {
    #[serde(default)]
    pub remaining_fraction: Option<f64>,
    #[serde(default)]
    pub used_fraction: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ModelStat {
    pub provider: String,
    pub model: String,
    #[serde(default)]
    pub total_requests: Option<u64>,
    #[serde(default)]
    pub total_cost: Option<f64>,
    #[serde(default)]
    pub avg_ttft: Option<f64>,
    #[serde(default)]
    pub avg_tokens_per_second: Option<f64>,
    #[serde(default)]
    pub error_rate: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelChoice {
    pub selector: String,
    pub provider: String,
    pub model_class: String,
    pub reason: String,
}

pub fn model_policy_path() -> PathBuf {
    lyra_home().join("models.policy.yaml")
}

pub fn ensure_model_policy() -> Result<PathBuf> {
    let path = model_policy_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("create {}", parent.display()))?;
    }
    if !path.exists() {
        fs::write(&path, DEFAULT_MODEL_POLICY)
            .with_context(|| format!("write {}", path.display()))?;
    }
    Ok(path)
}

fn candidates(value: &YamlValue) -> Vec<String> {
    if let Some(sequence) = value.as_sequence() {
        return sequence
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect();
    }
    if let Some(map) = value.as_mapping() {
        if let Some(sequence) = map.get("candidates").and_then(YamlValue::as_sequence) {
            return sequence
                .iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect();
        }
    }
    Vec::new()
}

pub fn parse_policy(value: YamlValue) -> Result<ModelPolicy> {
    let map = value
        .as_mapping()
        .ok_or_else(|| anyhow::anyhow!("model policy must be a mapping"))?;
    let mut classes = HashMap::new();
    if let Some(entries) = map.get("classes").and_then(YamlValue::as_mapping) {
        for (key, value) in entries {
            if let Some(name) = key.as_str() {
                let values = candidates(value);
                if !values.is_empty() {
                    classes.insert(name.to_string(), values);
                }
            }
        }
    }
    let mut roles = HashMap::new();
    if let Some(entries) = map.get("roles").and_then(YamlValue::as_mapping) {
        for (key, value) in entries {
            if let Some(name) = key.as_str() {
                let values = candidates(value);
                if !values.is_empty() {
                    roles.insert(name.to_string(), values);
                }
            }
        }
    }
    let mut providers = HashMap::new();
    if let Some(entries) = map
        .get("limits")
        .and_then(|v| v.as_mapping())
        .and_then(|v| v.get("providers"))
        .and_then(YamlValue::as_mapping)
    {
        for (key, value) in entries {
            if let (Some(name), Some(number)) = (key.as_str(), value.as_u64()) {
                providers.insert(name.to_string(), number as u32);
            }
        }
    }
    Ok(ModelPolicy {
        classes,
        roles,
        limits: ModelLimits { providers },
    })
}

pub fn load_policy() -> Result<ModelPolicy> {
    let path = model_policy_path();
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            DEFAULT_MODEL_POLICY.to_string()
        }
        Err(error) => return Err(error).with_context(|| format!("read {}", path.display())),
    };
    let value: YamlValue =
        serde_yaml::from_str(&text).with_context(|| format!("parse {}", path.display()))?;
    parse_policy(value)
}

const LEVEL_SUFFIXES: &[&str] = &[
    "off", "minimal", "low", "medium", "high", "xhigh", "max", "auto",
];

pub fn model_key(selector: &str) -> String {
    if let Some((base, level)) = selector.rsplit_once(':') {
        if LEVEL_SUFFIXES.contains(&level) {
            return base.to_string();
        }
    }
    selector.to_string()
}

pub fn provider_of(selector: &str) -> String {
    let key = model_key(selector);
    key.split_once('/')
        .map(|(provider, _)| provider.to_string())
        .unwrap_or(key)
}

pub fn provider_exhausted(usage: &[UsageReport], provider: &str) -> bool {
    usage.iter().filter(|report| report.provider == provider).any(|report| {
        report.limits.iter().any(|limit| {
            let status = limit.status.as_deref().or(report.status.as_deref());
            matches!(status, Some("exhausted" | "blocked" | "limit_reached"))
                || limit
                    .amount
                    .as_ref()
                    .and_then(|amount| amount.remaining_fraction)
                    .is_some_and(|fraction| fraction <= 0.0)
        })
    })
}

pub fn cap_for(
    provider: &str,
    policy: &ModelPolicy,
    config_caps: &HashMap<String, u32>,
) -> u32 {
    let configured = config_caps.get(provider).copied();
    let policy_cap = policy.limits.providers.get(provider).copied();
    match (configured, policy_cap) {
        (Some(a), Some(b)) => a.min(b),
        (Some(value), None) | (None, Some(value)) => value,
        (None, None) => u32::MAX,
    }
}

pub fn select_model(
    policy: &ModelPolicy,
    models: &[ModelInfo],
    usage: &[UsageReport],
    model_class: Option<&str>,
    ticket_model: Option<&str>,
    active_by_provider: &HashMap<String, u32>,
    config_caps: &HashMap<String, u32>,
) -> Option<ModelChoice> {
    if let Some(selector) = ticket_model {
        return Some(ModelChoice {
            selector: selector.to_string(),
            provider: provider_of(selector),
            model_class: model_class.unwrap_or("ticket").to_string(),
            reason: "explicit ticket model".to_string(),
        });
    }
    let class_name = model_class
        .filter(|name| policy.classes.contains_key(*name))
        .map(str::to_string)
        .or_else(|| {
            if policy.classes.contains_key("default") {
                Some("default".to_string())
            } else {
                policy.classes.keys().next().cloned()
            }
        })?;
    let known: HashSet<String> = models
        .iter()
        .map(|model| model_key(&model.selector))
        .collect();
    for candidate in policy.classes.get(&class_name)? {
        let provider = provider_of(candidate);
        let key = model_key(candidate);
        if !known.contains(&key) {
            continue;
        }
        if active_by_provider.get(&provider).copied().unwrap_or(0)
            >= cap_for(&provider, policy, config_caps)
        {
            continue;
        }
        if provider_exhausted(usage, &provider) {
            continue;
        }
        return Some(ModelChoice {
            selector: candidate.clone(),
            provider,
            model_class: class_name.clone(),
            reason: format!("policy class {class_name}"),
        });
    }
    None
}

pub fn subagent_roles(policy: &ModelPolicy, selected: &str) -> HashMap<String, String> {
    let mut roles = HashMap::new();
    roles.insert("task".to_string(), selected.to_string());
    for name in ["smol", "slow", "plan"] {
        let value = policy
            .roles
            .get(name)
            .and_then(|values| values.first())
            .cloned()
            .unwrap_or_else(|| selected.to_string());
        roles.insert(name.to_string(), value);
    }
    roles
}

pub fn role_overlay(policy: &ModelPolicy, selected: &str) -> YamlValue {
    let mut roles = serde_yaml::Mapping::new();
    roles.insert(
        YamlValue::String("default".to_string()),
        YamlValue::String(selected.to_string()),
    );
    for (name, value) in subagent_roles(policy, selected) {
        roles.insert(YamlValue::String(name), YamlValue::String(value));
    }
    let mut root = serde_yaml::Mapping::new();
    root.insert(
        YamlValue::String("modelRoles".to_string()),
        YamlValue::Mapping(roles),
    );
    YamlValue::Mapping(root)
}

pub fn write_role_overlay(path: &Path, policy: &ModelPolicy, selected: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("create {}", parent.display()))?;
    }
    let text = serde_yaml::to_string(&role_overlay(policy, selected))?;
    fs::write(path, text).with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

fn omp_bin() -> String {
    std::env::var("LYRA_OMP_BIN")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "omp".to_string())
}

fn json_command(args: &[&str]) -> Result<JsonValue> {
    let output = Command::new(omp_bin())
        .args(args)
        .output()
        .with_context(|| format!("run {} {}", omp_bin(), args.join(" ")))?;
    if !output.status.success() {
        anyhow::bail!(
            "{} {} failed: {}",
            omp_bin(),
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let trimmed = text.trim();
    let start = trimmed
        .char_indices()
        .find(|(_, ch)| *ch == '{' || *ch == '[')
        .map(|(index, _)| index)
        .ok_or_else(|| anyhow::anyhow!("{} {} returned no JSON", omp_bin(), args.join(" ")))?;
    serde_json::from_str(&trimmed[start..])
        .with_context(|| format!("parse {} {} output", omp_bin(), args.join(" ")))
}

pub fn load_model_catalog() -> Result<Vec<ModelInfo>> {
    let raw = json_command(&["models", "ls", "--json"])?;
    let list = match raw {
        JsonValue::Array(list) => list,
        JsonValue::Object(object) => object
            .get("models")
            .and_then(JsonValue::as_array)
            .cloned()
            .unwrap_or_default(),
        _ => Vec::new(),
    };
    if list.is_empty() {
        anyhow::bail!("omp models ls returned no models");
    }
    let mut models = Vec::new();
    for value in list {
        if let Ok(model) = serde_json::from_value::<ModelInfo>(value) {
            models.push(model);
        }
    }
    Ok(models)
}

pub fn load_usage() -> Result<Vec<UsageReport>> {
    let raw = json_command(&["usage", "--json"])?;
    let reports = raw
        .get("reports")
        .and_then(JsonValue::as_array)
        .cloned()
        .unwrap_or_default();
    let mut usage = Vec::new();
    for value in reports {
        if let Ok(report) = serde_json::from_value::<UsageReport>(value) {
            usage.push(report);
        }
    }
    Ok(usage)
}

pub fn load_stats() -> Result<Vec<ModelStat>> {
    let raw = json_command(&["stats", "--json"])?;
    let stats = raw
        .get("byModel")
        .and_then(JsonValue::as_array)
        .cloned()
        .unwrap_or_default();
    let mut result = Vec::new();
    for value in stats {
        if let Ok(stat) = serde_json::from_value::<ModelStat>(value) {
            result.push(stat);
        }
    }
    Ok(result)
}

pub fn active_providers(tickets: &[crate::schema::Ticket], run_running: &HashSet<String>) -> HashMap<String, u32> {
    let mut counts = HashMap::new();
    for ticket in tickets {
        if !run_running.contains(&ticket.id) {
            continue;
        }
        if let Some(model) = &ticket.model {
            *counts.entry(provider_of(model)).or_insert(0) += 1;
        }
    }
    counts
}
