//! Tamper-evident audit receipts: append-only JSONL, each line sealed with
//! `hash = sha256(prev_hash || canonical_json(receipt without hash))`. The app writes
//! them; [`verify_chain`] checks a whole log. Receipts never contain secret values: the
//! summary is masked by the caller and tool input is recorded only as a SHA-256.

use serde::{Deserialize, Serialize};

/// Hash of the empty chain (prev_hash of the first receipt).
pub const GENESIS: &str = "0000000000000000000000000000000000000000000000000000000000000000";

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Receipt {
    pub seq: u64,
    pub ts: u64,
    pub session_id: String,
    /// Hook event (`PreToolUse`, `PermissionRequest`, `UserPromptSubmit`, `Gateway`…).
    pub event: String,
    pub tool: String,
    /// Masked human summary.
    pub summary: String,
    /// SHA-256 hex of the canonical tool input (or request body for gateway receipts).
    pub input_sha256: String,
    /// `allow`, `ask`, `deny`, `defer`, `approved`, `denied_by_user`, `masked`.
    pub verdict: String,
    pub tier: String,
    pub score: u8,
    /// Policy rules and invariant ids that fired.
    pub rules: Vec<String>,
    /// Vault keys involved (masked or rehydrated) — keys only, never values.
    pub keys: Vec<String>,
    pub policy_digest: String,
    pub prev_hash: String,
    pub hash: String,
}

impl Receipt {
    /// Sets `prev_hash` and computes `hash`.
    pub fn seal(&mut self, prev_hash: &str) {
        let _ = prev_hash;
        todo!()
    }

    /// The hash this receipt should have given its `prev_hash`.
    pub fn compute_hash(&self) -> String {
        todo!()
    }
}

/// Verifies a JSONL log. `Ok(n)` with the number of receipts, or `Err((line_no, why))`
/// at the first broken link (bad JSON, wrong prev_hash, wrong hash, non-increasing seq).
pub fn verify_chain<'a>(lines: impl IntoIterator<Item = &'a str>) -> Result<u64, (u64, String)> {
    let _ = lines;
    todo!()
}

/// SHA-256 hex of `bytes`.
pub fn sha256_hex(bytes: &[u8]) -> String {
    let _ = bytes;
    todo!()
}

/// Canonical JSON (object keys sorted recursively, no whitespace).
pub fn canonical_json(v: &serde_json::Value) -> String {
    let _ = v;
    todo!()
}
