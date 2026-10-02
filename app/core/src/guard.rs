//! The decision for one tool call: policy + taint invariants + risk → verdict, with
//! reasons for the user and for the model, friction for the UI, and the rehydrated
//! tool input.
//!
//! Order (first match wins for the verdict; everything is still reported):
//! 1. `SELF_PROTECT` violation → **Deny**.
//! 2. Policy `Deny` hit → **Deny**.
//! 3. Invariant `Deny` (`SECRET_EGRESS`, `UNTRUSTED_EXEC`) → **Deny**.
//! 4. Risk tier ≥ `approvals.block_from` → **Deny**.
//! 5. Policy `Ask` hit, invariant `Ask`, or tier ≥ `approvals.hold_to_approve_from`
//!    → **Ask** (friction: hold-to-approve for High/Critical).
//! 6. Tier Low and `approvals.auto_allow_low_risk` → **Allow**.
//! 7. Otherwise → **Defer** (no opinion: Claude Code's normal permission flow runs, and
//!    if it prompts, the island shows the card with the risk).
//!
//! `Mode::Monitor` computes everything but returns **Defer** with the would-be verdict
//! in [`Decision::monitored`].
//!
//! Rehydration (sink policy, see [`crate::anthropic::SinkPolicy`]): placeholders in the
//! tool input are replaced with vault values when
//! * the tool is a local file write (Write, Edit, MultiEdit, NotebookEdit) and the
//!   target path is not protected, or
//! * the tool is Bash/PowerShell, the command has no network egress, and no invariant
//!   fired — otherwise the placeholder-bearing command is **Ask**ed with an explicit
//!   "this command would contain your secret API_KEY_1 and sends data to <host>"
//!   (SECRET_EGRESS → Deny when the host is not allowed for that secret).
//!
//! WebFetch, WebSearch and MCP inputs are never rehydrated.
//! When rehydration happens and the verdict would be Defer, the verdict becomes Allow
//! only if the tier is Low and auto-allow is on; otherwise Ask (Claude Code requires
//! `allow` or `ask` alongside `updatedInput` to show the rewritten input).

use crate::action::Action;
use crate::detect::Detector;
use crate::policy::{Policy, PolicyHit};
use crate::risk::{RiskReport, Tier};
use crate::taint::{Ledger, Violation};
use crate::vault::Vault;
use crate::Ctx;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Verdict {
    Allow,
    Ask,
    Deny,
    #[default]
    Defer,
}

impl Verdict {
    pub fn as_str(self) -> &'static str {
        match self {
            Verdict::Allow => "allow",
            Verdict::Ask => "ask",
            Verdict::Deny => "deny",
            Verdict::Defer => "defer",
        }
    }
}

/// How hard the island makes approval.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Friction {
    /// Plain Allow / Deny.
    #[default]
    None,
    /// Allow must be held for `ms` milliseconds.
    Hold { ms: u32 },
    /// No Allow button: the action is blocked.
    Blocked,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Decision {
    pub verdict: Verdict,
    /// In monitor mode, what would have happened.
    pub monitored: Option<Verdict>,
    pub risk: RiskReport,
    pub policy_hits: Vec<PolicyHit>,
    pub violations: Vec<Violation>,
    /// Shown to the user (native prompt for `ask`, island card, activity feed).
    /// Starts with the tier and headline: "⚠ HIGH RISK — SENDS data to webhook.site …".
    pub reason_user: String,
    /// Shown to the model on `deny`: what was blocked and why, and what to do instead
    /// ("Zuko blocked this: pastebin.com is on the user's blocked list. Do not retry;
    /// ask the user if this is needed.").
    pub reason_model: String,
    /// The tool input with placeholders filled in, when rehydration applies.
    pub updated_input: Option<Value>,
    /// Vault keys filled into `updated_input`.
    pub rehydrated: Vec<String>,
    pub friction: Friction,
    pub action: Action,
}

