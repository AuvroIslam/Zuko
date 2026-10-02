//! The vault: a deterministic, two-way map between sensitive values and placeholder keys.
//!
//! * The same value always maps to the same key (`API_KEY_1`), across turns and
//!   sessions, so re-masked conversation history is byte-identical (prompt caching and
//!   thinking-block signatures keep working).
//! * Counters are per kind and never reused, even after [`Vault::remove`].
//! * The vault itself holds plaintext; persistence and encryption are the caller's job
//!   (the app encrypts it with a key held in the OS keyring).
//! * Exact-value matching across texts goes through [`Vault::find_values`], an
//!   Aho-Corasick automaton over every stored value (and its JSON-escaped forms),
//!   rebuilt lazily on the first search after a change. Value/key lookups are kept up
//!   to date incrementally, so interning many values in a row stays cheap.
//! * Values that contain a well-formed placeholder are never interned (they would make
//!   rehydration ambiguous), and loading a hand-edited file drops duplicate values/keys.

use crate::detect::{Category, Finding};
use crate::placeholder;
use aho_corasick::{AhoCorasick, AhoCorasickBuilder, AhoCorasickKind, MatchKind};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, OnceLock};

/// Values shorter than this are never interned (too likely to collide with ordinary
/// text), except custom terms (`Category::Custom`), which the user chose explicitly.
pub const MIN_VALUE_LEN: usize = 6;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    /// `"API_KEY_1"` — no braces.
    pub key: String,
    /// The sensitive value.
    pub value: String,
    pub kind: String,
    pub label: String,
    pub category: Category,
    /// Non-sensitive hint for the model legend (see [`Finding::hint`]).
    pub hint: Option<String>,
    /// Where it was first seen: `gateway`, `hook`, `chat`, `file`, `browser`, `manual`.
    pub source: String,
    /// Unix seconds when first interned.
    pub created: u64,
    /// Unix seconds when last masked or rehydrated.
    pub last_used: u64,
    /// How many times it was masked.
    pub hits: u64,
}

/// Public, value-free view of an entry for UIs and logs.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EntryView {
    pub key: String,
    pub kind: String,
    pub label: String,
    pub category: Category,
    pub hint: Option<String>,
    /// e.g. `sk-p…9fQa` — at most the first 4 and last 4 chars, fewer for short values.
    pub preview: String,
    pub source: String,
    pub created: u64,
    pub last_used: u64,
    pub hits: u64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Vault {
    entries: Vec<Entry>,
    /// Next number per kind.
    counters: BTreeMap<String, u32>,
    #[serde(skip)]
    index: VaultIndex,
}

/// Lookup tables derived from `entries` (not serialized). Shared by clones until a
/// mutation touches them.
#[derive(Clone, Debug, Default)]
struct VaultIndex {
    /// Value/key → entry index; updated in place on insert, rebuilt after removals.
    lookup: OnceLock<Arc<Lookup>>,
    /// Aho-Corasick over values; rebuilt lazily after any change.
    matcher: OnceLock<Arc<Matcher>>,
}

#[derive(Clone, Debug, Default)]
struct Lookup {
    by_value: HashMap<String, usize>,
    by_key: HashMap<String, usize>,
}

#[derive(Debug)]
struct Matcher {
    /// Patterns: every raw value first, then JSON-escaped forms that are not themselves
    /// a stored raw value.
    ac: Option<AhoCorasick>,
    /// Pattern index → (entry index, is an escaped form).
    pattern_entry: Vec<(usize, bool)>,
}

/// One exact occurrence of a stored value inside a text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValueMatch {
    pub start: usize,
    pub end: usize,
    pub key: String,
}

fn json_escaped(v: &str) -> String {
    let s = serde_json::to_string(v).unwrap_or_default();
    s[1..s.len() - 1].to_string()
}

