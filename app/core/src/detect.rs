//! Secret and PII detection.
//!
//! Design (see plan.md §2 and the detection report):
//! * Secret rules are ported from gitleaks (MIT) and compiled with
//!   `regex::bytes::RegexBuilder::unicode(false)` so `\b \w \d \s` are ASCII like RE2.
//!   An Aho-Corasick keyword prefilter decides which rules run on a text. The secret span
//!   is always the rule's capture group, never the whole match.
//! * PII rules (email, phone incl. +880, card + Luhn + IIN, IBAN + mod-97, IPv4/IPv6,
//!   SSN, context-word NID/passport/DOB) use Unicode regexes; prev/next-char guards are
//!   checked in code.
//! * Custom terms (user deny-list: client names, codenames…) are matched with
//!   Aho-Corasick, leftmost-longest, ASCII case-insensitive.
//! * Context-free entropy detection is **off by default** (it masks git SHAs, lockfile
//!   hashes and identifiers and breaks the agent's work). Keyword-anchored generic
//!   assignments (`password = "…"`, `api_key: …`) with an entropy floor are on.
//! * Allowlists drop templated values (`${VAR}`, `{{x}}`, `%VAR%`, `your_api_key_here`,
//!   `xxxx`, `true/false/null`), UUIDs where a secret is not expected, and anything in
//!   [`DetectorConfig::allowlist`].
//! * Overlaps resolve by priority: existing placeholders (never re-masked) → prefixed
//!   secret rules → validated PII → generic/context rules → custom terms; ties go to the
//!   longer span, then the higher confidence.
//! * Findings never overlap an existing `{{KIND_N}}` placeholder, so masking is idempotent.

use serde::{Deserialize, Serialize};

/// Broad class of a finding, used for toggles and the UI.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    Secret,
    Pii,
    Custom,
}

/// One detected sensitive value.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Finding {
    /// Byte offset of the value in the scanned text (char boundary).
    pub start: usize,
    /// Byte offset one past the value (char boundary).
    pub end: usize,
    /// The exact sensitive substring, `&text[start..end]`.
    pub value: String,
    /// Placeholder kind, one of [`kinds`]: `API_KEY`, `TOKEN`, `PRIVATE_KEY`, `PASSWORD`,
    /// `SECRET`, `CONN_STRING`, `JWT`, `EMAIL`, `PHONE`, `CARD`, `IBAN`, `IP`, `NID`, `TERM`.
    pub kind: String,
    /// Stable rule id, e.g. `openai-api-key`, `email`, `custom-term`.
    pub rule: String,
    /// Human label, e.g. "OpenAI API key", "Email address".
    pub label: String,
    pub category: Category,
    /// A **non-sensitive** attribute that keeps answers useful without revealing the
    /// value: "Visa card ending 4242", "Bangladesh mobile number", "Gmail address",
    /// "OpenAI project key". Never contains the value or a reversible part of it beyond
    /// what the hint states (e.g. last 4 card digits).
    pub hint: Option<String>,
    /// 0.0–1.0. Findings below 0.5 are not returned by [`Detector::scan`].
    pub confidence: f32,
}

/// Placeholder kinds the detector emits.
pub mod kinds {
    pub const API_KEY: &str = "API_KEY";
    pub const TOKEN: &str = "TOKEN";
    pub const PRIVATE_KEY: &str = "PRIVATE_KEY";
    pub const PASSWORD: &str = "PASSWORD";
    pub const SECRET: &str = "SECRET";
    pub const CONN_STRING: &str = "CONN_STRING";
    pub const JWT: &str = "JWT";
    pub const EMAIL: &str = "EMAIL";
    pub const PHONE: &str = "PHONE";
    pub const CARD: &str = "CARD";
    pub const IBAN: &str = "IBAN";
    pub const IP: &str = "IP";
    pub const NID: &str = "NID";
    pub const TERM: &str = "TERM";
}

/// What to detect. Serialized into the policy file under `privacy`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct DetectorConfig {
    /// Master switch for secret rules (API keys, tokens, private keys, passwords…).
    pub secrets: bool,
    /// Master switch for PII rules.
    pub pii: bool,
    pub emails: bool,
    pub phones: bool,
    pub cards: bool,
    pub ibans: bool,
    /// Public IP addresses (private, loopback and link-local ranges are never masked).
    pub ips: bool,
    pub national_ids: bool,
    /// Context-free high-entropy token detection. Off by default (see module docs).
    pub generic_entropy: bool,
    /// Minimum Shannon entropy (bits/char) for keyword-anchored generic secrets.
    pub min_entropy: f32,
    /// User deny-list, matched case-insensitively as whole words where possible.
    pub custom_terms: Vec<String>,
    /// Values or domains never masked: exact values, or email/URL domains such as
    /// `example.com` (matches the domain and its subdomains).
    pub allowlist: Vec<String>,
}

impl Default for DetectorConfig {
    fn default() -> Self {
        Self {
            secrets: true,
            pii: true,
            emails: true,
            phones: true,
            cards: true,
            ibans: true,
            ips: false,
            national_ids: true,
            generic_entropy: false,
            min_entropy: 3.5,
            custom_terms: Vec::new(),
            allowlist: vec![
                "example.com".into(),
                "example.org".into(),
                "example.net".into(),
                "localhost".into(),
                "noreply@anthropic.com".into(),
                "users.noreply.github.com".into(),
            ],
        }
    }
}

/// A compiled detector. Build once and reuse: compiling the rule set costs a few ms.
pub struct Detector {
    cfg: DetectorConfig,
    // Implementation-defined compiled state (rule tables, regexes, Aho-Corasick
    // automatons). Must be Send + Sync.
    inner: Box<dyn std::any::Any + Send + Sync>,
}

impl Detector {
    /// Compiles the rule set for `cfg`.
    pub fn new(cfg: &DetectorConfig) -> Self {
        let _ = &cfg;
        todo!("compile rules")
    }

    /// All findings in `text` with confidence ≥ 0.5: non-overlapping, sorted by `start`,
    /// never overlapping an existing `{{KIND_N}}` placeholder.
    pub fn scan(&self, text: &str) -> Vec<Finding> {
        let _ = (&self.inner, text);
        todo!("scan")
    }

    pub fn config(&self) -> &DetectorConfig {
        &self.cfg
    }
}

/// Shannon entropy of `s` in bits per character.
pub fn shannon_entropy(s: &str) -> f32 {
    let _ = s;
    todo!()
}

/// Luhn checksum over the ASCII digits of `s` (non-digits ignored).
pub fn luhn_valid(s: &str) -> bool {
    let _ = s;
    todo!()
}

/// IBAN mod-97 check (spaces ignored, case-insensitive).
pub fn iban_valid(s: &str) -> bool {
    let _ = s;
    todo!()
}
