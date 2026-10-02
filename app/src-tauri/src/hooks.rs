// Claude Code integration: Zuko's pieces in ~/.claude/settings.json.
//
// The rule is strict and is followed to the letter: read the settings file, show
// the diff, take a dated backup, and write only after an explicit click — and
// only the bytes the user looked at (the preview's fingerprint). Uninstalling
// removes exactly what Zuko added and nothing else, so for any settings file x,
// uninstall(install(x)) == x.
//
// Three independent pieces (`InstallOptions`):
// * hooks     — one "zuko-hook" entry per event in HOOK_EVENTS. Entries whose
//               command contains "zuko-hook" are ours; Coucou's "coucou-hook"
//               leftovers are cleaned up too.
// * gateway   — env.ANTHROPIC_BASE_URL → the Zuko gateway. A previous, different
//               base URL becomes the gateway's upstream and is restored verbatim on
//               uninstall. DISABLE_BUG_COMMAND=1 and DISABLE_ERROR_REPORTING=1 are
//               set only when unset (those features upload transcripts straight to
//               Anthropic, around the gateway) and removed only if Zuko set them.
// * denyRules — policy blocked paths and domains mirrored into permissions.deny,
//               which Claude Code enforces itself, even when Zuko is not running.
//
// What Zuko added — rules, env vars, the previous base URL, and every container
// (`hooks`, `env`, `permissions.deny`, …) it had to create — is remembered in an
// install-state file under the local data dir, so the restore is exact. Without
// that file (an older install), containers left empty by the removal are dropped.
//
// The hook command is only the quoted exe path in forward slashes plus the event
// name: on Windows Claude Code runs hook commands through Git Bash, and anything
// with PowerShell or cmd in it breaks.
//
// Paths are injectable for tests and development: ZUKO_CLAUDE_SETTINGS (the
// settings file) and ZUKO_DATA_DIR (the install-state file, via platform).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use tauri::{AppHandle, Manager};

use crate::{platform, settings};

/// Every event Zuko listens to, with the hook timeout written to settings.json.
/// Tool, prompt and session events answer within the relay's 1.5 s budget, well
/// inside 10 s. PermissionRequest waits for a human: the relay's 110 s plus slack.
pub const HOOK_EVENTS: &[(&str, u64)] = &[
    ("SessionStart", 10),
    ("SessionEnd", 10),
    ("UserPromptSubmit", 10),
    ("PreToolUse", 10),
    ("PostToolUse", 10),
    ("PostToolUseFailure", 10),
    ("PermissionRequest", 120),
    ("Notification", 10),
    ("Stop", 10),
    ("StopFailure", 10),
    ("SubagentStart", 10),
    ("SubagentStop", 10),
];

