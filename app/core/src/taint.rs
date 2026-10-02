//! Per-session taint ledger and chain invariants (inspired by ChainBreak, made
//! value-aware to avoid "one read taints the session forever" false positives).
//!
//! The ledger only grows. It records, per step: what was read (sensitive paths), which
//! secret values entered the session (from tool results, via the detector, and from the
//! vault), which untrusted inputs arrived (WebFetch results, downloaded files, content
//! from outside the project), and which hosts were contacted.
//!
//! Invariants, checked before a tool runs ([`check`]):
//! * `SELF_PROTECT` (Deny): the action writes, deletes or (for shell) reads with intent
//!   to modify any of `ctx.protected_paths`, or kills one of `ctx.protected_processes`.
//!   Reading Zuko's policy file is allowed; modifying it is not.
//! * `SECRET_EGRESS` (Deny): an egress action's `egress_text` contains a secret value
//!   the session has seen or the vault holds — raw, base64 (standard and URL-safe,
//!   with or without padding), hex (lower/upper) or URL-encoded — unless every host of
//!   the action is in `policy.privacy.secret_hosts[key]`. A vault **placeholder** in an
//!   egress action's text also counts when the caller intends to rehydrate it.
//! * `TAINTED_EGRESS` (Ask): the session read sensitive data (sensitive path or secret
//!   finding) and the action egresses to a host not in `network.allowed`.
//! * `UNTRUSTED_EXEC` (Deny): pipe-to-shell, or a shell command that executes a file
//!   the session downloaded or a URL it fetched.
//! * `UNKNOWN_TOOL` (Ask): an MCP tool not in `tools.allowed_mcp` and not used before in
//!   this session.
//! * `UNANALYZABLE` (Ask): an obfuscated or unparseable shell command.
//!
//! Each violation cites the earlier steps that caused it (`triggered_by`) so the UI can
//! say "Step 2 read .env (OpenAI key) → this curl sends it to webhook.site".

use crate::action::Action;
use crate::detect::Detector;
use crate::policy::{Policy, RuleVerdict};
use crate::vault::Vault;
use crate::Ctx;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Step {
    pub index: usize,
    pub tool: String,
    pub summary: String,
    /// Verdict given at PreToolUse: `allow`, `ask`, `deny`, `defer`.
    pub verdict: String,
    pub ts: u64,
}

/// A secret the session has seen. The raw value is kept in memory only
/// (`#[serde(skip)]`), so a persisted ledger never contains secrets.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SeenSecret {
    #[serde(skip)]
    pub value: String,
    /// SHA-256 hex of the value.
    pub hash: String,
    pub label: String,
    /// Vault key if the value is in the vault.
    pub key: Option<String>,
    pub step: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Ledger {
    pub session_id: String,
    pub steps: Vec<Step>,
    pub secrets: Vec<SeenSecret>,
    /// (normalized path, step)
    pub sensitive_reads: Vec<(String, usize)>,
    /// (source description — URL or path — , step)
    pub untrusted: Vec<(String, usize)>,
    /// Paths written by downloads (curl -o, wget, iwr -OutFile), (path, step)
    pub downloads: Vec<(String, usize)>,
    /// (host, step)
    pub egress: Vec<(String, usize)>,
    pub mcp_tools: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Violation {
    /// `SELF_PROTECT`, `SECRET_EGRESS`, `TAINTED_EGRESS`, `UNTRUSTED_EXEC`,
    /// `UNKNOWN_TOOL`, `UNANALYZABLE`.
    pub id: String,
    pub verdict: RuleVerdict,
    /// Plain English, citing the cause.
    pub reason: String,
    /// Step indices that led here.
    pub triggered_by: Vec<usize>,
}

impl Ledger {
    pub fn new(session_id: &str) -> Self {
        Self { session_id: session_id.to_string(), ..Default::default() }
    }

    /// Records the action at PreToolUse time with the verdict given. Returns the step
    /// index. Sensitive reads, egress hosts, downloads and MCP tool use are recorded
    /// here (only if the verdict is not `deny`).
    pub fn record_pre(&mut self, action: &Action, verdict: &str, policy: &Policy, ctx: &Ctx) -> usize {
        let _ = (action, verdict, policy, ctx);
        todo!()
    }

    /// Records a tool result (PostToolUse): runs the detector over `response_text` and
    /// remembers secret values; marks WebFetch output and files read from outside the
    /// project as untrusted.
    pub fn record_post(&mut self, action: &Action, response_text: &str, det: &Detector, vault: &Vault, ctx: &Ctx) {
        let _ = (action, response_text, det, vault, ctx);
        todo!()
    }

    pub fn tainted(&self) -> bool {
        !self.sensitive_reads.is_empty() || !self.secrets.is_empty()
    }
}

/// Checks every invariant for `action`. `ledger` may be empty (stateless fallback in
/// the relay), in which case only the stateless invariants (`SELF_PROTECT`,
/// `UNTRUSTED_EXEC` for pipe-to-shell, `UNANALYZABLE`, `SECRET_EGRESS` against the
/// vault) can fire.
pub fn check(ledger: &Ledger, action: &Action, vault: &Vault, policy: &Policy, ctx: &Ctx) -> Vec<Violation> {
    let _ = (ledger, action, vault, policy, ctx);
    todo!()
}

/// The forms a secret can take when smuggled out: raw, base64 (std/url, padded and
/// unpadded), hex lower/upper, percent-encoded. Values shorter than 6 chars yield only
/// the raw form.
pub fn encodings(value: &str) -> Vec<String> {
    let _ = value;
    todo!()
}
