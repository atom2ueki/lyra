use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

pub const PROJECT_CONFIG_FILE: &str = "lyra.toml";

pub const TEMPLATE: &str = r#"# Lyra 0.7 project config
[project]
base_ref = "main"
max_parallel = 2

# The worker command runs inside the ticket's worktree.
# `{task}` is replaced with the generated TASK.md path; if omitted, Lyra
# appends the path as the last argument.
[worker]
build = ["omp", "--approval-mode", "yolo"]
# plan = ["omp", "--approval-mode", "yolo"]   # optional: enables the plan gate

# Optional environment passed to every worker:
# [worker.env]
# DATABASE_URL = "postgres://localhost/app"
"#;

#[derive(Debug, Clone, Deserialize, Default)]
pub struct ProjectConfig {
    #[serde(default)]
    pub project: ProjectSection,
    #[serde(default)]
    pub worker: WorkerSection,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct ProjectSection {
    #[serde(default)]
    pub base_ref: Option<String>,
    #[serde(default)]
    pub max_parallel: Option<u32>,
    #[serde(default)]
    pub max_seconds: Option<u64>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct WorkerSection {
    #[serde(default)]
    pub build: Vec<String>,
    #[serde(default)]
    pub plan: Option<Vec<String>>,
    #[serde(default)]
    pub env: HashMap<String, String>,
}

impl ProjectConfig {
    pub fn load(root: &Path) -> Result<Self> {
        let path = config_path(root);
        let text = fs::read_to_string(&path)
            .with_context(|| format!("read {} (run `lyra init` to create it)", path.display()))?;
        let config: Self =
            toml::from_str(&text).with_context(|| format!("parse {}", path.display()))?;
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<()> {
        if self.worker.build.is_empty() {
            bail!("[worker].build must contain a command");
        }
        if let Some(plan) = &self.worker.plan
            && plan.is_empty()
        {
            bail!("[worker].plan is empty; remove it or add a command");
        }
        Ok(())
    }

    pub fn max_parallel(&self) -> u32 {
        self.project.max_parallel.unwrap_or(2).max(1)
    }
}

pub fn config_path(root: &Path) -> PathBuf {
    root.join(PROJECT_CONFIG_FILE)
}

pub fn ensure_template(root: &Path) -> Result<PathBuf> {
    let path = config_path(root);
    if !path.exists() {
        fs::write(&path, TEMPLATE).with_context(|| format!("write {}", path.display()))?;
    }
    Ok(path)
}

/// Expand `{task}`, `{plan}`, `{worktree}`, `{ticket}` in every argument.
/// If the task placeholder is absent, append the task path as the last arg so
/// the common `build = ["omp"]` form still works.
pub fn expand_command(
    command: &[String],
    task: &Path,
    plan: Option<&Path>,
    worktree: &Path,
    ticket_id: &str,
) -> Vec<String> {
    let task_text = task.display().to_string();
    let plan_text = plan
        .map(|path| path.display().to_string())
        .unwrap_or_default();
    let worktree_text = worktree.display().to_string();
    let mut expanded = Vec::with_capacity(command.len() + 1);
    let mut saw_task = false;
    for arg in command {
        let mut value = arg
            .replace("{task}", &task_text)
            .replace("{plan}", &plan_text)
            .replace("{worktree}", &worktree_text)
            .replace("{ticket}", ticket_id);
        if arg.contains("{task}") {
            saw_task = true;
        }
        if value.is_empty() {
            continue;
        }
        value = value.replace("{plan}", &plan_text);
        expanded.push(value);
    }
    if !saw_task && !task_text.is_empty() {
        expanded.push(task_text);
    }
    expanded
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn template_parses_and_has_a_build_command() {
        let config: ProjectConfig = toml::from_str(TEMPLATE).unwrap();
        config.validate().unwrap();
        assert_eq!(config.max_parallel(), 2);
        assert_eq!(config.worker.build[0], "omp");
    }

    #[test]
    fn expand_keeps_placeholder_and_appends_when_missing() {
        let task = Path::new("/run/TASK.md");
        let worktree = Path::new("/run/wt");
        let with = expand_command(
            &["omp".to_string(), "@{task}".to_string()],
            task,
            None,
            worktree,
            "QuietHarbor",
        );
        assert_eq!(with, vec!["omp", "@/run/TASK.md"]);
        let without = expand_command(&["omp".to_string()], task, None, worktree, "QuietHarbor");
        assert_eq!(without, vec!["omp", "/run/TASK.md"]);
    }

    #[test]
    fn missing_build_is_rejected() {
        let config = ProjectConfig::default();
        assert!(config.validate().is_err());
    }
}
