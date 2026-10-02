// Tauri commands for Zuko's protection features (CONTRACTS.md §2). Thin wrappers:
// the work happens in engine.rs, hooks.rs, gateway.rs, sanitize.rs and the stores.
//
// Rules the commands follow:
// * Anything that changes what is protected (policy, vault, install) ends with a
//   `protection-changed` event, which reaches every window.
// * Masking or restoring text (mask_text, unmask_text, sanitize_file, the clipboard
//   hotkeys, the island chat) tells the user through `events::announce_privacy`:
//   a `privacy` toast, an activity item and an audit receipt, never with values.
// * Values leave the vault only through `vault_reveal` (an explicit click, audited by
//   key) and `unmask_text` / `clipboard_unmask` (the user's own text, restored).
// * Slow work (PDF extraction, verifying a large audit log) runs on a blocking thread
//   so the UI loop never waits for it.
// * The global hotkeys Ctrl+Alt+M (mask the clipboard) and Ctrl+Alt+U (restore it) are
//   registered at startup; a combination another program already owns is logged and
//   skipped, never fatal.

use serde::Serialize;
use tauri::{AppHandle, Manager, State};
use tauri_plugin_clipboard_manager::ClipboardExt;
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};

use zuko_core::audit::Receipt;
use zuko_core::mask::{self, MaskCtx, MaskReport};
use zuko_core::localai::LocalAiConfig;
use zuko_core::policy::Policy;
use zuko_core::vault::EntryView;

use crate::engine::{self, Engine};
use crate::events::{self, ActivityItem, PrivacyNote};
use crate::hooks::{self, HookPreview, InstallOptions};
use crate::localai::{self, LocalAiStatus, LocalAiTest};
use crate::sanitize::{self, SanitizeResult};
use crate::{auditlog, gateway, log, policystore};

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProtectionStatus {
    pub hooks_installed: bool,
    pub hook_ready: bool,
    pub gateway_configured: bool,
    pub gateway_running: bool,
    pub gateway_url: Option<String>,
    pub gateway_port: u16,
    pub deny_rules_installed: bool,
    pub mode: String,
    pub vault_size: usize,
    pub masked_total: u64,
    pub blocked_total: u64,
    pub asked_total: u64,
    pub auto_allowed_total: u64,
    pub extension_connected: bool,
    pub policy_path: String,
    pub audit_path: String,
}

pub fn status_snapshot(app: &AppHandle) -> ProtectionStatus {
    use std::sync::atomic::Ordering::Relaxed;
    let engine = app.state::<Engine>();
    let installed = hooks::installed_options();
    let gw = gateway::status();
    let policy = engine.policy();
    let seen = engine.stats.extension_seen.load(Relaxed);
    ProtectionStatus {
        hooks_installed: installed.hooks,
        hook_ready: hooks::status().hook_ready,
        gateway_configured: installed.gateway,
        gateway_running: gw.running,
        gateway_url: gw.url,
        gateway_port: gw.port,
        deny_rules_installed: installed.deny_rules,
        mode: match policy.mode {
            zuko_core::policy::Mode::Enforce => "enforce".into(),
            zuko_core::policy::Mode::Monitor => "monitor".into(),
        },
        vault_size: engine.with_vault(|v| v.len()),
        masked_total: engine.stats.masked.load(Relaxed),
        blocked_total: engine.stats.blocked.load(Relaxed),
        asked_total: engine.stats.asked.load(Relaxed),
        auto_allowed_total: engine.stats.auto_allowed.load(Relaxed),
        extension_connected: seen > 0 && engine::now().saturating_sub(seen) < 60,
        policy_path: policystore::path().to_string_lossy().to_string(),
        audit_path: auditlog::path().to_string_lossy().to_string(),
    }
}

