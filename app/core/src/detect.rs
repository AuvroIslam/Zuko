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
//!
//! NOTE: this is the baseline rule set; the full gitleaks-derived table replaces it.

use aho_corasick::{AhoCorasick, AhoCorasickBuilder, MatchKind};
use regex::Regex;
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

/// Validator applied to a candidate value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Check {
    None,
    Luhn,
    Iban,
    Entropy,
    Email,
}

struct Rule {
    id: &'static str,
    label: &'static str,
    kind: &'static str,
    category: Category,
    /// Lower is stronger when spans overlap.
    priority: u8,
    re: Regex,
    /// Capture group holding the value.
    group: usize,
    check: Check,
    confidence: f32,
}

struct Compiled {
    rules: Vec<Rule>,
    terms: Option<AhoCorasick>,
    placeholder: Regex,
}

/// A compiled detector. Build once and reuse: compiling the rule set costs a few ms.
pub struct Detector {
    cfg: DetectorConfig,
    inner: Compiled,
}

fn rule(
    id: &'static str,
    label: &'static str,
    kind: &'static str,
    category: Category,
    priority: u8,
    pattern: &str,
    group: usize,
    check: Check,
    confidence: f32,
) -> Rule {
    Rule {
        id,
        label,
        kind,
        category,
        priority,
        re: Regex::new(pattern).expect("valid rule regex"),
        group,
        check,
        confidence,
    }
}

