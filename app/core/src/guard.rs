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
use crate::risk::RiskReport;
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
    let _ = (policy, ctx, det, ledger, vault, tool_name, tool_input);
    todo!()
}