/// Decides one `PreToolUse`. `ledger` is `None` in the relay's stateless fallback;
/// `vault` is `None` when the vault is unavailable (no rehydration, vault-based
/// SECRET_EGRESS skipped).
pub fn decide(
    policy: &Policy,
    ctx: &Ctx,
    det: &Detector,
    ledger: Option<&Ledger>,
    vault: Option<&Vault>,
    tool_name: &str,
    tool_input: &Value,
) -> Decision {
    use crate::policy::RuleVerdict;

    let action = crate::action::from_tool_call(tool_name, tool_input, ctx);

    // Secret findings in outbound text.
    let secret_hits = action
        .egress_text
        .iter()
        .map(|t| det.scan(t).len())
        .sum::<usize>();

    let risk = crate::risk::assess(&action, policy, ctx, secret_hits);
    let policy_hits = crate::policy::evaluate(policy, &action, ctx);

    let empty_ledger = Ledger::default();
    let led = ledger.unwrap_or(&empty_ledger);
    let empty_vault = Vault::new();
    let vlt = vault.unwrap_or(&empty_vault);
    let violations = crate::taint::check(led, &action, vlt, policy, ctx);

    // --- Decision order ---
    let self_protect = violations.iter().find(|v| v.id == "SELF_PROTECT");
    let policy_deny = policy_hits.iter().find(|h| h.verdict == RuleVerdict::Deny);
    let inv_deny = violations
        .iter()
        .find(|v| v.verdict == RuleVerdict::Deny && v.id != "SELF_PROTECT");
    let risk_block = risk.tier >= policy.approvals.block_from;

    let policy_ask = policy_hits.iter().find(|h| h.verdict == RuleVerdict::Ask);
    let inv_ask = violations.iter().find(|v| v.verdict == RuleVerdict::Ask);
    let risk_ask = risk.tier >= policy.approvals.hold_to_approve_from;

    let mut verdict;
    let mut reason_model = String::new();

    if let Some(v) = self_protect {
        verdict = Verdict::Deny;
        reason_model = blocked_model(&v.reason);
    } else if let Some(h) = policy_deny {
        verdict = Verdict::Deny;
        reason_model = blocked_model(&h.reason);
    } else if let Some(v) = inv_deny {
        verdict = Verdict::Deny;
        reason_model = blocked_model(&v.reason);
    } else if risk_block {
        verdict = Verdict::Deny;
        reason_model = blocked_model(&format!("this action is {} risk — {}", risk.tier.as_str(), risk.headline));
    } else if policy_ask.is_some() || inv_ask.is_some() || risk_ask {
        verdict = Verdict::Ask;
    } else if risk.tier == Tier::Low && policy.approvals.auto_allow_low_risk {
        verdict = Verdict::Allow;
    } else {
        verdict = Verdict::Defer;
    }

    // --- Rehydration (sink policy) ---
    let mut updated_input: Option<Value> = None;
    let mut rehydrated: Vec<String> = Vec::new();
    if verdict != Verdict::Deny {
        if let Some(v) = vault {
            let keys = crate::mask::keys_in_json(v, tool_input);
            if !keys.is_empty() {
                let (allow_rehydrate, forces_ask) = rehydration_mode(&action, ctx, &violations);
                if allow_rehydrate {
                    let mut clone = tool_input.clone();
                    let filled = crate::mask::rehydrate_json(v, &mut clone);
                    if !filled.is_empty() {
                        updated_input = Some(clone);
                        rehydrated = dedup(filled);
                        // Claude Code needs allow/ask alongside updatedInput.
                        if verdict == Verdict::Defer {
                            verdict = if risk.tier == Tier::Low && policy.approvals.auto_allow_low_risk {
                                Verdict::Allow
                            } else {
                                Verdict::Ask
                            };
                        }
                    }
                } else if forces_ask && verdict == Verdict::Defer {
                    verdict = Verdict::Ask;
                }
            }
        }
    }

    // --- Reasons for the user ---
    let mut reason_user = format!("{} — {}", tier_label(risk.tier), risk.headline);
    let mut extra: Vec<String> = Vec::new();
    for h in &policy_hits {
        if h.verdict != RuleVerdict::Allow {
            extra.push(h.reason.clone());
        }
    }
    for v in &violations {
        extra.push(v.reason.clone());
    }
    if !rehydrated.is_empty() {
        extra.push(format!("Zuko will fill in {} secret value(s) locally before this runs", rehydrated.len()));
    }
    if !extra.is_empty() {
        reason_user.push_str(" — ");
        reason_user.push_str(&extra.join("; "));
    }

    if reason_model.is_empty() && verdict == Verdict::Deny {
        reason_model = blocked_model(&risk.headline);
    }
    if verdict == Verdict::Ask && reason_model.is_empty() {
        reason_model = format!("Zuko needs the user's approval for this: {}.", risk.headline);
    }

    // --- Friction ---
    let friction = match verdict {
        Verdict::Deny => Friction::Blocked,
        Verdict::Ask if risk.tier >= policy.approvals.hold_to_approve_from => {
            let ms = if risk.tier == Tier::Critical {
                policy.approvals.hold_ms.saturating_mul(2)
            } else {
                policy.approvals.hold_ms
            };
            Friction::Hold { ms }
        }
        _ => Friction::None,
    };

    // --- Monitor mode: compute everything, enforce nothing ---
    let monitored = if policy.mode == crate::policy::Mode::Monitor {
        let would = verdict;
        verdict = Verdict::Defer;
        updated_input = None;
        rehydrated.clear();
        Some(would)
    } else {
        None
    };

    Decision {
        verdict,
        monitored,
        risk,
        policy_hits,
        violations,
        reason_user,
        reason_model,
        updated_input,
        rehydrated,
        friction: if monitored.is_some() { Friction::None } else { friction },
        action,
    }
}

