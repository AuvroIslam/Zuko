// What the gateway runs inside: the app (engine in Tauri state, UI events, vault
// persisted, audit receipts written) or headless (dev binary and tests: an
// in-memory engine, stderr logging, nothing written to the data directory).
//
// Everything the proxy reports goes through here, so the proxy itself never
// touches Tauri and can be tested without an AppHandle. Reports carry vault keys
// and labels only, never values.

use std::io::Write;
use std::path::PathBuf;

use tauri::{AppHandle, Manager};
use zuko_core::audit::{sha256_hex, Receipt};

use crate::engine::{self, Engine};
use crate::events::{self, ActivityItem, PrivacyEvent};

pub enum Host {
    App(AppHandle),
    Headless(Headless),
}

pub struct Headless {
    pub engine: Engine,
    /// Log every request to stderr (the dev binary). Tests keep this off.
    pub verbose: bool,
    /// `ZUKO_GATEWAY_DUMP`: append every body sent upstream on a masked route.
    pub dump: Option<PathBuf>,
}

/// One masked request worth telling the user about.
pub struct MaskedReport<'a> {
    pub session_id: Option<String>,
    /// Route, e.g. `/v1/messages`.
    pub route: &'a str,
    pub count: usize,
    pub keys: Vec<String>,
    pub labels: Vec<String>,
    pub new_keys: Vec<String>,
    /// SHA-256 of the request body as received (before masking).
    pub input_sha256: String,
}

impl Host {
    pub fn engine(&self) -> Option<&Engine> {
        match self {
            Host::App(app) => app.try_state::<Engine>().map(|s| s.inner()),
            Host::Headless(h) => Some(&h.engine),
        }
    }

    /// Request lines: stderr in verbose headless mode only (the app's log is for
    /// problems, not traffic).
    pub fn trace(&self, message: impl AsRef<str>) {
        if let Host::Headless(h) = self {
            if h.verbose {
                eprintln!("zuko-gateway: {}", message.as_ref());
            }
        }
    }

    /// Problems worth keeping: zuko.log in the app, stderr headless.
    pub fn warn(&self, message: impl AsRef<str>) {
        match self {
            Host::App(_) => crate::log::line(format!("gateway: {}", message.as_ref())),
            Host::Headless(_) => eprintln!("zuko-gateway: {}", message.as_ref()),
        }
    }

    /// Saves the vault after masking added entries. Headless engines are
    /// in-memory by design.
    pub fn persist_vault(&self) {
        if let (Host::App(_), Some(engine)) = (self, self.engine()) {
            engine.persist_vault();
        }
    }

    /// Appends `body` (exactly what was sent upstream) to the dump file.
    pub fn dump(&self, body: &[u8]) {
        let Host::Headless(Headless { dump: Some(path), .. }) = self else { return };
        let written = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .and_then(|mut f| {
                f.write_all(body)?;
                f.write_all(b"\n")
            });
        if let Err(e) = written {
            eprintln!("zuko-gateway: could not append to {}: {e}", path.display());
        }
    }

    /// Tells the UI (privacy toast, activity feed) and the audit log, whose receipt counts
    /// the masked values for today; then the protection status follows (throttled).
    pub fn masked(&self, r: &MaskedReport) {
        let Some(engine) = self.engine() else { return };
        let Host::App(app) = self else { return };

        let session_id = r.session_id.clone().unwrap_or_default();
        let plural = if r.count == 1 { "value" } else { "values" };
        let summary = format!("Masked {} {plural} before sending ({})", r.count, r.keys.join(", "));
        events::privacy(
            app,
            &PrivacyEvent {
                source: "gateway".into(),
                direction: "masked".into(),
                count: r.count,
                keys: r.keys.clone(),
                labels: r.labels.clone(),
                new_keys: r.new_keys.clone(),
                session_id: r.session_id.clone(),
                masked_prompt: None,
            },
        );
        events::activity(
            app,
            &ActivityItem {
                id: events::next_id(),
                ts: engine::now_ms(),
                session_id: session_id.clone(),
                event: "Gateway".into(),
                tool: r.route.to_string(),
                summary: summary.clone(),
                verdict: "masked".into(),
                tier: "low".into(),
                headline: summary.clone(),
                keys: r.keys.clone(),
                ..Default::default()
            },
        );
        crate::auditlog::append(Receipt {
            ts: engine::now(),
            session_id,
            event: "Gateway".into(),
            tool: r.route.to_string(),
            summary,
            input_sha256: r.input_sha256.clone(),
            verdict: "masked".into(),
            tier: "low".into(),
            keys: r.keys.clone(),
            policy_digest: engine.policy().digest(),
            ..Default::default()
        });
        events::protection_changed_soon(app);
    }

    /// A local-AI deep scan taught the vault new values (keys and labels only).
    pub fn ai_learned(&self, learned: &crate::localai::Learned, session_id: Option<String>) {
        if learned.new_keys.is_empty() {
            return;
        }
        self.trace(format!("  local AI learned [{}]", learned.new_keys.join(", ")));
        self.persist_vault();
        if let Host::App(app) = self {
            crate::localai::announce_learned(app, learned, "gateway", session_id);
        }
    }

    /// Placeholders filled with real values in a response.
    pub fn rehydrated(&self, session_id: Option<String>, keys: &[String]) {
        if keys.is_empty() {
            return;
        }
        let mut distinct: Vec<String> = Vec::new();
        for k in keys {
            if !distinct.contains(k) {
                distinct.push(k.clone());
            }
        }
        self.trace(format!("  rehydrated {} [{}]", keys.len(), distinct.join(", ")));
        let Host::App(app) = self else { return };
        let labels = self
            .engine()
            .map(|e| e.with_vault(|v| distinct.iter().map(|k| v.get(k).map(|e| e.label.clone()).unwrap_or_default()).collect()))
            .unwrap_or_default();
        events::privacy(
            app,
            &PrivacyEvent {
                source: "gateway".into(),
                direction: "rehydrated".into(),
                count: keys.len(),
                keys: distinct,
                labels,
                session_id,
                ..Default::default()
            },
        );
    }
}

/// SHA-256 hex of a request body, for receipts.
pub fn body_digest(body: &[u8]) -> String {
    sha256_hex(body)
}