/// A receipt for a settings change or a sensitive read, so the audit log shows who
/// changed the rules and when. Keys only, never values.
fn audit_note(engine: &Engine, event: &str, tool: &str, summary: &str, verdict: &str, keys: Vec<String>) {
    auditlog::append(Receipt {
        ts: engine::now(),
        event: event.into(),
        tool: tool.into(),
        summary: summary.into(),
        verdict: verdict.into(),
        tier: "low".into(),
        keys,
        policy_digest: engine.policy().digest(),
        ..Default::default()
    });
}

#[tauri::command]
pub fn protection_status(app: AppHandle) -> ProtectionStatus {
    status_snapshot(&app)
}

#[tauri::command]
pub fn protection_preview(options: InstallOptions) -> Result<HookPreview, String> {
    hooks::preview_options(&options)
}

/// Only ever called from an explicit click in the settings window.
#[tauri::command]
pub fn protection_apply(app: AppHandle, engine: State<Engine>, options: InstallOptions, fingerprint: String) -> Result<String, String> {
    let backup = hooks::write_options(&options, &fingerprint)?;
    audit_note(
        &engine,
        "Install",
        "settings.json",
        &format!(
            "Protection installed: hooks {}, gateway {}, deny rules {}",
            on_off(options.hooks),
            on_off(options.gateway),
            on_off(options.deny_rules)
        ),
        "info",
        Vec::new(),
    );
    events::protection_changed(&app);
    Ok(backup)
}

fn on_off(b: bool) -> &'static str {
    if b {
        "on"
    } else {
        "off"
    }
}

// ── Policy ────────────────────────────────────────────────────────────────────

const MAX_LIST_ENTRIES: usize = 2000;
const MAX_ENTRY_LEN: usize = 500;
const MAX_HOLD_MS: u32 = 30_000;

/// Trims every pattern list (dropping blanks and repeats) and rejects what cannot
/// work, so a typo in the settings window is an error message, not a silent no-op.
fn clean_policy(mut p: Policy) -> Result<Policy, String> {
    fn tidy(name: &str, list: &mut Vec<String>) -> Result<(), String> {
        let mut out: Vec<String> = Vec::with_capacity(list.len());
        for item in list.iter() {
            let item = item.trim();
            if item.is_empty() || out.iter().any(|x| x == item) {
                continue;
            }
            if item.chars().count() > MAX_ENTRY_LEN {
                return Err(format!("An entry in {name} is longer than {MAX_ENTRY_LEN} characters."));
            }
            out.push(item.to_string());
        }
        if out.len() > MAX_LIST_ENTRIES {
            return Err(format!("{name} has more than {MAX_LIST_ENTRIES} entries."));
        }
        *list = out;
        Ok(())
    }
    tidy("network.blocked", &mut p.network.blocked)?;
    tidy("network.allowed", &mut p.network.allowed)?;
    tidy("filesystem.blockedRead", &mut p.filesystem.blocked_read)?;
    tidy("filesystem.blockedWrite", &mut p.filesystem.blocked_write)?;
    tidy("filesystem.sensitive", &mut p.filesystem.sensitive)?;
    tidy("commands.blocked", &mut p.commands.blocked)?;
    tidy("commands.ask", &mut p.commands.ask)?;
    tidy("commands.allowed", &mut p.commands.allowed)?;
    tidy("tools.blocked", &mut p.tools.blocked)?;
    tidy("tools.ask", &mut p.tools.ask)?;
    tidy("tools.allowedMcp", &mut p.tools.allowed_mcp)?;
    tidy("privacy.detector.customTerms", &mut p.privacy.detector.custom_terms)?;
    tidy("privacy.detector.allowlist", &mut p.privacy.detector.allowlist)?;
    if p.privacy.detector.custom_terms.iter().any(|t| t.chars().count() < 2) {
        return Err("Custom terms need at least 2 characters.".into());
    }
    if !p.privacy.detector.min_entropy.is_finite() || !(0.0..=8.0).contains(&p.privacy.detector.min_entropy) {
        return Err("The entropy threshold must be between 0 and 8.".into());
    }
    if p.approvals.hold_ms > MAX_HOLD_MS {
        return Err(format!("Hold-to-approve can be at most {} seconds.", MAX_HOLD_MS / 1000));
    }
    // Loopback only, a plausible model name (the client checks again on every call).
    p.local_ai.endpoint = p.local_ai.endpoint.trim().to_string();
    p.local_ai.model = p.local_ai.model.trim().to_string();
    p.local_ai.validate()?;
    let t = p.local_ai.timeout_ms;
    if !(zuko_core::localai::MIN_TIMEOUT_MS..=zuko_core::localai::MAX_TIMEOUT_MS).contains(&t) {
        return Err("The local AI timeout must be between 0.5 and 120 seconds.".into());
    }
    Ok(p)
}

