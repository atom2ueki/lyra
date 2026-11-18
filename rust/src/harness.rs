use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;

use crate::config::{self, LyraConfig};

pub const TEMPLATE_SCHEMA_VERSION: u32 = 1;

/// How a harness worker is launched into its pane.
#[derive(Debug, Clone, PartialEq)]
pub enum LaunchStyle {
    /// omp: the conductor builds the full argv itself (extension, approval
    /// mode, session dir) and starts it as herdr agent kind "omp".
    Omp,
    /// Pane TUI through `agent start --kind`: herdr recognizes the harness and
    /// tracks its lifecycle. Extra args (e.g. permission-bypass flags) are
    /// appended after `--`. The prompt is injected with send-text.
    Agent { kind: String, args: Vec<String> },
    /// Pane TUI herdr does not recognize: open a pane running the command and
    /// inject the prompt with send-text. Visible, but no agent state machine.
    Raw { command: Vec<String> },
}

#[derive(Debug, Clone, PartialEq)]
pub struct HarnessSpec {
    pub name: String,
    pub binary: String,
    pub launch: LaunchStyle,
}

impl HarnessSpec {
    /// Resolve the binary to probe/run: `LYRA_<NAME>_BIN` overrides the
    /// template (LYRA_OMP_BIN keeps its legacy spelling for omp).
    pub fn resolve_binary(&self) -> String {
        let env_name = match self.name.as_str() {
            "omp" => "LYRA_OMP_BIN".to_string(),
            other => format!("LYRA_{}_BIN", other.to_uppercase()),
        };
        std::env::var(&env_name)
            .ok()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| self.binary.clone())
    }
}

/// Built-in harness templates. Permission-bypass flags are only pre-filled
/// where they are verified; harnesses without one simply ask in their TUI and
/// the operator approves in the visible pane.
pub fn builtin_registry() -> Vec<HarnessSpec> {
    use LaunchStyle::{Agent, Omp, Raw};
    vec![
        HarnessSpec {
            name: "omp".to_string(),
            binary: "omp".to_string(),
            launch: Omp,
        },
        HarnessSpec {
            name: "dsh".to_string(),
            binary: "dsh".to_string(),
            launch: Raw {
                command: vec!["dsh".to_string(), "tui".to_string()],
            },
        },
        HarnessSpec {
            name: "claude".to_string(),
            binary: "claude".to_string(),
            launch: Agent {
                kind: "claude".to_string(),
                args: vec!["--dangerously-skip-permissions".to_string()],
            },
        },
        HarnessSpec {
            name: "codex".to_string(),
            binary: "codex".to_string(),
            launch: Agent {
                kind: "codex".to_string(),
                args: vec!["--dangerously-bypass-approvals-and-sandbox".to_string()],
            },
        },
        HarnessSpec {
            name: "gemini".to_string(),
            binary: "gemini".to_string(),
            launch: Agent {
                kind: "gemini".to_string(),
                args: vec!["--yolo".to_string()],
            },
        },
        HarnessSpec {
            name: "opencode".to_string(),
            binary: "opencode".to_string(),
            launch: Agent {
                kind: "opencode".to_string(),
                args: Vec::new(),
            },
        },
        HarnessSpec {
            name: "kimi".to_string(),
            binary: "kimi".to_string(),
            launch: Agent {
                kind: "kimi".to_string(),
                args: Vec::new(),
            },
        },
        HarnessSpec {
            name: "qwen".to_string(),
            binary: "qwen".to_string(),
            launch: Agent {
                kind: "qwen".to_string(),
                args: Vec::new(),
            },
        },
    ]
}

#[derive(Debug, Deserialize)]
struct RawTemplate {
    schema_version: u32,
    binary: String,
    #[serde(default)]
    style: Option<String>,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    args: Option<Vec<String>>,
    #[serde(default)]
    command: Option<Vec<String>>,
}

fn parse_template(name: &str, path: &PathBuf) -> Result<HarnessSpec> {
    let text = std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let raw: RawTemplate =
        toml::from_str(&text).with_context(|| format!("parse {}", path.display()))?;
    if raw.schema_version != TEMPLATE_SCHEMA_VERSION {
        bail!(
            "{}: schema_version must be {}",
            path.display(),
            TEMPLATE_SCHEMA_VERSION
        );
    }
    let launch = match raw.style.as_deref() {
        Some("agent") => LaunchStyle::Agent {
            kind: raw.kind.unwrap_or_else(|| name.to_string()),
            args: raw.args.unwrap_or_default(),
        },
        Some("raw") => LaunchStyle::Raw {
            command: raw.command.unwrap_or_else(|| vec![raw.binary.clone()]),
        },
        Some(other) => bail!("{}: style must be agent|raw, got {other}", path.display()),
        None => bail!("{}: style is required (agent|raw)", path.display()),
    };
    Ok(HarnessSpec {
        name: name.to_string(),
        binary: raw.binary,
        launch,
    })
}

