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
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
            bail!(
                "herdr {} failed: {}",
                args.join(" "),
                match (stdout.is_empty(), stderr.is_empty()) {
                    (true, true) => "no output".to_string(),
                    (false, true) => stdout,
                    (true, false) => stderr,
                    (false, false) => format!("{stdout} | {stderr}"),
                }
            );
        }
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    fn json(&self, args: &[&str]) -> Result<Value> {
        let text = self.run(args)?;
        serde_json::from_str(&text)
            .with_context(|| format!("parse herdr {} output", args.join(" ")))
    }

    fn agent_exists(&self, name: &str) -> bool {
        self.json(&["agent", "get", name]).is_ok()
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
            name.clone(),
            "--kind".to_string(),
            options.kind.clone(),
            "--pane".to_string(),
            pane.pane.clone(),
            "--".to_string(),
        ];
        args.extend(options.args.iter().cloned());
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        if let Err(error) = self.run(&refs) {
            if format!("{error:#}").contains("agent_not_ready") && self.agent_exists(&name) {
                // herdr's startup gate waits for an interactively ready agent;
                // a worker that starts its task immediately is healthy, not failed.
                return Ok(());
            }
            return Err(error);
        }
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

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;

    use super::*;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn fake_herdr(dir: &Path, live_agent: bool) -> PathBuf {
        let live = if live_agent { "yes" } else { "no" };
        let path = dir.join("fake-herdr.sh");
        let script = format!(
            r#"#!/bin/sh
case "$1 $2" in
  "agent start")
    echo '{{"error":{{"code":"agent_not_ready","message":"agent is blocked during startup"}}}}'
    exit 1
    ;;
  "agent get")
    if [ "{live}" = "yes" ]; then
      echo '{{"id":"cli:agent:get","result":{{"agent":{{"name":"worker"}}}}}}'
      exit 0
    fi
    echo '{{"error":{{"code":"agent_not_found"}}}}'
    exit 1
    ;;
esac
echo 'unexpected command' >&2
exit 2
"#
        );
        fs::write(&path, script).unwrap();
        let mut perms = fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&path, perms).unwrap();
        path
    }

    fn pane() -> PaneHandle {
        PaneHandle {
            backend: "herdr".to_string(),
            pane: "w1:p2".to_string(),
            ..Default::default()
        }
    }

    fn start(backend: &HerdrBackend) -> Result<()> {
        backend.start_agent(
            &pane(),
            &StartAgentOptions {
                kind: "omp".to_string(),
                name: Some("lyra-worker".to_string()),
                args: vec!["@TASK.md".to_string()],
            },
        )
    }

    #[test]
    fn agent_not_ready_with_a_live_agent_is_not_a_failure() {
        let _guard = ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let bin = fake_herdr(dir.path(), true);
        unsafe { std::env::set_var("LYRA_HERDR_BIN", &bin) };
        start(&HerdrBackend::new()).unwrap();
    }

    #[test]
    fn agent_not_ready_without_a_live_agent_fails() {
        let _guard = ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let bin = fake_herdr(dir.path(), false);
        unsafe { std::env::set_var("LYRA_HERDR_BIN", &bin) };
        let error = start(&HerdrBackend::new()).unwrap_err();
        assert!(format!("{error:#}").contains("agent_not_ready"));
    }
}
