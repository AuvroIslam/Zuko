//! The vault: a deterministic, two-way map between sensitive values and placeholder keys.
//!
//! * The same value always maps to the same key (`API_KEY_1`), across turns and
//!   sessions, so re-masked conversation history is byte-identical (prompt caching and
//!   thinking-block signatures keep working).
//! * Counters are per kind and never reused, even after [`Vault::remove`].
//! * The vault itself holds plaintext; persistence and encryption are the caller's job
//!   (the app encrypts it with a key held in the OS keyring).
//! * Exact-value matching across texts goes through [`Vault::find_values`], an
//!   Aho-Corasick automaton over every stored value (and its JSON-escaped form),
//!   rebuilt lazily after changes.

use crate::detect::{Category, Finding};
use crate::placeholder;
use aho_corasick::{AhoCorasick, AhoCorasickBuilder, MatchKind};
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

/// Lookup tables rebuilt from `entries` (not serialized). Shared by clones until a
/// mutation invalidates them.
#[derive(Clone, Debug, Default)]
struct VaultIndex {
    tables: OnceLock<Arc<Tables>>,
}

#[derive(Debug)]
struct Tables {
    by_value: HashMap<String, usize>,
    by_key: HashMap<String, usize>,
    /// Patterns: each value, plus its JSON-escaped form when different.
    matcher: Option<AhoCorasick>,
    /// Pattern index → entry index.
    pattern_entry: Vec<usize>,
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

impl Vault {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_json(s: &str) -> Result<Self, String> {
        let mut v: Vault = serde_json::from_str(s).map_err(|e| e.to_string())?;
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

    fn tables(&self) -> Arc<Tables> {
        self.index
            .tables
            .get_or_init(|| {
                let mut by_value = HashMap::new();
                let mut by_key = HashMap::new();
                let mut patterns = Vec::new();
                let mut pattern_entry = Vec::new();
                for (i, e) in self.entries.iter().enumerate() {
                    by_value.insert(e.value.clone(), i);
                    by_key.insert(e.key.clone(), i);
                    patterns.push(e.value.clone());
                    pattern_entry.push(i);
                    let esc = json_escaped(&e.value);
                    if esc != e.value {
                        patterns.push(esc);
                        pattern_entry.push(i);
                    }
                }
                let matcher = if patterns.is_empty() {
                    None
                } else {
                    AhoCorasickBuilder::new()
                        .match_kind(MatchKind::LeftmostLongest)
                        .build(&patterns)
                        .ok()
                };
                Arc::new(Tables { by_value, by_key, matcher, pattern_entry })
            })
            .clone()
    }

    fn insert(&mut self, value: &str, kind: &str, label: &str, category: Category, hint: Option<String>, source: &str, now: u64) -> Option<String> {
        if value.is_empty() || (category != Category::Custom && value.chars().count() < MIN_VALUE_LEN) {
            return None;
        }
        if let Some(&i) = self.tables().by_value.get(value) {
            let e = &mut self.entries[i];
            e.hits += 1;
            e.last_used = now;
            return Some(e.key.clone());
        }
        let kind = if placeholder::valid_kind(kind) { kind.to_string() } else { "SECRET".to_string() };
        let n = self.counters.entry(kind.clone()).or_insert(0);
        *n += 1;
        let key = placeholder::key(&kind, *n);
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
        self.invalidate();
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
        let key = key.trim_start_matches(placeholder::OPEN).trim_end_matches(placeholder::CLOSE);
        self.tables().by_key.get(key).map(|&i| &self.entries[i])
    }

    pub fn key_for_value(&self, value: &str) -> Option<&str> {
        self.tables().by_value.get(value).map(|&i| self.entries[i].key.as_str())
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
    /// contain `"` or `\`.
    pub fn find_values(&self, text: &str) -> Vec<ValueMatch> {
        let t = self.tables();
        let Some(ac) = &t.matcher else { return Vec::new() };
        ac.find_iter(text)
            .map(|m| ValueMatch {
                start: m.start(),
                end: m.end(),
                key: self.entries[t.pattern_entry[m.pattern().as_usize()]].key.clone(),
            })
            .collect()
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
        if let Some(&i) = self.tables().by_key.get(key) {
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
