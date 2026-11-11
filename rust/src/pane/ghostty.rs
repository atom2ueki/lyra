use anyhow::{Context, Result, bail};
use std::process::Command;

use super::{Capabilities, PaneBackend, PaneHandle, SplitOptions};

const ANCHOR: &str = r#"
on run argv
  set requestedId to item 1 of argv
  tell application "Ghostty"
    if requestedId is not "" then
      repeat with t in terminals
        if (id of contents of t) is requestedId then return id of contents of t
      end repeat
      error "ghostty terminal not found: " & requestedId
    end if
    if frontmost is false then error "Ghostty is not frontmost; focus the calling window and retry"
    return id of (focused terminal of selected tab of front window)
  end tell
end run
"#;

const SPLIT: &str = r#"
on run argv
  set anchorId to item 1 of argv
  set paneCwd to item 2 of argv
  set paneCommand to item 3 of argv
  set paneDirection to item 4 of argv
  tell application "Ghostty"
    set anchorTerm to missing value
    repeat with t in terminals
      if (id of contents of t) is anchorId then
        set anchorTerm to contents of t
        exit repeat
      end if
    end repeat
    if anchorTerm is missing value then error "ghostty anchor terminal not found: " & anchorId
    set cfg to new surface configuration
    set initial working directory of cfg to paneCwd
    if paneCommand is not "" then set command of cfg to paneCommand
    set wait after command of cfg to false
    if paneDirection is "right" then
      set newTerm to split anchorTerm direction right with configuration cfg
    else if paneDirection is "left" then
      set newTerm to split anchorTerm direction left with configuration cfg
    else if paneDirection is "down" then
      set newTerm to split anchorTerm direction down with configuration cfg
    else if paneDirection is "up" then
      set newTerm to split anchorTerm direction up with configuration cfg
    else
      error "unsupported split direction: " & paneDirection
    end if
    return id of newTerm
  end tell
end run
"#;

const INPUT: &str = r#"
on run argv
  set targetId to item 1 of argv
  set inputText to item 2 of argv
  set submitIt to item 3 of argv
  tell application "Ghostty"
    repeat with t in terminals
      if (id of contents of t) is targetId then
        set targetTerm to contents of t
        input text inputText to targetTerm
        if submitIt is "1" then send key "enter" to targetTerm
        return "1"
      end if
    end repeat
  end tell
  return "0"
end run
"#;

const CLOSE: &str = r#"
on run argv
  set targetId to item 1 of argv
  tell application "Ghostty"
    repeat with t in terminals
      if (id of contents of t) is targetId then
        close (contents of t)
        return "1"
      end if
    end repeat
  end tell
  return "0"
end run
"#;

#[derive(Default)]
pub struct GhosttyBackend;

impl GhosttyBackend {
    pub fn new() -> Self {
        Self
    }

    fn script(&self, name: &str, source: &str, args: &[&str]) -> Result<String> {
        let output = Command::new("osascript")
            .arg("-e")
            .arg(source)
            .arg("--")
            .args(args)
            .output()
            .with_context(|| format!("run osascript {name}"))?;
        if !output.status.success() {
            bail!(
                "ghostty {name}: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }
}

impl PaneBackend for GhosttyBackend {
    fn name(&self) -> &str {
        "ghostty"
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            splits: true,
            run_command: true,
            send_text: true,
            send_keys: true,
            focus: true,
            close: true,
            ..Capabilities::default()
        }
    }

    fn anchor(&self) -> Result<PaneHandle> {
        let inherited = std::env::var("GHOSTTY_TERMINAL_ID").unwrap_or_default();
        let id = self.script("anchor", ANCHOR, &[&inherited])?;
        if id.is_empty() {
            bail!("ghostty anchor returned no terminal id");
        }
        Ok(PaneHandle {
            backend: "ghostty".to_string(),
            pane: id,
            ..Default::default()
        })
    }

    fn split(&self, anchor: &PaneHandle, options: &SplitOptions) -> Result<PaneHandle> {
        let command = options.command.clone().unwrap_or_default();
        let id = self.script(
            "split",
            SPLIT,
            &[
                &anchor.pane,
                &options.cwd.display().to_string(),
                &command,
                options.direction.as_str(),
            ],
        )?;
        if id.is_empty() {
            bail!("ghostty split returned no terminal id");
        }
        Ok(PaneHandle {
            backend: "ghostty".to_string(),
            pane: id,
            ..Default::default()
        })
    }

    fn send_text(&self, pane: &PaneHandle, text: &str, submit: bool) -> Result<()> {
        let ok = self.script(
            "input",
            INPUT,
            &[&pane.pane, text, if submit { "1" } else { "0" }],
        )?;
        if ok != "1" {
            bail!("ghostty input failed: pane not found");
        }
        Ok(())
    }

    fn send_keys(&self, pane: &PaneHandle, keys: &[String]) -> Result<()> {
        for key in keys {
            let mapped = match key.as_str() {
                "enter" => "enter",
                "esc" => "escape",
                other => other,
            };
            let script = "on run argv\n  set targetId to item 1 of argv\n  set keyName to item 2 of argv\n  tell application \"Ghostty\"\n    repeat with t in terminals\n      if (id of contents of t) is targetId then\n        send key keyName to (contents of t)\n        return \"1\"\n      end if\n    end repeat\n  end tell\n  return \"0\"\nend run\n".to_string();
            let ok = self.script("key", &script, &[&pane.pane, mapped])?;
            if ok != "1" {
                bail!("ghostty key failed: pane not found");
            }
        }
        Ok(())
    }

    fn close(&self, pane: &PaneHandle) -> Result<()> {
        let ok = self.script("close", CLOSE, &[&pane.pane])?;
        if ok != "1" {
            bail!("ghostty close failed: pane not found");
        }
        Ok(())
    }
}
