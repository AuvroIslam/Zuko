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
use crate::placeholder;
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
        self.count += other.count;
        for k in other.keys {
            if !self.keys.contains(&k) {
                self.keys.push(k);
            }
        }
        for k in other.new_keys {
            if !self.new_keys.contains(&k) {
                self.new_keys.push(k);
            }
        }
    }

    fn note(&mut self, key: &str, is_new: bool) {
        self.count += 1;
        if !self.keys.iter().any(|k| k == key) {
            self.keys.push(key.to_string());
        }
        if is_new && !self.new_keys.iter().any(|k| k == key) {
            self.new_keys.push(key.to_string());
        }
    }
}

/// Full masking of `text`: exact vault values first, then detector findings (interned
/// into the vault). Existing placeholders are kept as they are.
pub fn mask_text(det: &Detector, vault: &mut Vault, text: &str, ctx: &MaskCtx) -> (String, MaskReport) {
    let mut report = MaskReport::default();
    if text.is_empty() {
        return (String::new(), report);
    }
    // (start, end, key, is_new)
    let mut spans: Vec<(usize, usize, String, bool)> = vault
        .find_values(text)
        .into_iter()
        .map(|m| (m.start, m.end, m.key, false))
        .collect();
    for f in det.scan(text) {
        if spans.iter().any(|&(s, e, _, _)| f.start < e && s < f.end) {
            continue;
        }
        let existed = vault.key_for_value(&f.value).is_some();
        if let Some(key) = vault.intern(&f, &ctx.source, ctx.now) {
            spans.push((f.start, f.end, key, !existed));
        }
    }
    if spans.is_empty() {
        return (text.to_string(), report);
    }
    spans.sort_by_key(|s| s.0);
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    for (s, e, key, is_new) in spans {
        if s < last {
            continue;
        }
        out.push_str(&text[last..s]);
        out.push_str(&placeholder::wrap(&key));
        report.note(&key, is_new);
        last = e;
    }
    out.push_str(&text[last..]);
    (out, report)
}

/// Replaces exact vault values only. Returns the new text and the replacement count.
pub fn mask_known(vault: &Vault, text: &str) -> (String, usize) {
    let matches = vault.find_values(text);
    if matches.is_empty() {
        return (text.to_string(), 0);
    }
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    for m in &matches {
        out.push_str(&text[last..m.start]);
        out.push_str(&placeholder::wrap(&m.key));
        last = m.end;
    }
    out.push_str(&text[last..]);
    (out, matches.len())
}

/// Replaces placeholders whose key is in the vault with their values.
/// Returns the new text and the keys rehydrated (with repeats).
pub fn rehydrate_text(vault: &Vault, text: &str) -> (String, Vec<String>) {
    let mut keys = Vec::new();
    if vault.is_empty() || !text.contains(placeholder::OPEN) {
        return (text.to_string(), keys);
    }
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    for (s, e, key) in placeholder::find_all(text) {
        let Some(entry) = vault.get(&key) else { continue };
        out.push_str(&text[last..s]);
        out.push_str(&entry.value);
        keys.push(key);
        last = e;
    }
    out.push_str(&text[last..]);
    (out, keys)
}

fn walk_strings(v: &mut Value, f: &mut dyn FnMut(&mut String)) {
    match v {
        Value::String(s) => f(s),
        Value::Array(a) => a.iter_mut().for_each(|x| walk_strings(x, f)),
        Value::Object(o) => o.values_mut().for_each(|x| walk_strings(x, f)),
        _ => {}
    }
}

fn visit_strings(v: &Value, f: &mut dyn FnMut(&str)) {
    match v {
        Value::String(s) => f(s),
        Value::Array(a) => a.iter().for_each(|x| visit_strings(x, f)),
        Value::Object(o) => o.values().for_each(|x| visit_strings(x, f)),
        _ => {}
    }
}

