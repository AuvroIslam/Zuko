// Events pushed to the UI (see CONTRACTS.md §3). Every window listens, so the
// settings window's activity list and the island's feed stay in step.

use serde::Serialize;
use tauri::{AppHandle, Emitter};

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivityItem {
    pub id: String,
    /// Unix milliseconds.
    pub ts: u64,
    pub session_id: String,
    pub project: String,
    pub event: String,
    pub tool: String,
    /// Masked.
    pub summary: String,
    pub verdict: String,
    pub tier: String,
    pub score: u8,
    pub headline: String,
    pub rules: Vec<String>,
    pub keys: Vec<String>,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PrivacyEvent {
    /// gateway | hook | chat | file | browser | clipboard
    pub source: String,
    /// masked | rehydrated | blocked_prompt
    pub direction: String,
    pub count: usize,
    pub keys: Vec<String>,
    pub labels: Vec<String>,
    pub new_keys: Vec<String>,
    pub session_id: Option<String>,
    pub masked_prompt: Option<String>,
}

static NEXT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// A process-unique id for activity items.
pub fn next_id() -> String {
    let n = NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!("{}-{n}", std::process::id())
}

/// Last path component of a working directory, for display.
pub fn project_name(cwd: &str) -> String {
    let trimmed = cwd.trim_end_matches(['/', '\\']);
    trimmed
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(trimmed)
        .to_string()
}

pub fn activity(app: &AppHandle, item: &ActivityItem) {
    crate::auditlog::remember_activity(item);
    let _ = app.emit("activity", item);
}

pub fn privacy(app: &AppHandle, event: &PrivacyEvent) {
    let _ = app.emit("privacy", event);
}

pub fn protection_changed(app: &AppHandle) {
    let status = crate::commands::status_snapshot(app);
    let _ = app.emit("protection-changed", status);
}
