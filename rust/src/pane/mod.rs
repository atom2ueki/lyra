use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

pub mod ghostty;
pub mod headless;
pub mod herdr;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SplitDirection {
    Right,
    Left,
    Down,
    Up,
}

impl SplitDirection {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Right => "right",
            Self::Left => "left",
            Self::Down => "down",
            Self::Up => "up",
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Capabilities {
    pub workspaces: bool,
    pub tabs: bool,
    pub splits: bool,
    pub run_command: bool,
    pub start_agent: bool,
    pub send_text: bool,
    pub send_keys: bool,
    pub read_output: bool,
    pub wait_for_output: bool,
    pub agent_state: bool,
    pub session_identity: bool,
    pub focus: bool,
    pub resize: bool,
    pub close: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct PaneHandle {
    pub backend: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tab: Option<String>,
    pub pane: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub machine: Option<String>,
}

#[derive(Debug, Clone)]
pub struct SplitOptions {
    pub direction: SplitDirection,
    pub cwd: PathBuf,
    pub command: Option<String>,
    pub env: HashMap<String, String>,
    pub log_dir: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct StartAgentOptions {
    pub kind: String,
    pub name: Option<String>,
    pub args: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct WaitResult {
    pub matched: bool,
    pub state: Option<String>,
    pub output: Option<String>,
    pub timed_out: bool,
}

pub trait PaneBackend {
    fn name(&self) -> &str;
    fn capabilities(&self) -> Capabilities;
    fn anchor(&self) -> Result<PaneHandle>;
    fn split(&self, anchor: &PaneHandle, options: &SplitOptions) -> Result<PaneHandle>;
    /// Open a tab and return its initial pane. Backends without tabs fall back
    /// to a split so callers can honor a layout preference without gating on
    /// Capabilities.tabs.
    fn open_tab(&self, anchor: &PaneHandle, options: &SplitOptions) -> Result<PaneHandle> {
        self.split(anchor, options)
    }
    /// Label a pane for the operator UI (tab or pane title). Best-effort:
    /// the default does nothing and callers ignore errors.
    fn label(&self, _pane: &PaneHandle, _label: &str) -> Result<()> {
        Ok(())
    }
    fn start_agent(&self, _pane: &PaneHandle, _options: &StartAgentOptions) -> Result<()> {
        bail!("backend cannot start agents")
    }
    fn send_text(&self, _pane: &PaneHandle, _text: &str, _submit: bool) -> Result<()> {
        bail!("backend cannot send text")
    }
    fn send_keys(&self, _pane: &PaneHandle, _keys: &[String]) -> Result<()> {
        bail!("backend cannot send keys")
    }
    fn read(&self, _pane: &PaneHandle, _source: &str, _lines: usize) -> Result<String> {
        bail!("backend cannot read output")
    }
    fn wait(
        &self,
        _pane: &PaneHandle,
        _state: Option<&str>,
        _match: Option<&str>,
        _timeout_ms: u64,
    ) -> Result<WaitResult> {
        bail!("backend cannot wait")
    }
    fn close(&self, _pane: &PaneHandle) -> Result<()> {
        bail!("backend cannot close panes")
    }
    fn list(&self) -> Result<Vec<PaneHandle>> {
        Ok(Vec::new())
    }
}

pub fn backend_by_name(name: &str) -> Result<Box<dyn PaneBackend>> {
    match name {
        "herdr" => Ok(Box::new(herdr::HerdrBackend::new())),
        "ghostty" => Ok(Box::new(ghostty::GhosttyBackend::new())),
        "headless" => Ok(Box::new(headless::HeadlessBackend::new())),
        other => bail!("unknown pane backend: {other}"),
    }
}

pub fn select_backend() -> Result<Box<dyn PaneBackend>> {
    let env = std::env::vars().collect::<HashMap<_, _>>();
    select_backend_with_env(&env, std::io::IsTerminal::is_terminal(&std::io::stdout()))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackendChoice {
    pub name: String,
    pub source: &'static str,
}

/// Resolve the pane backend: an explicit environment override wins, then the
/// persisted user config (`pane_backend` in config.toml, written by
/// `lyra setup`), then ambient terminal detection.
pub fn resolve_backend_with_env(
    env: &HashMap<String, String>,
    stdout_is_tty: bool,
) -> Result<BackendChoice> {
    if let Some(explicit) = env.get("LYRA_PANE_BACKEND") {
        return backend_by_name(explicit)
            .map(|_| BackendChoice {
                name: explicit.clone(),
                source: "env",
            })
            .map_err(|_| anyhow::anyhow!("unknown LYRA_PANE_BACKEND: {explicit}"));
    }
    if let Ok(config) = crate::config::load_config()
        && let Some(configured) = config.pane_backend
    {
        return backend_by_name(&configured).map(|_| BackendChoice {
            name: configured,
            source: "config",
        });
    }
    if env.get("HERDR_ENV").map(String::as_str) == Some("1") {
        return Ok(BackendChoice {
            name: "herdr".to_string(),
            source: "detected",
        });
    }
    if env.contains_key("TMUX") {
        bail!("tmux backend is not implemented yet; use herdr, Ghostty, or headless");
    }
    if env.contains_key("ZELLIJ") {
        bail!("Zellij backend is not implemented yet; use herdr, Ghostty, or headless");
    }
    if env.get("TERM_PROGRAM").map(String::as_str) == Some("ghostty")
        || env.contains_key("GHOSTTY_TERMINAL_ID")
    {
        return Ok(BackendChoice {
            name: "ghostty".to_string(),
            source: "detected",
        });
    }
    if !stdout_is_tty {
        return Ok(BackendChoice {
            name: "headless".to_string(),
            source: "detected",
        });
    }
    bail!(
        "no pane backend detected; run `lyra setup` or set LYRA_PANE_BACKEND=herdr|ghostty|headless"
    )
}

pub fn resolve_backend() -> Result<BackendChoice> {
    let env = std::env::vars().collect::<HashMap<_, _>>();
    resolve_backend_with_env(&env, std::io::IsTerminal::is_terminal(&std::io::stdout()))
}

pub fn select_backend_with_env(
    env: &HashMap<String, String>,
    stdout_is_tty: bool,
) -> Result<Box<dyn PaneBackend>> {
    let choice = resolve_backend_with_env(env, stdout_is_tty)?;
    backend_by_name(&choice.name)
}
