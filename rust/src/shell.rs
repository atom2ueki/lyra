use std::collections::HashMap;

pub fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

pub fn command_line(bin: &str, args: &[String], env: &HashMap<String, String>) -> String {
    let mut parts = vec!["env".to_string()];
    for (key, value) in env {
        parts.push(format!("{key}={}", shell_quote(value)));
    }
    parts.push(shell_quote(bin));
    parts.extend(args.iter().map(|arg| shell_quote(arg)));
    parts.join(" ")
}