impl Detector {
    /// Compiles the rule set for `cfg`.
    pub fn new(cfg: &DetectorConfig) -> Self {
        use Category::*;
        let mut rules = Vec::new();
        if cfg.secrets {
            rules.push(rule("private-key", "Private key", kinds::PRIVATE_KEY, Secret, 0,
                r"(-----BEGIN[ A-Z0-9_-]{0,100}PRIVATE KEY(?: BLOCK)?-----[\s\S]{16,}?-----END[ A-Z0-9_-]{0,100}PRIVATE KEY(?: BLOCK)?-----)", 1, Check::None, 0.99));
            rules.push(rule("anthropic-api-key", "Anthropic API key", kinds::API_KEY, Secret, 1,
                r"\b(sk-ant-(?:api03|admin01|oat01)-[A-Za-z0-9_\-]{20,120})", 1, Check::None, 0.99));
            rules.push(rule("openai-api-key", "OpenAI API key", kinds::API_KEY, Secret, 1,
                r"\b(sk-(?:proj|svcacct|admin)-[A-Za-z0-9_\-]{20,200}|sk-[A-Za-z0-9]{20}T3BlbkFJ[A-Za-z0-9]{20}|sk-[A-Za-z0-9]{40,64})\b", 1, Check::None, 0.97));
            rules.push(rule("aws-access-key", "AWS access key ID", kinds::API_KEY, Secret, 1,
                r"\b((?:AKIA|ASIA|ABIA|ACCA)[A-Z0-9]{16})\b", 1, Check::None, 0.97));
            rules.push(rule("github-token", "GitHub token", kinds::TOKEN, Secret, 1,
                r"\b((?:ghp|gho|ghu|ghs|ghr)_[A-Za-z0-9]{36}|github_pat_[A-Za-z0-9_]{60,100})\b", 1, Check::None, 0.98));
            rules.push(rule("google-api-key", "Google API key", kinds::API_KEY, Secret, 1,
                r"\b(AIza[0-9A-Za-z_\-]{35})\b", 1, Check::None, 0.95));
            rules.push(rule("stripe-key", "Stripe secret key", kinds::API_KEY, Secret, 1,
                r"\b((?:sk|rk)_(?:live|test)_[A-Za-z0-9]{20,99})\b", 1, Check::None, 0.97));
            rules.push(rule("slack-token", "Slack token", kinds::TOKEN, Secret, 1,
                r"\b(xox[baprs]-[A-Za-z0-9-]{10,200})\b", 1, Check::None, 0.95));
            rules.push(rule("jwt", "JSON Web Token", kinds::JWT, Secret, 2,
                r"\b(ey[A-Za-z0-9_-]{8,}\.ey[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,})\b", 1, Check::None, 0.9));
            rules.push(rule("connection-string", "Connection string with password", kinds::CONN_STRING, Secret, 2,
                r#"\b((?:postgres(?:ql)?|mysql|mongodb(?:\+srv)?|redis|amqps?|mssql)://[^\s:@/"']+:[^\s@/"']+@[^\s"'<>]+)"#, 1, Check::None, 0.93));
            rules.push(rule("generic-secret", "Secret value", kinds::SECRET, Secret, 4,
                r#"(?i)(?:password|passwd|pwd|secret|api[_-]?key|apikey|access[_-]?token|auth[_-]?token|client[_-]?secret|private[_-]?key)["']?\s*[:=]\s*["']?([^\s"'`;,{}()<>]{8,200})"#, 1, Check::Entropy, 0.75));
        }
        if cfg.pii {
            if cfg.emails {
                rules.push(rule("email", "Email address", kinds::EMAIL, Pii, 3,
                    r"\b([A-Za-z0-9._%+\-]{1,64}@[A-Za-z0-9.\-]{1,253}\.[A-Za-z]{2,24})\b", 1, Check::Email, 0.9));
            }
            if cfg.cards {
                rules.push(rule("credit-card", "Payment card number", kinds::CARD, Pii, 3,
                    r"\b((?:\d[ -]?){12,18}\d)\b", 1, Check::Luhn, 0.9));
            }
            if cfg.ibans {
                rules.push(rule("iban", "IBAN", kinds::IBAN, Pii, 3,
                    r"\b([A-Z]{2}\d{2}(?: ?[A-Z0-9]{4}){2,7}(?: ?[A-Z0-9]{1,4})?)\b", 1, Check::Iban, 0.9));
            }
            if cfg.phones {
                rules.push(rule("phone-bd", "Bangladesh mobile number", kinds::PHONE, Pii, 3,
                    r"(?:^|[^\d])((?:\+?880[ -]?|0)1[3-9]\d{2}[ -]?\d{6})\b", 1, Check::None, 0.85));
                rules.push(rule("phone-intl", "Phone number", kinds::PHONE, Pii, 4,
                    r"(?:^|[^\w+])(\+\d{1,3}[ .-]?\(?\d{1,4}\)?(?:[ .-]?\d{2,4}){2,4})\b", 1, Check::None, 0.7));
            }
        }
        let terms: Vec<&str> = cfg
            .custom_terms
            .iter()
            .map(|t| t.trim())
            .filter(|t| !t.is_empty())
            .collect();
        let terms = if terms.is_empty() {
            None
        } else {
            AhoCorasickBuilder::new()
                .match_kind(MatchKind::LeftmostLongest)
                .ascii_case_insensitive(true)
                .build(&terms)
                .ok()
        };
        let placeholder = Regex::new(r"\{\{ ?[A-Z][A-Z0-9_]*_[1-9][0-9]* ?\}\}").unwrap();
        Self { cfg: cfg.clone(), inner: Compiled { rules, terms, placeholder } }
    }

    /// All findings in `text` with confidence ≥ 0.5: non-overlapping, sorted by `start`,
    /// never overlapping an existing `{{KIND_N}}` placeholder.
    pub fn scan(&self, text: &str) -> Vec<Finding> {
        let protected: Vec<(usize, usize)> = self
            .inner
            .placeholder
            .find_iter(text)
            .map(|m| (m.start(), m.end()))
            .collect();
        let mut cands: Vec<(u8, Finding)> = Vec::new();
        for r in &self.inner.rules {
            for caps in r.re.captures_iter(text) {
                let Some(m) = caps.get(r.group) else { continue };
                let value = m.as_str();
                if !self.passes(r, value) {
                    continue;
                }
                cands.push((
                    r.priority,
                    Finding {
                        start: m.start(),
                        end: m.end(),
                        value: value.to_string(),
                        kind: r.kind.to_string(),
                        rule: r.id.to_string(),
                        label: r.label.to_string(),
                        category: r.category,
                        hint: hint_for(r.id, value),
                        confidence: r.confidence,
                    },
                ));
            }
        }
        if let Some(ac) = &self.inner.terms {
            for m in ac.find_iter(text) {
                if !word_bounded(text, m.start(), m.end()) {
                    continue;
                }
                cands.push((
                    6,
                    Finding {
                        start: m.start(),
                        end: m.end(),
                        value: text[m.start()..m.end()].to_string(),
                        kind: kinds::TERM.to_string(),
                        rule: "custom-term".into(),
                        label: "Custom term".into(),
                        category: Category::Custom,
                        hint: None,
                        confidence: 1.0,
                    },
                ));
            }
        }
        // Strongest first, then longest, then earliest.
        cands.sort_by(|a, b| {
            a.0.cmp(&b.0)
                .then((b.1.end - b.1.start).cmp(&(a.1.end - a.1.start)))
                .then(a.1.start.cmp(&b.1.start))
        });
        let mut taken: Vec<(usize, usize)> = protected.clone();
        let mut out = Vec::new();
        for (_, f) in cands {
            if f.confidence < 0.5 {
                continue;
            }
            if taken.iter().any(|&(s, e)| f.start < e && s < f.end) {
                continue;
            }
            taken.push((f.start, f.end));
            out.push(f);
        }
        out.sort_by_key(|f| f.start);
        out
    }

    pub fn config(&self) -> &DetectorConfig {
        &self.cfg
    }

    fn passes(&self, r: &Rule, value: &str) -> bool {
        if is_templated(value) || self.allowlisted(value) {
            return false;
        }
        match r.check {
            Check::None => true,
            Check::Luhn => {
                let digits: String = value.chars().filter(|c| c.is_ascii_digit()).collect();
                (13..=19).contains(&digits.len()) && luhn_valid(&digits)
            }
            Check::Iban => iban_valid(value),
            Check::Entropy => shannon_entropy(value) >= self.cfg.min_entropy,
            Check::Email => {
                let domain = value.rsplit('@').next().unwrap_or("").to_ascii_lowercase();
                !self.allowlisted(&domain)
            }
        }
    }

    fn allowlisted(&self, value: &str) -> bool {
        let v = value.to_ascii_lowercase();
        self.cfg.allowlist.iter().any(|a| {
            let a = a.to_ascii_lowercase();
            v == a || v.ends_with(&format!(".{a}")) || v.ends_with(&format!("@{a}"))
        })
    }
}

fn word_bounded(text: &str, start: usize, end: usize) -> bool {
    let before = text[..start].chars().next_back();
    let after = text[end..].chars().next();
    !before.is_some_and(|c| c.is_alphanumeric()) && !after.is_some_and(|c| c.is_alphanumeric())
}

/// Template-looking values that are not real secrets.
fn is_templated(v: &str) -> bool {
    let l = v.to_ascii_lowercase();
    v.starts_with('$')
        || v.starts_with('%')
        || v.starts_with("{{")
        || v.starts_with('<')
        || l.contains("your_")
        || l.contains("your-")
        || l.contains("example")
        || l.contains("placeholder")
        || l.contains("changeme")
        || l.contains("xxxx")
        || l.contains("****")
        || matches!(l.as_str(), "true" | "false" | "null" | "none" | "undefined")
}

fn hint_for(rule: &str, value: &str) -> Option<String> {
    match rule {
        "credit-card" => {
            let digits: String = value.chars().filter(|c| c.is_ascii_digit()).collect();
            let brand = match digits.as_bytes().first() {
                Some(b'4') => "Visa",
                Some(b'5') | Some(b'2') => "Mastercard",
                Some(b'3') => "American Express",
                Some(b'6') => "Discover",
                _ => "Card",
            };
            Some(format!("{brand} card ending {}", &digits[digits.len().saturating_sub(4)..]))
        }
        "email" => value
            .rsplit('@')
            .next()
            .map(|d| format!("email address at {}", d.to_ascii_lowercase())),
        "phone-bd" => Some("Bangladesh mobile number".into()),
        "openai-api-key" => Some("OpenAI API key".into()),
        "anthropic-api-key" => Some("Anthropic API key".into()),
        "aws-access-key" => Some("AWS access key ID".into()),
        "github-token" => Some("GitHub token".into()),
        "stripe-key" => Some(if value.contains("_test_") { "Stripe test key" } else { "Stripe live key" }.into()),
        _ => None,
    }
}

/// Shannon entropy of `s` in bits per character.
pub fn shannon_entropy(s: &str) -> f32 {
    let mut counts = std::collections::HashMap::new();
    let mut n = 0usize;
    for c in s.chars() {
        *counts.entry(c).or_insert(0usize) += 1;
        n += 1;
    }
    if n == 0 {
        return 0.0;
    }
    let n = n as f32;
    counts
        .values()
        .map(|&c| {
            let p = c as f32 / n;
            -p * p.log2()
        })
        .sum()
}

/// Luhn checksum over the ASCII digits of `s` (non-digits ignored).
pub fn luhn_valid(s: &str) -> bool {
    let digits: Vec<u32> = s.chars().filter_map(|c| c.to_digit(10)).collect();
    if digits.len() < 2 {
        return false;
    }
    let sum: u32 = digits
        .iter()
        .rev()
        .enumerate()
        .map(|(i, &d)| {
            if i % 2 == 1 {
                let x = d * 2;
                if x > 9 { x - 9 } else { x }
            } else {
                d
            }
        })
        .sum();
    sum % 10 == 0
}

/// IBAN mod-97 check (spaces ignored, case-insensitive).
pub fn iban_valid(s: &str) -> bool {
    let compact: String = s.chars().filter(|c| !c.is_whitespace()).collect::<String>().to_ascii_uppercase();
    if compact.len() < 15 || compact.len() > 34 || !compact.chars().all(|c| c.is_ascii_alphanumeric()) {
        return false;
    }
    let (head, tail) = compact.split_at(4);
    let mut rem: u64 = 0;
    for c in tail.chars().chain(head.chars()) {
        let v = if c.is_ascii_digit() { c as u64 - '0' as u64 } else { c as u64 - 'A' as u64 + 10 };
        rem = if v >= 10 { (rem * 100 + v) % 97 } else { (rem * 10 + v) % 97 };
    }
    rem == 1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn baseline_detects_common_secrets_and_pii() {
        let d = Detector::new(&DetectorConfig::default());
        let t = "key sk-proj-abcdefghijklmnopqrstuvwx1234 mail bob@acme.io card 4242 4242 4242 4242 phone +8801712345678";
        let kinds: Vec<_> = d.scan(t).into_iter().map(|f| f.kind).collect();
        assert_eq!(kinds, vec!["API_KEY", "EMAIL", "CARD", "PHONE"]);
    }

    #[test]
    fn skips_placeholders_and_templates() {
        let d = Detector::new(&DetectorConfig::default());
        assert!(d.scan("api_key = {{API_KEY_1}} password=${DB_PASS}").is_empty());
        assert!(d.scan("contact me at someone@example.com").is_empty());
    }

    #[test]
    fn validators() {
        assert!(luhn_valid("4242424242424242"));
        assert!(!luhn_valid("4242424242424241"));
        assert!(iban_valid("GB82 WEST 1234 5698 7654 32"));
        assert!(!iban_valid("GB82 WEST 1234 5698 7654 33"));
    }
}
