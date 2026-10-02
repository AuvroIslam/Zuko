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
//!
//! Guarantees (tested): masking is idempotent (`mask(mask(x)) == mask(x)`), existing
//! placeholders are never altered (a vault value is never replaced *inside* a
//! placeholder), offsets are always char boundaries (any UTF-8 text), and
//! `rehydrate(mask_known(rehydrate(t))) == rehydrate(t)`. A value found in its
//! JSON-escaped form (`pa\"ss` inside a JSON text) is masked too; rehydration always
//! writes the raw value, so JSON contexts must be rehydrated at the value level
//! ([`rehydrate_json`]), never on serialized JSON text.

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

/// Every well-formed placeholder in `text` as `(byte_start, byte_end, key)` — the same
/// result as [`placeholder::find_all`], but safe on any UTF-8 text (the bounded search
/// window is snapped to a char boundary).
pub fn find_placeholders(text: &str) -> Vec<(usize, usize, String)> {
    let mut out = Vec::new();
    if !text.contains(placeholder::OPEN) {
        return out;
    }
    let len = text.len();
    let mut i = 0;
    while i < len {
        let Some(off) = text[i..].find(placeholder::OPEN) else { break };
        let start = i + off;
        let inner_start = start + placeholder::OPEN.len();
        let mut limit = (start + placeholder::MAX_LEN + 2).min(len);
        while limit > inner_start && !text.is_char_boundary(limit) {
            limit -= 1;
        }
        let mut matched = None;
        if limit > inner_start {
            if let Some(rel) = text[inner_start..limit].find(placeholder::CLOSE) {
                let inner = &text[inner_start..inner_start + rel];
                let trimmed = inner.trim_matches(' ');
                if placeholder::parse(trimmed).is_some() {
                    matched = Some((start, inner_start + rel + placeholder::CLOSE.len(), trimmed.to_string()));
                }
            }
        }
        match matched {
            Some(m) => {
                i = m.1;
                out.push(m);
            }
            None => i = start + 1,
        }
    }
    out
}

/// True if `[s, e)` overlaps any span in `sorted` (sorted by start, non-overlapping).
fn overlaps(sorted: &[(usize, usize)], s: usize, e: usize) -> bool {
    let idx = sorted.partition_point(|&(ps, _)| ps < e);
    idx > 0 && sorted[idx - 1].1 > s
}

/// Exact vault matches that do not touch an existing placeholder.
fn value_spans(vault: &Vault, text: &str, protected: &[(usize, usize)]) -> Vec<(usize, usize, String)> {
    if vault.is_empty() {
        return Vec::new();
    }
    vault
        .find_values(text)
        .into_iter()
        .filter(|m| !overlaps(protected, m.start, m.end))
        .map(|m| (m.start, m.end, m.key))
        .collect()
}

fn placeholder_spans(text: &str) -> Vec<(usize, usize)> {
    find_placeholders(text).into_iter().map(|(s, e, _)| (s, e)).collect()
}

