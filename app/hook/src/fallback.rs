//! The relay's own verdict when the app is unreachable or too slow
//! (CONTRACTS.md §1, "App unreachable / timeout").
//!
//! Hooks fail open by Claude Code's design, so a closed Zuko would otherwise mean
//! no firewall at all. Instead the relay runs `zuko_core::guard::decide` itself,
//! with the saved policy (`%APPDATA%\Zuko\policy.json`, defaults when missing or
//! unreadable), **no** session ledger and **no** vault:
//! * `PreToolUse`: Deny → deny (the model reads why), Ask → ask (the user reads
//!   why). Allow and Defer print nothing — the relay never approves on its own;
//!   Claude Code's normal permission flow runs instead.
//! * `UserPromptSubmit`: a prompt carrying a secret is blocked with
//!   `suppressOriginalPrompt`, telling the user Zuko is not running. A session
//!   routed through the Zuko gateway cannot reach Claude while Zuko is closed, so
//!   its prompts are blocked with an explanation instead of a network error.
//! * Everything else prints nothing.
//!
//! Cost matters: this runs inside a process Claude Code spawns per event. The
//! detector's rule set is only compiled when there is outbound text to scan.

use std::path::{Path, PathBuf};

use serde_json::Value;
use zuko_core::detect::{Category, Detector, DetectorConfig};
use zuko_core::guard::{self, Verdict};
use zuko_core::hookio;
use zuko_core::policy::{Mode, Policy};
use zuko_core::Ctx;

/// Environment variable holding the home directory.
#[cfg(windows)]
pub const HOME_VAR: &str = "USERPROFILE";
#[cfg(not(windows))]
pub const HOME_VAR: &str = "HOME";

/// Appended to every reason the user sees, so an offline verdict is never
/// mistaken for the full firewall (which also knows the session's history).
const OFFLINE_NOTE: &str = "checked by zuko-hook's built-in policy because the Zuko app is not running";

/// The output to print for `event`, or `None` for nothing.
pub fn decide(event: &str, payload: &Value) -> Option<Value> {
    match event {
        "PreToolUse" => pre_tool_use(&load_policy(&config_dir()), &ctx_for(payload), payload),
        "UserPromptSubmit" => user_prompt(&load_policy(&config_dir()), payload),
        _ => None,
    }
}

fn pre_tool_use(policy: &Policy, ctx: &Ctx, payload: &Value) -> Option<Value> {
    let tool = payload.get("tool_name").and_then(Value::as_str)?;
    let empty = Value::Object(Default::default());
    let input = payload.get("tool_input").unwrap_or(&empty);

    // Only outbound text is scanned for secrets inside `decide`; without any, a
    // detector with every rule off is enough and costs nothing to build.
    let action = zuko_core::action::from_tool_call(tool, input, ctx);
    let det = if action.egress_text.is_empty() {
        Detector::new(&rules_off())
    } else {
        Detector::new(&policy.privacy.detector)
    };

    let mut d = guard::decide(policy, ctx, &det, None, None, tool, input);
    match d.verdict {
        Verdict::Deny | Verdict::Ask => {
            d.reason_user = format!("{} ({OFFLINE_NOTE})", d.reason_user);
            // No vault: nothing was rehydrated, nothing to rewrite.
            d.updated_input = None;
            hookio::pre_tool_use(&d)
        }
        Verdict::Allow | Verdict::Defer => None,
    }
}

fn user_prompt(policy: &Policy, payload: &Value) -> Option<Value> {
    let prompt = payload.get("prompt").and_then(Value::as_str).unwrap_or_default();
    let base_url = payload
        .get("zuko_env")
        .and_then(|e| e.get("anthropicBaseUrl"))
        .and_then(Value::as_str)
        .unwrap_or_default();

    let enforce = policy.mode == Mode::Enforce && policy.privacy.block_secret_prompts_without_gateway;
    let labels = if enforce && !prompt.is_empty() { secret_labels(policy, prompt) } else { Vec::new() };

    if !labels.is_empty() {
        let reason = format!(
            "Zuko is not running, so this prompt was not sent: it contains {} ({}). \
Start Zuko to have secrets masked automatically, or remove them and send again.",
            if labels.len() == 1 { "a secret".to_string() } else { format!("{} secrets", labels.len()) },
            labels.join(", ")
        );
        return Some(hookio::user_prompt_block(&reason, true));
    }
    if is_zuko_gateway_url(base_url) {
        // Not a policy decision: the request could not reach Claude anyway. The
        // original prompt stays in the message so nothing typed is lost.
        let reason = "Zuko is not running. This Claude Code session sends its requests through \
the Zuko gateway, so it can't reach Claude until Zuko is started. Start Zuko, then send your prompt again.";
        return Some(hookio::user_prompt_block(reason, false));
    }
    None
}