/// Marker that identifies a Zuko entry inside settings.json.
const MARKER: &str = "zuko-hook";
/// Coucou's relay, the app Zuko is forked from: its entries are cleaned up too.
const LEGACY_MARKER: &str = "coucou-hook";
/// Env vars set alongside the gateway, only when the user has not set them.
const PRIVACY_ENV: &[&str] = &["DISABLE_BUG_COMMAND", "DISABLE_ERROR_REPORTING"];
const BASE_URL: &str = "ANTHROPIC_BASE_URL";

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HookStatus {
    pub installed: bool,
    pub settings_path: String,
    pub hook_path: String,
    pub hook_ready: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HookPreview {
    pub diff: String,
    pub backup: String,
    pub settings_path: String,
    /// Identifies the bytes this diff was computed from; handed back to `write`
    /// so we only ever apply what the user actually looked at.
    pub fingerprint: String,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct InstallOptions {
    /// Zuko's hook entries.
    pub hooks: bool,
    /// env.ANTHROPIC_BASE_URL → the Zuko gateway (plus privacy env vars).
    pub gateway: bool,
    /// Policy blocked paths/domains mirrored into permissions.deny.
    pub deny_rules: bool,
}

/// What Zuko changed in settings.json, kept beside it so uninstalling restores
/// the file exactly. Never contains a secret: base URLs and rule strings only.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct InstallState {
    version: u32,
    /// JSON pointers of containers Zuko created (`/hooks`, `/hooks/Stop`, `/env`,
    /// `/permissions`, `/permissions/deny`); dropped again once empty.
    created: Vec<String>,
    gateway: Option<GatewayState>,
    /// permissions.deny is managed by Zuko.
    deny_enabled: bool,
    /// Rules Zuko added (ones the user already had are not listed: they stay).
    deny_rules: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct GatewayState {
    /// The gateway URL Zuko wrote.
    url: String,
    /// ANTHROPIC_BASE_URL before Zuko, restored verbatim on uninstall.
    previous: Option<String>,
    /// Privacy env vars Zuko set (they were unset before).
    added_env: Vec<String>,
}

/// Everything the planner needs that comes from outside settings.json.
#[derive(Clone, Debug, Default)]
struct Inputs {
    /// (event, command, timeout) for each hook entry.
    hooks: Vec<(String, String, u64)>,
    /// The gateway base URL; empty when the gateway is unavailable.
    gateway_url: String,
    /// permissions.deny rules mirroring the policy.
    deny_rules: Vec<String>,
}

/// Where the planner reads and writes. Tests point it at a temp dir.
#[derive(Clone, Debug)]
struct Target {
    settings: PathBuf,
    state: PathBuf,
}

impl Target {
    fn current() -> Target {
        Target { settings: settings_path(), state: state_path() }
    }
}

/// `~/.claude/settings.json`, or `ZUKO_CLAUDE_SETTINGS` when set (absolute path).
pub fn settings_path() -> PathBuf {
    std::env::var_os("ZUKO_CLAUDE_SETTINGS")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| platform::home_dir().join(".claude").join("settings.json"))
}

fn state_path() -> PathBuf {
    platform::local_dir().join("install-state.json")
}

/// Reads the settings file.
///
/// The only error that means "start from nothing" is the file not being there.
/// Everything else — a lock held by another process, a permission problem, JSON
/// we cannot parse — is reported, because the alternative is treating somebody's
/// unreadable settings as an empty object and then writing that back over them.
fn read_settings(path: &Path) -> Result<Value, String> {
    match std::fs::read(path) {
        Ok(bytes) => parse_settings(&bytes, &path.display().to_string()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(json!({})),
        Err(err) => Err(format!("Can't read {}: {err}", path.display())),
    }
}

/// The parsing half of `read_settings`, split out so it can be tested without a
/// home directory.
fn parse_settings(bytes: &[u8], path: &str) -> Result<Value, String> {
    // PowerShell writes a UTF-8 BOM with `Set-Content -Encoding utf8`, and
    // serde_json refuses it. Stripping it is safe and well defined; guessing at
    // anything else is not.
    let text = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
    if text.iter().all(u8::is_ascii_whitespace) {
        return Ok(json!({}));
    }
    match serde_json::from_slice::<Value>(text) {
        Ok(v) if v.is_object() => Ok(v),
        Ok(_) => Err(format!("{path} isn't a JSON object — Zuko won't touch it.")),
        Err(err) => Err(format!(
            "{path} isn't valid JSON ({err}). Fix or move it, then try again — Zuko won't overwrite it."
        )),
    }
}

/// The install state, or `None` when there is none (never installed, or an
/// install from before the state file existed). An unreadable file counts as
/// none: the fallback is the conservative legacy cleanup, never a guess.
fn read_state(path: &Path) -> Option<InstallState> {
    let bytes = std::fs::read(path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn write_state(path: &Path, state: &InstallState) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        platform::ensure_private_dir(dir)?;
    }
    let temp = path.with_extension(format!("json.tmp-{}", std::process::id()));
    let text = serde_json::to_vec_pretty(state).map_err(std::io::Error::other)?;
    std::fs::write(&temp, text)?;
    std::fs::rename(&temp, path)
}

#[cfg(windows)]
fn hook_command(event: &str) -> String {
    let exe = settings::hook_exe_path().to_string_lossy().replace('\\', "/");
    format!("\"{exe}\" {event}")
}

/// Claude Code runs the command through `sh`, which still reads `$`, `` ` ``
/// and `\` inside double quotes. Single quotes keep the path a path, whatever
/// the home directory is called.
#[cfg(unix)]
fn hook_command(event: &str) -> String {
    format!("{} {event}", sh_quote(&settings::hook_exe_path().to_string_lossy()))
}

/// `s` as one single-quoted shell word: `'` becomes `'\''`, nothing else is
/// special inside single quotes.
#[cfg(unix)]
fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

fn entry_matches(entry: &Value, markers: &[&str]) -> bool {
    entry
        .get("hooks")
        .and_then(Value::as_array)
        .map(|hooks| {
            hooks.iter().any(|h| {
                h.get("command")
                    .and_then(Value::as_str)
                    .map(|c| markers.iter().any(|m| c.contains(m)))
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false)
}

/// A Zuko entry (what `status` reports as installed).
fn entry_is_ours(entry: &Value) -> bool {
    entry_matches(entry, &[MARKER])
}

/// A Zuko entry or a Coucou leftover (what uninstall removes).
fn entry_is_removable(entry: &Value) -> bool {
    entry_matches(entry, &[MARKER, LEGACY_MARKER])
}

/// True for the URL shape the Zuko gateway hands out:
/// `http://127.0.0.1:<port>/t/<token>`. Lets a stale gateway URL (old port or
/// token) still be recognised as ours. Same rule as the relay's.
pub fn is_zuko_gateway_url(url: &str) -> bool {
    let url = url.trim();
    let Some(rest) = url
        .strip_prefix("http://127.0.0.1:")
        .or_else(|| url.strip_prefix("http://localhost:"))
    else {
        return false;
    };
    let (port, path) = rest.split_once('/').unwrap_or((rest, ""));
    !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()) && path.starts_with("t/") && path.len() > 2
}

// ── Planning: settings + state + wanted options → new settings + state ───────

/// The result of planning a change.
#[derive(Debug)]
struct Plan {
    settings: Value,
    state: InstallState,
    /// A previous ANTHROPIC_BASE_URL the gateway must forward to.
    upstream: Option<String>,
}

/// Computes the settings with exactly the `want`ed pieces installed. Pure.
fn plan(current: &Value, state: Option<&InstallState>, want: &InstallOptions, inputs: &Inputs) -> Result<Plan, String> {
    let legacy = state.is_none();
    let mut state = state.cloned().unwrap_or_default();
    state.version = 1;
    let mut root = current.as_object().cloned().ok_or("settings.json isn't a JSON object")?;
    let mut upstream = None;

    plan_hooks(&mut root, &mut state, want.hooks, legacy, inputs)?;
    plan_gateway(&mut root, &mut state, want.gateway, legacy, inputs, &mut upstream)?;
    plan_deny(&mut root, &mut state, want.deny_rules, legacy, inputs)?;

    Ok(Plan { settings: Value::Object(root), state, upstream })
}

/// `root[key]` as an object, created (and remembered) when missing. A value of
/// another type is somebody else's data: refuse rather than overwrite it.
fn object_at<'a>(
    root: &'a mut Map<String, Value>,
    key: &str,
    pointer: &str,
    state: &mut InstallState,
) -> Result<&'a mut Map<String, Value>, String> {
    if !root.contains_key(key) {
        root.insert(key.to_string(), json!({}));
        remember(state, pointer);
    }
    root.get_mut(key)
        .and_then(Value::as_object_mut)
        .ok_or_else(|| format!("\"{}\" in settings.json isn't an object — Zuko won't touch it.", pointer.trim_start_matches('/').replace('/', ".")))
}

/// `map[key]` as an array, created (and remembered) when missing.
fn array_at<'a>(
    map: &'a mut Map<String, Value>,
    key: &str,
    pointer: &str,
    state: &mut InstallState,
) -> Result<&'a mut Vec<Value>, String> {
    if !map.contains_key(key) {
        map.insert(key.to_string(), json!([]));
        remember(state, pointer);
    }
    map.get_mut(key)
        .and_then(Value::as_array_mut)
        .ok_or_else(|| format!("\"{}\" in settings.json isn't a list — Zuko won't touch it.", pointer.trim_start_matches('/').replace('/', ".")))
}

fn remember(state: &mut InstallState, pointer: &str) {
    if !state.created.iter().any(|p| p == pointer) {
        state.created.push(pointer.to_string());
    }
}

/// Drops `map[key]` if it is an empty container that Zuko created — or, without
/// an install state, one Zuko's removal just emptied. Keys keep their order.
fn drop_if_empty(map: &mut Map<String, Value>, key: &str, pointer: &str, state: &mut InstallState, legacy: bool, emptied_by_us: bool) {
    let empty = match map.get(key) {
        Some(Value::Object(o)) => o.is_empty(),
        Some(Value::Array(a)) => a.is_empty(),
        _ => false,
    };
    let created = state.created.iter().any(|p| p == pointer);
    if empty && (created || (legacy && emptied_by_us)) {
        map.shift_remove(key);
    }
    if !map.contains_key(key) {
        state.created.retain(|p| p != pointer);
    }
}

fn plan_hooks(root: &mut Map<String, Value>, state: &mut InstallState, install: bool, legacy: bool, inputs: &Inputs) -> Result<(), String> {
    let Some(existing) = root.get("hooks") else {
        if !install {
            return Ok(());
        }
        let hooks = object_at(root, "hooks", "/hooks", state)?;
        for (event, command, timeout) in &inputs.hooks {
            hooks.insert(event.clone(), json!([entry(command, *timeout)]));
            remember(state, &format!("/hooks/{event}"));
        }
        return Ok(());
    };
    if !existing.is_object() {
        return Err("\"hooks\" in settings.json isn't an object — Zuko won't touch it.".into());
    }

    let hooks = root.get_mut("hooks").and_then(Value::as_object_mut).expect("checked above");
    // Remove ours (and Coucou's) everywhere first: a reinstall must not duplicate.
    let mut emptied: Vec<String> = Vec::new();
    for (event, value) in hooks.iter_mut() {
        if let Some(list) = value.as_array_mut() {
            let before = list.len();
            list.retain(|e| !entry_is_removable(e));
            if list.len() != before && list.is_empty() {
                emptied.push(event.clone());
            }
        }
    }
    if install {
        for (event, command, timeout) in &inputs.hooks {
            let pointer = format!("/hooks/{event}");
            let list = array_at(hooks, event, &pointer, state)?;
            list.push(entry(command, *timeout));
        }
    } else {
        let events: Vec<String> = hooks.keys().cloned().collect();
        for event in events {
            let emptied_by_us = emptied.contains(&event);
            drop_if_empty(hooks, &event, &format!("/hooks/{event}"), state, legacy, emptied_by_us);
        }
        let emptied_root = !emptied.is_empty();
        drop_if_empty(root, "hooks", "/hooks", state, legacy, emptied_root);
    }
    Ok(())
}

