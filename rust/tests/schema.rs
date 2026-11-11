use lyra::schema::{
    ready_tickets, validate_graph, Ticket, TicketStatus, PRIORITY_DEFAULT,
};

fn ticket(id: &str) -> Ticket {
    Ticket::new(
        id.to_string(),
        "p".to_string(),
        format!("Subject {id}"),
        format!("Do {id}"),
    )
}

#[test]
fn valid_graph_passes() {
    let mut first = ticket("T-001");
    first.status = TicketStatus::Done;
    let mut second = ticket("T-002");
    second.blocked_by = vec!["T-001".to_string()];
    assert!(validate_graph(&[first, second]).is_empty());
}

#[test]
fn unknown_blocker_and_self_dependency_fail() {
    let mut first = ticket("T-001");
    first.blocked_by = vec!["T-999".to_string(), "T-001".to_string()];
    let errors = validate_graph(&[first]);
    assert!(errors.iter().any(|e| e.contains("unknown ticket T-999")));
    assert!(errors.iter().any(|e| e.contains("cannot contain itself")));
}

#[test]
fn cycle_fails() {
    let mut first = ticket("T-001");
    first.blocked_by = vec!["T-002".to_string()];
    let mut second = ticket("T-002");
    second.blocked_by = vec!["T-001".to_string()];
    let errors = validate_graph(&[first, second]);
    assert!(errors.iter().any(|e| e.starts_with("dependency cycle:")));
}

#[test]
fn ready_tickets_respect_dependencies_and_priority() {
    let mut first = ticket("T-001");
    first.status = TicketStatus::Done;
    let mut second = ticket("T-002");
    second.blocked_by = vec!["T-001".to_string()];
    second.priority = 1;
    let mut third = ticket("T-003");
    third.priority = 0;
    let tickets = [first, second, third];
    let ready = ready_tickets(&tickets);
    assert_eq!(ready.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(), vec!["T-003", "T-002"]);
    assert_eq!(tickets[2].priority, 0);
    assert_eq!(PRIORITY_DEFAULT, 2);
}