/// Distinct labels of the secrets (and the user's own sensitive terms) in `text`.
/// Labels only — a value never leaves this function.
fn secret_labels(policy: &Policy, text: &str) -> Vec<String> {
    let det = Detector::new(&policy.privacy.detector);
    let mut labels: Vec<String> = Vec::new();
    for f in det.scan(text) {
        if matches!(f.category, Category::Secret | Category::Custom) && !labels.contains(&f.label) {
            labels.push(f.label);
        }
    }
    labels
}

/// A detector that matches nothing (only the placeholder pattern is compiled).
fn rules_off() -> DetectorConfig {
    DetectorConfig {
        secrets: false,
        pii: false,
        custom_terms: Vec::new(),
        ..DetectorConfig::default()
    }
}

/// True for the URL shape the Zuko gateway hands out:
/// `http://127.0.0.1:<port>/t/<token>` (loopback, a port, a path token).
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

// ── Policy and machine facts ──────────────────────────────────────────────────

/// The saved policy, or the defaults when the file is missing or unreadable. A
/// broken file must never leave the user with less than the default protection.
fn load_policy(dir: &Path) -> Policy {
    let Ok(bytes) = std::fs::read(dir.join("policy.json")) else { return Policy::default() };
    let bytes = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(&bytes);
    std::str::from_utf8(bytes)
        .ok()
        .and_then(|text| Policy::from_json(text).ok())
        .unwrap_or_default()
}

/// An absolute directory from an override variable.
fn env_dir(var: &str) -> Option<PathBuf> {
    std::env::var_os(var).map(PathBuf::from).filter(|p| p.is_absolute())
}

fn home_dir() -> PathBuf {
    std::env::var_os(HOME_VAR).map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."))
}

/// Where policy.json lives. Must match the app's `platform::config_dir()`.
#[cfg(windows)]
fn config_dir() -> PathBuf {
    env_dir("ZUKO_CONFIG_DIR").unwrap_or_else(|| {
        std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(".")).join("Zuko")
    })
}

/// Where the relay, the vault and the audit log live. Must match `platform::local_dir()`.
#[cfg(windows)]
fn data_dir() -> PathBuf {
    env_dir("ZUKO_DATA_DIR").unwrap_or_else(|| {
        std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(".")).join("Zuko")
    })
}

#[cfg(not(windows))]
fn config_dir() -> PathBuf {
    env_dir("ZUKO_CONFIG_DIR")
        .unwrap_or_else(|| env_dir("XDG_CONFIG_HOME").unwrap_or_else(|| home_dir().join(".config")).join("zuko"))
}

#[cfg(not(windows))]
fn data_dir() -> PathBuf {
    env_dir("ZUKO_DATA_DIR")
        .unwrap_or_else(|| env_dir("XDG_DATA_HOME").unwrap_or_else(|| home_dir().join(".local/share")).join("zuko"))
}