/// Full masking of `text`: exact vault values first, then detector findings (interned
/// into the vault). Existing placeholders are kept as they are.
pub fn mask_text(det: &Detector, vault: &mut Vault, text: &str, ctx: &MaskCtx) -> (String, MaskReport) {
    let mut report = MaskReport::default();
    if text.is_empty() {
        return (String::new(), report);
    }
    let protected = placeholder_spans(text);
    // (start, end, key, is_new), vault matches first (sorted, non-overlapping).
    let mut spans: Vec<(usize, usize, String, bool)> = value_spans(vault, text, &protected)
        .into_iter()
        .map(|(s, e, k)| (s, e, k, false))
        .collect();
    let taken: Vec<(usize, usize)> = spans.iter().map(|&(s, e, _, _)| (s, e)).collect();
    for f in det.scan(text) {
        // Findings never overlap placeholders (the detector guarantees it) nor each other.
        if overlaps(&taken, f.start, f.end) {
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
    if vault.is_empty() || text.is_empty() {
        return (text.to_string(), 0);
    }
    let protected = placeholder_spans(text);
    let matches = value_spans(vault, text, &protected);
    if matches.is_empty() {
        return (text.to_string(), 0);
    }
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    for (s, e, key) in &matches {
        out.push_str(&text[last..*s]);
        out.push_str(&placeholder::wrap(key));
        last = *e;
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
    for (s, e, key) in find_placeholders(text) {
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

/// Rehydrates the string *values* of a serialized JSON document without re-serializing
/// the rest: object keys, key order, whitespace and number formatting stay byte-for-byte.
/// Each changed string is re-encoded with JSON escaping. Returns `None` if `json` does
/// not parse (callers then pass it through unchanged).
pub fn rehydrate_json_text(vault: &Vault, json: &str) -> Option<(String, Vec<String>)> {
    if serde_json::from_str::<serde::de::IgnoredAny>(json).is_err() {
        return None;
    }
    let mut keys = Vec::new();
    if vault.is_empty() || !json.contains(placeholder::OPEN) {
        return Some((json.to_string(), keys));
    }
    let b = json.as_bytes();
    let mut out = String::with_capacity(json.len());
    let mut last = 0;
    let mut i = 0;
    while i < b.len() {
        if b[i] != b'"' {
            i += 1;
            continue;
        }
        // String token [i, j].
        let mut j = i + 1;
        while j < b.len() && b[j] != b'"' {
            j += if b[j] == b'\\' { 2 } else { 1 };
        }
        if j >= b.len() {
            break;
        }
        let token = &json[i..=j];
        // An object key is followed by ':'.
        let mut k = j + 1;
        while k < b.len() && b[k].is_ascii_whitespace() {
            k += 1;
        }
        let is_key = k < b.len() && b[k] == b':';
        if !is_key && token.contains(placeholder::OPEN) {
            if let Ok(decoded) = serde_json::from_str::<String>(token) {
                let (new, ks) = rehydrate_text(vault, &decoded);
                if !ks.is_empty() {
                    out.push_str(&json[last..i]);
                    out.push_str(&serde_json::to_string(&new).unwrap_or_else(|_| token.to_string()));
                    last = j + 1;
                    keys.extend(ks);
                }
            }
        }
        i = j + 1;
    }
    out.push_str(&json[last..]);
    Some((out, keys))
}

/// Placeholder keys present in `text` that exist in the vault (distinct, in order).
pub fn keys_in_text(vault: &Vault, text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for (_, _, key) in find_placeholders(text) {
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

/// Intro of [`legend`], exactly as shown in its documentation.
pub const LEGEND_INTRO: &str = "Privacy note from Zuko: some values in this conversation were replaced on the
user's machine by placeholders like {{API_KEY_1}}. Treat each placeholder as the real
value: use it verbatim (exact spelling, keep the braces) wherever the value is
needed, e.g. in code, config files and commands; it is restored locally before
anything runs. Never ask the user for the real value and never guess it.
Placeholders in this conversation:";

/// The note given to the model. Empty string if `keys` is empty. Deterministic for a
/// given vault and key list (keys are sorted), so it does not churn prompt caches.
/// Keys not in the vault are skipped; descriptions are the entry's hint, else its
/// label, on one line.
///
/// Shape (exact output for a vault holding an OpenAI key and a Visa card):
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
    let mut keys: Vec<(String, u32, &str)> = keys
        .iter()
        .filter_map(|k| {
            let e = vault.get(k)?;
            let (kind, n) = placeholder::parse(&e.key)?;
            Some((kind, n, e.key.as_str()))
        })
        .collect();
    if keys.is_empty() {
        return String::new();
    }
    keys.sort();
    keys.dedup();
    let mut s = String::from(LEGEND_INTRO);
    for (_, _, k) in keys {
        let e = vault.get(k).expect("filtered above");
        let desc = e.hint.clone().filter(|h| !h.trim().is_empty()).unwrap_or_else(|| e.label.clone());
        let desc: String = desc.split_whitespace().collect::<Vec<_>>().join(" ");
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

    #[test]
    fn find_placeholders_is_safe_on_multibyte_text() {
        let t = format!("{{{{a{}", "é".repeat(30));
        assert!(find_placeholders(&t).is_empty());
        let t = format!("{}{{{{API_KEY_1}}}}{}", "ঢাকা".repeat(5), "é".repeat(40));
        let got = find_placeholders(&t);
        assert_eq!(got.len(), 1);
        assert_eq!(&t[got[0].0..got[0].1], "{{API_KEY_1}}");
    }
}
