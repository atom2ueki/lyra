use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::runner;
use crate::schema::{Ticket, now_iso};
use crate::worktree::safe_segment;

/// Files a worker exchanges with the conductor. Everything lives under the run
/// directory, never inside the worktree, so a dirty worktree check stays
/// meaningful.
#[derive(Debug, Clone)]
pub struct WorkerPaths {
    pub root: PathBuf,
    pub prompt: PathBuf,
    pub plan_prompt: PathBuf,
    pub plan: PathBuf,
    pub result_md: PathBuf,
    pub result_json: PathBuf,
    pub diff: PathBuf,
    pub artifact: PathBuf,
    pub artifacts: PathBuf,
    pub feedback: PathBuf,
    pub state: PathBuf,
    pub log: PathBuf,
}

impl WorkerPaths {
    pub fn new(run_dir: &Path, ticket_id: &str) -> Self {
        let root = run_dir.join("workers").join(safe_segment(ticket_id));
        Self {
            prompt: root.join("TASK.md"),
            plan_prompt: root.join("PLAN_TASK.md"),
            plan: root.join("PLAN.md"),
            result_md: root.join("RESULT.md"),
            result_json: root.join("result.json"),
            diff: root.join("DIFF.patch"),
            artifact: root.join("ARTIFACT.json"),
            artifacts: root.join("artifacts"),
            feedback: root.join("FEEDBACK.md"),
            state: root.join("state.json"),
            log: root.join("worker.log"),
            root,
        }
    }

