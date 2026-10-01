//! The JSON snapshot kimi-code writes to the command's stdin
//! (`StatusLinePayload` in apps/kimi-code/src/tui/utils/status-line-command.ts).

use serde::Deserialize;
use std::io::Read;
use std::sync::mpsc;
use std::time::Duration;

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Payload {
    /// Already the display name (upstream runs it through modelDisplayName).
    pub model: String,
    pub cwd: String,
    pub git_branch: Option<String>,
    pub permission_mode: String,
    pub plan_mode: bool,
    pub context_usage: f64,
    pub context_tokens: u64,
    pub max_context_tokens: u64,
    pub session_id: String,
    pub version: String,
}

/// Read the snapshot, giving up after `timeout` so a writer that never closes
/// the pipe can't hold us past the TUI's 300ms cap.
pub fn read_stdin(timeout: Duration) -> Payload {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = std::io::stdin().lock().read_to_end(&mut buf);
        let _ = tx.send(buf);
    });
    match rx.recv_timeout(timeout) {
        Ok(buf) => parse(&buf),
        Err(_) => {
            crate::paths::debug("stdin read timed out");
            Payload::default()
        }
    }
}

pub fn parse(buf: &[u8]) -> Payload {
    let text = String::from_utf8_lossy(buf);
    let text = text.trim();
    if text.is_empty() {
        return Payload::default();
    }
    serde_json::from_str(text).unwrap_or_else(|e| {
        crate::paths::debug(&format!("bad payload: {e}"));
        Payload::default()
    })
}
