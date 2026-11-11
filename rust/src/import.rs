use anyhow::{Context, Result};
use serde::Serialize;
use serde_json::{json, Map, Value};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use crate::schema::{validate_graph, Ticket};
use crate::store::{append_event, ensure_store, write_ticket};

const MAPPED_FIELDS: &[&str] = &[
    "id",
    "subject",
    "prompt",
    "status",
    "blockedBy",
    "priority",
    "labels",
    "source",
    "externalId",
    "externalUrl",
    "attempts",
    "lastError",
    "result",
    "checkpoint",
    "createdAt",
    "updatedAt",
    "startedAt",
    "finishedAt",
    "parent",
    "requires",
    "actor",
    "harness",
    "model",
];

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportError {
    pub file: String,
    pub message: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportReport {
    pub source: String,
    pub target: String,
    pub project_id: String,
    pub imported: usize,
    pub errors: Vec<ImportError>,
    pub validation_errors: Vec<String>,
    pub attachments_copied: bool,
    pub events_copied: bool,
}

pub fn resolve_root(source: &Path) -> Result<PathBuf> {
    for candidate in [source.to_path_buf(), source.join(".omp").join("kanban")] {
        if candidate.join("tasks").is_dir() {
            return Ok(candidate);
        }
    }
    anyhow::bail!("not an omp-kanban store (no tasks/): {}", source.display())
}

fn normalize_execution(value: &Value) -> Option<Value> {
    let object = value.as_object()?;
    let run_id = object.get("runId")?.as_str()?;
    let claim_id = object.get("claimId")?.as_str()?;
    let claimed_at = object
        .get("claimedAt")
        .and_then(Value::as_str)
        .unwrap_or_else(|| "")
        .to_string();
    let claimed_at = if claimed_at.is_empty() {
        crate::schema::now_iso()
    } else {
        claimed_at
    };
    let mut mapped = Map::new();
    mapped.insert("runId".to_string(), json!(run_id));
    mapped.insert("claimId".to_string(), json!(claim_id));
    mapped.insert("claimedAt".to_string(), json!(claimed_at));
    if let Some(phase) = object.get("phase").and_then(Value::as_str) {
        mapped.insert("phase".to_string(), json!(phase));
    }
    if let Some(detail) = object.get("detail").and_then(Value::as_str) {
        mapped.insert("detail".to_string(), json!(detail));
    }
    if let Some(blockers) = object.get("pendingBlockers").and_then(Value::as_array) {
        let blockers: Vec<&str> = blockers.iter().filter_map(Value::as_str).collect();
        mapped.insert("pendingBlockers".to_string(), json!(blockers));
    }
    Some(Value::Object(mapped))
}

fn normalize_attachments(value: &Value) -> Option<Value> {
    let array = value.as_array()?;
    let mut attachments = Vec::new();
    for entry in array {
        let object = entry.as_object()?;
        let filename = object.get("filename")?.as_str()?;
        let stored_as = object.get("storedAs")?.as_str()?;
        let mut mapped = Map::new();
        mapped.insert("filename".to_string(), json!(filename));
        mapped.insert("storedAs".to_string(), json!(stored_as));
        for field in ["id", "contentType", "sha256", "createdAt"] {
            if let Some(text) = object.get(field).and_then(Value::as_str) {
                mapped.insert(field.to_string(), json!(text));
            }
        }
        if let Some(bytes) = object.get("bytes").and_then(Value::as_u64) {
            mapped.insert("bytes".to_string(), json!(bytes));
        }
        attachments.push(Value::Object(mapped));
    }
    Some(Value::Array(attachments))
}

fn normalize_discoveries(value: &Value) -> Option<Value> {
    let array = value.as_array()?;
    let mut discoveries = Vec::new();
    for entry in array {
        let object = entry.as_object()?;
        let kind = object
            .get("kind")
            .and_then(Value::as_str)
            .unwrap_or("blocker");
        let prompt = object
            .get("prompt")
            .and_then(Value::as_str)
            .or_else(|| object.get("reason").and_then(Value::as_str))
            .or_else(|| object.get("subject").and_then(Value::as_str))
            .unwrap_or("");
        let mut mapped = Map::new();
        mapped.insert("kind".to_string(), json!(kind));
        mapped.insert("prompt".to_string(), json!(prompt));
        for field in ["id", "requestId", "createdAt"] {
            if let Some(text) = object.get(field).and_then(Value::as_str) {
                mapped.insert(field.to_string(), json!(text));
            }
        }
        discoveries.push(Value::Object(mapped));
    }
    Some(Value::Array(discoveries))
}

pub fn map_ticket(raw: &Value, project_id: &str) -> Result<Ticket> {
    let object = raw
        .as_object()
        .ok_or_else(|| anyhow::anyhow!("ticket must be a JSON object"))?;
    let id = object
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("ticket id is required"))?;
    let mut mapped = Map::new();
    mapped.insert("schemaVersion".to_string(), json!(1));
    mapped.insert("id".to_string(), json!(id));
    mapped.insert("projectId".to_string(), json!(project_id));
    for field in MAPPED_FIELDS {
        if *field == "id" {
            continue;
        }
        if let Some(value) = object.get(*field) {
            mapped.insert((*field).to_string(), value.clone());
        }
    }
    let mut consumed: HashSet<&str> = MAPPED_FIELDS.iter().copied().collect();
    if let Some(value) = object.get("execution").and_then(normalize_execution) {
        mapped.insert("execution".to_string(), value);
        consumed.insert("execution");
    }
    if let Some(value) = object.get("attachments").and_then(normalize_attachments) {
        mapped.insert("attachments".to_string(), value);
        consumed.insert("attachments");
    }
    if let Some(value) = object.get("discoveries").and_then(normalize_discoveries) {
        mapped.insert("discoveries".to_string(), value);
        consumed.insert("discoveries");
    }
    let mut unknown = Map::new();
    for (key, value) in object {
        if !consumed.contains(key.as_str()) {
            unknown.insert(key.clone(), value.clone());
        }
    }
    if !unknown.is_empty() {
        mapped.insert("extra".to_string(), json!({"ompKanban": unknown}));
    }
    let ticket: Ticket = serde_json::from_value(Value::Object(mapped))
        .with_context(|| format!("map ticket {id}"))?;
    Ok(ticket)
}

