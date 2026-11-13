use lyra::schema::{TicketCheckpoint, TicketStatus, TicketSuspension};
use lyra::store::{read_ticket, write_ticket};
use lyra::tickets::{BatchInput, BatchTicket, TicketUpdate, create_batch, update_ticket};
use serde_json::Value;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use tempfile::tempdir;

fn batch(key: &str, blocked_by: Vec<&str>) -> BatchInput {
    BatchInput {
        tickets: vec![BatchTicket {
            key: key.to_string(),
            subject: format!("Subject {key}"),
            prompt: format!("Do {key}"),
            blocked_by: blocked_by.into_iter().map(str::to_string).collect(),
            priority: 2,
            labels: Vec::new(),
            model_class: None,
            acceptance: None,
            plan: None,
            parent: None,
            harness: None,
        }],
    }
}

fn store_with_tickets(count: usize) -> (tempfile::TempDir, Vec<String>) {
    let dir = tempdir().unwrap();
    let store = dir.path();
    let mut tickets = Vec::new();
    for index in 0..count {
        let key = format!("t{index}");
        let output = create_batch(store, "p", batch(&key, Vec::new())).unwrap();
        tickets.push(output.created[0].id.clone());
    }
    (dir, tickets)
}

fn snapshot(store: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn walk(dir: &Path, out: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(&path, out);
            } else {
                let bytes = fs::read(&path).unwrap();
                out.insert(path, bytes);
            }
        }
    }
    let mut map = BTreeMap::new();
    walk(store, &mut map);
    map
}

fn last_event(store: &Path) -> Value {
    let text = fs::read_to_string(store.join("events.jsonl")).unwrap();
    let line = text.lines().last().unwrap();
    serde_json::from_str(line).unwrap()
}

#[test]
fn update_changes_fields_and_appends_event() {
    let (dir, ids) = store_with_tickets(2);
    let store = dir.path();
    let before = read_ticket(store, &ids[0]).unwrap().unwrap();

    let ticket = update_ticket(
        store,
        &ids[0],
        TicketUpdate {
            subject: Some("New subject".to_string()),
            prompt: Some("New prompt".to_string()),
            acceptance: Some("New acceptance".to_string()),
            priority: Some(4),
            blocked_by: vec![ids[1].clone()],
            labels: vec!["backend".to_string()],
            ..TicketUpdate::default()
        },
    )
    .unwrap();

    assert_eq!(ticket.subject, "New subject");
    assert_eq!(ticket.prompt, "New prompt");
    assert_eq!(ticket.acceptance.as_deref(), Some("New acceptance"));
    assert_eq!(ticket.priority, 4);
    assert_eq!(ticket.blocked_by, vec![ids[1].clone()]);
    assert_eq!(ticket.labels, vec!["backend".to_string()]);
    assert_eq!(ticket.created_at, before.created_at);
    assert_eq!(ticket.attempts, before.attempts);
    assert!(ticket.updated_at > before.updated_at);
    assert_eq!(ticket.status, TicketStatus::Todo);

    let event = last_event(store);
    assert_eq!(event["type"], "ticket.updated");
    assert_eq!(event["ticketId"], ids[0]);
    assert_eq!(event["changes"]["subject"]["from"], before.subject);
    assert_eq!(event["changes"]["subject"]["to"], "New subject");
    assert_eq!(event["changes"]["priority"]["from"], 2);
    assert_eq!(event["changes"]["priority"]["to"], 4);
    assert_eq!(
        event["changes"]["blockedBy"]["to"],
        serde_json::json!([ids[1]])
    );
}

#[test]
fn update_can_clear_fields() {
    let (dir, ids) = store_with_tickets(2);
    let store = dir.path();
    update_ticket(
        store,
        &ids[0],
        TicketUpdate {
            acceptance: Some("keep or clear".to_string()),
            blocked_by: vec![ids[1].clone()],
            labels: vec!["old".to_string()],
            ..TicketUpdate::default()
        },
    )
    .unwrap();

    let ticket = update_ticket(
        store,
        &ids[0],
        TicketUpdate {
            clear_acceptance: true,
            clear_blocked_by: true,
            clear_labels: true,
            ..TicketUpdate::default()
        },
    )
    .unwrap();

    assert_eq!(ticket.acceptance, None);
    assert!(ticket.blocked_by.is_empty());
    assert!(ticket.labels.is_empty());
}

#[test]
fn update_accepts_failed_tickets_and_no_ops() {
    let (dir, ids) = store_with_tickets(1);
    let store = dir.path();
    let mut ticket = read_ticket(store, &ids[0]).unwrap().unwrap();
    ticket.status = TicketStatus::Failed;
    write_ticket(store, &ticket).unwrap();

    let ticket = update_ticket(
        store,
        &ids[0],
        TicketUpdate {
            subject: Some(ticket.subject.clone()),
            ..TicketUpdate::default()
        },
    )
    .unwrap();
    assert_eq!(ticket.status, TicketStatus::Failed);
    let event = last_event(store);
    assert_eq!(event["type"], "ticket.updated");
    assert_eq!(event["changes"].as_object().unwrap().len(), 0);
}

fn make_in_progress(store: &Path, id: &str) {
    let mut ticket = read_ticket(store, id).unwrap().unwrap();
    ticket.status = TicketStatus::InProgress;
    write_ticket(store, &ticket).unwrap();
}

fn make_done(store: &Path, id: &str) {
    let mut ticket = read_ticket(store, id).unwrap().unwrap();
    ticket.status = TicketStatus::Done;
    write_ticket(store, &ticket).unwrap();
}