fn entry(command: &str, timeout: u64) -> Value {
    json!({ "hooks": [{ "type": "command", "command": command, "timeout": timeout }] })
}

fn plan_gateway(
    root: &mut Map<String, Value>,
    state: &mut InstallState,
    install: bool,
    legacy: bool,
    inputs: &Inputs,
    upstream: &mut Option<String>,
) -> Result<(), String> {
    if install {
        let url = inputs.gateway_url.trim();
        if url.is_empty() {
            return Err("The Zuko gateway isn't available, so Claude Code can't be pointed at it yet.".into());
        }
        let env = object_at(root, "env", "/env", state)?;
        let current = env.get(BASE_URL).and_then(Value::as_str).map(str::to_string);
        let mut gw = state.gateway.take().unwrap_or_default();
        // Whatever is there and is not a Zuko gateway is the user's own proxy:
        // it becomes the upstream and is restored on uninstall.
        match current {
            Some(c) if c != url && c != gw.url && !is_zuko_gateway_url(&c) => gw.previous = Some(c),
            _ if gw.url.is_empty() => gw.previous = None,
            _ => {}
        }
        if let Some(prev) = &gw.previous {
            *upstream = Some(prev.clone());
        }
        gw.url = url.to_string();
        env.insert(BASE_URL.into(), Value::String(url.to_string()));
        for var in PRIVACY_ENV {
            if !env.contains_key(*var) {
                env.insert((*var).into(), json!("1"));
                if !gw.added_env.iter().any(|v| v == var) {
                    gw.added_env.push((*var).to_string());
                }
            }
        }
        state.gateway = Some(gw);
        return Ok(());
    }

    let Some(env) = root.get_mut("env") else {
        state.gateway = None;
        return Ok(());
    };
    let Some(env) = env.as_object_mut() else {
        return Err("\"env\" in settings.json isn't an object — Zuko won't touch it.".into());
    };
    let gw = state.gateway.take();
    let ours = |v: &str| gw.as_ref().map(|g| g.url == v).unwrap_or(false) || is_zuko_gateway_url(v);
    let mut removed = false;
    if env.get(BASE_URL).and_then(Value::as_str).map(ours).unwrap_or(false) {
        match gw.as_ref().and_then(|g| g.previous.clone()) {
            Some(prev) => {
                env.insert(BASE_URL.into(), Value::String(prev));
            }
            None => {
                env.shift_remove(BASE_URL);
                removed = true;
            }
        }
    }
    if let Some(g) = &gw {
        for var in &g.added_env {
            // Only if still Zuko's value; a user who changed it keeps the change.
            if env.get(var).and_then(Value::as_str) == Some("1") {
                env.shift_remove(var);
                removed = true;
            }
        }
    }
    drop_if_empty(root, "env", "/env", state, legacy, removed);
    Ok(())
}

fn plan_deny(root: &mut Map<String, Value>, state: &mut InstallState, install: bool, legacy: bool, inputs: &Inputs) -> Result<(), String> {
    if install {
        let permissions = object_at(root, "permissions", "/permissions", state)?;
        let deny = array_at(permissions, "deny", "/permissions/deny", state)?;
        // Rules Zuko added for a policy that has since changed go first.
        let stale: Vec<String> = state.deny_rules.iter().filter(|r| !inputs.deny_rules.contains(r)).cloned().collect();
        deny.retain(|v| !v.as_str().map(|s| stale.iter().any(|r| r == s)).unwrap_or(false));
        state.deny_rules.retain(|r| !stale.contains(r));
        for rule in &inputs.deny_rules {
            let present = deny.iter().any(|v| v.as_str() == Some(rule.as_str()));
            if !present {
                deny.push(Value::String(rule.clone()));
                state.deny_rules.push(rule.clone());
            }
        }
        state.deny_enabled = true;
        return Ok(());
    }

    let added = std::mem::take(&mut state.deny_rules);
    state.deny_enabled = false;
    let Some(permissions) = root.get_mut("permissions") else { return Ok(()) };
    let Some(permissions) = permissions.as_object_mut() else {
        return Err("\"permissions\" in settings.json isn't an object — Zuko won't touch it.".into());
    };
    let mut removed = false;
    if let Some(deny) = permissions.get_mut("deny") {
        let Some(deny) = deny.as_array_mut() else {
            return Err("\"permissions.deny\" in settings.json isn't a list — Zuko won't touch it.".into());
        };
        let before = deny.len();
        deny.retain(|v| !v.as_str().map(|s| added.iter().any(|r| r == s)).unwrap_or(false));
        removed = deny.len() != before;
        drop_if_empty(permissions, "deny", "/permissions/deny", state, legacy, removed);
    }
    drop_if_empty(root, "permissions", "/permissions", state, legacy, removed);
    Ok(())
}

/// permissions.deny rules mirroring the policy's blocked paths and domains.
///
/// Claude Code's path rules are gitignore patterns where `//` is the filesystem
/// root and Windows paths are POSIX-ised (`C:\x` → `//c/x`); `~/` is home; a
/// pattern without a slash would only match in the current directory, so it is
/// anchored at `//**/` to keep the policy's "any depth" meaning. A `Read` rule
/// also covers edits, so blocked reads need no `Edit` twin. Domains become
/// `WebFetch(domain:…)`; a plain domain also gets its `*.` twin, because the
/// policy's `example.com` covers subdomains and Claude Code's does not.
pub fn deny_rules_for(policy: &zuko_core::policy::Policy, windows: bool) -> Vec<String> {
    let mut rules: Vec<String> = Vec::new();
    let mut push = |r: String| {
        if !rules.contains(&r) {
            rules.push(r);
        }
    };
    for p in &policy.filesystem.blocked_read {
        if let Some(path) = rule_path(p, windows) {
            push(format!("Read({path})"));
        }
    }
    for p in &policy.filesystem.blocked_write {
        if let Some(path) = rule_path(p, windows) {
            push(format!("Edit({path})"));
        }
    }
    for d in &policy.network.blocked {
        let d = d.trim().trim_end_matches('.').to_lowercase();
        if d.is_empty() || d.contains(['/', '(', ')', ' ']) {
            continue;
        }
        push(format!("WebFetch(domain:{d})"));
        let is_ip = d.parse::<std::net::IpAddr>().is_ok();
        if !d.starts_with("*.") && !is_ip {
            push(format!("WebFetch(domain:*.{d})"));
        }
    }
    rules
}

