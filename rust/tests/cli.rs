use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_lyra")
}

fn repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    for args in [
        vec!["init", "-q"],
        vec!["config", "user.email", "test@example.com"],
        vec!["config", "user.name", "Test"],
    ] {
        let status = Command::new("git")
            .args(args)
            .current_dir(dir.path())
            .status()
            .unwrap();
        assert!(status.success());
    }
    fs::write(dir.path().join("README.md"), "test\n").unwrap();
    for args in [vec!["add", "README.md"], vec!["commit", "-qm", "init"]] {
        let status = Command::new("git")
            .args(args)
            .current_dir(dir.path())
            .status()
            .unwrap();
        assert!(status.success());
    }
    dir
}

fn run(repo: &Path, home: &Path, args: &[&str], stdin: Option<&str>) -> Output {
    let mut command = Command::new(bin());
    command
        .args(args)
        .current_dir(repo)
        .env("LYRA_HOME", home)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().unwrap();
    if let Some(input) = stdin {
        child
            .stdin
            .as_mut()
            .unwrap()
            .write_all(input.as_bytes())
            .unwrap();
    }
    child.wait_with_output().unwrap()
}

fn json(output: &Output) -> serde_json::Value {
    assert!(
        output.status.success(),
        "command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn init_add_status_and_graph_work_without_a_pane_backend() {
    let repo = repo();
    let home = repo.path().join(".home");
    fs::create_dir_all(&home).unwrap();

    let init = run(repo.path(), &home, &["init"], None);
    assert!(init.status.success());
    assert!(repo.path().join("lyra.toml").exists());

    let add = run(repo.path(), &home, &["add", "Build the parser"], None);
    assert!(add.status.success());
    let stdout = String::from_utf8_lossy(&add.stdout);
    let id = stdout.split_whitespace().next().unwrap().to_string();

    let status = json(&run(
        repo.path(),
        &home,
        &["status", "--json", "--concise"],
        None,
    ));
    assert_eq!(status["next"], "start");
    assert_eq!(status["graph"]["ready"][0]["id"], id);

    let graph = run(repo.path(), &home, &["graph", "--format", "mermaid"], None);
    assert!(graph.status.success());
    assert!(String::from_utf8_lossy(&graph.stdout).contains("flowchart TD"));

    let closed = json(&run(
        repo.path(),
        &home,
        &["tickets", "close", &id, "--json"],
        None,
    ));
    assert_eq!(closed["status"], "closed");
    let after = json(&run(
        repo.path(),
        &home,
        &["status", "--json", "--concise"],
        None,
    ));
    assert_eq!(after["graph"]["closed"], 1);
    assert!(after["graph"]["ready"].as_array().unwrap().is_empty());
}

#[test]
fn batch_create_dag_and_dry_run_start() {
    let repo = repo();
    let home = repo.path().join(".home");
    fs::create_dir_all(&home).unwrap();
    let init = run(repo.path(), &home, &["init"], None);
    assert!(init.status.success());

    let batch = r#"{"tickets":[
      {"key":"core","subject":"Core","prompt":"Build core","blockedBy":[]},
      {"key":"ui","subject":"UI","prompt":"Build UI","blockedBy":["core"]}
    ]}"#;
    let created = json(&run(
        repo.path(),
        &home,
        &["tickets", "create", "--file", "-", "--json"],
        Some(batch),
    ));
    assert_eq!(created["created"].as_array().unwrap().len(), 2);

    let status = json(&run(repo.path(), &home, &["status", "--json"], None));
    assert_eq!(status["counts"]["todo"], 2);
    assert_eq!(status["ready"].as_array().unwrap().len(), 1);

    let dry = json(&run(
        repo.path(),
        &home,
        &["start", "--dry-run", "--json", "--concise"],
        None,
    ));
    assert_eq!(dry["status"], "planned");
    assert_eq!(dry["backend"], "process");
}