/// Returns (may_rehydrate, forces_ask_if_not_rehydrated) for an input carrying
/// placeholders.
fn rehydration_mode(action: &crate::action::Action, ctx: &Ctx, violations: &[Violation]) -> (bool, bool) {
    use crate::action::ActionKind;
    let any_violation = !violations.is_empty();
    match action.tool.as_str() {
        "Write" | "Edit" | "MultiEdit" | "NotebookEdit" => {
            // Not into a protected path.
            let mut prot = ctx.protected_paths.clone();
            prot.push("**/.claude/settings.json".into());
            prot.push("**/.zuko/**".into());
            let protected = action.writes.iter().any(|p| {
                prot.iter().any(|pat| {
                    crate::policy::glob_match(pat, p, ctx)
                        || crate::action::is_within(p, &crate::action::normalize_path(pat, ctx))
                })
            });
            (!protected, false)
        }
        "Bash" | "PowerShell" => {
            let networked = action.shell.as_ref().map(|s| s.network || s.pipe_to_shell).unwrap_or(false);
            if networked || any_violation {
                (false, true)
            } else {
                (true, false)
            }
        }
        _ => {
            // WebFetch, WebSearch, MCP and others are never rehydrated.
            let _ = ActionKind::Mcp;
            (false, false)
        }
    }
}

fn dedup(mut v: Vec<String>) -> Vec<String> {
    let mut out = Vec::new();
    for k in v.drain(..) {
        if !out.contains(&k) {
            out.push(k);
        }
    }
    out
}

fn blocked_model(reason: &str) -> String {
    format!("Zuko blocked this: {reason}. Do not retry; ask the user if this is really needed, or find another way that does not require it.")
}

fn tier_label(t: Tier) -> &'static str {
    match t {
        Tier::Low => "LOW",
        Tier::Medium => "⚠ MEDIUM RISK",
        Tier::High => "⚠ HIGH RISK",
        Tier::Critical => "⛔ CRITICAL",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detect::{Detector, DetectorConfig};
    use crate::policy::Mode;
    use serde_json::json;

    fn ctx() -> Ctx {
        Ctx {
            cwd: "c:/users/me/proj".into(),
            home: "c:/users/me".into(),
            project_dir: "c:/users/me/proj".into(),
            protected_processes: vec!["zuko.exe".into()],
            windows: true,
            ..Default::default()
        }
    }

    fn det() -> Detector {
        Detector::new(&DetectorConfig::default())
    }

    #[test]
    fn blocks_blocked_domain() {
        let d = decide(&Policy::default(), &ctx(), &det(), None, None, "WebFetch", &json!({"url": "https://pastebin.com/raw/x"}));
        assert_eq!(d.verdict, Verdict::Deny);
        assert!(d.reason_model.contains("blocked"));
    }

    #[test]
    fn allows_low_risk_read() {
        let d = decide(&Policy::default(), &ctx(), &det(), None, None, "Read", &json!({"file_path": "src/main.rs"}));
        assert_eq!(d.verdict, Verdict::Allow);
    }

    #[test]
    fn asks_high_risk_with_hold() {
        let d = decide(&Policy::default(), &ctx(), &det(), None, None, "Bash", &json!({"command": "rm -rf build"}));
        assert_eq!(d.verdict, Verdict::Ask);
        assert!(matches!(d.friction, Friction::Hold { .. }));
    }

    #[test]
    fn monitor_mode_defers() {
        let mut p = Policy::default();
        p.mode = Mode::Monitor;
        let d = decide(&p, &ctx(), &det(), None, None, "WebFetch", &json!({"url": "https://pastebin.com/raw/x"}));
        assert_eq!(d.verdict, Verdict::Defer);
        assert_eq!(d.monitored, Some(Verdict::Deny));
    }

    #[test]
    fn rehydrates_local_write() {
        let c = ctx();
        let mut vault = Vault::new();
        let key = vault.add_manual("sk-proj-REALSECRETVALUE123456", "API_KEY", "OpenAI API key", 0).unwrap();
        let ph = crate::placeholder::wrap(&key);
        let d = decide(&Policy::default(), &c, &det(), None, Some(&vault), "Write", &json!({"file_path": ".env", "content": format!("OPENAI_API_KEY={ph}")}));
        assert!(d.updated_input.is_some());
        assert!(!d.rehydrated.is_empty());
        let content = d.updated_input.unwrap()["content"].as_str().unwrap().to_string();
        assert!(content.contains("sk-proj-REALSECRETVALUE123456"));
        assert_ne!(d.verdict, Verdict::Deny);
    }
}