/// A policy path glob as a Claude Code permission path, or `None` when it does not
/// apply on this platform (a drive path on Linux, a root path on Windows).
fn rule_path(pattern: &str, windows: bool) -> Option<String> {
    let p = pattern.trim().replace('\\', "/");
    if p.is_empty() {
        return None;
    }
    let bytes = p.as_bytes();
    if p.starts_with("~/") {
        return Some(p);
    }
    if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        if !windows {
            return None;
        }
        let drive = (bytes[0] as char).to_ascii_lowercase();
        let rest = p[2..].trim_start_matches('/');
        return Some(format!("//{drive}/{rest}"));
    }
    if p.starts_with("//") {
        return Some(p);
    }
    if p.starts_with('/') {
        return (!windows).then(|| format!("/{p}"));
    }
    if p.starts_with("**") {
        return Some(format!("//{p}"));
    }
    if !p.contains('/') {
        return Some(format!("//**/{p}"));
    }
    Some(p)
}

// ── Reading, previewing, writing ──────────────────────────────────────────────

/// Gathers the outside inputs the wanted options need (gateway URL, policy).
fn inputs_for(want: &InstallOptions) -> Inputs {
    Inputs {
        hooks: HOOK_EVENTS
            .iter()
            .map(|(e, t)| ((*e).to_string(), hook_command(e), *t))
            .collect(),
        gateway_url: if want.gateway { crate::gateway::base_url() } else { String::new() },
        deny_rules: if want.deny_rules {
            deny_rules_for(&crate::policystore::load(), cfg!(windows))
        } else {
            Vec::new()
        },
    }
}

fn pretty(v: &Value) -> String {
    serde_json::to_string_pretty(v).unwrap_or_default()
}

/// Down to the second: installing then uninstalling in the same minute must not
/// quietly overwrite the first backup.
fn stamp() -> String {
    let t = platform::local_time();
    format!(
        "{:04}{:02}{:02}-{:02}{:02}{:02}",
        t.year, t.month, t.day, t.hour, t.minute, t.second
    )
}

fn backup_path(settings: &Path) -> PathBuf {
    settings.with_file_name(format!("settings.json.bak-{}", stamp()))
}

/// Identifies the exact bytes a preview was computed from. FNV-1a is plenty:
/// the question is only "is this still the file I showed the user?".
fn fingerprint(bytes: &[u8]) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        hash ^= *b as u64;
        hash = hash.wrapping_mul(0x1000_0000_01b3);
    }
    format!("{hash:016x}")
}

fn current_fingerprint(path: &Path) -> String {
    match std::fs::read(path) {
        Ok(bytes) => fingerprint(&bytes),
        Err(_) => fingerprint(b""),
    }
}

fn preview_at(target: &Target, want: &InstallOptions, inputs: &Inputs) -> Result<HookPreview, String> {
    let current = read_settings(&target.settings)?;
    let state = read_state(&target.state);
    let next = plan(&current, state.as_ref(), want, inputs)?;
    Ok(HookPreview {
        diff: unified_diff(&pretty(&current), &pretty(&next.settings)),
        backup: backup_path(&target.settings).to_string_lossy().to_string(),
        settings_path: target.settings.to_string_lossy().to_string(),
        fingerprint: current_fingerprint(&target.settings),
    })
}

/// Writes the planned settings after taking a dated backup. Returns the backup
/// path (empty when nothing changed) and the upstream the gateway must use.
///
/// `fingerprint` is the one the preview was computed from. If the file changed
/// in between — another tool, another window, the user's own editor — we stop
/// and make them look at a fresh diff, because the only thing worse than not
/// installing is silently reverting somebody else's edit.
fn write_at(target: &Target, want: &InstallOptions, fingerprint: &str, inputs: &Inputs) -> Result<(String, Option<String>), String> {
    let path = &target.settings;
    // Read before the backup: an unreadable file must abort before we touch
    // anything at all.
    let current = read_settings(path)?;
    if current_fingerprint(path) != fingerprint {
        return Err(format!(
            "{} changed since the preview. Nothing was written — review the new diff.",
            path.display()
        ));
    }
    let state = read_state(&target.state);
    let next = plan(&current, state.as_ref(), want, inputs)?;
    if next.settings == current && path.exists() {
        save_state(target, &next.state);
        return Ok((String::new(), next.upstream));
    }

    let dir = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let backup = backup_path(path);
    if path.exists() {
        std::fs::copy(path, &backup).map_err(|e| format!("backup failed: {e}"))?;
    }

    let mut text = pretty(&next.settings);
    text.push('\n');

    // A dotfiles setup often makes settings.json a symlink: write to the file it
    // points at, so the link survives the rename below.
    #[cfg(unix)]
    let path = &std::fs::canonicalize(path).unwrap_or_else(|_| path.clone());

    // Write beside the target and rename over it: a crash or a full disk leaves
    // the original settings.json intact rather than half a file.
    let temp = path.with_extension(format!("json.zuko-{}", std::process::id()));
    if let Err(err) = write_like(&temp, path, text.as_bytes()) {
        let _ = std::fs::remove_file(&temp);
        return Err(format!("write failed: {err}"));
    }
    if let Err(err) = std::fs::rename(&temp, path) {
        let _ = std::fs::remove_file(&temp);
        return Err(format!("write failed: {err}"));
    }
    save_state(target, &next.state);
    Ok((backup.to_string_lossy().to_string(), next.upstream))
}

/// A state that records nothing is removed rather than kept as an empty file.
fn save_state(target: &Target, state: &InstallState) {
    let empty = state.created.is_empty() && state.gateway.is_none() && !state.deny_enabled && state.deny_rules.is_empty();
    let result = if empty {
        match std::fs::remove_file(&target.state) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    } else {
        write_state(&target.state, state)
    };
    if let Err(err) = result {
        crate::log::line(format!("could not save the install state: {err}"));
    }
}

fn installed_at(target: &Target) -> InstallOptions {
    let current = read_settings(&target.settings).unwrap_or_else(|_| json!({}));
    let state = read_state(&target.state).unwrap_or_default();
    InstallOptions {
        hooks: hooks_present(&current),
        gateway: current
            .pointer("/env/ANTHROPIC_BASE_URL")
            .and_then(Value::as_str)
            .map(|u| state.gateway.as_ref().map(|g| g.url == u).unwrap_or(false) || is_zuko_gateway_url(u))
            .unwrap_or(false),
        deny_rules: state.deny_enabled,
    }
}

fn hooks_present(settings: &Value) -> bool {
    settings
        .get("hooks")
        .and_then(Value::as_object)
        .map(|hooks| hooks.values().filter_map(Value::as_array).flatten().any(entry_is_ours))
        .unwrap_or(false)
}

// ── Public API ────────────────────────────────────────────────────────────────

pub fn status() -> HookStatus {
    let path = settings_path();
    let installed = hooks_present(&read_settings(&path).unwrap_or_else(|_| json!({})));
    let hook_path = settings::hook_exe_path();
    HookStatus {
        installed,
        settings_path: path.to_string_lossy().to_string(),
        hook_ready: hook_path.exists(),
        hook_path: hook_path.to_string_lossy().to_string(),
    }
}

