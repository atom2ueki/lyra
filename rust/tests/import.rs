use lyra::import::import_omp_kanban;
use lyra::store::list_tickets;
use std::fs;
use tempfile::tempdir;

#[test]
fn imports_tickets_and_preserves_unknown_fields() {
    let source = tempdir().unwrap();
    let target = tempdir().unwrap();
    fs::create_dir_all(source.path().join("tasks")).unwrap();
    fs::create_dir_all(source.path().join("attachments/T-001")).unwrap();
    fs::write(source.path().join("attachments/T-001/shot.txt"), "image").unwrap();
    fs::write(
        source.path().join("events.jsonl"),
        "{\"seq\":1,\"event\":\"task.created\"}\n",
    )
    .unwrap();
    fs::write(
        source.path().join("tasks/T-001.json"),
        r#"{
          "id":"T-001",
          "subject":"First",
          "prompt":"Do first",
          "status":"done",
          "blockedBy":[],
          "labels":["test"],
          "priority":1,
          "attempts":1,
          "lastError":null,
          "result":"ok",
          "held":true,
          "legacyField":{"keep":true},
          "createdAt":"2026-01-01T00:00:00.000Z",
          "updatedAt":"2026-01-01T00:00:00.000Z",
          "startedAt":"2026-01-01T00:00:00.000Z",
          "finishedAt":"2026-01-01T00:00:00.000Z"
        }"#,
    )
    .unwrap();
    fs::write(
        source.path().join("tasks/T-002.json"),
        r#"{
          "id":"T-002",
          "subject":"Second",
          "prompt":"Do second",
          "status":"todo",
          "blockedBy":["T-001"],
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

    let report = import_omp_kanban(source.path(), target.path(), "proj-test").unwrap();
    assert_eq!(report.imported, 2);
    assert!(report.errors.is_empty());
    assert!(report.validation_errors.is_empty());
    assert!(report.attachments_copied);
    assert!(report.events_copied);

    let tickets = list_tickets(target.path()).unwrap();
    let first = tickets.iter().find(|t| t.id == "T-001").unwrap();
    assert_eq!(first.priority, 1);
    assert_eq!(first.labels, vec!["test"]);
    let extra = first.extra.as_ref().unwrap();
    assert_eq!(extra["ompKanban"]["held"], true);
    let second = tickets.iter().find(|t| t.id == "T-002").unwrap();
    assert_eq!(second.blocked_by, vec!["T-001"]);
}
