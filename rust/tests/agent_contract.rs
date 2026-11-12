use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

use tempfile::TempDir;

struct Fixture {
    repo: TempDir,
    home: PathBuf,
}

fn fixture() -> Fixture {
    let repo = tempfile::tempdir().unwrap();
    let status = Command::new("git")
        .args(["init", "-q"])
        .current_dir(repo.path())
        .status()
        .unwrap();
    assert!(status.success());
    let status = Command::new("git")
        .args(["config", "user.email", "test@example.com"])
        .current_dir(repo.path())
        .status()
        .unwrap();
    assert!(status.success());
    let status = Command::new("git")
        .args(["config", "user.name", "Test"])
        .current_dir(repo.path())
        .status()
        .unwrap();
    assert!(status.success());
    fs::write(repo.path().join("README.md"), "test\n").unwrap();
    let status = Command::new("git")
        .args(["add", "README.md"])
        .current_dir(repo.path())
        .status()
        .unwrap();
    assert!(status.success());
    let status = Command::new("git")
        .args(["commit", "-qm", "init"])
        .current_dir(repo.path())
        .status()
        .unwrap();
    assert!(status.success());
    let home = repo.path().join(".home");
    fs::create_dir_all(&home).unwrap();
    Fixture { repo, home }
}

fn run(fixture: &Fixture, args: &[&str], stdin: Option<&str>) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_lyra"));
    command
        .args(args)
        .current_dir(fixture.repo.path())
        .env("LYRA_HOME", &fixture.home)
        .env("LYRA_OMP_BIN", "/usr/bin/true")
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

fn batch_input() -> &'static str {
    r#"{
      "tickets": [
        {"key":"core","subject":"Core","prompt":"Build core","blockedBy":[],"acceptance":"Core works"},
        {"key":"ui","subject":"UI","prompt":"Build UI","blockedBy":["core"]},
        {"key":"docs","subject":"Docs","prompt":"Write docs","blockedBy":["core"]},
        {"key":"release","subject":"Release","prompt":"Ship it","blockedBy":["ui","docs"]}
      ]
    }"#
}

#[test]
fn creates_a_batch_dag_and_reports_agent_status() {
    let fixture = fixture();
    let created = json(&run(
        &fixture,
        &["tickets", "create", "--file", "-", "--json"],
        Some(batch_input()),
    ));
    let created = created["created"].as_array().unwrap();
    assert_eq!(created.len(), 4);
    let ids: Vec<&str> = created
        .iter()
        .map(|item| item["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["T-001", "T-002", "T-003", "T-004"]);

    let status = json(&run(&fixture, &["status", "--json", "--concise"], None));
    assert_eq!(status["schemaVersion"], 1);
    assert_eq!(status["next"], "start");
    assert_eq!(status["graph"]["ready"][0]["id"], "T-001");
    assert_eq!(status["graph"]["ready"][0]["subject"], "Core");
    let blocked: Vec<&str> = status["graph"]["blocked"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["id"].as_str().unwrap())
        .collect();
    assert_eq!(blocked, ["T-002", "T-003", "T-004"]);
    assert!(status["graph"].get("modelCatalog").is_none());

    let listed = json(&run(&fixture, &["tickets", "list", "--json"], None));
    assert_eq!(listed.as_array().unwrap().len(), 4);
    assert_eq!(listed[0]["subject"], "Core");
    assert_eq!(listed[0]["ready"], true);

    let shown = json(&run(
        &fixture,
        &["tickets", "show", "T-001", "--json"],
        None,
    ));
    assert_eq!(shown["prompt"], "Build core");
    assert_eq!(shown["acceptance"], "Core works");

    let started = json(&run(
        &fixture,
        &[
            "conduct",
            "start",
            "--dry-run",
            "--json",
            "--concise",
            "--backend",
            "headless",
        ],
        None,
    ));
    assert_eq!(started["status"], "planned");
    assert!(started.get("modelCatalog").is_none());
    assert!(started.get("modelPolicy").is_none());
    assert!(started.get("usage").is_none());
}

#[test]
fn rejects_bad_batches_without_writing_tickets() {
    let fixture = fixture();
    let bad = r#"{
      "tickets": [
        {"key":"a","subject":"A","prompt":"A","blockedBy":["b"]},
        {"key":"b","subject":"B","prompt":"B","blockedBy":["a"]}
      ]
    }"#;
    let output = run(
        &fixture,
        &["tickets", "create", "--file", "-", "--json"],
        Some(bad),
    );
    assert!(!output.status.success());
    let body: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(body["ok"], false);
    assert_eq!(body["error"]["code"], "invalid_input");
    let tasks = fixture
        .home
        .join("stores")
        .read_dir()
        .ok()
        .into_iter()
        .flatten()
        .flat_map(|entry| entry.ok())
        .flat_map(|entry| {
            entry
                .path()
                .join("tasks")
                .read_dir()
                .ok()
                .into_iter()
                .flatten()
        })
        .count();
    assert_eq!(tasks, 0);
}

#[test]
fn rejects_unknown_dependency() {
    let fixture = fixture();
    let bad = r#"{
      "tickets": [
        {"key":"a","subject":"A","prompt":"A","blockedBy":["missing"]}
      ]
    }"#;
    let output = run(
        &fixture,
        &["tickets", "create", "--file", "-", "--json"],
        Some(bad),
    );
    assert!(!output.status.success());
    let body: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(body["ok"], false);
    assert_eq!(body["error"]["code"], "invalid_input");
}

#[test]
fn default_status_json_keeps_the_existing_shape() {
    let fixture = fixture();
    let output = run(
        &fixture,
        &["tickets", "create", "--file", "-", "--json"],
        Some(batch_input()),
    );
    assert!(output.status.success());
    let status = json(&run(&fixture, &["status", "--json"], None));
    assert_eq!(status["counts"]["todo"], 4);
    assert!(status.get("graph").is_none());
}