/// What is currently installed: (hooks, gateway, deny rules).
pub fn installed_options() -> InstallOptions {
    installed_at(&Target::current())
}

/// settings.json routes Claude Code through a Zuko gateway (read on every
/// UserPromptSubmit to spot sessions that bypass it; one small file read).
pub fn gateway_configured() -> bool {
    installed_options().gateway
}

pub fn preview_options(options: &InstallOptions) -> Result<HookPreview, String> {
    preview_at(&Target::current(), options, &inputs_for(options))
}

/// Only ever called from an explicit click (see `write_at`).
pub fn write_options(options: &InstallOptions, fingerprint: &str) -> Result<String, String> {
    let (backup, upstream) = write_at(&Target::current(), options, fingerprint, &inputs_for(options))?;
    if let Some(url) = upstream {
        crate::gateway::set_upstream(&url);
    }
    Ok(backup)
}

/// The legacy hooks-only switch: the other pieces stay as they are.
pub fn preview(install: bool) -> Result<HookPreview, String> {
    preview_options(&InstallOptions { hooks: install, ..installed_options() })
}

pub fn write(install: bool, fingerprint: &str) -> Result<String, String> {
    write_options(&InstallOptions { hooks: install, ..installed_options() }, fingerprint)
}

/// Writes `bytes` to `temp`, which is about to replace `original`.
///
/// On Linux a fresh file would get the umask's 0644, and settings.json can hold
/// API keys in its `env` block: the new file is created readable by us only,
/// then given the original's permissions, so the rename never widens them.
fn write_like(temp: &Path, original: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut file = options.open(temp)?;
    file.write_all(bytes)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(original)
            .map(|m| m.permissions().mode() & 0o777)
            .unwrap_or(0o600);
        file.set_permissions(std::fs::Permissions::from_mode(mode))?;
    }
    #[cfg(not(unix))]
    let _ = original;
    Ok(())
}

/// Copies the relay (zuko-hook.exe / zuko-hook) into the local data dir's
/// bin/ on launch. In a bundled install it comes from the app resources; in
/// `tauri dev` it sits next to the app binary in the workspace target directory.
///
/// Every candidate is tried rather than just the first, because getting this
/// wrong is silent and fatal: `resources` used to be a glob, which made NSIS
/// mirror the source path into `_up_\target\release\`, no candidate matched, and
/// the relay was simply never installed. It only looked healthy on a developer
/// machine, where a leftover copy from `tauri dev` was already sitting in bin/.
pub fn ensure_hook_exe(app: &AppHandle) {
    let dest = settings::hook_exe_path();
    let Some(dir) = dest.parent() else { return };
    // Nobody else may swap the relay Claude Code runs: its folder is ours only.
    if platform::ensure_private_dir(&settings::local_dir()).is_err()
        || std::fs::create_dir_all(dir).is_err()
    {
        return;
    }

    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(p) = app.path().resolve(platform::HOOK_EXE, tauri::path::BaseDirectory::Resource) {
        candidates.push(p);
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            // Installed build, then `tauri dev` (target/debug) next to the
            // release hook the pre-build step produces.
            candidates.push(parent.join(platform::HOOK_EXE));
            candidates.push(parent.join("../release").join(platform::HOOK_EXE));
            // Belt and braces: where the old glob form used to land it.
            candidates.push(parent.join("_up_/target/release").join(platform::HOOK_EXE));
        }
    }

    let tried: Vec<String> = candidates.iter().map(|p| p.display().to_string()).collect();
    let Some(src) = candidates.into_iter().find(|p| p.exists()) else {
        crate::log::line(format!(
            "{} not found — Claude Code hooks cannot work. Looked in: {}",
            platform::HOOK_EXE,
            tried.join(", ")
        ));
        return;
    };
    install_relay(&src, &dest);
}

#[cfg(windows)]
fn install_relay(src: &Path, dest: &Path) {
    let same = match (std::fs::metadata(src), std::fs::metadata(dest)) {
        (Ok(a), Ok(b)) => a.len() == b.len() && a.modified().ok() == b.modified().ok(),
        _ => false,
    };
    if same {
        return;
    }
    // A hook may be running right now and hold the file open; keeping the old
    // copy is fine, it is the same relay.
    if let Err(err) = std::fs::copy(src, dest) {
        if !dest.exists() {
            crate::log::line(format!("could not install {}: {err}", platform::HOOK_EXE));
        }
    }
}

/// Linux does not keep the modification time on copy, so the contents decide.
/// The new relay is written beside the old one and renamed over it: a hook
/// starting at that moment runs either the old relay or the new one, never half
/// of one, and a relay that is running right now does not block the update.
#[cfg(unix)]
fn install_relay(src: &Path, dest: &Path) {
    use std::os::unix::fs::PermissionsExt;
    if matches!((std::fs::read(src), std::fs::read(dest)), (Ok(a), Ok(b)) if a == b) {
        return;
    }
    let temp = dest.with_extension(format!("new-{}", std::process::id()));
    let result = std::fs::copy(src, &temp)
        .and_then(|_| std::fs::set_permissions(&temp, std::fs::Permissions::from_mode(0o755)))
        .and_then(|_| std::fs::rename(&temp, dest));
    if let Err(err) = result {
        let _ = std::fs::remove_file(&temp);
        crate::log::line(format!("could not install {}: {err}", platform::HOOK_EXE));
    }
}

// ── Minimal unified diff (LCS) ────────────────────────────────────────────────

