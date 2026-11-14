use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::PathBuf;

pub const DEFAULT_CONFIG: &str = r#"# Lyra user config
max_parallel = 2
approval_posture = "ask"
merge = "auto"

[max_per_provider]
astralform = 2
"#;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ApprovalPosture {
    Safe,
    Ask,
    Unsafe,
}

impl ApprovalPosture {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "safe" => Some(Self::Safe),
            "ask" => Some(Self::Ask),
            "unsafe" => Some(Self::Unsafe),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MergeMode {
    Auto,
    Manual,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum WorkerLayout {
    Pane,
    Tab,
    #[default]
    Auto,
}

impl WorkerLayout {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "pane" => Some(Self::Pane),
            "tab" => Some(Self::Tab),
            "auto" => Some(Self::Auto),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct LyraConfig {
    pub max_parallel: u32,
    pub max_per_provider: HashMap<String, u32>,
    pub approval_posture: ApprovalPosture,
    pub merge: MergeMode,
    pub pane_backend: Option<String>,
    pub worker_layout: WorkerLayout,
}
#[derive(Debug, Deserialize)]
struct RawConfig {
    max_parallel: Option<u32>,
    max_per_provider: Option<HashMap<String, u32>>,
    approval_posture: Option<String>,
    merge: Option<String>,
    pane_backend: Option<String>,
    worker_layout: Option<String>,
}

impl Default for LyraConfig {
    fn default() -> Self {
        Self {
            max_parallel: 2,
            max_per_provider: HashMap::new(),
            approval_posture: ApprovalPosture::Ask,
            merge: MergeMode::Auto,
            pane_backend: None,
            worker_layout: WorkerLayout::Auto,
        }
    }
}

pub fn lyra_home() -> PathBuf {
    if let Ok(value) = env::var("LYRA_HOME")
        && !value.trim().is_empty()
    {
        return PathBuf::from(value);
    }
    let home = env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".lyra")
}

pub fn config_path() -> PathBuf {
    lyra_home().join("config.toml")
}

pub fn ensure_config() -> Result<PathBuf> {
    let path = config_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    if !path.exists() {
        fs::write(&path, DEFAULT_CONFIG).with_context(|| format!("write {}", path.display()))?;
    }
    Ok(path)
}

pub fn load_config() -> Result<LyraConfig> {
    let path = config_path();
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => DEFAULT_CONFIG.to_string(),
        Err(error) => return Err(error).with_context(|| format!("read {}", path.display())),
    };
    let raw: RawConfig =
        toml::from_str(&text).with_context(|| format!("parse {}", path.display()))?;
    let max_parallel = raw.max_parallel.unwrap_or(2);
    if max_parallel == 0 {
        anyhow::bail!("{}: max_parallel must be positive", path.display());
    }
    let approval_posture = match raw.approval_posture.as_deref() {
        None => ApprovalPosture::Ask,
        Some(value) => ApprovalPosture::parse(value).ok_or_else(|| {
            anyhow::anyhow!(
                "{}: approval_posture must be safe|ask|unsafe",
                path.display()
            )
        })?,
    };
    let merge = match raw.merge.as_deref() {
        Some("manual") => MergeMode::Manual,
        _ => MergeMode::Auto,
    };
    let pane_backend = match raw.pane_backend.as_deref() {
        None => None,
        Some(value) => {
            if PANE_BACKENDS.contains(&value) {
                Some(value.to_string())
            } else {
                anyhow::bail!(
                    "{}: pane_backend must be {}",
                    path.display(),
                    PANE_BACKENDS.join("|")
                );
            }
        }
    };
    let worker_layout = match raw.worker_layout.as_deref() {
        None => WorkerLayout::default(),
        Some(value) => WorkerLayout::parse(value).ok_or_else(|| {
            anyhow::anyhow!("{}: worker_layout must be pane|tab|auto", path.display())
        })?,
    };
    Ok(LyraConfig {
        max_parallel,
        max_per_provider: raw.max_per_provider.unwrap_or_default(),
        approval_posture,
        merge,
        pane_backend,
        worker_layout,
    })
}

pub const PANE_BACKENDS: [&str; 3] = ["herdr", "ghostty", "headless"];

/// Persist the pane backend choice in the user config, preserving comments and
/// unknown keys by upserting a single top-level line.
pub fn set_pane_backend(name: &str) -> Result<PathBuf> {
    if !PANE_BACKENDS.contains(&name) {
        anyhow::bail!("pane_backend must be {}", PANE_BACKENDS.join("|"));
    }
    let path = ensure_config()?;
    let text = fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    let line = format!("pane_backend = \"{name}\"");
    let mut replaced = false;
    let mut lines: Vec<String> = Vec::new();
    let mut inserted = false;
    for existing in text.lines() {
        let trimmed = existing.trim_start();
        if !inserted
            && trimmed.starts_with('[')
            && !replaced
            && !text
                .lines()
                .any(|line| line.trim_start().starts_with("pane_backend"))
        {
            lines.push(line.clone());
            inserted = true;
        }
        if trimmed.starts_with("pane_backend") {
            lines.push(line.clone());
            replaced = true;
        } else {
            lines.push(existing.to_string());
        }
    }
    if !replaced && !inserted {
        lines.push(line);
    }
    let mut updated = lines.join("\n");
    if !updated.ends_with('\n') {
        updated.push('\n');
    }
    fs::write(&path, &updated).with_context(|| format!("write {}", path.display()))?;
    Ok(path)
}