pub fn import_omp_kanban(source: &Path, target: &Path, project_id: &str) -> Result<ImportReport> {
    let root = resolve_root(source)?;
    ensure_store(target)?;
    let source_tasks = root.join("tasks");
    let mut entries: Vec<PathBuf> = fs::read_dir(&source_tasks)
        .with_context(|| format!("read {}", source_tasks.display()))?
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|path| path.extension().and_then(|e| e.to_str()) == Some("json"))
        .collect();
    entries.sort();
    let mut imported = 0usize;
    let mut errors = Vec::new();
    let mut tickets = Vec::new();
    for path in entries {
        let result = (|| -> Result<Ticket> {
            let text = fs::read_to_string(&path)
                .with_context(|| format!("read {}", path.display()))?;
            let raw: Value = serde_json::from_str(&text)
                .with_context(|| format!("parse {}", path.display()))?;
            map_ticket(&raw, project_id)
        })();
        match result {
            Ok(ticket) => {
                write_ticket(target, &ticket)?;
                tickets.push(ticket);
                imported += 1;
            }
            Err(error) => errors.push(ImportError {
                file: path.display().to_string(),
                message: format!("{error:#}"),
            }),
        }
    }

    let source_attachments = root.join("attachments");
    let attachments_copied = source_attachments.is_dir();
    if attachments_copied {
        copy_dir(&source_attachments, &target.join("attachments"))?;
    }

    let source_events = root.join("events.jsonl");
    let events_copied = source_events.is_file();
    if events_copied {
        fs::copy(&source_events, target.join("events.imported.jsonl"))
            .with_context(|| format!("copy {}", source_events.display()))?;
    }

    let validation_errors = validate_graph(&tickets);
    append_event(
        target,
        json!({
            "type": "import.omp-kanban",
            "source": root,
            "projectId": project_id,
            "imported": imported,
            "errors": errors.len()
        }),
    )?;

    Ok(ImportReport {
        source: root.display().to_string(),
        target: target.display().to_string(),
        project_id: project_id.to_string(),
        imported,
        errors,
        validation_errors,
        attachments_copied,
        events_copied,
    })
}

fn copy_dir(source: &Path, target: &Path) -> Result<()> {
    fs::create_dir_all(target).with_context(|| format!("create {}", target.display()))?;
    for entry in fs::read_dir(source).with_context(|| format!("read {}", source.display()))? {
        let entry = entry?;
        let from = entry.path();
        let to = target.join(entry.file_name());
        if from.is_dir() {
            copy_dir(&from, &to)?;
        } else {
            fs::copy(&from, &to)
                .with_context(|| format!("copy {} -> {}", from.display(), to.display()))?;
        }
    }
    Ok(())
}