/// Full masking of every string leaf in `v` (object keys are left alone).
pub fn mask_json(det: &Detector, vault: &mut Vault, v: &mut Value, ctx: &MaskCtx) -> MaskReport {
    let mut report = MaskReport::default();
    walk_strings(v, &mut |s| {
        let (out, r) = mask_text(det, vault, s, ctx);
        if r.count > 0 {
            *s = out;
            report.absorb(r);
        }
    });
    report
}

/// Known-value masking of every string leaf in `v`. Returns the replacement count.
pub fn mask_known_json(vault: &Vault, v: &mut Value) -> usize {
    let mut n = 0;
    walk_strings(v, &mut |s| {
        let (out, c) = mask_known(vault, s);
        if c > 0 {
            *s = out;
            n += c;
        }
    });
    n
}

/// Rehydrates every string leaf in `v`. Returns the keys rehydrated (with repeats).
pub fn rehydrate_json(vault: &Vault, v: &mut Value) -> Vec<String> {
    let mut keys = Vec::new();
    walk_strings(v, &mut |s| {
        let (out, k) = rehydrate_text(vault, s);
        if !k.is_empty() {
            *s = out;
            keys.extend(k);
        }
    });
    keys
}

/// Placeholder keys present in `text` that exist in the vault (distinct, in order).
pub fn keys_in_text(vault: &Vault, text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for (_, _, key) in placeholder::find_all(text) {
        if vault.get(&key).is_some() && !out.contains(&key) {
            out.push(key);
        }
    }
    out
}

/// Placeholder keys present anywhere in `v`'s string leaves that exist in the vault.
pub fn keys_in_json(vault: &Vault, v: &Value) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    visit_strings(v, &mut |s| {
        for k in keys_in_text(vault, s) {
            if !out.contains(&k) {
                out.push(k);
            }
        }
    });
    out
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
    let mut keys: Vec<&String> = keys.iter().filter(|k| vault.get(k).is_some()).collect();
    if keys.is_empty() {
        return String::new();
    }
    keys.sort_by(|a, b| {
        let pa = placeholder::parse(a);
        let pb = placeholder::parse(b);
        pa.cmp(&pb)
    });
    keys.dedup();
    let mut s = String::from(
        "Privacy note from Zuko: some values in this conversation were replaced on the user's machine by placeholders like {{API_KEY_1}}. \
Treat each placeholder as the real value: use it verbatim (exact spelling, keep the braces) wherever the value is needed, \
e.g. in code, config files and commands; it is restored locally before anything runs. \
Never ask the user for the real value and never guess it.\nPlaceholders in this conversation:",
    );
    for k in keys {
        let e = vault.get(k).expect("filtered above");
        let desc = e.hint.clone().unwrap_or_else(|| e.label.clone());
        s.push_str(&format!("\n- {}: {}", placeholder::wrap(k), desc));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detect::DetectorConfig;

    #[test]
    fn mask_then_rehydrate_round_trips() {
        let det = Detector::new(&DetectorConfig::default());
        let mut v = Vault::new();
        let ctx = MaskCtx { source: "test".into(), now: 1 };
        let text = "my key is sk-proj-abcdefghijklmnopqrstuvwx1234 and mail bob@acme.io";
        let (masked, r) = mask_text(&det, &mut v, text, &ctx);
        assert_eq!(masked, "my key is {{API_KEY_1}} and mail {{EMAIL_1}}");
        assert_eq!(r.count, 2);
        assert_eq!(r.new_keys.len(), 2);
        let (back, keys) = rehydrate_text(&v, &masked);
        assert_eq!(back, text);
        assert_eq!(keys.len(), 2);
        // Known-only masking is the exact inverse.
        assert_eq!(mask_known(&v, &back).0, masked);
        // Idempotent.
        assert_eq!(mask_text(&det, &mut v, &masked, &ctx).0, masked);
        assert!(legend(&v, &r.keys).contains("- {{API_KEY_1}}: OpenAI API key"));
    }
}
