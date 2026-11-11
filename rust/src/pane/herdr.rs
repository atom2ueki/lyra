use anyhow::{Context, Result, bail};
use serde_json::Value;
use std::process::Command;

use super::{Capabilities, PaneBackend, PaneHandle, SplitOptions, StartAgentOptions, WaitResult};

#[derive(Default)]
pub struct HerdrBackend;

impl HerdrBackend {
    pub fn new() -> Self {
        Self
    }

    fn bin(&self) -> String {
        std::env::var("LYRA_HERDR_BIN").unwrap_or_else(|_| "herdr".to_string())
    }

    fn run(&self, args: &[&str]) -> Result<String> {
        let output = Command::new(self.bin())
            .args(args)
            .output()
            .with_context(|| format!("run herdr {}", args.join(" ")))?;
        if !output.status.success() {
            bail!(
                "herdr {} failed: {}",
                args.join(" "),
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    fn json(&self, args: &[&str]) -> Result<Value> {
        let text = self.run(args)?;
        serde_json::from_str(&text)
            .with_context(|| format!("parse herdr {} output", args.join(" ")))
    }

    fn pane_from_value(value: &Value) -> Result<PaneHandle> {
        let pane = value
            .get("result")
            .and_then(|result| result.get("pane"))
            .or_else(|| value.get("pane"))
            .unwrap_or(value);
        let pane_id = pane
            .get("pane_id")
            .or_else(|| pane.get("id"))
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("herdr returned a pane without pane_id"))?;
        Ok(PaneHandle {
            backend: "herdr".to_string(),
            workspace: pane_id
                .split_once(':')
                .map(|(workspace, _)| workspace.to_string()),
            tab: pane
                .get("tab_id")
                .and_then(Value::as_str)
                .map(str::to_string),
            pane: pane_id.to_string(),
            agent: pane
                .get("agent")
                .and_then(Value::as_str)
                .map(str::to_string),
            session_file: pane
                .get("agent_session")
                .and_then(|session| session.get("value"))
                .and_then(Value::as_str)
                .map(str::to_string),
            machine: None,
        })
    }
}

impl PaneBackend for HerdrBackend {
    fn name(&self) -> &str {
        "herdr"
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            workspaces: true,
            tabs: true,
            splits: true,
            run_command: true,
            start_agent: true,
            send_text: true,
            send_keys: true,
            read_output: true,
            wait_for_output: true,
            agent_state: true,
            session_identity: true,
            focus: true,
            resize: false,
            close: true,
        }
    }

    fn anchor(&self) -> Result<PaneHandle> {
        if let Ok(pane) = std::env::var("HERDR_PANE_ID")
            && !pane.trim().is_empty()
        {
            return Ok(PaneHandle {
                backend: "herdr".to_string(),
                pane: pane.trim().to_string(),
                ..Default::default()
            });
        }
        let value = self.json(&["pane", "current", "--current"])?;
        Self::pane_from_value(&value)
    }

    fn split(&self, anchor: &PaneHandle, options: &SplitOptions) -> Result<PaneHandle> {
        let mut args = vec![
            "pane".to_string(),
            "split".to_string(),
            "--pane".to_string(),
            anchor.pane.clone(),
            "--direction".to_string(),
            options.direction.as_str().to_string(),
            "--cwd".to_string(),
            options.cwd.display().to_string(),
            "--no-focus".to_string(),
        ];
        for (key, value) in &options.env {
            args.push("--env".to_string());
            args.push(format!("{key}={value}"));
        }
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        let value = self.json(&refs)?;
        Self::pane_from_value(&value)
    }

    fn start_agent(&self, pane: &PaneHandle, options: &StartAgentOptions) -> Result<()> {
        let name = options
            .name
            .clone()
            .unwrap_or_else(|| format!("worker-{}", pane.pane));
        let mut args = vec![
            "agent".to_string(),
            "start".to_string(),
            name,
            "--kind".to_string(),
            options.kind.clone(),
            "--pane".to_string(),
            pane.pane.clone(),
            "--".to_string(),
        ];
        args.extend(options.args.iter().cloned());
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        self.run(&refs)?;
        Ok(())
    }

    fn send_text(&self, pane: &PaneHandle, text: &str, submit: bool) -> Result<()> {
        if submit && let Some(agent) = &pane.agent {
            self.run(&["agent", "prompt", agent, text])?;
            return Ok(());
        }
        self.run(&["pane", "send-text", &pane.pane, text])?;
        if submit {
            self.send_keys(pane, &["enter".to_string()])?;
        }
        Ok(())
    }

    fn send_keys(&self, pane: &PaneHandle, keys: &[String]) -> Result<()> {
        let mut args = if let Some(agent) = &pane.agent {
            vec!["agent".to_string(), "send-keys".to_string(), agent.clone()]
        } else {
            vec![
                "pane".to_string(),
                "send-keys".to_string(),
                pane.pane.clone(),
            ]
        };
        args.extend(keys.iter().cloned());
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        self.run(&refs)?;
        Ok(())
    }

    fn read(&self, pane: &PaneHandle, source: &str, lines: usize) -> Result<String> {
        let mut args: Vec<&str> = if let Some(agent) = pane.agent.as_deref() {
            vec!["agent", "read", agent]
        } else {
            vec!["pane", "read", pane.pane.as_str()]
        };
        let lines = lines.to_string();
        args.extend(["--source", source, "--lines", &lines]);
        self.run(&args)
    }

    fn wait(
        &self,
        pane: &PaneHandle,
        state: Option<&str>,
        match_text: Option<&str>,
        timeout_ms: u64,
    ) -> Result<WaitResult> {
        let timeout = timeout_ms.to_string();
        let result = if let Some(state) = state {
            let target = pane.agent.as_deref().unwrap_or(&pane.pane);
            self.run(&[
                "agent",
                "wait",
                target,
                "--until",
                state,
                "--timeout",
                &timeout,
            ])
        } else if let Some(match_text) = match_text {
            self.run(&[
                "pane",
                "wait-output",
                &pane.pane,
                "--match",
                match_text,
                "--timeout",
                &timeout,
            ])
        } else {
            bail!("herdr wait needs state or match")
        };
        Ok(match result {
            Ok(output) => WaitResult {
                matched: true,
                state: state.map(str::to_string),
                output: Some(output),
                timed_out: false,
            },
            Err(_) => WaitResult {
                matched: false,
                state: state.map(str::to_string),
                output: None,
                timed_out: true,
            },
        })
    }

    fn close(&self, pane: &PaneHandle) -> Result<()> {
        self.run(&["pane", "close", &pane.pane])?;
        Ok(())
    }

    fn list(&self) -> Result<Vec<PaneHandle>> {
        let value = self.json(&["agent", "list"])?;
        let agents = value
            .get("result")
            .and_then(|result| result.get("agents"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let mut panes = Vec::new();
        for agent in agents {
            if let Ok(mut pane) = Self::pane_from_value(&agent) {
                if let Some(status) = agent
                    .get("agent_status")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                {
                    pane.machine = Some(status);
                }
                panes.push(pane);
            }
        }
        Ok(panes)
    }
}