#[tauri::command]
pub fn policy_get(engine: State<Engine>) -> Policy {
    (*engine.policy()).clone()
}

#[tauri::command]
pub fn policy_set(app: AppHandle, engine: State<Engine>, policy: Policy) -> Result<(), String> {
    let policy = clean_policy(policy)?;
    engine.set_policy(policy)?;
    audit_note(&engine, "Policy", "policy.json", "Policy updated", "info", Vec::new());
    events::protection_changed(&app);
    Ok(())
}

#[tauri::command]
pub fn policy_reset(app: AppHandle, engine: State<Engine>) -> Result<Policy, String> {
    let p = Policy::default();
    engine.set_policy(p.clone())?;
    audit_note(&engine, "Policy", "policy.json", "Policy reset to the defaults", "info", Vec::new());
    events::protection_changed(&app);
    Ok(p)
}

// ── Local AI ──────────────────────────────────────────────────────────────────

/// Is Ollama reachable, is the model installed, which models are there. `config`:
/// the settings window's unsaved draft (endpoint/model), else the saved policy.
#[tauri::command]
pub async fn localai_status(engine: State<'_, Engine>, config: Option<LocalAiConfig>) -> Result<LocalAiStatus, String> {
    let cfg = config.unwrap_or_else(|| engine.policy().local_ai.clone());
    Ok(engine.localai().status(&cfg).await)
}

/// Runs a deep scan on a made-up sentence (fake name and address) and returns the
/// verified findings. Works before the feature is enabled; never touches the vault.
#[tauri::command]
pub async fn localai_test(engine: State<'_, Engine>, config: Option<LocalAiConfig>) -> Result<LocalAiTest, String> {
    let cfg = config.unwrap_or_else(|| engine.policy().local_ai.clone());
    let started = std::time::Instant::now();
    let result = engine.localai().deep_scan_fresh(&cfg, localai::TEST_SAMPLE).await;
    let ms = started.elapsed().as_millis() as u64;
    Ok(match result {
        Ok(findings) => LocalAiTest { sample: localai::TEST_SAMPLE.into(), findings, ms, error: None },
        Err(e) => LocalAiTest { sample: localai::TEST_SAMPLE.into(), findings: Vec::new(), ms, error: Some(e.to_string()) },
    })
}

// ── Vault ─────────────────────────────────────────────────────────────────────

#[tauri::command]
pub fn vault_list(engine: State<Engine>) -> Vec<EntryView> {
    engine.with_vault(|v| v.views())
}

/// Adds a value by hand. Written to disk at once: the user just asked for it.
#[tauri::command]
pub fn vault_add(app: AppHandle, engine: State<Engine>, value: String, kind: String, label: String) -> Result<String, String> {
    let value = value.trim();
    let label = label.trim();
    let kind = kind.trim().to_uppercase();
    let label = if label.is_empty() { kind.as_str() } else { label };
    let key = engine
        .with_vault(|v| v.add_manual(value, &kind, label, engine::now()))
        .ok_or("That value is too short to protect reliably.")?;
    engine.persist_vault();
    engine.flush_vault();
    events::protection_changed(&app);
    Ok(key)
}

