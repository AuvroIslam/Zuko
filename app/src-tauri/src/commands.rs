// Tauri commands for Zuko's protection features (CONTRACTS.md §2). Thin wrappers:
// the work happens in engine.rs, hooks.rs, gateway.rs, sanitize.rs and the stores.
//
// OWNER: state & features (wave 2). The bodies below are the minimal working
// versions; vault_reveal and the clipboard commands are still stubs.

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, State};

use zuko_core::mask::{self, MaskCtx, MaskReport};
use zuko_core::policy::Policy;
use zuko_core::vault::EntryView;

use crate::engine::{self, Engine};
use crate::events::ActivityItem;
use crate::hooks::{self, HookPreview, InstallOptions};
use crate::sanitize::{self, SanitizeResult};
use crate::{auditlog, gateway, policystore};

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
pub fn protection_apply(app: AppHandle, options: InstallOptions, fingerprint: String) -> Result<String, String> {
    let backup = hooks::write_options(&options, &fingerprint)?;
    crate::events::protection_changed(&app);
    Ok(backup)
}

#[tauri::command]
pub fn policy_get(engine: State<Engine>) -> Policy {
    (*engine.policy()).clone()
}

#[tauri::command]
pub fn policy_set(app: AppHandle, engine: State<Engine>, policy: Policy) -> Result<(), String> {
    engine.set_policy(policy)?;
    crate::events::protection_changed(&app);
    Ok(())
}

#[tauri::command]
pub fn policy_reset(app: AppHandle, engine: State<Engine>) -> Result<Policy, String> {
    let p = Policy::default();
    engine.set_policy(p.clone())?;
    crate::events::protection_changed(&app);
    Ok(p)
}

#[tauri::command]
pub fn vault_list(engine: State<Engine>) -> Vec<EntryView> {
    engine.with_vault(|v| v.views())
}

#[tauri::command]
pub fn vault_add(engine: State<Engine>, value: String, kind: String, label: String) -> Result<String, String> {
    let key = engine
        .with_vault(|v| v.add_manual(&value, &kind, &label, engine::now()))
        .ok_or("That value is too short to protect reliably.")?;
    engine.persist_vault();
    Ok(key)
}

#[tauri::command]
pub fn vault_forget(engine: State<Engine>, key: String) -> bool {
    let removed = engine.with_vault(|v| v.remove(&key));
    if removed {
        engine.persist_vault();
    }
    removed
}

#[tauri::command]
pub fn vault_clear(engine: State<Engine>) {
    engine.with_vault(|v| v.clear());
    engine.persist_vault();
}

/// Returns a stored value. The UI only calls this on an explicit click.
#[tauri::command]
pub fn vault_reveal(engine: State<Engine>, key: String) -> Option<String> {
    engine.with_vault(|v| v.get(&key).map(|e| e.value.clone()))
}

#[tauri::command]
pub fn activity_recent(limit: usize) -> Vec<ActivityItem> {
    auditlog::recent(limit)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VerifyResult {
    ok: bool,
    count: u64,
    error: Option<String>,
}

#[tauri::command]
pub fn audit_verify() -> VerifyResult {
    match auditlog::verify() {
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

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MaskedText {
    text: String,
    report: MaskReport,
}

#[tauri::command]
pub fn mask_text(app: AppHandle, engine: State<Engine>, text: String) -> MaskedText {
    let det = engine.detector();
    let ctx = MaskCtx { source: "manual".into(), now: engine::now() };
    let (text, report) = engine.with_vault(|v| mask::mask_text(&det, v, &text, &ctx));
    if !report.new_keys.is_empty() {
        engine.persist_vault();
    }
    let _ = &app;
    MaskedText { text, report }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UnmaskedText {
    text: String,
    keys: Vec<String>,
}

#[tauri::command]
pub fn unmask_text(engine: State<Engine>, text: String) -> UnmaskedText {
    let (text, keys) = engine.with_vault(|v| mask::rehydrate_text(v, &text));
    UnmaskedText { text, keys }
}

#[tauri::command]
pub fn sanitize_file(engine: State<Engine>, path: String) -> Result<SanitizeResult, String> {
    sanitize::sanitize_file(&engine, &path)
}

#[tauri::command]
pub fn reveal_path(path: String) {
    let p = std::path::Path::new(&path);
    if p.is_absolute() && p.exists() {
        let folder = if p.is_dir() { p } else { p.parent().unwrap_or(p) };
        crate::platform::reveal_folder(&folder.to_string_lossy());
    }
}

#[derive(Serialize, Deserialize)]
pub struct ClipboardResult {
    count: usize,
}

#[tauri::command]
pub fn clipboard_mask() -> Result<ClipboardResult, String> {
    Err("not implemented yet".into())
}

#[tauri::command]
pub fn clipboard_unmask() -> Result<ClipboardResult, String> {
    Err("not implemented yet".into())
}
