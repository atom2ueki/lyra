use anyhow::{Context, Result, bail};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::env;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

pub const DEFAULT_REPO: &str = "atom2ueki/lyra";

#[derive(Debug, Default)]
pub struct UpgradeOptions {
    pub version: Option<String>,
    pub repo: Option<String>,
    pub force: bool,
    pub check: bool,
}

fn token() -> Option<String> {
    env::var("LYRA_GITHUB_TOKEN")
        .ok()
        .or_else(|| env::var("GITHUB_TOKEN").ok())
        .or_else(|| env::var("GH_TOKEN").ok())
        .filter(|value| !value.trim().is_empty())
}

fn curl_args(url: &str) -> Vec<String> {
    let mut args = vec![
        "-fsSL".to_string(),
        "--retry".to_string(),
        "3".to_string(),
        "--retry-all-errors".to_string(),
        "--connect-timeout".to_string(),
        "10".to_string(),
        "--max-time".to_string(),
        "300".to_string(),
    ];
    if let Some(token) = token() {
        args.push("-H".to_string());
        args.push(format!("Authorization: Bearer {token}"));
    }
    args.push(url.to_string());
    args
}

fn curl_text(url: &str) -> Result<String> {
    let output = Command::new("curl")
        .args(curl_args(url))
        .output()
        .context("run curl")?;
    if !output.status.success() {
        bail!(
            "curl failed for {url}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

fn curl_download(url: &str, destination: &Path) -> Result<()> {
    let mut args = curl_args(url);
    let output_index = args.len() - 1;
    args.insert(output_index, "-o".to_string());
    args.insert(output_index + 1, destination.display().to_string());
    let output = Command::new("curl")
        .args(args)
        .output()
        .with_context(|| format!("download {url}"))?;
    if !output.status.success() {
        bail!(
            "curl failed for {url}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

fn latest_tag(repo: &str) -> Result<String> {
    let body = curl_text(&format!(
        "https://api.github.com/repos/{repo}/releases/latest"
    ))?;
    let value: Value = serde_json::from_str(&body).context("parse GitHub release response")?;
    value
        .get("tag_name")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| anyhow::anyhow!("latest release has no tag_name"))
}

pub fn current_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

fn target_asset() -> Result<(&'static str, &'static str)> {
    match (env::consts::OS, env::consts::ARCH) {
        ("macos", "aarch64") => Ok(("aarch64-apple-darwin", "lyra-aarch64-apple-darwin.tar.gz")),
        ("linux", "x86_64") => Ok((
            "x86_64-unknown-linux-gnu",
            "lyra-x86_64-unknown-linux-gnu.tar.gz",
        )),
        ("linux", "aarch64") => Ok((
            "aarch64-unknown-linux-gnu",
            "lyra-aarch64-unknown-linux-gnu.tar.gz",
        )),
        (os, arch) => bail!("lyra upgrade is not available for {os}/{arch}"),
    }
}

fn version_key(value: &str) -> Option<Vec<u64>> {
    let trimmed = value.trim().trim_start_matches('v');
    let mut parts = Vec::new();
    for part in trimmed.split('.') {
        let numeric: String = part.chars().take_while(char::is_ascii_digit).collect();
        if numeric.is_empty() {
            return None;
        }
        parts.push(numeric.parse().ok()?);
    }
    Some(parts)
}

fn is_newer(latest: &str, current: &str) -> bool {
    match (version_key(latest), version_key(current)) {
        (Some(latest), Some(current)) => {
            let max = latest.len().max(current.len());
            for index in 0..max {
                let a = latest.get(index).copied().unwrap_or(0);
                let b = current.get(index).copied().unwrap_or(0);
                if a != b {
                    return a > b;
                }
            }
            false
        }
        _ => latest != current,
    }
}

fn expected_checksum(checksums: &str, asset: &str) -> Option<String> {
    for line in checksums.lines() {
        let mut fields = line.split_whitespace();
        let hash = fields.next()?;
        let name = fields.next()?.trim_start_matches('*');
        if name == asset {
            return Some(hash.to_string());
        }
    }
    None
}

fn verify_sha256(path: &Path, expected: &str) -> Result<()> {
    let bytes = fs::read(path).with_context(|| format!("read {}", path.display()))?;
    let digest = Sha256::digest(&bytes);
    let actual: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    if actual != expected.to_ascii_lowercase() {
        bail!("checksum mismatch for {}", path.display());
    }
    Ok(())
}

fn staging_dir() -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    env::temp_dir().join(format!("lyra-upgrade-{}-{stamp}", std::process::id()))
}

fn install_archive(archive: &Path, target: &str) -> Result<PathBuf> {
    let staging = staging_dir();
    let extract = staging.join("extract");
    fs::create_dir_all(&extract).with_context(|| format!("create {}", extract.display()))?;
    let status = Command::new("tar")
        .arg("-xzf")
        .arg(archive)
        .arg("-C")
        .arg(&extract)
        .status()
        .context("run tar")?;
    if !status.success() {
        bail!("tar failed to extract {}", archive.display());
    }
    let candidate = extract.join("lyra");
    if !candidate.exists() {
        bail!("release archive for {target} did not contain lyra");
    }
    let version = Command::new(&candidate)
        .arg("--version")
        .output()
        .with_context(|| format!("run {}", candidate.display()))?;
    if !version.status.success() {
        bail!("downloaded lyra binary did not run");
    }
    Ok(candidate)
}

fn replace_current(candidate: &Path) -> Result<()> {
    let current = env::current_exe().context("resolve current lyra binary")?;
    let parent = current
        .parent()
        .ok_or_else(|| anyhow::anyhow!("current binary has no parent: {}", current.display()))?;
    let staged = parent.join(format!(".lyra-upgrade-{}.tmp", std::process::id()));
    fs::copy(candidate, &staged).with_context(|| format!("stage {}", staged.display()))?;
    let mut permissions = fs::metadata(&staged)?.permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&staged, permissions)?;
    fs::rename(&staged, &current).with_context(|| format!("replace {}", current.display()))?;
    Ok(())
}

pub fn run(options: UpgradeOptions) -> Result<()> {
    let repo = options
        .repo
        .clone()
        .or_else(|| env::var("LYRA_REPO").ok())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_REPO.to_string());
    let explicit_version = options
        .version
        .clone()
        .or_else(|| env::var("LYRA_UPGRADE_VERSION").ok())
        .filter(|value| !value.trim().is_empty());
    let releases_url = env::var("LYRA_RELEASES_URL")
        .ok()
        .filter(|value| !value.trim().is_empty());
    let tag = match &explicit_version {
        Some(version) => Some(version.clone()),
        None if releases_url.is_some() => None,
        None => Some(latest_tag(&repo)?),
    };
    let base = if let Some(base) = releases_url {
        base
    } else if let Some(tag) = &tag {
        format!("https://github.com/{repo}/releases/download/{tag}")
    } else {
        format!("https://github.com/{repo}/releases/latest/download")
    };
    let current = current_version();
    if let Some(tag) = &tag
        && !options.force
        && !is_newer(tag, current)
    {
        println!("lyra is up to date ({current})");
        return Ok(());
    }
    let (target, asset) = target_asset()?;
    if options.check {
        println!(
            "current: {current}\nlatest:  {}\ntarget:  {target}",
            tag.as_deref().unwrap_or("latest")
        );
        return Ok(());
    }
    println!(
        "Installing lyra {} for {target}",
        tag.as_deref().unwrap_or("latest")
    );
    let staging = staging_dir();
    fs::create_dir_all(&staging).with_context(|| format!("create {}", staging.display()))?;
    let archive = staging.join(asset);
    let checksums_path = staging.join("SHA256SUMS");
    let result = (|| -> Result<()> {
        curl_download(&format!("{base}/{asset}"), &archive)?;
        curl_download(&format!("{base}/SHA256SUMS"), &checksums_path)?;
        let checksums = fs::read_to_string(&checksums_path).context("read SHA256SUMS")?;
        let expected = expected_checksum(&checksums, asset)
            .ok_or_else(|| anyhow::anyhow!("SHA256SUMS has no entry for {asset}"))?;
        verify_sha256(&archive, &expected)?;
        let candidate = install_archive(&archive, target)?;
        replace_current(&candidate)?;
        Ok(())
    })();
    let _ = fs::remove_dir_all(&staging);
    result?;
    println!(
        "Upgraded lyra to {}",
        tag.as_deref().unwrap_or("the latest release")
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{expected_checksum, is_newer, version_key};

    #[test]
    fn compares_release_versions() {
        assert!(is_newer("0.2.0", "0.1.9"));
        assert!(!is_newer("0.1.0", "0.1.0"));
        assert!(!is_newer("v0.1.0", "0.1.0"));
        assert_eq!(version_key("v1.2.3"), Some(vec![1, 2, 3]));
        assert_eq!(version_key("not-a-version"), None);
    }

    #[test]
    fn reads_sha256sum_entries() {
        let sums = "abc123  lyra-aarch64-apple-darwin.tar.gz\ndef456  lyra-x86_64-unknown-linux-gnu.tar.gz\n";
        assert_eq!(
            expected_checksum(sums, "lyra-aarch64-apple-darwin.tar.gz").as_deref(),
            Some("abc123")
        );
        assert_eq!(expected_checksum(sums, "missing.tar.gz"), None);
    }
}
