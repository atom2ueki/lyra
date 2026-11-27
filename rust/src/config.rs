use std::env;
use std::path::PathBuf;

/// Lyra's local state root. Project config is per-repo (`lyra.toml`); this
/// directory only holds stores and runs.
pub fn lyra_home() -> PathBuf {
    if let Ok(value) = env::var("LYRA_HOME")
        && !value.trim().is_empty()
    {
        return PathBuf::from(value);
    }
    let home = env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".lyra")
}
