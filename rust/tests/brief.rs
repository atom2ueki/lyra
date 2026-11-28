use std::path::Path;

use lyra::store::{NewTicket, create_ticket, ensure_store};
use lyra::worker::render_task_brief;

#[test]
fn worker_brief_contains_the_actual_ticket_prompt() {
    let dir = tempfile::tempdir().unwrap();
    let store = dir.path().join("store");
    ensure_store(&store).unwrap();
    let ticket = create_ticket(
        &store,
        NewTicket {
            project_id: "p".to_string(),
            subject: "Brief regression".to_string(),
            prompt: "UNIQUE_TICKET_PROMPT_MUST_REACH_THE_WORKER".to_string(),
            blocked_by: Vec::new(),
            priority: 2,
            labels: Vec::new(),
            model_class: None,
            acceptance: Some("UNIQUE_ACCEPTANCE_MUST_ALSO_REACH_THE_WORKER".to_string()),
            plan: None,
            parent: None,
            origin: Some("test".to_string()),
            harness: None,
        },
    )
    .unwrap();
    let brief = render_task_brief(
        &ticket,
        Path::new("/tmp/worktree"),
        Path::new("/tmp/TASK.md"),
        Path::new("/tmp/RESULT.md"),
        None,
    );
    assert!(brief.contains("UNIQUE_TICKET_PROMPT_MUST_REACH_THE_WORKER"));
    assert!(brief.contains("UNIQUE_ACCEPTANCE_MUST_ALSO_REACH_THE_WORKER"));
}
