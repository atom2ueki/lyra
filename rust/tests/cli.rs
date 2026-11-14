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
    assert!(
        import.status.success(),
        "{}",
        String::from_utf8_lossy(&import.stderr)
    );
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
    assert!(
        status.status.success(),
        "{}",
        String::from_utf8_lossy(&status.stderr)
    );
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

#[test]
fn setup_persists_pane_backend_and_doctor_reports_the_source() {
    let home = tempdir().unwrap();
    let repo = tempdir().unwrap();
    let run = |args: &[&str], pane_env: Option<&str>| {
        let mut command = Command::new(bin());
        command
            .args(args)
            .env("LYRA_HOME", home.path())
            .env("LYRA_OMP_BIN", "/bin/echo")
            .env_remove("HERDR_ENV")
            .env_remove("HERDR_PANE_ID")
            .env_remove("TERM_PROGRAM")
            .env_remove("GHOSTTY_TERMINAL_ID")
            .env_remove("TMUX")
            .env_remove("ZELLIJ");
        if let Some(value) = pane_env {
            command.env("LYRA_PANE_BACKEND", value);
        } else {
            command.env_remove("LYRA_PANE_BACKEND");
        }
        command.current_dir(repo.path()).output().unwrap()
    };

    // Explicit env wins and is not persisted.
    let env_run = run(&["setup"], Some("headless"));
    let env_out = String::from_utf8_lossy(&env_run.stdout).to_string();
    assert!(env_run.status.success(), "setup failed: {env_out}");
    assert!(
        env_out.contains("headless (env)"),
        "unexpected output: {env_out}"
    );
    let config = fs::read_to_string(home.path().join("config.toml")).unwrap();
    assert!(
        !config.contains("pane_backend"),
        "env override must not persist: {config}"
    );

    // Without env or config, detection runs and persists the choice.
    let detected_run = run(&["setup"], None);
    let detected_out = String::from_utf8_lossy(&detected_run.stdout).to_string();
    assert!(
        detected_run.status.success(),
        "setup failed: {detected_out}"
    );
    assert!(
        detected_out.contains("headless (detected) — saved to config"),
        "unexpected output: {detected_out}"
    );
    let config = fs::read_to_string(home.path().join("config.toml")).unwrap();
    assert_eq!(
        config.matches("pane_backend").count(),
        1,
        "config: {config}"
    );

    // Once persisted, config wins over detection and setup is idempotent.
    let again = run(&["setup"], None);
    assert!(again.status.success());
    let config = fs::read_to_string(home.path().join("config.toml")).unwrap();
    assert_eq!(
        config.matches("pane_backend").count(),
        1,
        "config: {config}"
    );

    let doctor = run(&["doctor"], None);
    let doctor_out = String::from_utf8_lossy(&doctor.stdout).to_string();
    assert!(
        doctor_out.contains("pane backend  headless (config)"),
        "doctor output: {doctor_out}"
    );

    // An invalid configured backend must fail loudly, not silently fall back.
    fs::write(
        home.path().join("config.toml"),
        "max_parallel = 2\npane_backend = \"bogus\"\n",
    )
    .unwrap();
    let broken = run(&["doctor"], None);
    assert_eq!(broken.status.code(), Some(1));
    let broken_out = String::from_utf8_lossy(&broken.stdout).to_string();
    assert!(
        broken_out.contains("pane_backend must be herdr|ghostty|headless"),
        "doctor output: {broken_out}"
    );
}