fn make_suspended(store: &Path, id: &str) {
    let mut ticket = read_ticket(store, id).unwrap().unwrap();
    ticket.suspension = Some(TicketSuspension {
        run_id: "run-1".to_string(),
        claim_id: "claim-1".to_string(),
        reason: "waiting".to_string(),
        needs_input: false,
        blocked_by: Vec::new(),
        worktree: "/tmp/w".to_string(),
        branch: "b".to_string(),
        base_head: "HEAD".to_string(),
        checkpoint: TicketCheckpoint {
            head: "HEAD".to_string(),
            worktree: "/tmp/w".to_string(),
            branch: "b".to_string(),
        },
        suspended_at: ticket.updated_at.clone(),
    });
    write_ticket(store, &ticket).unwrap();
}

#[test]
fn update_rejects_in_progress_done_and_suspended() {
    let (dir, ids) = store_with_tickets(3);
    let store = dir.path();
    make_in_progress(store, &ids[0]);
    make_done(store, &ids[1]);
    make_suspended(store, &ids[2]);

    for (id, expected) in [
        (&ids[0], "in_progress"),
        (&ids[1], "done"),
        (&ids[2], "suspended"),
    ] {
        let before = snapshot(store);
        let error = update_ticket(
            store,
            id,
            TicketUpdate {
                subject: Some("nope".to_string()),
                ..TicketUpdate::default()
            },
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains(expected), "expected {expected} in: {error}");
        assert_eq!(snapshot(store), before, "store must stay byte-identical");
    }
}

#[test]
fn update_rejects_invalid_dependencies() {
    let (dir, ids) = store_with_tickets(2);
    let store = dir.path();

    let cases: Vec<(Vec<String>, &str)> = vec![
        (vec![ids[0].clone()], "cannot contain itself"),
        (vec!["Missing".to_string()], "unknown ticket Missing"),
        (vec![ids[1].clone()], ""),
    ];
    for (blocked_by, expected) in cases {
        let before = snapshot(store);
        // ids[0] blocked by ids[1] would be fine alone, so also reverse ids[1] first for the cycle.
        let result = update_ticket(
            store,
            &ids[0],
            TicketUpdate {
                blocked_by,
                ..TicketUpdate::default()
            },
        );
        if expected.is_empty() {
            assert!(result.is_ok());
        } else {
            let error = result.unwrap_err().to_string();
            assert!(error.contains(expected), "expected {expected} in: {error}");
            assert_eq!(snapshot(store), before, "store must stay byte-identical");
        }
    }

    // Full-store cycle: A -> B and B -> A.
    update_ticket(
        store,
        &ids[0],
        TicketUpdate {
            blocked_by: vec![ids[1].clone()],
            ..TicketUpdate::default()
        },
    )
    .unwrap();
    let before = snapshot(store);
    let error = update_ticket(
        store,
        &ids[1],
        TicketUpdate {
            blocked_by: vec![ids[0].clone()],
            ..TicketUpdate::default()
        },
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("dependency cycle"), "got: {error}");
    assert_eq!(snapshot(store), before, "store must stay byte-identical");
}

#[test]
fn update_rejects_empty_and_conflicting_requests() {
    let (dir, ids) = store_with_tickets(1);
    let store = dir.path();

    let before = snapshot(store);
    let error = update_ticket(store, &ids[0], TicketUpdate::default())
        .unwrap_err()
        .to_string();
    assert!(error.contains("at least one field"), "got: {error}");

    let error = update_ticket(
        store,
        &ids[0],
        TicketUpdate {
            acceptance: Some("a".to_string()),
            clear_acceptance: true,
            ..TicketUpdate::default()
        },
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("both set and cleared"), "got: {error}");
    assert_eq!(snapshot(store), before);
}

#[test]
fn update_rejects_unknown_ticket() {
    let (dir, _ids) = store_with_tickets(1);
    let store = dir.path();
    let error = update_ticket(
        store,
        "Missing",
        TicketUpdate {
            subject: Some("x".to_string()),
            ..TicketUpdate::default()
        },
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("unknown ticket: Missing"), "got: {error}");
}

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_lyra")
}

#[test]
fn cli_update_writes_and_reports_json() {
    let (dir, ids) = store_with_tickets(1);
    let store = dir.path().to_str().unwrap().to_string();

    let ok = Command::new(bin())
        .args([
            "tickets",
            "update",
            &ids[0],
            "--store",
            &store,
            "--subject",
            "CLI subject",
            "--prompt",
            "CLI prompt",
            "--priority",
            "4",
            "--label",
            "backend",
            "--json",
        ])
        .output()
        .unwrap();
    assert!(ok.status.success());
    let payload: Value = serde_json::from_slice(&ok.stdout).unwrap();
    assert_eq!(payload["subject"], "CLI subject");
    assert_eq!(payload["prompt"], "CLI prompt");
    assert_eq!(payload["priority"], 4);
    assert_eq!(payload["labels"], serde_json::json!(["backend"]));

    let bad = Command::new(bin())
        .args([
            "tickets",
            "update",
            &ids[0],
            "--store",
            &store,
            "--blocked-by",
            "Bogus",
            "--json",
        ])
        .output()
        .unwrap();
    assert_eq!(bad.status.code(), Some(1));
    let error: Value = serde_json::from_slice(&bad.stdout).unwrap();
    assert_eq!(error["ok"], false);
    assert_eq!(error["error"]["code"], "unknown_ticket");
    assert!(
        error["error"]["message"]
            .as_str()
            .unwrap()
            .contains("unknown ticket Bogus")
    );

    let ticket = read_ticket(dir.path(), &ids[0]).unwrap().unwrap();
    assert_eq!(ticket.subject, "CLI subject");
    assert!(
        ticket.blocked_by.is_empty(),
        "rejected update must not write"
    );
}
