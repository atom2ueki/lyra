fn error_code(message: &str) -> &'static str {
    let lower = message.to_ascii_lowercase();
    if lower.contains("unknown ticket")
        || lower.contains("unknown tickets")
        || lower.contains("unknown blocker")
    {
        "unknown_ticket"
    } else if lower.contains("not ready")
        || lower.contains("cycle")
        || lower.contains("invalid")
        || lower.contains("unknown dependency")
        || lower.contains("must ")
        || lower.contains("cannot ")
    {
        "invalid_input"
    } else if lower.contains("no runs") || lower.contains("missing") {
        "not_found"
    } else {
        "cli_error"
    }
}

fn main() {
    let wants_json = std::env::args()
        .skip(1)
        .any(|argument| argument == "--json" || argument == "--concise");
    if let Err(error) = lyra::cli::run() {
        let message = format!("{error:#}");
        if wants_json {
            let payload = serde_json::json!({
                "ok": false,
                "error": {
                    "code": error_code(&message),
                    "message": message,
                    "next": "Inspect the graph with `lyra status --json --concise` or `lyra tickets list --json`."
                }
            });
            println!(
                "{}",
                serde_json::to_string_pretty(&payload).unwrap_or_else(|_| "{}".to_string())
            );
        } else {
            eprintln!("lyra: {message}");
        }
        std::process::exit(1);
    }
}