    pub fn ensure(&self) -> Result<()> {
        fs::create_dir_all(&self.root)
            .with_context(|| format!("create {}", self.root.display()))?;
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerResultStatus {
    Done,
    Blocked,
    Failed,
    NeedsInput,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkerEvidence {
    pub command: String,
    pub exit_code: i64,
    pub output: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkerResult {
    pub schema_version: u8,
    pub ticket_id: String,
    pub run_id: String,
    pub claim_id: String,
    pub status: WorkerResultStatus,
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub verification: Option<String>,
    #[serde(default)]
    pub evidence: Vec<WorkerEvidence>,
    #[serde(default)]
    pub blockers: Vec<String>,
    pub finished_at: String,
}

pub fn write_task_brief(paths: &WorkerPaths, body: &str) -> Result<()> {
    paths.ensure()?;
    fs::write(&paths.prompt, body).with_context(|| format!("write {}", paths.prompt.display()))?;
    Ok(())
}

pub fn write_plan_brief(paths: &WorkerPaths, body: &str) -> Result<()> {
    paths.ensure()?;
    fs::write(&paths.plan_prompt, body)
        .with_context(|| format!("write {}", paths.plan_prompt.display()))?;
    Ok(())
}

pub fn write_feedback(paths: &WorkerPaths, note: &str) -> Result<()> {
    paths.ensure()?;
    fs::write(&paths.feedback, note)
        .with_context(|| format!("write {}", paths.feedback.display()))?;
    Ok(())
}

fn feedback_block(feedback: Option<&str>) -> String {
    match feedback.map(str::trim).filter(|note| !note.is_empty()) {
        Some(note) => format!("\n## Reviewer feedback\n\n{note}\n\nAddress this in this phase.\n"),
        None => String::new(),
    }
}

pub fn render_task_brief(
    ticket: &Ticket,
    worktree: &Path,
    prompt_file: &Path,
    result_file: &Path,
    feedback: Option<&str>,
) -> String {
    format!(
        "# Lyra worker: {id}\n\n\
         You are the only worker for ticket {id}.\n\n\
         - Work in: {worktree}\n\
         - Read the task: {prompt}\n\
         - Commit your work on the current branch.\n\
         - Do not edit the Lyra store or any other worktree.\n\n\
         When finished, write {result} with exactly these fields:\n\n\
         status: done|blocked|failed|needs_input\n\
         summary: <what changed, or what you need>\n\
         verification: <checks you actually ran>\n\
         blockers: <comma-separated ticket ids, blocked only>\n{feedback}",
        id = ticket.id,
        worktree = worktree.display(),
        prompt = prompt_file.display(),
        result = result_file.display(),
        feedback = feedback_block(feedback),
    )
}

pub fn render_plan_brief(
    ticket: &Ticket,
    worktree: &Path,
    prompt_file: &Path,
    plan_file: &Path,
    result_file: &Path,
    feedback: Option<&str>,
) -> String {
    format!(
        "# Lyra plan task: {id}\n\n\
         Read {prompt} in {worktree} and write an implementation plan to {plan}.\n\
         Do not modify code. Include files, steps, risks, and verification.\n\
         When done, write {result} with status: done and a one-line summary.\n{feedback}",
        id = ticket.id,
        prompt = prompt_file.display(),
        worktree = worktree.display(),
        plan = plan_file.display(),
        result = result_file.display(),
        feedback = feedback_block(feedback),
    )
}

/// Read `result.json` first, then `RESULT.md`. A worker that only writes
/// RESULT.md is a first-class worker; no extension or SDK is required.
pub fn read_result(
    paths: &WorkerPaths,
    ticket_id: &str,
    run_id: &str,
    claim_id: &str,
) -> Result<Option<WorkerResult>> {
    if let Ok(text) = fs::read_to_string(&paths.result_json)
        && let Ok(mut result) = serde_json::from_str::<WorkerResult>(&text)
    {
        if result.ticket_id.is_empty() {
            result.ticket_id = ticket_id.to_string();
        }
        if result.run_id.is_empty() {
            result.run_id = run_id.to_string();
        }
        if result.claim_id.is_empty() {
            result.claim_id = claim_id.to_string();
        }
        return Ok(Some(result));
    }
    let Ok(text) = fs::read_to_string(&paths.result_md) else {
        return Ok(None);
    };
    Ok(parse_result_fields(&text, ticket_id, run_id, claim_id))
}

fn parse_result_fields(
    text: &str,
    ticket_id: &str,
    run_id: &str,
    claim_id: &str,
) -> Option<WorkerResult> {
    let mut fields = HashMap::new();
    for line in text.lines() {
        if let Some((key, value)) = line.trim().split_once(':') {
            fields.insert(key.trim().to_ascii_lowercase(), value.trim().to_string());
        }
    }
    let status = match fields.get("status").map(String::as_str) {
        Some("done") => WorkerResultStatus::Done,
        Some("blocked") => WorkerResultStatus::Blocked,
        Some("failed") => WorkerResultStatus::Failed,
        Some("needs_input") => WorkerResultStatus::NeedsInput,
        _ => return None,
    };
    let blockers = fields
        .get("blockers")
        .map(|value| {
            value
                .split(|ch: char| ch == ',' || ch.is_whitespace())
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    Some(WorkerResult {
        schema_version: 1,
        ticket_id: ticket_id.to_string(),
        run_id: run_id.to_string(),
        claim_id: claim_id.to_string(),
        status,
        summary: fields.get("summary").cloned(),
        verification: fields.get("verification").cloned(),
        evidence: Vec::new(),
        blockers,
        finished_at: now_iso(),
    })
}

/// Optional progress file written by richer workers. The generic contract does
/// not require it; the board shows it when present.
pub fn read_state(paths: &WorkerPaths) -> Option<Value> {
    let text = fs::read_to_string(&paths.state).ok()?;
    serde_json::from_str(&text).ok()
}

pub fn log_tail(paths: &WorkerPaths, lines: usize) -> String {
    runner::tail(&paths.log, lines)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(dir: &Path) -> WorkerPaths {
        WorkerPaths::new(dir, "QuietHarbor")
    }

    #[test]
    fn reads_the_minimal_markdown_contract() {
        let dir = tempfile::tempdir().unwrap();
        let paths = paths(dir.path());
        paths.ensure().unwrap();
        fs::write(
            &paths.result_md,
            "status: blocked\nsummary: need the tenant id\nverification: read schema\nblockers: AUTH\n",
        )
        .unwrap();
        let result = read_result(&paths, "QuietHarbor", "run-1", "claim-1")
            .unwrap()
            .unwrap();
        assert_eq!(result.status, WorkerResultStatus::Blocked);
        assert_eq!(result.summary.as_deref(), Some("need the tenant id"));
        assert_eq!(result.blockers, vec!["AUTH"]);
    }

    #[test]
    fn missing_result_is_not_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let paths = paths(dir.path());
        paths.ensure().unwrap();
        assert!(
            read_result(&paths, "QuietHarbor", "run-1", "claim-1")
                .unwrap()
                .is_none()
        );
    }
}
