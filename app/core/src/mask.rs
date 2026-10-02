//! Masking (value → placeholder) and rehydration (placeholder → value) over plain
//! text and JSON trees, plus the legend that tells the model what placeholders mean.
//!
//! Two masking strengths:
//! * [`mask_text`] — **full**: exact vault values *and* detector findings. For content
//!   the user or the machine produced (user prompts, tool results, files, system
//!   prompt).
//! * [`mask_known`] — **known values only**: the exact inverse of rehydration. For
//!   content the model produced (assistant text and tool_use input in history), so a
//!   history round-trip is byte-identical and no new placeholder ever appears in text
//!   the model wrote.
//!
//! Rehydration only replaces placeholders whose key exists in the vault; unknown
//! `{{…}}` tokens are left alone. The tolerant `{{ KEY }}` spacing variant is accepted.

use crate::detect::Detector;
use crate::vault::Vault;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Who is masking and when, recorded on new vault entries.
#[derive(Clone, Debug, Default)]
pub struct MaskCtx {
    /// `gateway`, `hook`, `chat`, `file`, `browser`.
    pub source: String,
    /// Unix seconds.
    pub now: u64,
}

/// What a masking pass did.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MaskReport {
    /// Number of replacements made.
    pub count: usize,
    /// Distinct keys used, in first-seen order.
    pub keys: Vec<String>,
    /// Keys created by this pass (values never seen before).
    pub new_keys: Vec<String>,
}

impl MaskReport {
    /// Folds `other` into `self`, keeping key order and uniqueness.
    pub fn absorb(&mut self, other: MaskReport) {
        let _ = other;
        todo!()
    }
}

/// Full masking of `text`: exact vault values first, then detector findings (interned
/// into the vault). Existing placeholders are kept as they are.
pub fn mask_text(det: &Detector, vault: &mut Vault, text: &str, ctx: &MaskCtx) -> (String, MaskReport) {
    let _ = (det, vault, text, ctx);
    todo!()
}

/// Replaces exact vault values only. Returns the new text and the replacement count.
pub fn mask_known(vault: &Vault, text: &str) -> (String, usize) {
    let _ = (vault, text);
    todo!()
}

/// Replaces placeholders whose key is in the vault with their values.
/// Returns the new text and the keys rehydrated (with repeats).
pub fn rehydrate_text(vault: &Vault, text: &str) -> (String, Vec<String>) {
    let _ = (vault, text);
    todo!()
}

/// Full masking of every string leaf in `v` (object keys are left alone).
pub fn mask_json(det: &Detector, vault: &mut Vault, v: &mut Value, ctx: &MaskCtx) -> MaskReport {
    let _ = (det, vault, v, ctx);
    todo!()
}

/// Known-value masking of every string leaf in `v`. Returns the replacement count.
pub fn mask_known_json(vault: &Vault, v: &mut Value) -> usize {
    let _ = (vault, v);
    todo!()
}

/// Rehydrates every string leaf in `v`. Returns the keys rehydrated (with repeats).
pub fn rehydrate_json(vault: &Vault, v: &mut Value) -> Vec<String> {
    let _ = (vault, v);
    todo!()
}

/// Placeholder keys present in `text` that exist in the vault (distinct, in order).
pub fn keys_in_text(vault: &Vault, text: &str) -> Vec<String> {
    let _ = (vault, text);
    todo!()
}

/// Placeholder keys present anywhere in `v`'s string leaves that exist in the vault.
pub fn keys_in_json(vault: &Vault, v: &Value) -> Vec<String> {
    let _ = (vault, v);
    todo!()
}

/// The note given to the model. Empty string if `keys` is empty. Deterministic for a
/// given vault and key list (keys are sorted), so it does not churn prompt caches.
///
/// Shape:
/// ```text
/// Privacy note from Zuko: some values in this conversation were replaced on the
/// user's machine by placeholders like {{API_KEY_1}}. Treat each placeholder as the real
/// value: use it verbatim (exact spelling, keep the braces) wherever the value is
/// needed, e.g. in code, config files and commands; it is restored locally before
/// anything runs. Never ask the user for the real value and never guess it.
/// Placeholders in this conversation:
/// - {{API_KEY_1}}: OpenAI API key
/// - {{CARD_1}}: Visa card ending 4242
/// ```
pub fn legend(vault: &Vault, keys: &[String]) -> String {
    let _ = (vault, keys);
    todo!()
}
