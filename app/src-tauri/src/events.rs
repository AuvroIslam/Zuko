// Events pushed to the UI (see CONTRACTS.md §3). Every window listens, so the
// settings window's activity list and the island's feed stay in step: `activity`,
// `privacy` and `protection-changed` go out with `AppHandle::emit`, which reaches every
// window (never `emit_to` a single label).

use serde::Serialize;
use tauri::{AppHandle, Emitter};
use zuko_core::audit::Receipt;
use zuko_core::mask::{self, MaskCtx};

use crate::engine::{self, Engine};

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

/// A masking or restoring done by one of the app's own features (documents, chat,
/// clipboard, the manual mask box), as the UI, the audit log and the counters see it.
pub struct PrivacyNote<'a> {
    /// `PrivacyEvent::source`: chat | file | clipboard | ...
    pub source: &'a str,
    /// `ActivityItem::event`: "File", "Chat", "Clipboard", ...
    pub event: &'a str,
    pub tool: &'a str,
    /// Human summary. It is masked again here, so a file name or title that happens to
    /// contain a value can never reach the feed or the log.
    pub summary: String,
    /// `masked` or `rehydrated`.
    pub direction: &'a str,
    pub count: usize,
    pub keys: Vec<String>,
    pub new_keys: Vec<String>,
    pub session_id: Option<String>,
}

/// Emits the `privacy` toast, adds an activity item and an audit receipt, counts the
/// masked values and refreshes the protection status (the vault size changed).
pub fn announce_privacy(app: &AppHandle, engine: &Engine, note: PrivacyNote) {
    let det = engine.detector();
    let ctx = MaskCtx { source: note.source.to_string(), now: engine::now() };
    let (summary, labels) = engine.with_vault(|vault| {
        let summary = mask::mask_text(&det, vault, &note.summary, &ctx).0;
        let labels: Vec<String> = note
            .keys
            .iter()
            .map(|k| vault.get(k).map(|e| e.label.clone()).unwrap_or_default())
            .collect();
        (summary, labels)
    });
    if note.direction == "masked" {
        engine::Stats::add(&engine.stats.masked, note.count as u64);
    }
    privacy(
        app,
        &PrivacyEvent {
            source: note.source.to_string(),
            direction: note.direction.to_string(),
            count: note.count,
            keys: note.keys.clone(),
            labels,
            new_keys: note.new_keys.clone(),
            session_id: note.session_id.clone(),
            masked_prompt: None,
        },
    );
    let session_id = note.session_id.clone().unwrap_or_default();
    crate::auditlog::append(Receipt {
        ts: engine::now(),
        session_id: session_id.clone(),
        event: note.event.to_string(),
        tool: note.tool.to_string(),
        summary: summary.clone(),
        verdict: note.direction.to_string(),
        tier: "low".into(),
        keys: note.keys.clone(),
        policy_digest: engine.policy().digest(),
        ..Default::default()
    });
    activity(
        app,
        &ActivityItem {
            id: next_id(),
            ts: engine::now_ms(),
            session_id,
            event: note.event.to_string(),
            tool: note.tool.to_string(),
            headline: summary.clone(),
            summary,
            verdict: note.direction.to_string(),
            tier: "low".into(),
            keys: note.keys,
            ..Default::default()
        },
    );
    protection_changed(app);
}
