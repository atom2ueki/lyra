use serde_json::Value;
use std::fs;
use std::process::Command;
use tempfile::tempdir;

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_lyra")
}

fn write_source(source: &std::path::Path) {
    fs::create_dir_all(source.join("tasks")).unwrap();
    fs::write(
        source.join("tasks/T-001.json"),
        r#"{
          "id":"T-001",
          "subject":"CLI ticket",
          "prompt":"Do it",
          "status":"todo",
          "blockedBy":[],
          "priority":2,
          "attempts":0,
          "lastError":null,
          "result":null,
          "createdAt":"2026-01-01T00:00:00.000Z",
          "updatedAt":"2026-01-01T00:00:00.000Z",
          "startedAt":null,
          "finishedAt":null
        }"#,
    )
    .unwrap();
}

#[test]
fn imports_and_reports_status() {
    let source = tempdir().unwrap();
    let target = tempdir().unwrap();
    write_source(source.path());

    let import = Command::new(bin())
        .args([
            "import",
            "omp-kanban",
            source.path().to_str().unwrap(),
            "--store",
            target.path().to_str().unwrap(),
            "--project-id",
            "proj-test",
            "--json",
        ])
        .output()
        .unwrap();
    assert!(import.status.success(), "{}", String::from_utf8_lossy(&import.stderr));
    let report: Value = serde_json::from_slice(&import.stdout).unwrap();
    assert_eq!(report["imported"], 1);

    let status = Command::new(bin())
        .args([
            "conduct",
            "status",
            "--store",
            target.path().to_str().unwrap(),
            "--json",
        ])
        .output()
        .unwrap();
    assert!(status.status.success(), "{}", String::from_utf8_lossy(&status.stderr));
    let payload: Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(payload["counts"]["todo"], 1);
    assert_eq!(payload["ready"], serde_json::json!(["T-001"]));
    assert_eq!(payload["errors"], serde_json::json!([]));

    let graph = Command::new(bin())
        .args([
            "graph",
            "--store",
            target.path().to_str().unwrap(),
            "--format",
            "mermaid",
        ])
        .output()
        .unwrap();
    assert!(graph.status.success());
    let graph_text = String::from_utf8_lossy(&graph.stdout);
    assert!(graph_text.contains("flowchart TD"));
    assert!(graph_text.contains("T-001"));
}
