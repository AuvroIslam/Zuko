// Events pushed to the UI (see CONTRACTS.md §3). Every window listens, so the
// settings window's activity list and the island's feed stay in step: `activity`,
// `privacy` and `protection-changed` go out with `AppHandle::emit`, which reaches every
// window (never `emit_to` a single label).
//
// `protection-changed` after a decision goes through `protection_changed_soon`: a busy
// agent makes dozens of decisions a second, and each status costs a read of
// ~/.claude/settings.json. At most one goes out every 250 ms, and the last change of a
// burst is always followed by one, so the counters never stop short of the truth.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
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
    /// Plain-English explanation from the local model (`localai.rs`), filled in after
    /// the fact; display text only, never read by any decision.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ai_explanation: Option<String>,
    /// Absolute path of the file a Write/Edit/MultiEdit/NotebookEdit call targets, so the
    /// UI can offer "Open file". A path, never contents; live items only (the audit log
    /// keeps the masked summary, not this).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// The session's working folder, set next to `path`: "Open file" hands it to VS Code
    /// first so the file opens in the window already running the task.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
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

// ── Throttled status ──────────────────────────────────────────────────────────

/// Fewest milliseconds between two throttled `protection-changed` events.
const STATUS_EVERY: Duration = Duration::from_millis(250);

/// Runs `sink` at most once per `every`, and always once more after the last request.
///
/// The first request after a quiet spell runs at once; requests that arrive while it is
/// cooling down are folded into one run at the end of the interval. The sink reads the
/// current state when it runs, so that run carries the final state of the burst. One
/// worker thread at a time, alive only while there is something to send.
pub struct Coalescer {
    every: Duration,
    sink: Box<dyn Fn() + Send + Sync>,
    dirty: AtomicBool,
    worker: AtomicBool,
    last: Mutex<Option<Instant>>,
}

impl Coalescer {
    pub fn new(every: Duration, sink: impl Fn() + Send + Sync + 'static) -> Arc<Coalescer> {
        Arc::new(Coalescer {
            every,
            sink: Box::new(sink),
            dirty: AtomicBool::new(false),
            worker: AtomicBool::new(false),
            last: Mutex::new(None),
        })
    }

    pub fn request(self: &Arc<Self>) {
        self.dirty.store(true, Ordering::SeqCst);
        if self.worker.swap(true, Ordering::SeqCst) {
            return;
        }
        let me = self.clone();
        let spawned = std::thread::Builder::new().name("zuko-status".into()).spawn(move || loop {
            let since = me.last.lock().unwrap_or_else(|e| e.into_inner()).map(|t| t.elapsed());
            if let Some(since) = since {
                if since < me.every {
                    std::thread::sleep(me.every - since);
                }
            }
            if me.dirty.swap(false, Ordering::SeqCst) {
                (me.sink)();
                *me.last.lock().unwrap_or_else(|e| e.into_inner()) = Some(Instant::now());
                continue;
            }
            me.worker.store(false, Ordering::SeqCst);
            // A request that slipped in after the check: take the job back, unless a newly
            // started worker already has it.
            if !me.dirty.load(Ordering::SeqCst) || me.worker.swap(true, Ordering::SeqCst) {
                break;
            }
        });
        if spawned.is_err() {
            // Out of threads: send now rather than not at all.
            self.worker.store(false, Ordering::SeqCst);
            if self.dirty.swap(false, Ordering::SeqCst) {
                (self.sink)();
            }
        }
    }
}

/// A throttled `protection-changed` (see [`Coalescer`]): for changes that come in bursts,
/// such as the counters moving with every decision.
pub fn protection_changed_soon(app: &AppHandle) {
    static STATUS: OnceLock<Arc<Coalescer>> = OnceLock::new();
    let handle = app.clone();
    STATUS.get_or_init(|| Coalescer::new(STATUS_EVERY, move || protection_changed(&handle))).request();
}

/// What the UI was last told about the browser extension, so a change is sent once.
static EXTENSION_SHOWN: AtomicBool = AtomicBool::new(false);