/// settings.json is short, so a plain O(n·m) LCS is the simplest honest diff.
fn unified_diff(before: &str, after: &str) -> String {
    let a: Vec<&str> = before.lines().collect();
    let b: Vec<&str> = after.lines().collect();
    let (n, m) = (a.len(), b.len());

    let mut lcs = vec![vec![0usize; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            lcs[i][j] = if a[i] == b[j] {
                lcs[i + 1][j + 1] + 1
            } else {
                lcs[i + 1][j].max(lcs[i][j + 1])
            };
        }
    }

    let mut out: Vec<String> = Vec::new();
    let (mut i, mut j) = (0usize, 0usize);
    while i < n && j < m {
        if a[i] == b[j] {
            out.push(format!("  {}", a[i]));
            i += 1;
            j += 1;
        } else if lcs[i + 1][j] >= lcs[i][j + 1] {
            out.push(format!("- {}", a[i]));
            i += 1;
        } else {
            out.push(format!("+ {}", b[j]));
            j += 1;
        }
    }
    while i < n {
        out.push(format!("- {}", a[i]));
        i += 1;
    }
    while j < m {
        out.push(format!("+ {}", b[j]));
        j += 1;
    }

    // Keep three lines of context around each change so the panel stays readable.
    let changed: Vec<usize> = out
        .iter()
        .enumerate()
        .filter(|(_, l)| l.starts_with('+') || l.starts_with('-'))
        .map(|(i, _)| i)
        .collect();
    if changed.is_empty() {
        return "No change.".into();
    }
    let mut keep = vec![false; out.len()];
    for idx in changed {
        let lo = idx.saturating_sub(3);
        let hi = (idx + 4).min(out.len());
        for k in lo..hi {
            keep[k] = true;
        }
    }
    let mut result = String::new();
    let mut gap = false;
    for (idx, line) in out.iter().enumerate() {
        if keep[idx] {
            result.push_str(line);
            result.push('\n');
            gap = false;
        } else if !gap {
            result.push_str("  …\n");
            gap = true;
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use zuko_core::policy::Policy;

    const WHERE: &str = "settings.json";
    const GW: &str = "http://127.0.0.1:47821/t/0123abcd";

    fn inputs() -> Inputs {
        Inputs {
            hooks: HOOK_EVENTS
                .iter()
                .map(|(e, t)| ((*e).to_string(), format!("\"C:/Users/a/AppData/Local/Zuko/bin/zuko-hook.exe\" {e}"), *t))
                .collect(),
            gateway_url: GW.into(),
            deny_rules: deny_rules_for(&Policy::default(), true),
        }
    }

    const ALL: InstallOptions = InstallOptions { hooks: true, gateway: true, deny_rules: true };
    const NONE: InstallOptions = InstallOptions { hooks: false, gateway: false, deny_rules: false };

    fn combos() -> Vec<InstallOptions> {
        let mut v = Vec::new();
        for bits in 0..8u8 {
            v.push(InstallOptions { hooks: bits & 1 != 0, gateway: bits & 2 != 0, deny_rules: bits & 4 != 0 });
        }
        v
    }

    /// Settings files in the shapes people actually have.
    fn shapes() -> Vec<Value> {
        vec![
            json!({}),
            json!({"model": "claude-opus-5", "theme": "dark"}),
            json!({"hooks": {}}),
            json!({"hooks": {"Stop": []}}),
            json!({"hooks": {
                "PreToolUse": [{"matcher": "Bash", "hooks": [{"type": "command", "command": "other-tool.exe"}]}],
                "SomeEventWeDoNotTouch": [{"hooks": [{"type": "command", "command": "keep-me.exe"}]}],
            }, "model": "x"}),
            json!({"env": {}}),
            json!({"env": {"ANTHROPIC_BASE_URL": "https://proxy.corp.example/anthropic", "FOO": "bar"}}),
            json!({"env": {"DISABLE_BUG_COMMAND": "0", "OTHER": "1"}}),
            json!({"env": {"DISABLE_ERROR_REPORTING": "1"}}),
            json!({"permissions": {"allow": ["Bash(npm test)"]}}),
            json!({"permissions": {"deny": []}}),
            json!({"permissions": {"deny": ["WebFetch(domain:pastebin.com)", "Read(~/.ssh/**)", "Bash(rm -rf *)"], "ask": ["Bash(git push*)"]}}),
            json!({"permissions": {"defaultMode": "acceptEdits"}, "env": {"X": "y"}, "hooks": {"Notification": [{"hooks": [{"type": "command", "command": "notify.sh"}]}]}}),
            json!({
                "$schema": "https://json.schemastore.org/claude-code-settings.json",
                "enabledPlugins": {"a@b": true},
                "statusLine": {"type": "command", "command": "echo é ✓"},
                "env": {"ANTHROPIC_BASE_URL": "http://localhost:8080", "DISABLE_BUG_COMMAND": "1"},
                "permissions": {"deny": ["Edit(//c/Windows/**)"], "additionalDirectories": ["D:/work"]},
                "hooks": {"PostToolUse": [{"matcher": "Write|Edit", "hooks": [{"type": "command", "command": "fmt"}]}]},
                "nested": {"deep": [1, {"x": null}, 2.5, false]},
            }),
        ]
    }

    #[test]
    fn uninstall_restores_every_shape_exactly() {
        let inputs = inputs();
        for shape in shapes() {
            for want in combos() {
                let installed = plan(&shape, None, &want, &inputs).unwrap();
                // Idempotent: installing again changes nothing.
                let again = plan(&installed.settings, Some(&installed.state), &want, &inputs).unwrap();
                assert_eq!(pretty(&again.settings), pretty(&installed.settings), "reinstall of {want:?} on {shape}");
                let removed = plan(&installed.settings, Some(&again.state), &NONE, &inputs).unwrap();
                assert_eq!(
                    pretty(&removed.settings),
                    pretty(&shape),
                    "uninstall(install({want:?})) differs for {shape}"
                );
                assert!(removed.state.created.is_empty(), "{:?}", removed.state);
                assert!(removed.state.gateway.is_none() && removed.state.deny_rules.is_empty());
            }
        }
    }

    #[test]
    fn pieces_come_and_go_independently() {
        let inputs = inputs();
        let shape = json!({"model": "m", "env": {"FOO": "1"}});
        let all = plan(&shape, None, &ALL, &inputs).unwrap();
        // Drop only the gateway: hooks and rules stay, the env is the user's again.
        let partial = plan(&all.settings, Some(&all.state), &InstallOptions { gateway: false, ..ALL }, &inputs).unwrap();
        assert!(hooks_present(&partial.settings));
        assert_eq!(partial.settings["env"], json!({"FOO": "1"}));
        assert!(partial.settings["permissions"]["deny"].as_array().unwrap().len() > 3);
        let none = plan(&partial.settings, Some(&partial.state), &NONE, &inputs).unwrap();
        assert_eq!(pretty(&none.settings), pretty(&shape));
    }

    #[test]
    fn the_gateway_keeps_the_users_proxy_as_upstream_and_restores_it() {
        let inputs = inputs();
        let shape = json!({"env": {"ANTHROPIC_BASE_URL": "https://proxy.corp.example/anthropic", "DISABLE_BUG_COMMAND": "0"}});
        let p = plan(&shape, None, &InstallOptions { gateway: true, ..NONE }, &inputs).unwrap();
        assert_eq!(p.settings["env"]["ANTHROPIC_BASE_URL"], GW);
        assert_eq!(p.upstream.as_deref(), Some("https://proxy.corp.example/anthropic"));
        // The user's own choice for DISABLE_BUG_COMMAND is respected; the other is added.
        assert_eq!(p.settings["env"]["DISABLE_BUG_COMMAND"], "0");
        assert_eq!(p.settings["env"]["DISABLE_ERROR_REPORTING"], "1");
        assert_eq!(p.state.gateway.as_ref().unwrap().added_env, vec!["DISABLE_ERROR_REPORTING".to_string()]);

        // The gateway moved to another port: the stored upstream survives.
        let moved = Inputs { gateway_url: "http://127.0.0.1:47900/t/ffff".into(), ..inputs.clone() };
        let p2 = plan(&p.settings, Some(&p.state), &InstallOptions { gateway: true, ..NONE }, &moved).unwrap();
        assert_eq!(p2.settings["env"]["ANTHROPIC_BASE_URL"], "http://127.0.0.1:47900/t/ffff");
        assert_eq!(p2.upstream.as_deref(), Some("https://proxy.corp.example/anthropic"));
        let back = plan(&p2.settings, Some(&p2.state), &NONE, &moved).unwrap();
        assert_eq!(pretty(&back.settings), pretty(&shape));

        // A stale Zuko URL left without state is never mistaken for the user's proxy.
        let stale = json!({"env": {"ANTHROPIC_BASE_URL": "http://127.0.0.1:1234/t/old"}});
        let p3 = plan(&stale, None, &InstallOptions { gateway: true, ..NONE }, &inputs).unwrap();
        assert!(p3.upstream.is_none());
        let off = plan(&stale, None, &NONE, &inputs).unwrap();
        assert!(off.settings.get("env").is_none() || off.settings["env"].get("ANTHROPIC_BASE_URL").is_none());

        // No gateway, no install.
        let none = Inputs { gateway_url: String::new(), ..inputs };
        assert!(plan(&json!({}), None, &InstallOptions { gateway: true, ..NONE }, &none).is_err());
    }

    #[test]
    fn deny_rules_mirror_the_policy_in_claude_codes_syntax() {
        let rules = deny_rules_for(&Policy::default(), true);
        for want in [
            "Read(~/.ssh/**)",
            "Edit(//c/Windows/**)",
            "Edit(//c/Program Files (x86)/**)",
            "WebFetch(domain:pastebin.com)",
            "WebFetch(domain:*.pastebin.com)",
            "WebFetch(domain:*.ngrok.io)",
        ] {
            assert!(rules.iter().any(|r| r == want), "missing {want} in {rules:?}");
        }
        assert!(!rules.iter().any(|r| r.contains("*.*.")), "{rules:?}");
        assert!(!rules.iter().any(|r| r.contains("//etc")), "root paths do not apply on Windows");
        let linux = deny_rules_for(&Policy::default(), false);
        assert!(linux.iter().any(|r| r == "Edit(//etc/**)"));
        assert!(!linux.iter().any(|r| r.contains("//c/")));

        assert_eq!(rule_path("D:\\Personal\\**", true).as_deref(), Some("//d/Personal/**"));
        assert_eq!(rule_path("D:/Personal/**", true).as_deref(), Some("//d/Personal/**"));
        assert_eq!(rule_path(".env", true).as_deref(), Some("//**/.env"));
        assert_eq!(rule_path("**/secrets/**", true).as_deref(), Some("//**/secrets/**"));
        assert_eq!(rule_path("~/x", false).as_deref(), Some("~/x"));
        assert_eq!(rule_path("  ", true), None);
        let mut p = Policy::default();
        p.network.blocked = vec!["10.0.0.5".into(), "Evil.Example.".into()];
        let r = deny_rules_for(&p, true);
        assert!(r.contains(&"WebFetch(domain:10.0.0.5)".to_string()));
        assert!(!r.iter().any(|x| x == "WebFetch(domain:*.10.0.0.5)"));
        assert!(r.contains(&"WebFetch(domain:*.evil.example)".to_string()));
    }

    #[test]
    fn a_policy_change_swaps_only_zukos_rules() {
        let mut inputs = inputs();
        inputs.deny_rules = vec!["Read(//d/a/**)".into(), "Read(//d/b/**)".into()];
        let shape = json!({"permissions": {"deny": ["Bash(curl *)", "Read(//d/b/**)"]}});
        let want = InstallOptions { deny_rules: true, ..NONE };
        let p = plan(&shape, None, &want, &inputs).unwrap();
        assert_eq!(p.settings["permissions"]["deny"], json!(["Bash(curl *)", "Read(//d/b/**)", "Read(//d/a/**)"]));
        assert_eq!(p.state.deny_rules, vec!["Read(//d/a/**)".to_string()], "the user's own rule is not ours");
        inputs.deny_rules = vec!["Read(//d/b/**)".into(), "Read(//d/c/**)".into()];
        let p2 = plan(&p.settings, Some(&p.state), &want, &inputs).unwrap();
        assert_eq!(p2.settings["permissions"]["deny"], json!(["Bash(curl *)", "Read(//d/b/**)", "Read(//d/c/**)"]));
        let off = plan(&p2.settings, Some(&p2.state), &NONE, &inputs).unwrap();
        assert_eq!(off.settings, shape);
    }

    #[test]
    fn other_peoples_data_of_the_wrong_type_is_refused_not_overwritten() {
        let inputs = inputs();
        assert!(plan(&json!({"hooks": []}), None, &ALL, &inputs).is_err());
        assert!(plan(&json!({"hooks": {"PreToolUse": {"x": 1}}}), None, &ALL, &inputs).is_err());
        assert!(plan(&json!({"env": "x"}), None, &InstallOptions { gateway: true, ..NONE }, &inputs).is_err());
        assert!(plan(&json!({"permissions": {"deny": "x"}}), None, &InstallOptions { deny_rules: true, ..NONE }, &inputs).is_err());
        assert!(plan(&json!({"permissions": 3}), None, &InstallOptions { deny_rules: true, ..NONE }, &inputs).is_err());
    }

    #[test]
    fn coucou_leftovers_and_stateless_installs_are_cleaned_up() {
        let inputs = inputs();
        let shape = json!({"hooks": {
            "Stop": [
                {"hooks": [{"type": "command", "command": "\"C:/old/coucou-hook.exe\" Stop"}]},
                {"hooks": [{"type": "command", "command": "mine.exe"}]},
            ],
            "PreToolUse": [{"hooks": [{"type": "command", "command": "\"C:/x/zuko-hook.exe\" PreToolUse"}]}],
        }, "a": 1});
        // No install state: lists emptied by the removal are dropped.
        let off = plan(&shape, None, &NONE, &inputs).unwrap();
        assert_eq!(off.settings, json!({"hooks": {"Stop": [{"hooks": [{"type": "command", "command": "mine.exe"}]}]}, "a": 1}));
        // Installing replaces the leftovers instead of adding next to them.
        let on = plan(&shape, None, &InstallOptions { hooks: true, ..NONE }, &inputs).unwrap();
        let stop = on.settings["hooks"]["Stop"].as_array().unwrap();
        assert_eq!(stop.len(), 2);
        assert!(!pretty(&on.settings).contains("coucou-hook"));
        assert_eq!(on.settings["hooks"]["PreToolUse"].as_array().unwrap().len(), 1);
        assert_eq!(on.settings["hooks"]["PermissionRequest"][0]["hooks"][0]["timeout"], 120);
        assert_eq!(on.settings["hooks"]["PreToolUse"][0]["hooks"][0]["timeout"], 10);
    }

    #[test]
    fn a_utf8_bom_is_stripped_not_treated_as_corruption() {
        // PowerShell 5's `Set-Content -Encoding utf8` produces exactly this.
        let mut bytes = vec![0xEF, 0xBB, 0xBF];
        bytes.extend_from_slice(br#"{"model":"opus","hooks":{}}"#);
        let parsed = parse_settings(&bytes, WHERE).expect("a BOM must not defeat the parser");
        assert_eq!(parsed["model"], "opus");
    }

    #[test]
    fn unreadable_content_is_an_error_never_an_empty_object() {
        // Returning {} here would mean writing a file containing nothing but Zuko's
        // pieces over everything the user had.
        for bad in [&b"{ not json"[..], &b"[1,2,3]"[..], &b"\"a string\""[..]] {
            assert!(parse_settings(bad, WHERE).is_err(), "content we cannot use must refuse, not come back empty");
        }
    }

    #[test]
    fn empty_and_whitespace_files_start_from_nothing() {
        assert_eq!(parse_settings(b"", WHERE).unwrap(), json!({}));
        assert_eq!(parse_settings(b"  \n\t ", WHERE).unwrap(), json!({}));
    }

    #[test]
    fn a_fingerprint_notices_any_change() {
        assert_eq!(fingerprint(b"{}"), fingerprint(b"{}"));
        assert_ne!(fingerprint(b"{}"), fingerprint(b"{ }"));
        assert_ne!(fingerprint(b""), fingerprint(b"{}"));
    }

    #[test]
    fn gateway_urls_are_recognised_by_shape() {
        assert!(is_zuko_gateway_url(GW));
        assert!(!is_zuko_gateway_url("http://127.0.0.1:47821/v1"));
        assert!(!is_zuko_gateway_url("https://api.anthropic.com"));
    }

    #[test]
    fn the_settings_path_can_be_pointed_elsewhere() {
        let elsewhere = std::env::temp_dir().join("zuko-settings-override").join("settings.json");
        std::env::set_var("ZUKO_CLAUDE_SETTINGS", &elsewhere);
        assert_eq!(settings_path(), elsewhere);
        std::env::set_var("ZUKO_CLAUDE_SETTINGS", "relative/settings.json");
        assert!(settings_path().ends_with(".claude/settings.json") || settings_path().ends_with(".claude\\settings.json"));
        std::env::remove_var("ZUKO_CLAUDE_SETTINGS");
    }

    #[cfg(unix)]
    #[test]
    fn the_hook_path_is_one_shell_word_whatever_it_contains() {
        assert_eq!(sh_quote("/home/a b/x"), "'/home/a b/x'");
        // $, backticks, backslashes and double quotes stay literal in single quotes.
        assert_eq!(sh_quote(r#"/h/$(id)`x`\"y"#), r#"'/h/$(id)`x`\"y'"#);
        // A single quote closes, escapes and reopens.
        assert_eq!(sh_quote("/h/it's"), r"'/h/it'\''s'");
    }

    /// settings.json can carry API keys in its `env` block: rewriting it must
    /// never make it readable by more people than before.
    #[cfg(unix)]
    #[test]
    fn rewriting_settings_never_widens_its_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("zuko-perm-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let original = dir.join("settings.json");
        let temp = dir.join("settings.json.new");
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;

        for wanted in [0o600, 0o640, 0o644] {
            std::fs::write(&original, b"{}").unwrap();
            std::fs::set_permissions(&original, std::fs::Permissions::from_mode(wanted)).unwrap();
            let _ = std::fs::remove_file(&temp);
            write_like(&temp, &original, b"{\"a\":1}").unwrap();
            assert_eq!(mode(&temp), wanted, "the rewrite must keep {wanted:o}");
        }

        // No original: ours only.
        std::fs::remove_file(&original).unwrap();
        let _ = std::fs::remove_file(&temp);
        write_like(&temp, &original, b"{}").unwrap();
        assert_eq!(mode(&temp), 0o600);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The whole file flow against a temp dir: nothing here touches the real
    /// ~/.claude or %LOCALAPPDATA%.
    #[test]
    fn writing_backs_up_preserves_restores_and_refuses_a_changed_file() {
        let tmp = std::env::temp_dir().join(format!("zuko-hooks-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join(".claude")).unwrap();
        let target = Target { settings: tmp.join(".claude").join("settings.json"), state: tmp.join("data").join("install-state.json") };
        let inputs = inputs();

        // A real-shaped file, written the way PowerShell 5 would: UTF-8 with BOM.
        let original = r#"{"model":"claude-opus-5","env":{"ANTHROPIC_BASE_URL":"https://proxy.corp.example"},"tui":{"x":1},"hooks":{"PreToolUse":[{"hooks":[{"type":"command","command":"other-tool.exe"}]}]}}"#;
        let mut bytes = vec![0xEF, 0xBB, 0xBF];
        bytes.extend_from_slice(original.as_bytes());
        std::fs::write(&target.settings, &bytes).unwrap();

        // Install everything.
        let plan_view = preview_at(&target, &ALL, &inputs).expect("a BOM must not stop the preview");
        assert!(plan_view.diff.contains("zuko-hook") && plan_view.diff.contains(GW));
        let (backup, upstream) = write_at(&target, &ALL, &plan_view.fingerprint, &inputs).expect("install should succeed");
        assert_eq!(upstream.as_deref(), Some("https://proxy.corp.example"));
        // The backup holds the original bytes, BOM and all.
        assert_eq!(std::fs::read(&backup).unwrap(), bytes);
        assert!(target.state.exists(), "what Zuko added is remembered");
        assert_eq!(installed_at(&target), ALL);

        let after: Value = serde_json::from_slice(&std::fs::read(&target.settings).unwrap()).unwrap();
        assert_eq!(after["model"], "claude-opus-5");
        assert_eq!(after["tui"]["x"], 1);
        let pre = after["hooks"]["PreToolUse"].as_array().unwrap();
        assert!(pre.iter().any(|e| serde_json::to_string(e).unwrap().contains("other-tool.exe")));

        // A file that moved since the preview is refused, and left alone.
        let stale = preview_at(&target, &NONE, &inputs).unwrap();
        let edited = after.to_string().replace("claude-opus-5", "someone-else-edited-this");
        std::fs::write(&target.settings, &edited).unwrap();
        let err = write_at(&target, &NONE, &stale.fingerprint, &inputs).unwrap_err();
        assert!(err.contains("changed since the preview"), "got: {err}");
        assert_eq!(std::fs::read_to_string(&target.settings).unwrap(), edited);

        // Uninstall restores the original content exactly (minus the BOM).
        std::fs::write(&target.settings, after.to_string()).unwrap();
        let fresh = preview_at(&target, &NONE, &inputs).unwrap();
        write_at(&target, &NONE, &fresh.fingerprint, &inputs).unwrap();
        let restored: Value = serde_json::from_slice(&std::fs::read(&target.settings).unwrap()).unwrap();
        assert_eq!(pretty(&restored), pretty(&serde_json::from_str::<Value>(original).unwrap()));
        assert!(!target.state.exists(), "nothing left to remember");
        assert_eq!(installed_at(&target), NONE);

        // Content we cannot parse is refused before anything is written.
        std::fs::write(&target.settings, b"{ broken").unwrap();
        assert!(preview_at(&target, &ALL, &inputs).is_err());
        assert!(write_at(&target, &ALL, "whatever", &inputs).is_err());
        assert_eq!(std::fs::read(&target.settings).unwrap(), b"{ broken");

        // No settings file at all: created from nothing.
        std::fs::remove_file(&target.settings).unwrap();
        let p = preview_at(&target, &InstallOptions { hooks: true, ..NONE }, &inputs).unwrap();
        write_at(&target, &InstallOptions { hooks: true, ..NONE }, &p.fingerprint, &inputs).unwrap();
        assert!(hooks_present(&read_settings(&target.settings).unwrap()));

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
