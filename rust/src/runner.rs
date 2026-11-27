use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread::sleep;
use std::time::{Duration, Instant};

/// A running worker process. The conductor persists this in run state so a
/// monitor can find the process after a conductor restart.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WorkerHandle {
    pub pid: u32,
    pub command: Vec<String>,
    pub log: String,
    pub started_at: String,
}

#[derive(Debug, Clone)]
pub struct SpawnOptions {
    pub ticket_id: String,
    pub command: Vec<String>,
    pub cwd: PathBuf,
    pub env: HashMap<String, String>,
    pub log_dir: PathBuf,
}

/// Spawn a worker detached from the conductor's stdio. Output goes to
/// `<log_dir>/<ticket>/worker.log`; the handle is also written to
/// `<log_dir>/<ticket>/worker.json` for crash recovery.
pub fn spawn(options: &SpawnOptions) -> Result<WorkerHandle> {
    let command = options
        .command
        .iter()
        .map(|part| part.trim())
        .filter(|part| !part.is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();
    if command.is_empty() {
        bail!("worker command is empty");
    }
    let dir = options
        .log_dir
        .join(crate::worktree::safe_segment(&options.ticket_id));
    fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
    let log_path = dir.join("worker.log");
    let mut log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .with_context(|| format!("open {}", log_path.display()))?;
    writeln!(
        log,
        "\n--- lyra worker {} ---\n$ {}",
        options.ticket_id,
        command.join(" ")
    )?;
    log.flush()?;

    let child = Command::new(&command[0])
        .args(&command[1..])
        .current_dir(&options.cwd)
        .envs(&options.env)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log.try_clone()?))
        .stderr(Stdio::from(log))
        .spawn()
        .with_context(|| format!("spawn worker for {}", options.ticket_id))?;

    let handle = WorkerHandle {
        pid: child.id(),
        command,
        log: log_path.display().to_string(),
        started_at: crate::schema::now_iso(),
    };
    let handle_path = dir.join("worker.json");
    fs::write(&handle_path, serde_json::to_string_pretty(&handle)? + "\n")
        .with_context(|| format!("write {}", handle_path.display()))?;
    // Reaping happens through `is_running` / `stop`; do not wait here.
    let _ = child;
    Ok(handle)
}

/// Reap the child if it is ours and has exited. Returns the exit code when it
/// was reaped. A conductor restart is not the parent anymore, so waitpid
/// returns ECHILD; callers fall back to `kill(pid, 0)`.
fn try_reap(pid: u32) -> Option<i32> {
    let mut status: libc::c_int = 0;
    let result = unsafe { libc::waitpid(pid as libc::pid_t, &mut status, libc::WNOHANG) };
    if result != pid as libc::pid_t {
        return None;
    }
    if libc::WIFEXITED(status) {
        return Some(libc::WEXITSTATUS(status));
    }
    if libc::WIFSIGNALED(status) {
        return Some(128 + libc::WTERMSIG(status));
    }
    Some(-1)
}

pub fn is_running(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    if try_reap(pid).is_some() {
        return false;
    }
    unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
}

/// Send SIGTERM, wait up to `grace`, then SIGKILL. Safe to call on an already
/// exited process.
pub fn stop(pid: u32, grace: Duration) -> Result<()> {
    if pid == 0 || !is_running(pid) {
        return Ok(());
    }
    unsafe {
        libc::kill(pid as libc::pid_t, libc::SIGTERM);
    }
    let deadline = Instant::now() + grace;
    while Instant::now() < deadline {
        if !is_running(pid) {
            return Ok(());
        }
        sleep(Duration::from_millis(80));
    }
    unsafe {
        libc::kill(pid as libc::pid_t, libc::SIGKILL);
    }
    sleep(Duration::from_millis(80));
    Ok(())
}

/// Last `lines` lines of a log file, bounded to 64 KiB. Missing files return
/// an empty string so status code does not depend on logging.
pub fn tail(path: &Path, lines: usize) -> String {
    let Ok(mut file) = File::open(path) else {
        return String::new();
    };
    let Ok(metadata) = file.metadata() else {
        return String::new();
    };
    let len = metadata.len();
    let read_len = len.min(64 * 1024);
    if read_len > 0 && file.seek(SeekFrom::End(-(read_len as i64))).is_err() {
        return String::new();
    }
    let mut buffer = String::new();
    if file.read_to_string(&mut buffer).is_err() {
        return String::new();
    }
    let all: Vec<&str> = buffer.lines().collect();
    let start = all.len().saturating_sub(lines.max(1));
    all[start..].join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options(dir: &Path, command: Vec<&str>) -> SpawnOptions {
        SpawnOptions {
            ticket_id: "QuietHarbor".to_string(),
            command: command.into_iter().map(str::to_string).collect(),
            cwd: dir.to_path_buf(),
            env: HashMap::new(),
            log_dir: dir.join("logs"),
        }
    }

    #[test]
    fn spawn_writes_log_and_reports_exit() {
        let dir = tempfile::tempdir().unwrap();
        let handle = spawn(&options(
            dir.path(),
            vec!["sh", "-c", "echo hello-from-worker; exit 7"],
        ))
        .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while is_running(handle.pid) && Instant::now() < deadline {
            sleep(Duration::from_millis(20));
        }
        assert!(!is_running(handle.pid));
        assert!(tail(Path::new(&handle.log), 10).contains("hello-from-worker"));
        assert!(dir.path().join("logs/QuietHarbor/worker.json").exists());
    }

    #[test]
    fn stop_terminates_a_running_worker() {
        let dir = tempfile::tempdir().unwrap();
        let handle = spawn(&options(dir.path(), vec!["sh", "-c", "sleep 30"])).unwrap();
        assert!(is_running(handle.pid));
        stop(handle.pid, Duration::from_millis(500)).unwrap();
        assert!(!is_running(handle.pid));
    }

    #[test]
    fn tail_is_bounded_and_returns_last_lines() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("worker.log");
        let mut file = File::create(&path).unwrap();
        for i in 0..100 {
            writeln!(file, "line {i}").unwrap();
        }
        let tailed = tail(&path, 3);
        assert!(tailed.contains("line 99"));
        assert!(tailed.contains("line 97"));
        assert!(!tailed.contains("line 96"));
    }
}