/// Built-ins shadowed by user templates from `~/.lyra/harnesses.d/<name>.toml`.
pub fn load_registry() -> Result<Vec<HarnessSpec>> {
    let mut by_name: HashMap<String, HarnessSpec> = builtin_registry()
        .into_iter()
        .map(|spec| (spec.name.clone(), spec))
        .collect();
    let dir = config::lyra_home().join("harnesses.d");
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let mut specs: Vec<HarnessSpec> = by_name.into_values().collect();
            specs.sort_by(|a, b| a.name.cmp(&b.name));
            return Ok(specs);
        }
        Err(error) => return Err(error).with_context(|| format!("read {}", dir.display())),
    };
    for entry in entries {
        let path = entry?.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("toml") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
            continue;
        };
        by_name.insert(stem.to_string(), parse_template(stem, &path)?);
    }
    let mut specs: Vec<HarnessSpec> = by_name.into_values().collect();
    specs.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(specs)
}

pub fn resolve(name: &str) -> Result<HarnessSpec> {
    load_registry()?
        .into_iter()
        .find(|spec| spec.name == name)
        .ok_or_else(|| anyhow::anyhow!("unknown harness: {name}"))
}

/// Probe a harness binary. Returns the first line of `--version`, or None when
/// the binary is not usable.
pub fn probe(spec: &HarnessSpec) -> Option<String> {
    let output = Command::new(spec.resolve_binary())
        .arg("--version")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let first = text.lines().next().unwrap_or_default().trim().to_string();
    Some(first)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HarnessStatus {
    pub name: String,
    pub installed: bool,
    pub version: Option<String>,
    pub enabled: bool,
}

/// Machine facts crossed with the enabled whitelist: the single inventory used
/// by setup, doctor, and fail-fast validation.
pub fn inventory(config: &LyraConfig) -> Result<Vec<HarnessStatus>> {
    Ok(load_registry()?
        .into_iter()
        .map(|spec| {
            let version = probe(&spec);
            HarnessStatus {
                enabled: config.enabled_harnesses.contains(&spec.name),
                installed: version.is_some(),
                version,
                name: spec.name,
            }
        })
        .collect())
}

/// Harnesses that can actually run a ticket right now.
pub fn available(config: &LyraConfig) -> Result<Vec<String>> {
    Ok(inventory(config)?
        .into_iter()
        .filter(|status| status.installed && status.enabled)
        .map(|status| status.name)
        .collect())
}

/// Validate a ticket's harness before any side effect. Returns an actionable
/// error listing what is available.
pub fn validate(name: &str, config: &LyraConfig) -> Result<HarnessSpec> {
    let spec = resolve(name)?;
    let inventory = inventory(config)?;
    let status = inventory
        .iter()
        .find(|status| status.name == name)
        .ok_or_else(|| anyhow::anyhow!("unknown harness: {name}"))?;
    if !status.installed {
        bail!(
            "harness {name} is enabled but not installed (probe {} failed); available: {}",
            spec.resolve_binary(),
            available(config)?.join(", ")
        );
    }
    if !status.enabled {
        bail!(
            "harness {name} is not enabled; run `lyra setup` to enable it; available: {}",
            available(config)?.join(", ")
        );
    }
    Ok(spec)
}

#[cfg(test)]
mod tests {
    use super::*;

    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn builtin_registry_covers_omp_and_dsh() {
        let registry = builtin_registry();
        let omp = registry.iter().find(|spec| spec.name == "omp").unwrap();
        assert_eq!(omp.launch, LaunchStyle::Omp);
        let dsh = registry.iter().find(|spec| spec.name == "dsh").unwrap();
        assert_eq!(
            dsh.launch,
            LaunchStyle::Raw {
                command: vec!["dsh".to_string(), "tui".to_string()]
            }
        );
    }

    #[test]
    fn registry_can_be_shadowed_by_user_templates() {
        let _guard = ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("lyra-home");
        std::fs::create_dir_all(home.join("harnesses.d")).unwrap();
        std::fs::write(
            home.join("harnesses.d/dsh.toml"),
            "schema_version = 1\nbinary = \"dsh-nightly\"\nstyle = \"agent\"\nkind = \"dsh\"\nargs = [\"--flag\"]\n",
        )
        .unwrap();
        std::fs::write(
            home.join("harnesses.d/custom.toml"),
            "schema_version = 1\nbinary = \"my-agent\"\nstyle = \"raw\"\ncommand = [\"my-agent\", \"tui\"]\n",
        )
        .unwrap();
        std::fs::write(
            home.join("harnesses.d/invalid.toml"),
            "schema_version = 9\nbinary = \"x\"\nstyle = \"raw\"\n",
        )
        .unwrap();
        unsafe { std::env::set_var("LYRA_HOME", &home) };
        let error = load_registry().unwrap_err();
        assert!(format!("{error:#}").contains("schema_version"));

        std::fs::remove_file(home.join("harnesses.d/invalid.toml")).unwrap();
        let registry = load_registry().unwrap();
        let dsh = registry.iter().find(|spec| spec.name == "dsh").unwrap();
        assert_eq!(dsh.binary, "dsh-nightly");
        assert_eq!(
            dsh.launch,
            LaunchStyle::Agent {
                kind: "dsh".to_string(),
                args: vec!["--flag".to_string()]
            }
        );
        assert!(registry.iter().any(|spec| spec.name == "custom"));
    }
}