/// The same machine facts the app's `Engine` puts into every `Ctx`.
fn ctx_for(payload: &Value) -> Ctx {
    let home = payload
        .get("zuko_env")
        .and_then(|e| e.get("home"))
        .and_then(Value::as_str)
        .filter(|h| !h.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| home_dir().to_string_lossy().to_string());
    let cwd = payload.get("cwd").and_then(Value::as_str).unwrap_or_default().to_string();
    let base_url = payload
        .get("zuko_env")
        .and_then(|e| e.get("anthropicBaseUrl"))
        .and_then(Value::as_str)
        .unwrap_or_default();

    let claude = Path::new(&home).join(".claude");
    let mut protected_paths = vec![
        config_dir().to_string_lossy().to_string(),
        data_dir().to_string_lossy().to_string(),
        claude.join("settings.json").to_string_lossy().to_string(),
        claude.join("settings.local.json").to_string_lossy().to_string(),
    ];
    if let Ok(exe) = std::env::current_exe() {
        protected_paths.push(exe.to_string_lossy().to_string());
    }
    Ctx {
        project_dir: cwd.clone(),
        cwd,
        home,
        protected_paths,
        protected_processes: vec!["zuko.exe".into(), "zuko".into(), "zuko-hook.exe".into()],
        windows: cfg!(windows),
        gateway_active: is_zuko_gateway_url(base_url),
        now: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ctx() -> Ctx {
        Ctx {
            cwd: "C:\\proj".into(),
            home: "C:\\Users\\a".into(),
            project_dir: "C:\\proj".into(),
            protected_paths: vec!["C:\\Users\\a\\AppData\\Roaming\\Zuko".into()],
            protected_processes: vec!["zuko.exe".into()],
            windows: true,
            gateway_active: false,
            now: 1,
        }
    }

    fn pre(tool: &str, input: Value) -> Option<Value> {
        let payload = json!({"hook_event_name": "PreToolUse", "tool_name": tool, "tool_input": input});
        pre_tool_use(&Policy::default(), &ctx(), &payload)
    }

    #[test]
    fn a_blocked_domain_is_denied_for_the_model() {
        let out = pre("WebFetch", json!({"url": "https://pastebin.com/raw/x", "prompt": "read"})).unwrap();
        let h = &out["hookSpecificOutput"];
        assert_eq!(h["permissionDecision"], "deny");
        assert!(h["permissionDecisionReason"].as_str().unwrap().contains("pastebin.com"));
    }

    #[test]
    fn a_destructive_command_is_asked_with_the_offline_note() {
        let out = pre("Bash", json!({"command": "rm -rf ~"})).unwrap();
        let h = &out["hookSpecificOutput"];
        let verdict = h["permissionDecision"].as_str().unwrap();
        assert!(verdict == "ask" || verdict == "deny", "got {verdict}");
        if verdict == "ask" {
            assert!(h["permissionDecisionReason"].as_str().unwrap().contains("Zuko app is not running"));
        }
    }

    #[test]
    fn harmless_calls_print_nothing_never_an_allow() {
        assert!(pre("Bash", json!({"command": "ls"})).is_none());
        assert!(pre("Read", json!({"file_path": "C:\\proj\\src\\main.rs"})).is_none());
    }

    #[test]
    fn monitor_mode_never_blocks() {
        let mut p = Policy::default();
        p.mode = Mode::Monitor;
        let payload = json!({"tool_name": "WebFetch", "tool_input": {"url": "https://pastebin.com/x"}});
        assert!(pre_tool_use(&p, &ctx(), &payload).is_none());
        let payload = json!({"prompt": "key sk-proj-abcdefghijklmnopqrstuvwx1234"});
        assert!(user_prompt(&p, &payload).is_none());
    }

    #[test]
    fn a_prompt_with_a_secret_is_blocked_and_suppressed() {
        let payload = json!({"prompt": "my key is sk-proj-abcdefghijklmnopqrstuvwx1234 put it in .env"});
        let out = user_prompt(&Policy::default(), &payload).unwrap();
        assert_eq!(out["decision"], "block");
        assert_eq!(out["hookSpecificOutput"]["suppressOriginalPrompt"], true);
        let reason = out["reason"].as_str().unwrap();
        assert!(reason.contains("Zuko is not running"));
        assert!(reason.contains("OpenAI API key"));
        assert!(!reason.contains("sk-proj"), "the value must never be echoed");
        // A plain prompt passes.
        assert!(user_prompt(&Policy::default(), &json!({"prompt": "fix the tests"})).is_none());
    }

    #[test]
    fn a_gateway_session_is_told_why_it_cannot_reach_claude() {
        let payload = json!({"prompt": "hello", "zuko_env": {"anthropicBaseUrl": "http://127.0.0.1:47821/t/ab12"}});
        let out = user_prompt(&Policy::default(), &payload).unwrap();
        assert_eq!(out["decision"], "block");
        assert_eq!(out["hookSpecificOutput"]["suppressOriginalPrompt"], false);
        let other = json!({"prompt": "hello", "zuko_env": {"anthropicBaseUrl": "https://proxy.corp/x"}});
        assert!(user_prompt(&Policy::default(), &other).is_none());
    }

    #[test]
    fn gateway_urls_are_recognised_by_shape() {
        assert!(is_zuko_gateway_url("http://127.0.0.1:47821/t/0123abcd"));
        assert!(is_zuko_gateway_url("http://localhost:5/t/x"));
        assert!(!is_zuko_gateway_url("http://127.0.0.1:47821/"));
        assert!(!is_zuko_gateway_url("http://127.0.0.1:47821/t/"));
        assert!(!is_zuko_gateway_url("https://api.anthropic.com"));
        assert!(!is_zuko_gateway_url("http://127.0.0.1:x/t/y"));
        assert!(!is_zuko_gateway_url(""));
    }

    #[test]
    fn a_missing_or_broken_policy_means_the_defaults() {
        let dir = std::env::temp_dir().join(format!("zuko-relay-policy-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(load_policy(&dir), Policy::default());
        std::fs::write(dir.join("policy.json"), b"{ broken").unwrap();
        assert_eq!(load_policy(&dir), Policy::default());
        let mut bytes = vec![0xEF, 0xBB, 0xBF];
        bytes.extend_from_slice(br#"{"network":{"blocked":["evil.example"]}}"#);
        std::fs::write(dir.join("policy.json"), bytes).unwrap();
        assert_eq!(load_policy(&dir).network.blocked, vec!["evil.example".to_string()]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
