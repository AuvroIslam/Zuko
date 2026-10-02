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
use serde::{Deserialize, Serialize};

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
    counters: std::collections::BTreeMap<String, u32>,
    #[serde(skip)]
    index: VaultIndex,
}

/// Lookup tables rebuilt from `entries` (not serialized).
#[derive(Clone, Debug, Default)]
struct VaultIndex {
    built_for: Option<usize>,
}

/// One exact occurrence of a stored value inside a text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValueMatch {
    pub start: usize,
    pub end: usize,
    pub key: String,
}

impl Vault {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_json(s: &str) -> Result<Self, String> {
        let _ = s;
        todo!()
    }

    pub fn to_json(&self) -> String {
        todo!()
    }

    /// Returns the key for `f.value`, creating an entry if the value is new. Bumps
    /// `hits` and `last_used`. Returns `None` if the value is too short to intern
    /// ([`MIN_VALUE_LEN`], custom terms exempt).
    pub fn intern(&mut self, f: &Finding, source: &str, now: u64) -> Option<String> {
        let _ = (f, source, now, &self.index);
        todo!()
    }

    /// Adds a value by hand (from the UI). Same rules as [`Vault::intern`].
    pub fn add_manual(&mut self, value: &str, kind: &str, label: &str, now: u64) -> Option<String> {
        let _ = (value, kind, label, now);
        todo!()
    }

    pub fn get(&self, key: &str) -> Option<&Entry> {
        let _ = key;
        todo!()
    }

    pub fn key_for_value(&self, value: &str) -> Option<&str> {
        let _ = value;
        todo!()
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    pub fn views(&self) -> Vec<EntryView> {
        todo!()
    }

    pub fn remove(&mut self, key: &str) -> bool {
        let _ = key;
        todo!()
    }

    pub fn clear(&mut self) {
        todo!()
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
        let _ = text;
        todo!()
    }

    /// Merges entries from `other` (e.g. the browser extension's session vault). Values
    /// already present keep their key; new values get fresh keys in this vault. Returns
    /// a map from `other`'s keys to this vault's keys where they differ.
    pub fn merge(&mut self, other: &Vault, now: u64) -> Vec<(String, String)> {
        let _ = (other, now, &self.counters);
        todo!()
    }

    /// Marks `key` as used (rehydrated) at `now`.
    pub fn touch(&mut self, key: &str, now: u64) {
        let _ = (key, now);
        todo!()
    }
}

/// `sk-proj-abcdef…wxyz` → `sk-p…wxyz`. Short values (≤ 8 chars) show only the first
/// char and the length.
pub fn preview(value: &str) -> String {
    let _ = value;
    todo!()
}
