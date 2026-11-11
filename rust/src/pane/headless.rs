use anyhow::{Context, Result, bail};
use std::collections::HashMap;
use std::fs::{self, File};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use super::{Capabilities, PaneBackend, PaneHandle, SplitOptions, WaitResult};

struct HeadlessProcess {
    child: Child,
    log: PathBuf,
    exit_code: Option<i32>,
}

#[derive(Default)]
pub struct HeadlessBackend {
    processes: Mutex<HashMap<String, HeadlessProcess>>,
    next: Mutex<u32>,
}

impl HeadlessBackend {
    pub fn new() -> Self {
        Self::default()
    }
}

fn pid_path(pane: &str) -> PathBuf {
    crate::config::lyra_home()
        .join("runtime")
        .join("headless")
        .join(format!("{pane}.pid"))
}

impl PaneBackend for HeadlessBackend {
    fn name(&self) -> &str {
        "headless"
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            splits: true,
            run_command: true,
            read_output: true,
            wait_for_output: true,
            close: true,
            ..Capabilities::default()
        }
    }

    fn anchor(&self) -> Result<PaneHandle> {
        Ok(PaneHandle {
            backend: "headless".to_string(),
            pane: "headless".to_string(),
            ..Default::default()
        })
    }

    fn split(&self, _anchor: &PaneHandle, options: &SplitOptions) -> Result<PaneHandle> {
        let Some(command) = options.command.as_deref() else {
            bail!("headless backend requires a command");
        };
        let mut next = self.next.lock().unwrap();
        *next += 1;
        let pane = format!("headless-{}", *next);
        drop(next);
        let log_dir = options
            .log_dir
            .clone()
            .unwrap_or_else(|| options.cwd.clone());
        fs::create_dir_all(&log_dir)?;
        let log = log_dir.join(format!("{pane}.log"));
        let file = File::create(&log).with_context(|| format!("create {}", log.display()))?;
        let stdout = file.try_clone()?;
        let stderr = file.try_clone()?;
        let child = Command::new("sh")
            .arg("-c")
            .arg(command)
            .current_dir(&options.cwd)
            .envs(&options.env)
            .stdin(Stdio::null())
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(stderr))
            .spawn()
            .with_context(|| format!("spawn {command}"))?;
        let pid = child.id();
        let pid_file = pid_path(&pane);
        if let Some(parent) = pid_file.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let _ = fs::write(&pid_file, pid.to_string());
        self.processes.lock().unwrap().insert(
            pane.clone(),
            HeadlessProcess {
                child,
                log,
                exit_code: None,
            },
        );
        Ok(PaneHandle {
            backend: "headless".to_string(),
            pane,
            ..Default::default()
        })
    }

    fn read(&self, pane: &PaneHandle, _source: &str, _lines: usize) -> Result<String> {
        let processes = self.processes.lock().unwrap();
        let process = processes
            .get(&pane.pane)
            .ok_or_else(|| anyhow::anyhow!("headless process not found: {}", pane.pane))?;
        Ok(fs::read_to_string(&process.log).unwrap_or_default())
    }

    fn wait(
        &self,
        pane: &PaneHandle,
        _state: Option<&str>,
        match_text: Option<&str>,
        timeout_ms: u64,
    ) -> Result<WaitResult> {
        let deadline = Instant::now() + Duration::from_millis(timeout_ms);
        loop {
            let mut processes = self.processes.lock().unwrap();
            let Some(process) = processes.get_mut(&pane.pane) else {
                return Ok(WaitResult::default());
            };
            if process.exit_code.is_none()
                && let Ok(Some(status)) = process.child.try_wait()
            {
                process.exit_code = Some(status.code().unwrap_or(1));
            }
            let output = fs::read_to_string(&process.log).unwrap_or_default();
            if let Some(match_text) = match_text {
                if output.contains(match_text) {
                    return Ok(WaitResult {
                        matched: true,
                        output: Some(output),
                        ..WaitResult::default()
                    });
                }
            } else if let Some(code) = process.exit_code {
                return Ok(WaitResult {
                    matched: true,
                    state: Some(format!("exit:{code}")),
                    output: Some(output),
                    timed_out: false,
                });
            }
            drop(processes);
            if Instant::now() >= deadline {
                return Ok(WaitResult {
                    matched: false,
                    timed_out: true,
                    ..WaitResult::default()
                });
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    fn close(&self, pane: &PaneHandle) -> Result<()> {
        let mut processes = self.processes.lock().unwrap();
        if let Some(process) = processes.get_mut(&pane.pane) {
            if process.exit_code.is_none() {
                let _ = process.child.kill();
                let _ = process.child.wait();
            }
        } else if let Ok(pid) = fs::read_to_string(pid_path(&pane.pane)) {
            let pid = pid.trim();
            if !pid.is_empty() {
                let _ = Command::new("kill")
                    .args(["-TERM", pid])
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status();
            }
        }
        let _ = fs::remove_file(pid_path(&pane.pane));
        Ok(())
    }

    fn list(&self) -> Result<Vec<PaneHandle>> {
        let processes = self.processes.lock().unwrap();
        Ok(processes
            .keys()
            .map(|pane| PaneHandle {
                backend: "headless".to_string(),
                pane: pane.clone(),
                ..Default::default()
            })
            .collect())
    }
}