#[tauri::command]
pub fn vault_forget(app: AppHandle, engine: State<Engine>, key: String) -> bool {
    let removed = engine.with_vault(|v| v.remove(&key));
    if removed {
        engine.persist_vault();
        engine.flush_vault();
        events::protection_changed(&app);
    }
    removed
}

#[tauri::command]
pub fn vault_clear(app: AppHandle, engine: State<Engine>) {
    engine.with_vault(|v| v.clear());
    engine.persist_vault();
    engine.flush_vault();
    events::protection_changed(&app);
}

/// Returns a stored value. The UI only calls this on an explicit click; the reveal is
/// recorded in the audit log (by key).
#[tauri::command]
pub fn vault_reveal(engine: State<Engine>, key: String) -> Option<String> {
    let value = engine.with_vault(|v| v.get(&key).map(|e| e.value.clone()))?;
    audit_note(&engine, "Vault", "reveal", &format!("Revealed {key} in the settings window"), "info", vec![key]);
    Some(value)
}

// ── Activity and audit ────────────────────────────────────────────────────────

#[tauri::command]
pub fn activity_recent(limit: usize) -> Vec<ActivityItem> {
    auditlog::recent(limit.min(500))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VerifyResult {
    ok: bool,
    count: u64,
    error: Option<String>,
}

/// Reads and hashes the whole log (up to tens of MB), so it runs off the UI thread.
#[tauri::command]
pub async fn audit_verify() -> VerifyResult {
    let result = tauri::async_runtime::spawn_blocking(auditlog::verify)
        .await
        .unwrap_or_else(|e| Err(format!("verification stopped: {e}")));
    match result {
        Ok(count) => VerifyResult { ok: true, count, error: None },
        Err(e) => VerifyResult { ok: false, count: 0, error: Some(e) },
    }
}

#[tauri::command]
pub fn audit_open_folder() {
    let dir = auditlog::path().parent().map(|p| p.to_path_buf()).unwrap_or_default();
    let _ = std::fs::create_dir_all(&dir);
    crate::platform::reveal_folder(&dir.to_string_lossy());
}

// ── Text, files and the clipboard ─────────────────────────────────────────────

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MaskedText {
    text: String,
    report: MaskReport,
}

/// Masks `text` with the engine, persists the vault if it learned something and, when
/// anything was masked, tells the user. `source` is the vault entry source; the
/// privacy event uses `event_source`.
fn mask_and_announce(
    app: &AppHandle,
    engine: &Engine,
    text: &str,
    source: &str,
    event_source: &str,
    event: &str,
    tool: &str,
    summary: &str,
) -> (String, MaskReport) {
    let det = engine.detector();
    let ctx = MaskCtx { source: source.into(), now: engine::now() };
    let (masked, report) = engine.with_vault(|v| mask::mask_text(&det, v, text, &ctx));
    if report.count > 0 {
        engine.persist_vault();
        events::announce_privacy(
            app,
            engine,
            PrivacyNote {
                source: event_source,
                event,
                tool,
                summary: summary.replace("{n}", &plural(report.count, "value")),
                direction: "masked",
                count: report.count,
                keys: report.keys.clone(),
                new_keys: report.new_keys.clone(),
                session_id: None,
            },
        );
    }
    (masked, report)
}

/// Restores placeholders in `text` from the vault and, when any were restored, tells
/// the user. Returns the text and the distinct keys restored.
fn unmask_and_announce(app: &AppHandle, engine: &Engine, text: &str, event_source: &str, event: &str, tool: &str) -> (String, Vec<String>) {
    let (out, restored) = engine.with_vault(|v| {
        let (out, restored) = mask::rehydrate_text(v, text);
        let now = engine::now();
        for k in &restored {
            v.touch(k, now);
        }
        (out, restored)
    });
    let mut keys: Vec<String> = Vec::new();
    for k in &restored {
        if !keys.contains(k) {
            keys.push(k.clone());
        }
    }
    if !restored.is_empty() {
        engine.persist_vault();
        events::announce_privacy(
            app,
            engine,
            PrivacyNote {
                source: event_source,
                event,
                tool,
                summary: format!("Restored {} on this machine", plural(restored.len(), "value")),
                direction: "rehydrated",
                count: restored.len(),
                keys: keys.clone(),
                new_keys: Vec::new(),
                session_id: None,
            },
        );
    }
    (out, keys)
}

fn plural(n: usize, word: &str) -> String {
    format!("{n} {word}{}", if n == 1 { "" } else { "s" })
}

#[tauri::command]
pub fn mask_text(app: AppHandle, engine: State<Engine>, text: String) -> MaskedText {
    let (text, report) = mask_and_announce(&app, &engine, &text, "manual", "file", "Documents", "mask", "Masked {n} in pasted text");
    MaskedText { text, report }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UnmaskedText {
    text: String,
    keys: Vec<String>,
}

#[tauri::command]
pub fn unmask_text(app: AppHandle, engine: State<Engine>, text: String) -> UnmaskedText {
    let (text, keys) = unmask_and_announce(&app, &engine, &text, "file", "Documents", "unmask");
    UnmaskedText { text, keys }
}

/// PDF extraction and masking of a large file can take seconds: off the UI thread.
#[tauri::command]
pub async fn sanitize_file(app: AppHandle, path: String) -> Result<SanitizeResult, String> {
    let handle = app.clone();
    let (result, report) = tauri::async_runtime::spawn_blocking(move || sanitize::sanitize_file_report(&handle.state::<Engine>(), &path))
        .await
        .map_err(|e| format!("The scan stopped unexpectedly: {e}"))??;
    if report.count > 0 {
        let engine = app.state::<Engine>();
        events::announce_privacy(
            &app,
            &engine,
            PrivacyNote {
                source: "file",
                event: "File",
                tool: "sanitize",
                summary: format!("Sanitized {}: {} masked", result.name, plural(report.count, "value")),
                direction: "masked",
                count: report.count,
                keys: report.keys.clone(),
                new_keys: report.new_keys.clone(),
                session_id: None,
            },
        );
    }
    Ok(result)
}

#[tauri::command]
pub fn reveal_path(path: String) {
    let p = std::path::Path::new(&path);
    if p.is_absolute() && p.exists() {
        let folder = if p.is_dir() { p } else { p.parent().unwrap_or(p) };
        crate::platform::reveal_folder(&folder.to_string_lossy());
    }
}

#[derive(Serialize)]
pub struct ClipboardResult {
    pub count: usize,
}

fn clipboard_text(app: &AppHandle) -> Result<String, String> {
    match app.clipboard().read_text() {
        Ok(t) if !t.is_empty() => Ok(t),
        // Images, files and an empty clipboard all read as "no text".
        _ => Err("The clipboard has no text.".into()),
    }
}

fn clipboard_write(app: &AppHandle, text: String) -> Result<(), String> {
    app.clipboard().write_text(text).map_err(|e| format!("Couldn't write the clipboard: {e}"))
}

/// Masks the clipboard text in place. `count` is the number of values replaced.
pub fn clipboard_mask_now(app: &AppHandle) -> Result<ClipboardResult, String> {
    let engine = app.state::<Engine>();
    let text = clipboard_text(app)?;
    let (masked, report) = mask_and_announce(app, &engine, &text, "clipboard", "clipboard", "Clipboard", "mask", "Masked {n} in the clipboard");
    if report.count > 0 {
        clipboard_write(app, masked)?;
    }
    Ok(ClipboardResult { count: report.count })
}

/// Restores placeholders in the clipboard text in place. `count` is the number of
/// distinct values restored.
pub fn clipboard_unmask_now(app: &AppHandle) -> Result<ClipboardResult, String> {
    let engine = app.state::<Engine>();
    let text = clipboard_text(app)?;
    let (out, keys) = unmask_and_announce(app, &engine, &text, "clipboard", "Clipboard", "unmask");
    if !keys.is_empty() {
        clipboard_write(app, out)?;
    }
    Ok(ClipboardResult { count: keys.len() })
}

/// The clipboard plugin talks to the OS, so the work runs off the UI thread.
#[tauri::command]
pub async fn clipboard_mask(app: AppHandle) -> Result<ClipboardResult, String> {
    tauri::async_runtime::spawn_blocking(move || clipboard_mask_now(&app)).await.map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn clipboard_unmask(app: AppHandle) -> Result<ClipboardResult, String> {
    tauri::async_runtime::spawn_blocking(move || clipboard_unmask_now(&app)).await.map_err(|e| e.to_string())?
}

// ── Global hotkeys ────────────────────────────────────────────────────────────

/// Registers Ctrl+Alt+M (mask the clipboard) and Ctrl+Alt+U (restore it). A combination
/// another program holds is logged and skipped.
pub fn register_hotkeys(app: &AppHandle) {
    let combo = Modifiers::CONTROL | Modifiers::ALT;
    register_hotkey(app, Shortcut::new(Some(combo), Code::KeyM), "Ctrl+Alt+M", clipboard_mask_now);
    register_hotkey(app, Shortcut::new(Some(combo), Code::KeyU), "Ctrl+Alt+U", clipboard_unmask_now);
}

fn register_hotkey(app: &AppHandle, shortcut: Shortcut, name: &'static str, action: fn(&AppHandle) -> Result<ClipboardResult, String>) {
    let registered = app.global_shortcut().on_shortcut(shortcut, move |app, _shortcut, event| {
        // The key-up event of the same press must not run the action twice.
        if event.state != ShortcutState::Pressed {
            return;
        }
        let app = app.clone();
        std::thread::spawn(move || match action(&app) {
            Ok(r) => log::line(format!("hotkey {name}: {} handled", plural(r.count, "value"))),
            Err(e) => log::line(format!("hotkey {name}: {e}")),
        });
    });
    match registered {
        Ok(()) => log::line(format!("hotkey {name} registered")),
        Err(e) => log::line(format!("hotkey {name} not available ({e}); use the Documents section instead")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_policy_trims_dedupes_and_validates() {
        let mut p = Policy::default();
        p.network.blocked = vec!["  evil.example ".into(), "".into(), "evil.example".into(), "   ".into(), "other.example".into()];
        let cleaned = clean_policy(p).unwrap();
        assert_eq!(cleaned.network.blocked, vec!["evil.example".to_string(), "other.example".to_string()]);

        let mut p = Policy::default();
        p.privacy.detector.custom_terms = vec!["x".into()];
        assert!(clean_policy(p).unwrap_err().contains("2 characters"));

        let mut p = Policy::default();
        p.privacy.detector.min_entropy = f32::NAN;
        assert!(clean_policy(p).unwrap_err().contains("entropy"));
        let mut p = Policy::default();
        p.privacy.detector.min_entropy = 9.0;
        assert!(clean_policy(p).is_err());

        let mut p = Policy::default();
        p.approvals.hold_ms = 120_000;
        assert!(clean_policy(p).unwrap_err().contains("seconds"));

        let mut p = Policy::default();
        p.commands.blocked = vec!["a".repeat(MAX_ENTRY_LEN + 1)];
        assert!(clean_policy(p).unwrap_err().contains("commands.blocked"));

        let mut p = Policy::default();
        p.tools.blocked = (0..=MAX_LIST_ENTRIES).map(|i| format!("tool{i}")).collect();
        assert!(clean_policy(p).unwrap_err().contains("tools.blocked"));

        // The defaults pass untouched.
        assert_eq!(clean_policy(Policy::default()).unwrap(), Policy::default());
    }

    #[test]
    fn plural_forms() {
        assert_eq!(plural(1, "value"), "1 value");
        assert_eq!(plural(3, "value"), "3 values");
    }
}