/// A message from the browser extension just arrived: if the UI still shows it as not
/// connected, tell it otherwise.
pub fn extension_heard(app: &AppHandle) {
    if !EXTENSION_SHOWN.swap(true, Ordering::SeqCst) {
        protection_changed_soon(app);
    }
}

/// Watches for what changes with the clock alone: the extension falling silent (no
/// heartbeat for a minute) and the local date turning over (today's counters start
/// again at zero). Either one sends `protection-changed`. One check every 5 seconds.
pub fn spawn_status_watch(app: &AppHandle) {
    let app = app.clone();
    let _ = std::thread::Builder::new().name("zuko-status-watch".into()).spawn(move || loop {
        std::thread::sleep(Duration::from_secs(5));
        let connected = app.state::<Engine>().stats.extension_connected(engine::now());
        let flipped = EXTENSION_SHOWN.swap(connected, Ordering::SeqCst) != connected;
        if crate::auditlog::roll_day() || flipped {
            protection_changed_soon(&app);
        }
    });
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

/// Emits the `privacy` toast, adds an activity item and an audit receipt (which counts the
/// masked values for today) and refreshes the protection status (the vault size changed).
pub fn announce_privacy(app: &AppHandle, engine: &Engine, note: PrivacyNote) {
    let det = engine.detector();
    let ctx = MaskCtx { source: note.source.to_string(), now: engine::now() };
    let (summary, labels, learned) = engine.with_vault(|vault| {
        let (summary, report) = mask::mask_text(&det, vault, &note.summary, &ctx);
        let labels: Vec<String> = note
            .keys
            .iter()
            .map(|k| vault.get(k).map(|e| e.label.clone()).unwrap_or_default())
            .collect();
        (summary, labels, report.count > 0)
    });
    if learned {
        // The summary itself held a value (a file name, a window title): it is in the vault now.
        engine.persist_vault();
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
    protection_changed_soon(app);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    fn counting(every_ms: u64) -> (Arc<Coalescer>, Arc<AtomicUsize>, Arc<Mutex<Vec<Instant>>>) {
        let runs = Arc::new(AtomicUsize::new(0));
        let at = Arc::new(Mutex::new(Vec::new()));
        let (r, a) = (runs.clone(), at.clone());
        let c = Coalescer::new(Duration::from_millis(every_ms), move || {
            r.fetch_add(1, Ordering::SeqCst);
            a.lock().unwrap().push(Instant::now());
        });
        (c, runs, at)
    }

    #[test]
    fn a_burst_is_one_status_now_and_one_with_the_final_state() {
        let (c, runs, _) = counting(200);
        c.request();
        std::thread::sleep(Duration::from_millis(40));
        assert_eq!(runs.load(Ordering::SeqCst), 1, "the first request goes out at once");
        // A burst while it cools down...
        for _ in 0..200 {
            c.request();
        }
        std::thread::sleep(Duration::from_millis(60));
        assert_eq!(runs.load(Ordering::SeqCst), 1, "...waits for the interval...");
        std::thread::sleep(Duration::from_millis(250));
        assert_eq!(runs.load(Ordering::SeqCst), 2, "...and is one more status, at its end");
        // Quiet again: the next change goes straight out, the worker is gone meanwhile.
        std::thread::sleep(Duration::from_millis(200));
        assert!(!c.worker.load(Ordering::SeqCst));
        c.request();
        std::thread::sleep(Duration::from_millis(60));
        assert_eq!(runs.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn a_steady_stream_is_capped_and_never_ends_short() {
        let (c, runs, at) = counting(100);
        let started = Instant::now();
        let mut last_request = started;
        while started.elapsed() < Duration::from_millis(600) {
            c.request();
            last_request = Instant::now();
            std::thread::sleep(Duration::from_millis(5));
        }
        std::thread::sleep(Duration::from_millis(300));
        let n = runs.load(Ordering::SeqCst);
        // 600 ms at one per 100 ms, plus the trailing one (and a little scheduling slack).
        assert!((5..=9).contains(&n), "{n} runs");
        let at = at.lock().unwrap();
        assert!(at.windows(2).all(|w| w[1] - w[0] >= Duration::from_millis(95)), "spaced out: {at:?}");
        assert!(*at.last().unwrap() >= last_request, "the last run comes after the last request");
    }
}
