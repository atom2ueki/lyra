use lyra::schema::{TicketStatus, ready_tickets};
use lyra::store::{claim_ticket, close_ticket, finish_ticket, read_ticket};
use lyra::tickets::{BatchInput, BatchTicket, TicketUpdate, create_batch, update_ticket};
use serde_json::json;
use std::fs;

fn batch(key: &str) -> BatchInput {
    BatchInput {
        tickets: vec![BatchTicket {
            key: key.to_string(),
            subject: format!("Subject {key}"),
            prompt: format!("Do {key}"),
            blocked_by: Vec::new(),
            priority: 2,
            labels: Vec::new(),
            model_class: None,
            acceptance: None,
            plan: None,
            parent: None,
            harness: Some("omp".to_string()),
        }],
    }
}

fn create(store: &std::path::Path, key: &str) -> String {
    create_batch(store, "p", batch(key)).unwrap().created[0]
        .id
        .clone()
}

fn close_events(store: &std::path::Path) -> usize {
    fs::read_to_string(store.join("events.jsonl"))
        .unwrap()
        .lines()
        .filter(|line| line.contains("\"ticket.closed\""))
        .count()
}

#[test]
fn close_removes_ticket_from_ready_pool_and_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let store = dir.path();
    let stale = create(store, "stale");
    let live = create(store, "live");

    let closed = close_ticket(store, &stale).unwrap().unwrap();
    assert_eq!(closed.status, TicketStatus::Closed);
    assert_eq!(closed.finished_at.is_some(), true);

    let ready: Vec<String> = ready_tickets(&lyra::store::list_tickets(store).unwrap())
        .into_iter()
        .map(|ticket| ticket.id.clone())
        .collect();
    assert_eq!(ready, vec![live]);

    let again = close_ticket(store, &stale).unwrap().unwrap();
    assert_eq!(again.status, TicketStatus::Closed);
    assert_eq!(close_events(store), 1);

    let error = update_ticket(store, &stale, TicketUpdate::default()).unwrap_err();
    assert!(error.to_string().contains("closed"));
}

#[test]
fn close_refuses_worker_held_and_completed_tickets() {
    let dir = tempfile::tempdir().unwrap();
    let store = dir.path();
    let running = create(store, "running");
    let done = create(store, "done");

    claim_ticket(store, &running, "run-1", "claim-1").unwrap();
    let error = close_ticket(store, &running).unwrap_err();
    assert!(error.to_string().contains("in_progress"));

    claim_ticket(store, &done, "run-1", "claim-2").unwrap();
    finish_ticket(
        store,
        &done,
        "run-1",
        "claim-2",
        TicketStatus::Done,
        json!({"result": "done"}),
        None,
    )
    .unwrap();
    let error = close_ticket(store, &done).unwrap_err();
    assert!(error.to_string().contains("done"));
    assert_eq!(
        read_ticket(store, &done).unwrap().unwrap().status,
        TicketStatus::Done
    );
}
