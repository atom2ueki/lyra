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
    assert_eq!(ids.len(), 4);
    for id in &ids {
        let first = id.chars().next().unwrap();
        assert!(
            first.is_ascii_uppercase(),
            "ticket id is not a word name: {id}"
        );
        assert!(!id.starts_with("T-"), "ticket id is still numeric: {id}");
    }
    let (core, ui, docs, release) = (ids[0], ids[1], ids[2], ids[3]);

    let status = json(&run(&fixture, &["status", "--json", "--concise"], None));
    assert_eq!(status["schemaVersion"], 1);
    assert_eq!(status["next"], "start");
    assert_eq!(status["graph"]["ready"][0]["id"], core);
    assert_eq!(status["graph"]["ready"][0]["subject"], "Core");
    let blocked: Vec<&str> = status["graph"]["blocked"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["id"].as_str().unwrap())
        .collect();
    let mut actual_blocked = blocked;
    let mut expected_blocked = vec![ui, docs, release];
    actual_blocked.sort_unstable();
    expected_blocked.sort_unstable();
    assert_eq!(actual_blocked, expected_blocked);
    assert!(status["graph"].get("modelCatalog").is_none());

    let listed = json(&run(&fixture, &["tickets", "list", "--json"], None));
    let listed = listed.as_array().unwrap();
    assert_eq!(listed.len(), 4);
    let core_listed = listed
        .iter()
        .find(|ticket| ticket["id"] == core)
        .expect("listed core ticket");
    assert_eq!(core_listed["subject"], "Core");
    assert_eq!(core_listed["ready"], true);

    let show_args = vec!["tickets", "show", core, "--json"];
    let shown = json(&run(&fixture, &show_args, None));
    assert_eq!(shown["prompt"], "Build core");
    assert_eq!(shown["acceptance"], "Core works");

    let not_ready_args = vec![
        "conduct",
        "start",
        "--dry-run",
        "--json",
        "--concise",
        "--backend",
        "headless",
        "--ticket",
        ui,
    ];
    let not_ready = run(&fixture, &not_ready_args, None);
    assert!(!not_ready.status.success());
    let not_ready_body: serde_json::Value = serde_json::from_slice(&not_ready.stdout).unwrap();
    assert_eq!(not_ready_body["error"]["code"], "invalid_input");
    let message = not_ready_body["error"]["message"].as_str().unwrap();
    assert!(message.contains(ui));
    assert!(message.contains(core));

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

#[test]
fn start_selects_repeated_tickets_and_rejects_not_ready_ones() {
    let fixture = fixture();
    let batch = r#"{
      "tickets": [
        {"key":"one","subject":"One","prompt":"One","blockedBy":[]},
        {"key":"two","subject":"Two","prompt":"Two","blockedBy":[]},
        {"key":"three","subject":"Three","prompt":"Three","blockedBy":[]}
      ]
    }"#;
    let created = json(&run(
        &fixture,
        &["tickets", "create", "--file", "-", "--json"],
        Some(batch),
    ));
    let ids: Vec<&str> = created["created"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids.len(), 3);
    let selected_args = vec![
        "conduct",
        "start",
        "--dry-run",
        "--json",
        "--concise",
        "--backend",
        "headless",
        "--ticket",
        ids[0],
        "--ticket",
        ids[2],
    ];
    let selected = json(&run(&fixture, &selected_args, None));
    assert_eq!(
        selected["plannedTickets"],
        serde_json::json!([ids[0], ids[2]])
    );
    let comma_ids = format!("{},{}", ids[0], ids[2]);
    let comma_args = vec![
        "conduct",
        "start",
        "--dry-run",
        "--json",
        "--concise",
        "--backend",
        "headless",
        "--tickets",
        comma_ids.as_str(),
    ];
    let selected_comma = json(&run(&fixture, &comma_args, None));
    assert_eq!(
        selected_comma["plannedTickets"],
        serde_json::json!([ids[0], ids[2]])
    );
    let run_id = selected["runId"].as_str().unwrap();
    let stopped = json(&run(
        &fixture,
        &["conduct", "stop", "--run", run_id, "--json", "--concise"],
        None,
    ));
    assert_eq!(stopped["status"], "stopped");
    let closed = json(&run(
        &fixture,
        &["conduct", "close", "--run", run_id, "--json", "--concise"],
        None,
    ));
    assert_eq!(closed["status"], "closed");

    let blocked = run(
        &fixture,
        &[
            "conduct",
            "start",
            "--dry-run",
            "--json",
            "--concise",
            "--backend",
            "headless",
            "--ticket",
            "T-999",
        ],
        None,
    );
    assert!(!blocked.status.success());
    let body: serde_json::Value = serde_json::from_slice(&blocked.stdout).unwrap();
    assert_eq!(body["error"]["code"], "unknown_ticket");
    assert!(body["error"]["message"].as_str().unwrap().contains("T-999"));
}
