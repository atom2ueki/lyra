use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::config::lyra_home;

pub fn resolve_project_root(cwd: &Path) -> PathBuf {
    if let Ok(output) = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .current_dir(cwd)
        .output()
        && output.status.success()
    {
        let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !text.is_empty() {
            return PathBuf::from(text);
        }
    }
    fs::canonicalize(cwd).unwrap_or_else(|_| cwd.to_path_buf())
}

pub fn project_id_for(root: &Path) -> String {
    let canonical = fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let mut hasher = Sha256::new();
    hasher.update(canonical.to_string_lossy().as_bytes());
    let digest = hasher.finalize();
    let mut hex = String::with_capacity(12);
    for byte in &digest[..6] {
        hex.push_str(&format!("{byte:02x}"));
    }
    hex
}

pub fn store_dir_for(root: &Path) -> PathBuf {
    lyra_home().join("stores").join(project_id_for(root))
}

pub fn runs_dir_for(root: &Path) -> PathBuf {
    lyra_home().join("runs").join(project_id_for(root))
}

pub fn current_project() -> Result<(PathBuf, String, PathBuf)> {
    let cwd = env::current_dir().context("current directory")?;
    let root = resolve_project_root(&cwd);
    let id = project_id_for(&root);
    let store = store_dir_for(&root);
    Ok((root, id, store))
}