/// JSON escaping with every non-ASCII char as `\uXXXX` (Python's `json.dumps` default).
fn json_escaped_ascii(v: &str) -> String {
    let mut out = String::with_capacity(v.len() + 8);
    for c in json_escaped(v).chars() {
        if c.is_ascii() {
            out.push(c);
        } else {
            let mut buf = [0u16; 2];
            for u in c.encode_utf16(&mut buf) {
                out.push_str(&format!("\\u{:04x}", u));
            }
        }
    }
    out
}

/// The forms of `v` searched for in texts: raw, JSON-escaped, ASCII-JSON-escaped.
fn value_forms(v: &str) -> Vec<String> {
    let mut forms = vec![v.to_string()];
    let esc = json_escaped(v);
    if esc != v {
        forms.push(esc);
    }
    if !v.is_ascii() {
        let a = json_escaped_ascii(v);
        if !forms.contains(&a) {
            forms.push(a);
        }
    }
    forms
}

impl Vault {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_json(s: &str) -> Result<Self, String> {
        let mut v: Vault = serde_json::from_str(s).map_err(|e| e.to_string())?;
        // Drop malformed or duplicate entries from hand-edited or merged files.
        let mut seen_values = std::collections::HashSet::new();
        let mut seen_keys = std::collections::HashSet::new();
        v.entries.retain(|e| {
            !e.value.is_empty()
                && placeholder::parse(&e.key).is_some()
                && seen_values.insert(e.value.clone())
                && seen_keys.insert(e.key.clone())
        });
        // Repair counters so a hand-edited or merged file never reuses a key.
        for e in &v.entries {
            if let Some((kind, n)) = placeholder::parse(&e.key) {
                let c = v.counters.entry(kind).or_insert(0);
                if *c < n {
                    *c = n;
                }
            }
        }
        v.invalidate();
        Ok(v)
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| "{}".into())
    }

    fn invalidate(&mut self) {
        self.index = VaultIndex::default();
    }

    fn lookup(&self) -> &Lookup {
        self.index.lookup.get_or_init(|| {
            let mut l = Lookup::default();
            for (i, e) in self.entries.iter().enumerate() {
                l.by_value.insert(e.value.clone(), i);
                l.by_key.insert(e.key.clone(), i);
            }
            Arc::new(l)
        })
    }

    fn matcher(&self) -> Arc<Matcher> {
        self.index
            .matcher
            .get_or_init(|| {
                let mut patterns = Vec::new();
                let mut pattern_entry = Vec::new();
                for (i, e) in self.entries.iter().enumerate() {
                    patterns.push(e.value.clone());
                    pattern_entry.push((i, false));
                }
                let raw: std::collections::HashSet<&str> = self.entries.iter().map(|e| e.value.as_str()).collect();
                for (i, e) in self.entries.iter().enumerate() {
                    for f in value_forms(&e.value).into_iter().skip(1) {
                        if !raw.contains(f.as_str()) {
                            patterns.push(f);
                            pattern_entry.push((i, true));
                        }
                    }
                }
                let ac = if patterns.is_empty() {
                    None
                } else {
                    // A contiguous NFA builds fast even for long values (private keys).
                    AhoCorasickBuilder::new()
                        .match_kind(MatchKind::LeftmostLongest)
                        .kind(Some(AhoCorasickKind::ContiguousNFA))
                        .build(&patterns)
                        .ok()
                };
                Arc::new(Matcher { ac, pattern_entry })
            })
            .clone()
    }

    fn insert(&mut self, value: &str, kind: &str, label: &str, category: Category, hint: Option<String>, source: &str, now: u64) -> Option<String> {
        if value.is_empty() || (category != Category::Custom && value.chars().count() < MIN_VALUE_LEN) {
            return None;
        }
        if value.contains(placeholder::OPEN) && !placeholder::find_all(value).is_empty() {
            return None;
        }
        if let Some(&i) = self.lookup().by_value.get(value) {
            let e = &mut self.entries[i];
            e.hits += 1;
            e.last_used = now;
            return Some(e.key.clone());
        }
        let kind = if placeholder::valid_kind(kind) { kind.to_string() } else { "SECRET".to_string() };
        let n = self.counters.entry(kind.clone()).or_insert(0);
        *n += 1;
        let key = placeholder::key(&kind, *n);
        let idx = self.entries.len();
        self.entries.push(Entry {
            key: key.clone(),
            value: value.to_string(),
            kind,
            label: label.to_string(),
            category,
            hint,
            source: source.to_string(),
            created: now,
            last_used: now,
            hits: 1,
        });
        // Keep the lookup current in place (copy-on-write if a clone shares it); the
        // matcher is rebuilt on the next search.
        if let Some(arc) = self.index.lookup.take() {
            let mut l = Arc::try_unwrap(arc).unwrap_or_else(|a| (*a).clone());
            l.by_value.insert(value.to_string(), idx);
            l.by_key.insert(key.clone(), idx);
            let _ = self.index.lookup.set(Arc::new(l));
        }
        self.index.matcher = OnceLock::new();
        Some(key)
    }

    /// Returns the key for `f.value`, creating an entry if the value is new. Bumps
    /// `hits` and `last_used`. Returns `None` if the value is too short to intern
    /// ([`MIN_VALUE_LEN`], custom terms exempt).
    pub fn intern(&mut self, f: &Finding, source: &str, now: u64) -> Option<String> {
        self.insert(&f.value, &f.kind, &f.label, f.category, f.hint.clone(), source, now)
    }

    /// Adds a value by hand (from the UI). Same rules as [`Vault::intern`].
    pub fn add_manual(&mut self, value: &str, kind: &str, label: &str, now: u64) -> Option<String> {
        self.insert(value, kind, label, Category::Custom, None, "manual", now)
    }

    pub fn get(&self, key: &str) -> Option<&Entry> {
        let key = key.trim_start_matches(placeholder::OPEN).trim_end_matches(placeholder::CLOSE).trim_matches(' ');
        self.lookup().by_key.get(key).map(|&i| &self.entries[i])
    }

    pub fn key_for_value(&self, value: &str) -> Option<&str> {
        self.lookup().by_value.get(value).map(|&i| self.entries[i].key.as_str())
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    pub fn views(&self) -> Vec<EntryView> {
        self.entries
            .iter()
            .map(|e| EntryView {
                key: e.key.clone(),
                kind: e.kind.clone(),
                label: e.label.clone(),
                category: e.category,
                hint: e.hint.clone(),
                preview: preview(&e.value),
                source: e.source.clone(),
                created: e.created,
                last_used: e.last_used,
                hits: e.hits,
            })
            .collect()
    }

    pub fn remove(&mut self, key: &str) -> bool {
        let before = self.entries.len();
        self.entries.retain(|e| e.key != key);
        let changed = self.entries.len() != before;
        if changed {
            self.invalidate();
        }
        changed
    }

    pub fn clear(&mut self) {
        // Counters survive: a forgotten key is never handed to a different value.
        self.entries.clear();
        self.invalidate();
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Every exact occurrence of any stored value in `text`, leftmost-longest,
    /// non-overlapping, sorted. Also matches the JSON-escaped form of values that
    /// contain `"`, `\` or control chars, and the `\uXXXX`-escaped form of non-ASCII
    /// values. Offsets are char boundaries.
    pub fn find_values(&self, text: &str) -> Vec<ValueMatch> {
        self.find_value_forms(text).into_iter().map(|(m, _)| m).collect()
    }

    /// [`Vault::find_values`], also telling whether each match is an escaped form of
    /// the entry's value (`true`) rather than the raw value itself (`false`).
    pub fn find_value_forms(&self, text: &str) -> Vec<(ValueMatch, bool)> {
        let t = self.matcher();
        let Some(ac) = &t.ac else { return Vec::new() };
        ac.find_iter(text)
            .map(|m| {
                let (i, escaped) = t.pattern_entry[m.pattern().as_usize()];
                (ValueMatch { start: m.start(), end: m.end(), key: self.entries[i].key.clone() }, escaped)
            })
            .collect()
    }

    /// Interns `escaped`, an escaped occurrence of the value stored under `key`, as its
    /// own entry (same kind, label, category and hint), so that rehydration writes back
    /// exactly the escaped text. Returns the new (or existing) key.
    pub fn intern_escaped_form(&mut self, key: &str, escaped: &str, source: &str, now: u64) -> Option<String> {
        let e = self.get(key)?.clone();
        self.insert(escaped, &e.kind, &e.label, e.category, e.hint.clone(), source, now)
    }

    /// Counts one more masking of `key` (an exact-value replacement) at `now`.
    pub fn note_masked(&mut self, key: &str, now: u64) {
        if let Some(&i) = self.lookup().by_key.get(key) {
            let e = &mut self.entries[i];
            e.hits += 1;
            e.last_used = now;
        }
    }

    /// Merges entries from `other` (e.g. the browser extension's session vault). Values
    /// already present keep their key; new values get fresh keys in this vault. Returns
    /// a map from `other`'s keys to this vault's keys where they differ.
    pub fn merge(&mut self, other: &Vault, now: u64) -> Vec<(String, String)> {
        let mut remap = Vec::new();
        for e in &other.entries {
            if let Some(k) = self.insert(&e.value, &e.kind, &e.label, e.category, e.hint.clone(), &e.source, now) {
                if k != e.key {
                    remap.push((e.key.clone(), k));
                }
            }
        }
        remap
    }

    /// Marks `key` as used (rehydrated) at `now`.
    pub fn touch(&mut self, key: &str, now: u64) {
        if let Some(&i) = self.lookup().by_key.get(key) {
            self.entries[i].last_used = now;
        }
    }
}

/// `sk-proj-abcdef…wxyz` → `sk-p…wxyz`. Short values (≤ 8 chars) show only the first
/// char and the length.
pub fn preview(value: &str) -> String {
    let chars: Vec<char> = value.chars().collect();
    if chars.len() <= 8 {
        let first: String = chars.iter().take(1).collect();
        return format!("{first}… ({} chars)", chars.len());
    }
    let head: String = chars[..4].iter().collect();
    let tail: String = chars[chars.len() - 4..].iter().collect();
    format!("{head}…{tail}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn finding(value: &str, kind: &str) -> Finding {
        Finding {
            start: 0,
            end: value.len(),
            value: value.into(),
            kind: kind.into(),
            rule: "t".into(),
            label: "Test".into(),
            category: Category::Secret,
            hint: None,
            confidence: 1.0,
        }
    }

    #[test]
    fn deterministic_keys_and_counters() {
        let mut v = Vault::new();
        assert_eq!(v.intern(&finding("abcdefgh", "API_KEY"), "t", 1).as_deref(), Some("API_KEY_1"));
        assert_eq!(v.intern(&finding("zzzzzzzz", "API_KEY"), "t", 1).as_deref(), Some("API_KEY_2"));
        assert_eq!(v.intern(&finding("abcdefgh", "API_KEY"), "t", 2).as_deref(), Some("API_KEY_1"));
        assert!(v.intern(&finding("abc", "API_KEY"), "t", 2).is_none());
        v.remove("API_KEY_2");
        assert_eq!(v.intern(&finding("yyyyyyyy", "API_KEY"), "t", 3).as_deref(), Some("API_KEY_3"));
        let back = Vault::from_json(&v.to_json()).unwrap();
        assert_eq!(back.key_for_value("abcdefgh"), Some("API_KEY_1"));
        assert_eq!(back.find_values("x abcdefgh y yyyyyyyy").len(), 2);
    }
}
