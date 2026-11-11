use crate::schema::Ticket;
use std::path::{Path, PathBuf};

pub fn validate_acceptance_paths(tickets: &[Ticket], project_root: &Path) -> Vec<String> {
    let mut errors = Vec::new();
    for ticket in tickets {
        let Some(value) = ticket.acceptance.as_deref() else {
            continue;
        };
        let value = value.trim();
        if value.is_empty() {
            continue;
        }
        let value = value.split('#').next().unwrap_or(value);
        let path_like = value.starts_with("./")
            || value.starts_with("../")
            || value.starts_with('/')
            || value.starts_with("docs/")
            || value.ends_with(".md");
        if !path_like {
            continue;
        }
        let target: PathBuf = if Path::new(value).is_absolute() {
            PathBuf::from(value)
        } else {
            project_root.join(value)
        };
        if !target.exists() {
            errors.push(format!("{}.acceptance: file not found: {}", ticket.id, value));
        }
    }
    errors
}
