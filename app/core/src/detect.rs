//! Secret and PII detection.
//!
//! Design (see plan.md §2 and the detection report):
//! * Secret rules are ported from gitleaks (MIT, see `core/THIRD_PARTY.md`) and compiled
//!   with `regex::bytes::RegexBuilder::unicode(false)` so `\b \w \d \s` are ASCII like RE2.
//!   An Aho-Corasick keyword prefilter decides which rules run on a text. The secret span
//!   is always the rule's capture group, never the whole match.
//! * PII rules (email, phone incl. +880, card + Luhn + IIN, IBAN + mod-97, IPv4/IPv6,
//!   SSN, context-word NID/passport/DOB) use ASCII regexes; prev/next-char guards are
//!   checked in code, never consumed by the regex (so two adjacent values both match).
//! * Custom terms (user deny-list: client names, codenames…) are matched with
//!   Aho-Corasick, leftmost-longest, ASCII case-insensitive.
//! * Context-free entropy detection is **off by default** (it masks git SHAs, lockfile
//!   hashes and identifiers and breaks the agent's work). Keyword-anchored generic
//!   assignments (`password = "…"`, `api_key: …`) with an entropy floor are on.
//! * Allowlists drop templated values (`${VAR}`, `{{x}}`, `%VAR%`, `your_api_key_here`,
//!   `xxxx`, `true/false/null`), references (`process.env.X`, `os.getenv`), type names,
//!   UUIDs where a secret is not expected, `data:` URIs and `sha512-…` integrity hashes,
//!   and anything in [`DetectorConfig::allowlist`].
//! * Overlaps resolve by priority: existing placeholders (never re-masked) → prefixed
//!   secret rules → validated PII → generic/context rules → custom terms; ties go to the
//!   longer span, then the higher confidence.
//! * Findings never overlap an existing `{{KIND_N}}` placeholder, so masking is idempotent.

use aho_corasick::{AhoCorasick, AhoCorasickBuilder, MatchKind};
use regex::bytes::{Regex as BRegex, RegexBuilder};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

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
    /// `SECRET`, `CONN_STRING`, `JWT`, `EMAIL`, `PHONE`, `CARD`, `IBAN`, `IP`, `NID`,
    /// `DOB`, `TERM`.
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
    /// Government identifiers: national ID, SSN, passport number.
    pub const NID: &str = "NID";
    /// Date of birth.
    pub const DOB: &str = "DOB";
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
    /// Minimum Shannon entropy (bits/char) for keyword-anchored generic secrets. The
    /// effective floor is length-adjusted and never exceeds this value.
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

// ---------------------------------------------------------------------------------------
// Rule tables
// ---------------------------------------------------------------------------------------

/// Validator applied to a secret candidate.
#[derive(Clone, Copy, Debug, PartialEq)]
enum V {
    None,
    /// At least one ASCII digit and one ASCII letter.
    AlnumMix,
    /// Minimum Shannon entropy.
    Entropy(f32),
    /// Base64url header decodes to a JSON object with `alg`/`typ`.
    Jwt,
    /// PEM body without an END line: trim trailing whitespace.
    PemTruncated,
    /// `scheme://user:pass@host…`: password not templated, host not allowlisted.
    ConnString,
    /// Password of a URL (`https://u:PASS@host`).
    UrlPassword,
    /// Only templated/placeholder checks (curl -u, mysql -p).
    Weak,
    /// Header/bearer tokens.
    TokenLike,
    /// Keyword-anchored assignment; group 1 = key name, group 2 = value.
    Assign,
    /// Context-free high-entropy token.
    HighEntropy,
    /// Azure AD client secret: delimiter guards checked in code, entropy floor.
    AzureAd,
}

struct Spec {
    id: &'static str,
    label: &'static str,
    kind: &'static str,
    /// Lower is stronger when spans overlap.
    prio: u8,
    conf: f32,
    /// Lowercase literals; the rule only runs if one occurs (empty = always).
    kw: &'static [&'static str],
    v: V,
    pat: &'static str,
}

const P_PREFIXED: u8 = 1;
const P_STRUCTURED: u8 = 2;
const P_PII: u8 = 3;
const P_GENERIC: u8 = 4;
const P_ENTROPY: u8 = 5;
const P_TERM: u8 = 6;

use kinds::*;

/// Secret rules. Regexes derived in part from gitleaks (MIT); see THIRD_PARTY.md.
const SECRET_SPECS: &[Spec] = &[
    Spec { id: "private-key", label: "Private key", kind: PRIVATE_KEY, prio: 0, conf: 0.99, kw: &["-----begin"], v: V::None,
        pat: r"(-----BEGIN[ A-Z0-9_-]{0,100}PRIVATE KEY(?: BLOCK)?-----(?:[A-Za-z0-9+/=\s]|\\[nr]|[A-Za-z][A-Za-z-]{1,30}:[^\n\\]*){32,}?-----END[ A-Z0-9_-]{0,100}PRIVATE KEY(?: BLOCK)?-----)" },
    Spec { id: "private-key-truncated", label: "Private key", kind: PRIVATE_KEY, prio: 0, conf: 0.9, kw: &["-----begin"], v: V::PemTruncated,
        pat: r"(-----BEGIN[ A-Z0-9_-]{0,100}PRIVATE KEY(?: BLOCK)?-----(?:[A-Za-z0-9+/=\s]|\\[nr]){64,})" },
    Spec { id: "anthropic-api-key", label: "Anthropic API key", kind: API_KEY, prio: P_PREFIXED, conf: 0.99, kw: &["sk-ant-"], v: V::None,
        pat: r"\b(sk-ant-[a-z]{2,6}[0-9]{2}-[A-Za-z0-9_-]{32,400})(?:[^A-Za-z0-9_-]|$)" },
    Spec { id: "openai-api-key", label: "OpenAI API key", kind: API_KEY, prio: P_PREFIXED, conf: 0.98, kw: &["sk-proj-", "sk-svcacct-", "sk-admin-", "sk-none-", "t3blbkfj"], v: V::None,
        pat: r"\b(sk-(?:proj|svcacct|admin|None)-[A-Za-z0-9_-]{20,400}|sk-[A-Za-z0-9]{20}T3BlbkFJ[A-Za-z0-9]{20})(?:[^A-Za-z0-9_-]|$)" },
    Spec { id: "openrouter-api-key", label: "OpenRouter API key", kind: API_KEY, prio: P_PREFIXED, conf: 0.98, kw: &["sk-or-v1-"], v: V::None,
        pat: r"\b(sk-or-v1-[a-f0-9]{64})\b" },
    Spec { id: "sk-api-key", label: "API key", kind: API_KEY, prio: P_PREFIXED, conf: 0.85, kw: &["sk-"], v: V::AlnumMix,
        pat: r"\b(sk-[A-Za-z0-9]{32,64})(?:[^A-Za-z0-9_-]|$)" },
    Spec { id: "groq-api-key", label: "Groq API key", kind: API_KEY, prio: P_PREFIXED, conf: 0.97, kw: &["gsk_"], v: V::None,
        pat: r"\b(gsk_[A-Za-z0-9]{52})\b" },
    Spec { id: "xai-api-key", label: "xAI API key", kind: API_KEY, prio: P_PREFIXED, conf: 0.97, kw: &["xai-"], v: V::None,
        pat: r"\b(xai-[A-Za-z0-9]{80})\b" },
    Spec { id: "huggingface-token", label: "Hugging Face token", kind: TOKEN, prio: P_PREFIXED, conf: 0.95, kw: &["hf_"], v: V::Entropy(3.8),
        pat: r"\b(hf_[A-Za-z0-9]{34,40})\b" },
    Spec { id: "replicate-api-token", label: "Replicate API token", kind: TOKEN, prio: P_PREFIXED, conf: 0.95, kw: &["r8_"], v: V::AlnumMix,
        pat: r"\b(r8_[A-Za-z0-9]{37})\b" },
    Spec { id: "perplexity-api-key", label: "Perplexity API key", kind: API_KEY, prio: P_PREFIXED, conf: 0.97, kw: &["pplx-"], v: V::None,
        pat: r"\b(pplx-[A-Za-z0-9]{48})\b" },
    Spec { id: "google-api-key", label: "Google API key", kind: API_KEY, prio: P_PREFIXED, conf: 0.95, kw: &["aiza"], v: V::Entropy(3.5),
        pat: r"\b(AIza[0-9A-Za-z_-]{35})(?:[^0-9A-Za-z_-]|$)" },
    Spec { id: "google-oauth-secret", label: "Google OAuth client secret", kind: SECRET, prio: P_PREFIXED, conf: 0.97, kw: &["gocspx-"], v: V::None,
        pat: r"\b(GOCSPX-[A-Za-z0-9_-]{28})(?:[^A-Za-z0-9_-]|$)" },
    Spec { id: "aws-access-key", label: "AWS access key ID", kind: API_KEY, prio: P_PREFIXED, conf: 0.97, kw: &["akia", "asia", "abia", "acca", "a3t"], v: V::Entropy(3.0),
        pat: r"\b((?:AKIA|ASIA|ABIA|ACCA|A3T[A-Z0-9])[A-Z2-7]{16})\b" },
    Spec { id: "aws-secret-key", label: "AWS secret access key", kind: SECRET, prio: P_PREFIXED, conf: 0.9, kw: &["secret"], v: V::Entropy(4.0),
        pat: r#"(?i)(?:aws)?_?secret_?(?:access)?_?key["'\x60]?[ \t]*(?:=|:=|:|=>)[ \t]*["'\x60]?([A-Za-z0-9/+]{40})(?:[^A-Za-z0-9/+=]|$)"# },
    Spec { id: "github-token", label: "GitHub token", kind: TOKEN, prio: P_PREFIXED, conf: 0.98, kw: &["ghp_", "gho_", "ghu_", "ghs_", "ghr_"], v: V::Entropy(3.0),
        pat: r"\b(gh[pousr]_[A-Za-z0-9]{36,251})\b" },
    Spec { id: "github-fine-grained-pat", label: "GitHub fine-grained token", kind: TOKEN, prio: P_PREFIXED, conf: 0.98, kw: &["github_pat_"], v: V::None,
        pat: r"\b(github_pat_[A-Za-z0-9_]{60,120})\b" },
    Spec { id: "gitlab-token", label: "GitLab token", kind: TOKEN, prio: P_PREFIXED, conf: 0.97, kw: &["glpat-", "glptt-", "gldt-", "glrt-"], v: V::None,
        pat: r"\b(gl(?:pat|ptt|dt|rt)-[A-Za-z0-9_-]{20,64})(?:[^A-Za-z0-9_-]|$)" },
    Spec { id: "stripe-key", label: "Stripe secret key", kind: API_KEY, prio: P_PREFIXED, conf: 0.97, kw: &["sk_live", "sk_test", "sk_prod", "rk_live", "rk_test", "rk_prod"], v: V::None,
        pat: r"\b((?:sk|rk)_(?:live|test|prod)_[A-Za-z0-9]{10,99})\b" },
    Spec { id: "stripe-webhook-secret", label: "Stripe webhook secret", kind: SECRET, prio: P_PREFIXED, conf: 0.97, kw: &["whsec_"], v: V::None,
        pat: r"\b(whsec_[A-Za-z0-9+/=]{32,100})" },
    Spec { id: "slack-token", label: "Slack token", kind: TOKEN, prio: P_PREFIXED, conf: 0.95, kw: &["xoxb-", "xoxp-", "xoxa-", "xoxe", "xoxo-", "xoxs-", "xoxr-"], v: V::AlnumMix,
        pat: r"\b((?:xoxe\.)?xox[abeoprs]-[0-9A-Za-z-]{10,250})" },
    Spec { id: "slack-app-token", label: "Slack app token", kind: TOKEN, prio: P_PREFIXED, conf: 0.95, kw: &["xapp-"], v: V::None,
        pat: r"(?i)\b(xapp-[0-9]-[A-Z0-9]+-[0-9]+-[a-z0-9]+)" },
    Spec { id: "webhook-url", label: "Webhook URL", kind: SECRET, prio: P_PREFIXED, conf: 0.95, kw: &["hooks.slack.com", "/api/webhooks/"], v: V::None,
        pat: r"(https://hooks\.slack\.com/(?:services|workflows|triggers)/[A-Za-z0-9+/]{43,56}|https://(?:ptb\.|canary\.)?discord(?:app)?\.com/api/webhooks/[0-9]{15,22}/[A-Za-z0-9_-]{60,72})" },
    Spec { id: "telegram-bot-token", label: "Telegram bot token", kind: TOKEN, prio: P_PREFIXED, conf: 0.9, kw: &[":a"], v: V::None,
        pat: r"\b([0-9]{5,16}:A[A-Za-z0-9_-]{34})(?:[^A-Za-z0-9_-]|$)" },
    Spec { id: "sendgrid-api-key", label: "SendGrid API key", kind: API_KEY, prio: P_PREFIXED, conf: 0.97, kw: &["sg."], v: V::None,
        pat: r"\b(SG\.[A-Za-z0-9_-]{22}\.[A-Za-z0-9_-]{43})(?:[^A-Za-z0-9_-]|$)" },
    Spec { id: "npm-token", label: "npm access token", kind: TOKEN, prio: P_PREFIXED, conf: 0.97, kw: &["npm_"], v: V::AlnumMix,
        pat: r"\b(npm_[A-Za-z0-9]{36})\b" },
    Spec { id: "pypi-token", label: "PyPI upload token", kind: TOKEN, prio: P_PREFIXED, conf: 0.98, kw: &["pypi-ageichlwas5vcmc"], v: V::None,
        pat: r"\b(pypi-AgEIcHlwaS5vcmc[A-Za-z0-9_-]{50,300})" },
    Spec { id: "shopify-token", label: "Shopify access token", kind: TOKEN, prio: P_PREFIXED, conf: 0.97, kw: &["shpat_", "shpss_", "shpca_", "shppa_"], v: V::None,
        pat: r"\b(shp(?:at|ss|ca|pa)_[a-fA-F0-9]{32})\b" },
    Spec { id: "digitalocean-token", label: "DigitalOcean token", kind: TOKEN, prio: P_PREFIXED, conf: 0.97, kw: &["dop_v1_", "doo_v1_", "dor_v1_"], v: V::None,
        pat: r"\b(do[opr]_v1_[a-f0-9]{64})\b" },
    Spec { id: "twilio-api-key", label: "Twilio API key", kind: API_KEY, prio: P_PREFIXED, conf: 0.85, kw: &["sk"], v: V::AlnumMix,
        pat: r"\b(SK[0-9a-fA-F]{32})\b" },
    Spec { id: "mailchimp-api-key", label: "Mailchimp API key", kind: API_KEY, prio: P_PREFIXED, conf: 0.9, kw: &["-us"], v: V::None,
        pat: r"\b([0-9a-f]{32}-us[0-9]{1,2})\b" },
    Spec { id: "hashicorp-vault-token", label: "HashiCorp Vault token", kind: TOKEN, prio: P_PREFIXED, conf: 0.95, kw: &["hvs.", "hvb."], v: V::None,
        pat: r"\b(hv[sb]\.[A-Za-z0-9_-]{90,300})" },
    Spec { id: "databricks-token", label: "Databricks token", kind: TOKEN, prio: P_PREFIXED, conf: 0.95, kw: &["dapi"], v: V::AlnumMix,
        pat: r"\b(dapi[a-f0-9]{32}(?:-[0-9])?)\b" },
    Spec { id: "linear-api-key", label: "Linear API key", kind: API_KEY, prio: P_PREFIXED, conf: 0.97, kw: &["lin_api_"], v: V::None,
        pat: r"\b(lin_api_[A-Za-z0-9]{40})\b" },
    Spec { id: "notion-token", label: "Notion integration token", kind: TOKEN, prio: P_PREFIXED, conf: 0.97, kw: &["ntn_"], v: V::None,
        pat: r"\b(ntn_[0-9]{11}[A-Za-z0-9]{35})\b" },
    Spec { id: "postman-api-key", label: "Postman API key", kind: API_KEY, prio: P_PREFIXED, conf: 0.97, kw: &["pmak-"], v: V::None,
        pat: r"\b(PMAK-[a-fA-F0-9]{24}-[a-fA-F0-9]{34})\b" },
    Spec { id: "atlassian-api-token", label: "Atlassian API token", kind: TOKEN, prio: P_PREFIXED, conf: 0.97, kw: &["atatt3"], v: V::None,
        pat: r"\b(ATATT3[A-Za-z0-9_=-]{186})" },
    Spec { id: "supabase-token", label: "Supabase access token", kind: TOKEN, prio: P_PREFIXED, conf: 0.97, kw: &["sbp_"], v: V::None,
        pat: r"\b(sbp_[a-f0-9]{40})\b" },
    Spec { id: "age-secret-key", label: "age secret key", kind: PRIVATE_KEY, prio: P_PREFIXED, conf: 0.99, kw: &["age-secret-key-1"], v: V::None,
        pat: r"\b(AGE-SECRET-KEY-1[QPZRY9X8GF2TVDW0S3JN54KHCE6MUA7L]{58})\b" },
    Spec { id: "facebook-access-token", label: "Facebook access token", kind: TOKEN, prio: P_PREFIXED, conf: 0.9, kw: &["eaam", "eaac"], v: V::AlnumMix,
        pat: r"\b(EAA[MC][A-Za-z0-9]{100,400})\b" },
    Spec { id: "azure-storage-key", label: "Azure storage account key", kind: SECRET, prio: P_PREFIXED, conf: 0.97, kw: &["accountkey="], v: V::None,
        pat: r"(?i)\bAccountKey=([A-Za-z0-9+/]{86}==)" },
    Spec { id: "azure-ad-client-secret", label: "Azure AD client secret", kind: SECRET, prio: P_PREFIXED, conf: 0.9, kw: &["q~"], v: V::AzureAd,
        pat: r"([a-zA-Z0-9_~.]{3}[0-9]Q~[a-zA-Z0-9_~.-]{31,34})" },
    Spec { id: "jwt", label: "JSON Web Token", kind: JWT, prio: P_PREFIXED, conf: 0.95, kw: &["eyj"], v: V::Jwt,
        pat: r"\b(eyJ[A-Za-z0-9_-]{10,}\.eyJ[A-Za-z0-9_-]{10,}\.(?:[A-Za-z0-9_-]{10,}={0,2})?)" },
    Spec { id: "connection-string", label: "Connection string with password", kind: CONN_STRING, prio: P_STRUCTURED, conf: 0.95, kw: &["://"], v: V::ConnString,
        pat: r#"(?i)\b((?:postgres(?:ql)?|mysql|mariadb|mongodb(?:\+srv)?|rediss?|amqps?|mssql|sqlserver|clickhouse|snowflake|jdbc:[a-z0-9]+)://[^\s:/@"'\x60]{0,128}:[^\s@/"'\x60]{1,256}@[^\s"'\x60<>]{1,512})"# },
    Spec { id: "url-password", label: "Password in URL", kind: PASSWORD, prio: P_STRUCTURED, conf: 0.9, kw: &["://"], v: V::UrlPassword,
        pat: r#"(?i)\b(?:https?|ftp|sftp|ssh|git|wss?)://[^\s:/@"'\x60]{1,128}:([^\s@/"'\x60]{3,256})@[A-Za-z0-9.-]+"# },
    Spec { id: "authorization-header", label: "Authorization header token", kind: TOKEN, prio: P_STRUCTURED, conf: 0.9, kw: &["authorization"], v: V::TokenLike,
        pat: r#"(?i)\b(?:proxy-)?authorization["'\x60]?[ \t]*[:=][ \t]*["'\x60]?(?:bearer|basic|token|digest)[ \t]+([A-Za-z0-9._~+/=-]{8,})"# },
    Spec { id: "api-key-header", label: "API key header", kind: API_KEY, prio: P_STRUCTURED, conf: 0.9, kw: &["api-key", "x-auth-token", "subscription-key"], v: V::TokenLike,
        pat: r#"(?i)\b(?:x-api-key|x-auth-token|api-key|x-goog-api-key|ocp-apim-subscription-key)["'\x60]?[ \t]*[:=][ \t]*["'\x60]?([A-Za-z0-9._~+/=-]{12,})"# },
    Spec { id: "curl-user-password", label: "Password in curl command", kind: PASSWORD, prio: P_STRUCTURED, conf: 0.85, kw: &["curl"], v: V::Weak,
        pat: r#"(?i)\bcurl\b[^\n]{0,300}?[ \t](?:-u|--user)(?:[ \t]+|=)["']?[^:\s"']{1,64}:([^\s"']{3,128})"# },
    Spec { id: "mysql-password-flag", label: "Password in mysql command", kind: PASSWORD, prio: P_STRUCTURED, conf: 0.85, kw: &["mysql"], v: V::Weak,
        pat: r#"(?i)\bmysql(?:dump|admin)?\b[^\n]{0,200}?[ \t](?-i:-p)([^\s"'\x60;|&]{3,128})"# },
    Spec { id: "bearer-token", label: "Bearer token", kind: TOKEN, prio: P_GENERIC, conf: 0.8, kw: &["bearer"], v: V::TokenLike,
        pat: r"(?i)\bbearer[ \t]+([A-Za-z0-9._~+/-]{20,}=*)" },
    Spec { id: "cli-secret-flag", label: "Secret command-line flag", kind: SECRET, prio: P_GENERIC, conf: 0.75, kw: &["--pass", "--token", "--secret", "--api", "--auth", "--access", "--db-", "--admin-", "--client-"], v: V::Assign,
        pat: r#"(?i)(?:^|[ \t])--((?:db-|admin-|client-|api-)?(?:password|passwd|token|secret|api-key|apikey|auth-token|access-token))(?:=|[ \t]+)["']?([^\s"'\x60;|&<>(){}\[\]\\]{4,200})"# },
    Spec { id: "generic-secret", label: "Secret value", kind: SECRET, prio: P_GENERIC, conf: 0.75, kw: &["pass", "pwd", "secret", "token", "key", "credential", "auth", "dsn"], v: V::Assign,
        pat: r#"(?i)\b([a-z0-9_.-]{0,40}?(?:passw(?:or)?d|passphrase|pwd|secret|token|api[_.-]?key|apikey|access[_.-]?key|private[_.-]?key|client[_.-]?secret|auth[_.-]?(?:key|token)|credentials?|session[_.-]?(?:key|secret)|signing[_.-]?key|encryption[_.-]?key|master[_.-]?key|dsn)[a-z0-9_.-]{0,20})\\?["'\x60]?[ \t]*(?:=|:=|:|=>|\?=)[ \t]*\\?["'\x60]?([^\s"'\x60;,<>(){}\[\]\\]{4,200})"# },
];

/// Only compiled when [`DetectorConfig::generic_entropy`] is on.
const ENTROPY_SPEC: Spec = Spec {
    id: "high-entropy-token", label: "High-entropy token", kind: SECRET, prio: P_ENTROPY, conf: 0.6, kw: &[], v: V::HighEntropy,
    pat: r"(?:^|[^A-Za-z0-9+/_=-])([A-Za-z0-9+/_-]{24,256}={0,2})(?:[^A-Za-z0-9+/_=-]|$)",
};

struct SecretRule {
    spec: &'static Spec,
    re: BRegex,
    /// Run only in windows around keyword hits: bytes after the keyword (0 = whole text).
    win: usize,
}

/// Keyword-anchored rules whose match lies within a bounded distance of a keyword hit;
/// they run on windows around the hits instead of the whole text (gitleaks fragments).
fn window_of(id: &str) -> usize {
    match id {
        "aws-secret-key" => 128,
        "url-password" => 640,
        "connection-string" => 1024,
        "authorization-header" | "api-key-header" => 96,
        "curl-user-password" => 560,
        "mysql-password-flag" => 400,
        "bearer-token" => 48,
        "cli-secret-flag" => 300,
        "generic-secret" => 320,
        "telegram-bot-token" => 64,
        "mailchimp-api-key" => 16,
        "twilio-api-key" => 48,
        _ => 0,
    }
}

/// Bytes before a keyword hit that a windowed match may start at (key prefixes, schemes).
const WIN_BACK: usize = 64;

/// Merged `[start, end)` windows around `hits`, widened so slicing never cuts a token:
/// starts move back over word-ish bytes, ends move forward to whitespace or a quote.
fn windows(b: &[u8], hits: &[(usize, usize)], fwd: usize) -> Vec<(usize, usize)> {
    let mut out: Vec<(usize, usize)> = Vec::new();
    for &(hs, he) in hits {
        let mut ws = hs.saturating_sub(WIN_BACK);
        let floor = ws.saturating_sub(256);
        while ws > floor && (b[ws - 1].is_ascii_alphanumeric() || matches!(b[ws - 1], b'_' | b'.' | b'-' | b'+' | b':')) {
            ws -= 1;
        }
        let mut we = (he + fwd).min(b.len());
        let cap = (we + 4096).min(b.len());
        while we < cap && !b[we].is_ascii_whitespace() && !matches!(b[we], b'"' | b'\'' | b'`' | b'<' | b'>') {
            we += 1;
        }
        match out.last_mut() {
            Some(last) if ws <= last.1 => last.1 = last.1.max(we),
            _ => out.push((ws, we)),
        }
    }
    out
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Pii {
    Email,
    PhoneBd,
    PhoneIntl,
    PhoneNational,
    Card,
    Iban,
    Ipv4,
    Ipv6,
    Ssn,
    Nid,
    Passport,
    Dob,
}

struct PiiRule {
    which: Pii,
    re: BRegex,
    /// Context words, one of which must appear shortly before the value (empty = none
    /// needed). ASCII words must stand alone (`nid` does not match `unidentified`).
    ctx: &'static [&'static str],
}

/// How far (chars) before a value its context word may be.
const CTX_WINDOW_CHARS: usize = 40;
/// Bytes after a context word scanned for values: 40 chars of up to 4 bytes, plus the
/// longest value.
const CTX_WINDOW_BYTES: usize = CTX_WINDOW_CHARS * 4 + 64;

const CTX_SSN: &[&str] = &["ssn", "social security", "social-security", "ss#", "soc sec", "soc. sec"];
const CTX_NID: &[&str] = &[
    "nid", "national id", "national-id", "nationalid", "national identity", "voter id", "id card", "id no", "id number",
    "aadhaar", "cnic",
    "\u{99C}\u{9BE}\u{9A4}\u{9C0}\u{9DF} \u{9AA}\u{9B0}\u{9BF}\u{99A}\u{9DF}", // jatiyo porichoy
    "\u{98F}\u{9A8}\u{986}\u{987}\u{9A1}\u{9BF}",                              // NID in Bangla letters
    "\u{98F}\u{9A8} \u{986}\u{987} \u{9A1}\u{9BF}",
];
const CTX_PASSPORT: &[&str] = &["passport", "\u{9AA}\u{9BE}\u{9B8}\u{9AA}\u{9CB}\u{9B0}\u{9CD}\u{99F}"];
const CTX_DOB: &[&str] = &[
    "dob", "d.o.b", "date of birth", "date-of-birth", "birth date", "birthdate", "birthday", "born",
    "\u{99C}\u{9A8}\u{9CD}\u{9AE}", // jonmo
];
const CTX_PHONE: &[&str] = &[
    "phone", "mobile", "tel", "cell", "whatsapp", "call", "contact", "fax",
    "\u{9AB}\u{9CB}\u{9A8}",                         // phone
    "\u{9AE}\u{9CB}\u{9AC}\u{9BE}\u{987}\u{9B2}", // mobile
];

struct Compiled {
    secrets: Vec<SecretRule>,
    /// Secret rules with no keywords (always run).
    always: Vec<usize>,
    kw_ac: Option<AhoCorasick>,
    /// Keyword pattern index → secret rule indices.
    kw_rules: Vec<Vec<usize>>,
    pii: Vec<PiiRule>,
    ctx_ac: Option<AhoCorasick>,
    /// Context pattern index → PII rule indices.
    ctx_rules: Vec<Vec<usize>>,
    terms: Option<AhoCorasick>,
    placeholder: Regex,
    /// `data:` URIs and SRI integrity hashes: never findings.
    protect: BRegex,
}

/// A compiled detector. Build once and reuse: compiling the rule set costs a few ms.
pub struct Detector {
    cfg: DetectorConfig,
    inner: Compiled,
    /// `cfg.allowlist`, trimmed and lowercased.
    allow: Vec<String>,
}

fn bre(pattern: &str) -> BRegex {
    RegexBuilder::new(pattern)
        .unicode(false)
        .build()
        .expect("valid rule regex")
}

/// One candidate before overlap resolution.
struct Cand {
    prio: u8,
    f: Finding,
}

impl Detector {
    /// Compiles the rule set for `cfg`.
    pub fn new(cfg: &DetectorConfig) -> Self {
        let mut secrets = Vec::new();
        if cfg.secrets {
            for spec in SECRET_SPECS {
                secrets.push(SecretRule { spec, re: bre(spec.pat), win: window_of(spec.id) });
            }
            if cfg.generic_entropy {
                secrets.push(SecretRule { spec: &ENTROPY_SPEC, re: bre(ENTROPY_SPEC.pat), win: 0 });
            }
        }
        let mut always = Vec::new();
        let mut kw_patterns: Vec<&str> = Vec::new();
        let mut kw_rules: Vec<Vec<usize>> = Vec::new();
        for (i, r) in secrets.iter().enumerate() {
            if r.spec.kw.is_empty() {
                always.push(i);
            }
            for k in r.spec.kw {
                match kw_patterns.iter().position(|p| p == k) {
                    Some(p) => kw_rules[p].push(i),
                    None => {
                        kw_patterns.push(k);
                        kw_rules.push(vec![i]);
                    }
                }
            }
        }
        let kw_ac = if kw_patterns.is_empty() {
            None
        } else {
            AhoCorasickBuilder::new()
                .match_kind(MatchKind::Standard)
                .ascii_case_insensitive(true)
                .build(&kw_patterns)
                .ok()
        };

        let mut pii = Vec::new();
        if cfg.pii {
            let mut add = |which: Pii, pat: &str, ctx: &'static [&'static str]| {
                pii.push(PiiRule { which, re: bre(pat), ctx });
            };
            if cfg.emails {
                add(Pii::Email, r"[A-Za-z0-9._%+-]{1,64}@(?:[A-Za-z0-9](?:[A-Za-z0-9-]{0,61}[A-Za-z0-9])?\.){1,8}[A-Za-z]{2,24}", &[]);
            }
            if cfg.phones {
                add(Pii::PhoneBd, r"(?:\+?880[ -]?|0)1[3-9][0-9]{2}[ -]?[0-9]{6}", &[]);
                add(Pii::PhoneIntl, r"\+[1-9][0-9]{0,3}(?:[ .-]?(?:\([0-9]{1,4}\)|[0-9]{1,5})){1,6}", &[]);
                add(Pii::PhoneNational, r"\(?[0-9]{3}\)?[ .-]?[0-9]{3}[ .-]?[0-9]{4}", CTX_PHONE);
            }
            if cfg.cards {
                add(Pii::Card, r"[0-9][0-9 -]{11,35}[0-9]", &[]);
            }
            if cfg.ibans {
                add(Pii::Iban, r"[A-Z]{2}[0-9]{2}[ A-Z0-9]{11,42}", &[]);
            }
            if cfg.ips {
                add(Pii::Ipv4, r"[0-9]{1,3}\.[0-9]{1,3}\.[0-9]{1,3}\.[0-9]{1,3}", &[]);
                add(Pii::Ipv6, r"[0-9A-Fa-f]{0,4}(?::[0-9A-Fa-f]{0,4}){2,7}", &[]);
            }
            if cfg.national_ids {
                add(Pii::Ssn, r"[0-9]{3}-[0-9]{2}-[0-9]{4}", CTX_SSN);
                add(Pii::Nid, r"[0-9]{17}|[0-9]{13}|[0-9]{10}", CTX_NID);
                add(Pii::Passport, r"[A-Z0-9]{6,9}", CTX_PASSPORT);
                add(Pii::Dob, r"[0-9]{1,4}[./-][0-9]{1,2}[./-][0-9]{1,4}", CTX_DOB);
            }
        }
        let mut ctx_patterns: Vec<String> = Vec::new();
        let mut ctx_rules: Vec<Vec<usize>> = Vec::new();
        for (i, r) in pii.iter().enumerate() {
            for w in r.ctx {
                let variants = [w.to_string(), decompose_bengali(w)];
                for (n, v) in variants.into_iter().enumerate() {
                    if n == 1 && v == *w {
                        continue;
                    }
                    match ctx_patterns.iter().position(|p| *p == v) {
                        Some(p) => ctx_rules[p].push(i),
                        None => {
                            ctx_patterns.push(v);
                            ctx_rules.push(vec![i]);
                        }
                    }
                }
            }
        }
        let ctx_ac = if ctx_patterns.is_empty() {
            None
        } else {
            AhoCorasickBuilder::new()
                .match_kind(MatchKind::Standard)
                .ascii_case_insensitive(true)
                .build(&ctx_patterns)
                .ok()
        };

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
        let protect = bre(r"data:[A-Za-z]+/[A-Za-z0-9.+-]+;base64,[A-Za-z0-9+/=]+|\bsha(?:1|256|384|512)-[A-Za-z0-9+/]{20,}={0,2}");
        let allow = cfg
            .allowlist
            .iter()
            .map(|a| a.trim().to_ascii_lowercase())
            .filter(|a| !a.is_empty())
            .collect();
        Self {
            cfg: cfg.clone(),
            allow,
            inner: Compiled { secrets, always, kw_ac, kw_rules, pii, ctx_ac, ctx_rules, terms, placeholder, protect },
        }
    }

    /// All findings in `text` with confidence ≥ 0.5: non-overlapping, sorted by `start`,
    /// never overlapping an existing `{{KIND_N}}` placeholder.
    pub fn scan(&self, text: &str) -> Vec<Finding> {
        if text.is_empty() {
            return Vec::new();
        }
        let mut protected: Vec<(usize, usize)> = self
            .inner
            .placeholder
            .find_iter(text)
            .map(|m| (m.start(), m.end()))
            .collect();
        if text.contains("data:") || text.contains("sha") {
            protected.extend(self.inner.protect.find_iter(text.as_bytes()).map(|m| (m.start(), m.end())));
        }
        let mut cands: Vec<Cand> = Vec::new();
        self.scan_secrets(text, &mut cands);
        self.scan_pii(text, &mut cands);
        self.scan_terms(text, &mut cands);
        resolve(cands, protected)
    }

    pub fn config(&self) -> &DetectorConfig {
        &self.cfg
    }

    /// Ids of the rules this detector runs (secret rules, PII rules, `custom-term`).
    pub fn rule_ids(&self) -> Vec<&'static str> {
        let mut out: Vec<&'static str> = self.inner.secrets.iter().map(|r| r.spec.id).collect();
        for r in &self.inner.pii {
            out.push(pii_id(r.which));
        }
        if self.inner.terms.is_some() {
            out.push("custom-term");
        }
        out
    }

    // -- secrets ---------------------------------------------------------------------

    fn scan_secrets(&self, text: &str, out: &mut Vec<Cand>) {
        let rules = &self.inner.secrets;
        if rules.is_empty() {
            return;
        }
        let mut active = vec![false; rules.len()];
        let mut remaining = rules.len();
        for &i in &self.inner.always {
            active[i] = true;
            remaining -= 1;
        }
        let b = text.as_bytes();
        let any_windowed = rules.iter().any(|r| r.win > 0);
        let mut hits: Vec<Vec<(usize, usize)>> = vec![Vec::new(); rules.len()];
        if let Some(ac) = &self.inner.kw_ac {
            for m in ac.find_overlapping_iter(b) {
                for &i in &self.inner.kw_rules[m.pattern().as_usize()] {
                    if !active[i] {
                        active[i] = true;
                        remaining -= 1;
                    }
                    if rules[i].win > 0 {
                        hits[i].push((m.start(), m.end()));
                    }
                }
                if remaining == 0 && !any_windowed {
                    break;
                }
            }
        }
        for (i, r) in rules.iter().enumerate() {
            if !active[i] {
                continue;
            }
            let ranges = if r.win > 0 && !hits[i].is_empty() {
                hits[i].sort_unstable();
                windows(b, &hits[i], r.win)
            } else {
                vec![(0, b.len())]
            };
            for (ws, we) in ranges {
                self.run_secret_rule(text, r, ws, we, out);
            }
        }
    }

    fn run_secret_rule(&self, text: &str, r: &SecretRule, ws: usize, we: usize, out: &mut Vec<Cand>) {
        let b = text.as_bytes();
        let group = if r.spec.v == V::Assign { 2 } else { 1 };
        for caps in r.re.captures_iter(&b[ws..we]) {
            let Some(m) = caps.get(group) else { continue };
            let (s, mut e) = (ws + m.start(), ws + m.end());
            if !text.is_char_boundary(s) {
                continue;
            }
            while e > s && !text.is_char_boundary(e) {
                e -= 1;
            }
            if e <= s {
                continue;
            }
            let key = if group == 2 {
                caps.get(1).and_then(|k| std::str::from_utf8(k.as_bytes()).ok())
            } else {
                None
            };
            let whole = caps.get(0).map(|m| m.as_bytes()).unwrap_or(&[]);
            if let Some(c) = self.check_secret(text, r.spec, s, e, key, whole) {
                out.push(c);
            }
        }
    }

    fn check_secret(&self, text: &str, spec: &'static Spec, s: usize, mut e: usize, key: Option<&str>, whole: &[u8]) -> Option<Cand> {
        let mut kind = spec.kind;
        let mut label = spec.label;
        let mut hint: Option<String> = None;
        {
            let v = &text[s..e];
            if is_templated(v) || self.allowlisted(v) {
                return None;
            }
        }
        match spec.v {
            V::None => {}
            V::AlnumMix => {
                let v = &text[s..e];
                if !(v.bytes().any(|c| c.is_ascii_digit()) && v.bytes().any(|c| c.is_ascii_alphabetic())) {
                    return None;
                }
            }
            V::Entropy(min) => {
                if shannon_entropy(&text[s..e]) < min {
                    return None;
                }
            }
            V::AzureAd => {
                // gitleaks: preceded by start or one of \ ' " ` space > = : ( , ) and
                // followed by end or one of \ ' " ` space < ) ,
                let bytes = text.as_bytes();
                let ok_prev = s == 0 || matches!(bytes[s - 1], b'\\' | b'\'' | b'"' | b'`' | b' ' | b'\t' | b'\n' | b'\r' | b'>' | b'=' | b':' | b'(' | b',' | b')');
                let ok_next = e == bytes.len() || matches!(bytes[e], b'\\' | b'\'' | b'"' | b'`' | b' ' | b'\t' | b'\n' | b'\r' | b'<' | b')' | b',');
                if !ok_prev || !ok_next || shannon_entropy(&text[s..e]) < 3.0 {
                    return None;
                }
            }
            V::Jwt => {
                hint = Some(jwt_hint(&text[s..e])?);
            }
            V::PemTruncated => {
                let bytes = text.as_bytes();
                loop {
                    if e > s && bytes[e - 1].is_ascii_whitespace() {
                        e -= 1;
                    } else if e > s + 1 && bytes[e - 2] == b'\\' && matches!(bytes[e - 1], b'n' | b'r') {
                        e -= 2;
                    } else {
                        break;
                    }
                }
            }
            V::ConnString => {
                // Trim trailing sentence punctuation.
                let bytes = text.as_bytes();
                while e > s && matches!(bytes[e - 1], b'.' | b',' | b';' | b':' | b')' | b']' | b'}' | b'>' | b'!' | b'?') {
                    e -= 1;
                }
                let v = &text[s..e];
                let after_scheme = v.find("://").map(|p| p + 3)?;
                let rest = &v[after_scheme..];
                let colon = rest.find(':')?;
                let at = rest[colon..].find('@')? + colon;
                let pass = &rest[colon + 1..at];
                if weak_value(pass) || is_templated(pass) {
                    return None;
                }
                let host = host_of(&rest[at + 1..]);
                if self.url_host_allowlisted(host) {
                    return None;
                }
                let scheme = v[..after_scheme - 3].to_ascii_lowercase();
                hint = Some(format!("{} connection string with password", scheme_name(&scheme)));
            }
            V::UrlPassword => {
                let v = &text[s..e];
                if weak_value(v) {
                    return None;
                }
                let host = host_of(&text[(e + 1).min(text.len())..]);
                if self.url_host_allowlisted(host) {
                    return None;
                }
            }
            V::Weak => {
                let v = &text[s..e];
                if weak_value(v) || is_reference(v) {
                    return None;
                }
            }
            V::TokenLike => {
                let v = &text[s..e];
                if weak_value(v) || is_reference_prefix(v) || v.len() < 8 {
                    return None;
                }
                let has_digit = v.bytes().any(|c| c.is_ascii_digit());
                let mixed = v.bytes().any(|c| c.is_ascii_uppercase()) && v.bytes().any(|c| c.is_ascii_lowercase());
                if !(has_digit || mixed) || shannon_entropy(v) < 3.0 {
                    return None;
                }
            }
            V::Assign => {
                let key = key.unwrap_or("");
                let (k, l, h) = self.check_assign(text, key, s, e)?;
                kind = k;
                label = l;
                hint = h;
            }
            V::HighEntropy => {
                let v = &text[s..e];
                if !high_entropy_ok(v) {
                    return None;
                }
            }
        }
        let value = &text[s..e];
        if hint.is_none() {
            hint = secret_hint(spec.id, value, whole);
        }
        Some(Cand {
            prio: spec.prio,
            f: Finding {
                start: s,
                end: e,
                value: value.to_string(),
                kind: kind.to_string(),
                rule: spec.id.to_string(),
                label: label.to_string(),
                category: Category::Secret,
                hint,
                confidence: spec.conf,
            },
        })
    }

    /// Keyword-anchored assignment (`password = …`, `API_TOKEN: …`, `--token …`).
    fn check_assign(&self, text: &str, key: &str, s: usize, e: usize) -> Option<(&'static str, &'static str, Option<String>)> {
        let v = &text[s..e];
        if key_denied(key) {
            return None;
        }
        let l = v.to_ascii_lowercase();
        if weak_value_l(v, &l) || is_reference_prefix_l(&l) || is_dotted_ident(v) || is_type_name(v) || looks_like_path(v, &l) || looks_like_url(&l) {
            return None;
        }
        let bytes = text.as_bytes();
        let quoted = s > 0 && matches!(bytes[s - 1], b'"' | b'\'' | b'`');
        let next = bytes.get(e).copied();
        if !quoted && matches!(next, Some(b'(') | Some(b'[') | Some(b'{')) {
            return None; // a call or index expression, not a literal
        }
        let kl = key.to_ascii_lowercase();
        let is_pw = kl.contains("passw") || kl.contains("pwd") || kl.contains("passphrase");
        let key_hint = clean_key(key).map(|k| format!("value of {k}"));
        if is_pw {
            if v.chars().count() < 4 {
                return None;
            }
            let env_key = key.bytes().any(|c| c.is_ascii_uppercase()) && !key.bytes().any(|c| c.is_ascii_lowercase());
            if !quoted && !env_key {
                // Code or prose: a bare word is far more likely an identifier or a word.
                let has_digit = v.bytes().any(|c| c.is_ascii_digit());
                let has_sym = v.bytes().any(|c| !c.is_ascii_alphanumeric() && !matches!(c, b'_' | b'.' | b'-'));
                if !(has_digit || has_sym) {
                    return None;
                }
            }
            return Some((PASSWORD, "Password", key_hint));
        }
        let n = v.chars().count();
        if n < 8 {
            return None;
        }
        if v.bytes().all(|c| c.is_ascii_alphabetic() || matches!(c, b'_' | b'.' | b'-')) {
            return None; // identifier or word(s)
        }
        if v.bytes().all(|c| c.is_ascii_digit() || matches!(c, b'.' | b'-' | b':')) {
            return None; // number, version, date
        }
        let floor = if is_hexish(v) {
            let digits_only = v.bytes().all(|c| c.is_ascii_digit() || c == b'-');
            if digits_only {
                return None;
            }
            3.0
        } else {
            entropy_floor(n, self.cfg.min_entropy)
        };
        if shannon_entropy(v) < floor {
            return None;
        }
        Some((SECRET, "Secret value", key_hint))
    }

    // -- PII ---------------------------------------------------------------------------

    fn scan_pii(&self, text: &str, out: &mut Vec<Cand>) {
        let rules = &self.inner.pii;
        if rules.is_empty() {
            return;
        }
        let b = text.as_bytes();
        // Context-word rules only run just after a context word (value must follow it
        // within ~40 chars on the same line).
        let mut ctx_hits: Vec<Vec<(usize, usize)>> = vec![Vec::new(); rules.len()];
        if let Some(ac) = &self.inner.ctx_ac {
            for m in ac.find_overlapping_iter(b) {
                for &i in &self.inner.ctx_rules[m.pattern().as_usize()] {
                    ctx_hits[i].push((m.start(), m.end()));
                }
            }
        }
        for (i, r) in rules.iter().enumerate() {
            match r.which {
                Pii::Email if !text.contains('@') => continue,
                Pii::Card => {
                    self.scan_cards(text, r, out);
                    continue;
                }
                _ => {}
            }
            if r.ctx.is_empty() {
                for m in r.re.find_iter(b) {
                    if let Some(c) = self.check_pii(text, r, m.start(), m.end()) {
                        out.push(c);
                    }
                }
                continue;
            }
            if ctx_hits[i].is_empty() {
                continue;
            }
            ctx_hits[i].sort_unstable();
            let mut wins: Vec<(usize, usize)> = Vec::new();
            for &(hs, he) in &ctx_hits[i] {
                let we = (he + CTX_WINDOW_BYTES).min(b.len());
                match wins.last_mut() {
                    Some(last) if hs <= last.1 => last.1 = last.1.max(we),
                    _ => wins.push((hs, we)),
                }
            }
            for (ws, we) in wins {
                for m in r.re.find_iter(&b[ws..we]) {
                    if let Some(c) = self.check_pii(text, r, ws + m.start(), ws + m.end()) {
                        out.push(c);
                    }
                }
            }
        }
    }

    fn check_pii(&self, text: &str, r: &PiiRule, s: usize, e: usize) -> Option<Cand> {
        let b = text.as_bytes();
        let prev = if s > 0 { Some(b[s - 1]) } else { None };
        let next = b.get(e).copied();
        let alnum = |c: Option<u8>| c.is_some_and(|c| c.is_ascii_alphanumeric());
        let digit = |c: Option<u8>| c.is_some_and(|c| c.is_ascii_digit());
        if !r.ctx.is_empty() {
            // Cheap guards first: every context rule needs a standalone token.
            if alnum(prev) || alnum(next) {
                return None;
            }
            if r.which == Pii::Passport && !b[s..e].iter().any(|c| c.is_ascii_digit()) {
                return None;
            }
            if !has_context(text, s, r.ctx, CTX_WINDOW_CHARS) {
                return None;
            }
        }
        let v = &text[s..e];
        let (s, e, hint, conf) = match r.which {
            Pii::Email => {
                if prev.is_some_and(|c| c.is_ascii_alphanumeric() || b"._%+-@/".contains(&c)) {
                    return None;
                }
                if next.is_some_and(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_' || c == b'@')
                    || (next == Some(b'.') && alnum(b.get(e + 1).copied()))
                {
                    return None;
                }
                let (local, domain) = v.rsplit_once('@')?;
                if local.starts_with('.') || local.ends_with('.') || local.contains("..") {
                    return None;
                }
                let dl = domain.to_ascii_lowercase();
                let tld = dl.rsplit('.').next().unwrap_or("");
                if FILE_EXTS.contains(&tld) || matches!(tld, "test" | "invalid" | "example" | "localhost" | "local") {
                    return None;
                }
                let ll = local.to_ascii_lowercase();
                if matches!(ll.as_str(), "git" | "noreply" | "no-reply" | "donotreply" | "do-not-reply") {
                    return None;
                }
                if self.allowlisted(v) || self.domain_allowlisted(&dl) {
                    return None;
                }
                (s, e, Some(email_hint(&dl)), 0.95)
            }
            Pii::PhoneBd => {
                if alnum(prev) || prev == Some(b'+') || alnum(next) {
                    return None;
                }
                let digits: String = v.chars().filter(|c| c.is_ascii_digit()).collect();
                let local = digits.trim_start_matches("880");
                let local = local.strip_prefix('0').unwrap_or(local);
                (s, e, Some(bd_operator_hint(local)), 0.9)
            }
            Pii::PhoneIntl => {
                if alnum(prev) || matches!(prev, Some(b'+') | Some(b'=') | Some(b'.') | Some(b'/')) {
                    return None;
                }
                // The regex may end on a separator-less group glued to text.
                if alnum(next) || (next == Some(b'.') && digit(b.get(e + 1).copied())) {
                    return None;
                }
                let digits: String = v.chars().filter(|c| c.is_ascii_digit()).collect();
                let has_sep = v[1..].bytes().any(|c| !c.is_ascii_digit());
                let n = digits.len();
                if !(8..=15).contains(&n) || (!has_sep && n < 10) {
                    return None;
                }
                if v.matches('(').count() != v.matches(')').count() {
                    return None;
                }
                (s, e, Some(phone_country_hint(&digits)), 0.75)
            }
            Pii::PhoneNational => {
                if alnum(prev) || matches!(prev, Some(b'+') | Some(b'-') | Some(b'.')) || alnum(next) {
                    return None;
                }
                if next == Some(b'-') || (next == Some(b'.') && digit(b.get(e + 1).copied())) {
                    return None;
                }
                (s, e, Some("Phone number".to_string()), 0.65)
            }
            Pii::Iban => {
                if alnum(prev) {
                    return None;
                }
                let country = &v[..2];
                let len = iban_length(country)?;
                // Walk alphanumerics until the country's length; spaces only between groups of 4.
                let mut count = 0;
                let mut end = s;
                let mut prev_space = false;
                for (i, c) in v.char_indices() {
                    if c == ' ' {
                        if count % 4 != 0 || prev_space {
                            return None;
                        }
                        prev_space = true;
                        continue;
                    }
                    prev_space = false;
                    count += 1;
                    end = s + i + 1;
                    if count == len {
                        break;
                    }
                }
                if count != len || alnum(b.get(end).copied()) {
                    return None;
                }
                let val = &text[s..end];
                if !iban_valid(val) {
                    return None;
                }
                (s, end, Some(format!("IBAN ({})", country_name(country).unwrap_or(country))), 0.97)
            }
            Pii::Ipv4 => {
                if alnum(prev) || prev == Some(b'.') || alnum(next) || (next == Some(b'.') && digit(b.get(e + 1).copied())) {
                    return None;
                }
                if matches!(prev_word(b, s).as_str(), "version" | "ver" | "v" | "build" | "release" | "firmware" | "oid") {
                    return None;
                }
                let ip: std::net::Ipv4Addr = v.parse().ok()?;
                if !ipv4_public(ip) {
                    return None;
                }
                (s, e, Some("Public IPv4 address".to_string()), 0.7)
            }
            Pii::Ipv6 => {
                if prev.is_some_and(|c| c.is_ascii_alphanumeric() || c == b':' || c == b'.') || next.is_some_and(|c| c.is_ascii_alphanumeric() || c == b':') {
                    return None;
                }
                if v.matches(':').count() < 2 {
                    return None;
                }
                let ip: std::net::Ipv6Addr = v.parse().ok()?;
                if !ipv6_public(ip) {
                    return None;
                }
                (s, e, Some("Public IPv6 address".to_string()), 0.7)
            }
            Pii::Ssn => {
                if alnum(prev) || alnum(next) || prev == Some(b'-') || next == Some(b'-') {
                    return None;
                }
                let area: u32 = v[0..3].parse().ok()?;
                let group = &v[4..6];
                let serial = &v[7..11];
                if area == 0 || area == 666 || area >= 900 || group == "00" || serial == "0000" {
                    return None;
                }
                (s, e, Some("US Social Security number".to_string()), 0.85)
            }
            Pii::Nid => {
                if alnum(prev) || alnum(next) {
                    return None;
                }
                (s, e, Some(format!("National ID number ({} digits)", v.len())), 0.85)
            }
            Pii::Passport => {
                if alnum(prev) || alnum(next) || !v.bytes().any(|c| c.is_ascii_digit()) {
                    return None;
                }
                if !has_context(text, s, CTX_PASSPORT, 24) {
                    return None;
                }
                (s, e, Some("Passport number".to_string()), 0.75)
            }
            Pii::Dob => {
                if alnum(prev) || alnum(next) || matches!(prev, Some(b'.') | Some(b'/') | Some(b'-')) {
                    return None;
                }
                let parts: Vec<&str> = v.split(|c| c == '.' || c == '/' || c == '-').collect();
                let year = parts.iter().find(|p| p.len() == 4).and_then(|p| p.parse::<u32>().ok());
                if let Some(y) = year {
                    if !(1900..=2100).contains(&y) {
                        return None;
                    }
                }
                let hint = match year {
                    Some(y) => format!("Date of birth (year {y})"),
                    None => "Date of birth".to_string(),
                };
                (s, e, Some(hint), 0.8)
            }
            Pii::Card => unreachable!("cards are scanned separately"),
        };
        if self.allowlisted(&text[s..e]) {
            return None;
        }
        Some(Cand {
            prio: P_PII,
            f: Finding {
                start: s,
                end: e,
                value: text[s..e].to_string(),
                kind: pii_kind(r.which).to_string(),
                rule: pii_id(r.which).to_string(),
                label: pii_label(r.which).to_string(),
                category: Category::Pii,
                hint,
                confidence: conf,
            },
        })
    }

    /// Cards need their own loop: a rejected greedy candidate may contain a valid card
    /// (`4242…4242 5555…` → the first 16 digits), and the next card may start inside it.
    fn scan_cards(&self, text: &str, r: &PiiRule, out: &mut Vec<Cand>) {
        let b = text.as_bytes();
        let mut pos = 0;
        while pos < b.len() {
            let Some(m) = r.re.find_at(b, pos) else { break };
            let (s, e) = (m.start(), m.end());
            let prev = if s > 0 { Some(b[s - 1]) } else { None };
            let bad_prev = prev.is_some_and(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'/' | b'#' | b'$'))
                || (prev == Some(b'-') && s > 1 && b[s - 2].is_ascii_digit());
            // Group boundaries (byte offsets where a digit run ends).
            let mut ends = Vec::new();
            for i in s..e {
                if b[i].is_ascii_digit() && (i + 1 == e || !b[i + 1].is_ascii_digit()) {
                    ends.push(i + 1);
                }
            }
            let mut accepted = None;
            if !bad_prev {
                for &end in ends.iter().rev() {
                    let next = b.get(end).copied();
                    if next.is_some_and(|c| c.is_ascii_alphanumeric() || c == b'_') {
                        continue;
                    }
                    if next == Some(b'.') && b.get(end + 1).is_some_and(|c| c.is_ascii_digit()) {
                        continue;
                    }
                    if let Some(hint) = card_check(&text[s..end]) {
                        accepted = Some((end, hint));
                        break;
                    }
                }
            }
            match accepted {
                Some((end, hint)) => {
                    if !self.allowlisted(&text[s..end]) {
                        out.push(Cand {
                            prio: P_PII,
                            f: Finding {
                                start: s,
                                end,
                                value: text[s..end].to_string(),
                                kind: CARD.to_string(),
                                rule: "credit-card".to_string(),
                                label: "Payment card number".to_string(),
                                category: Category::Pii,
                                hint: Some(hint),
                                confidence: 0.95,
                            },
                        });
                    }
                    pos = end;
                }
                None => {
                    // Retry from the start of the next digit group, or past this run.
                    pos = match ends.first() {
                        Some(&first) if first < e => first,
                        _ => e,
                    };
                    if pos <= s {
                        pos = s + 1;
                    }
                }
            }
        }
    }

    // -- custom terms ------------------------------------------------------------------

    fn scan_terms(&self, text: &str, out: &mut Vec<Cand>) {
        let Some(ac) = &self.inner.terms else { return };
        for m in ac.find_iter(text) {
            let (s, e) = (m.start(), m.end());
            if !text.is_char_boundary(s) || !text.is_char_boundary(e) || !word_bounded(text, s, e) {
                continue;
            }
            out.push(Cand {
                prio: P_TERM,
                f: Finding {
                    start: s,
                    end: e,
                    value: text[s..e].to_string(),
                    kind: TERM.to_string(),
                    rule: "custom-term".into(),
                    label: "Custom term".into(),
                    category: Category::Custom,
                    hint: Some("Private name or term chosen by the user".into()),
                    confidence: 1.0,
                },
            });
        }
    }

    // -- allowlists --------------------------------------------------------------------

    /// Exact value (case-insensitive) or, for email-shaped values, an allowlisted domain.
    fn allowlisted(&self, value: &str) -> bool {
        if self.allow.is_empty() {
            return false;
        }
        let v = value.to_ascii_lowercase();
        let email = v.contains('@');
        self.allow.iter().any(|a| v == *a || (email && suffix_after(&v, a, b"@.")))
    }

    fn domain_allowlisted(&self, domain: &str) -> bool {
        let d = domain.to_ascii_lowercase();
        self.allow.iter().any(|a| !a.contains('@') && (d == *a || suffix_after(&d, a, b".")))
    }

    /// URL hosts: only dotted allowlist entries count (`example.com`), so a password in
    /// `postgres://u:p@localhost` is still masked.
    fn url_host_allowlisted(&self, host: &str) -> bool {
        let h = host.to_ascii_lowercase();
        self.allow.iter().any(|a| a.contains('.') && !a.contains('@') && (h == *a || suffix_after(&h, a, b".")))
    }
}

/// `v` ends with `suffix` and the byte before it is one of `seps`.
fn suffix_after(v: &str, suffix: &str, seps: &[u8]) -> bool {
    v.len() > suffix.len() && v.ends_with(suffix) && seps.contains(&v.as_bytes()[v.len() - suffix.len() - 1])
}

/// Overlap resolution: strongest priority first, then longest, then most confident.
fn resolve(mut cands: Vec<Cand>, protected: Vec<(usize, usize)>) -> Vec<Finding> {
    cands.sort_by(|a, b| {
        a.prio
            .cmp(&b.prio)
            .then((b.f.end - b.f.start).cmp(&(a.f.end - a.f.start)))
            .then(b.f.confidence.partial_cmp(&a.f.confidence).unwrap_or(std::cmp::Ordering::Equal))
            .then(a.f.start.cmp(&b.f.start))
    });
    // Taken intervals (non-overlapping among findings), keyed by start.
    let mut taken: BTreeMap<usize, usize> = BTreeMap::new();
    // Protected spans merged into disjoint sorted intervals.
    let mut prot = protected;
    prot.sort();
    let mut merged: Vec<(usize, usize)> = Vec::with_capacity(prot.len());
    for (s, e) in prot {
        match merged.last_mut() {
            Some(last) if s <= last.1 => last.1 = last.1.max(e),
            _ => merged.push((s, e)),
        }
    }
    let overlaps_prot = |s: usize, e: usize| -> bool {
        let idx = merged.partition_point(|&(ps, _)| ps < e);
        idx > 0 && merged[idx - 1].1 > s
    };
    let mut out = Vec::new();
    for c in cands {
        let f = c.f;
        if f.confidence < 0.5 || f.start >= f.end {
            continue;
        }
        if overlaps_prot(f.start, f.end) {
            continue;
        }
        if let Some((_, &pe)) = taken.range(..f.end).next_back() {
            if pe > f.start {
                continue;
            }
        }
        taken.insert(f.start, f.end);
        out.push(f);
    }
    out.sort_by_key(|f| f.start);
    out
}

// ---------------------------------------------------------------------------------------
// Filters
// ---------------------------------------------------------------------------------------

fn word_bounded(text: &str, start: usize, end: usize) -> bool {
    let first = text[start..end].chars().next();
    let last = text[start..end].chars().next_back();
    let before = text[..start].chars().next_back();
    let after = text[end..].chars().next();
    let ok_start = !first.is_some_and(|c| c.is_alphanumeric()) || !before.is_some_and(|c| c.is_alphanumeric() || c == '_');
    let ok_end = !last.is_some_and(|c| c.is_alphanumeric()) || !after.is_some_and(|c| c.is_alphanumeric() || c == '_');
    ok_start && ok_end
}

/// Template-looking values that are never real secrets (applies to every rule).
fn is_templated(v: &str) -> bool {
    is_templated_l(v, &v.to_ascii_lowercase())
}

/// [`is_templated`] with the ASCII-lowercased value precomputed.
fn is_templated_l(v: &str, l: &str) -> bool {
    let b = v.as_bytes();
    v.starts_with('$')
        || (v.starts_with('%') && v.ends_with('%') && v.len() > 2)
        || v.contains("{{")
        || v.contains("${")
        || (v.starts_with('<') && v.ends_with('>'))
        || l.contains("your_")
        || l.contains("your-")
        || l.contains("yourapikey")
        || l.contains("example")
        || l.contains("placeholder")
        || l.contains("changeme")
        || l.contains("change_me")
        || l.contains("redacted")
        || l.contains("xxxx")
        || l.contains("****")
        || l.contains("....")
        || l.contains('…')
        || matches!(l, "true" | "false" | "null" | "none" | "nil" | "undefined")
        || (b.len() >= 4 && b.iter().all(|&c| c == b[0]))
}

/// Placeholder-ish or reserved words for generic rules.
fn weak_value(v: &str) -> bool {
    weak_value_l(v, &v.to_ascii_lowercase())
}

fn weak_value_l(v: &str, l: &str) -> bool {
    if is_templated_l(v, l) {
        return true;
    }
    const PREFIXES: &[&str] = &[
        "your", "my_", "my-", "the_", "the-", "sample", "dummy", "fake_", "fake-", "test_", "test-", "insert", "replace",
        "enter_", "enter-", "put_", "put-", "some_", "some-", "default_", "demo_", "demo-", "xxx",
    ];
    const WORDS: &[&str] = &[
        "password", "passwd", "pass", "pwd", "secret", "token", "apikey", "api_key", "api-key", "key", "value", "string",
        "required", "optional", "hidden", "masked", "empty", "todo", "tbd", "fixme", "none", "null", "nil", "undefined",
        "secret_value", "secretvalue", "mysecret", "mypassword", "password123", "test", "testing", "foo", "bar", "baz",
        "foobar", "user", "username",
    ];
    PREFIXES.iter().any(|p| l.starts_with(p))
        || WORDS.contains(&l)
        || l.ends_with("_here")
        || l.ends_with("-here")
        || (l.ends_with("here") && l.contains("key"))
}

/// References to a secret held elsewhere (`process.env.X`, `os.getenv`, `self.password`).
fn is_reference(v: &str) -> bool {
    is_reference_prefix(v) || is_dotted_ident(v)
}

fn is_reference_prefix(v: &str) -> bool {
    is_reference_prefix_l(&v.to_ascii_lowercase())
}

fn is_reference_prefix_l(l: &str) -> bool {
    const PREFIXES: &[&str] = &[
        "process.env", "import.meta.env", "os.environ", "os.getenv", "getenv", "env.", "env(", "env[", "config.", "settings.",
        "secrets.", "vars.", "self.", "this.", "deno.env", "system.getenv", "environment.", "context.", "ctx.", "request.",
        "req.", "args.", "opts.", "options.", "params.", "props.",
    ];
    PREFIXES.iter().any(|p| l.starts_with(p))
}

/// The ASCII word just before `pos`, skipping spaces and `:`/`=`, lowercased.
fn prev_word(b: &[u8], pos: usize) -> String {
    let mut i = pos;
    while i > 0 && matches!(b[i - 1], b' ' | b'\t' | b':' | b'=') {
        i -= 1;
    }
    let end = i;
    while i > 0 && b[i - 1].is_ascii_alphabetic() {
        i -= 1;
    }
    String::from_utf8_lossy(&b[i..end]).to_ascii_lowercase()
}

/// `a.b`, `foo.bar_baz.qux` — member access, not a secret.
fn is_dotted_ident(v: &str) -> bool {
    let mut parts = 0;
    for p in v.split('.') {
        let b = p.as_bytes();
        if b.is_empty() || !(b[0].is_ascii_alphabetic() || b[0] == b'_' || b[0] == b'$') {
            return false;
        }
        if !b.iter().all(|c| c.is_ascii_alphanumeric() || *c == b'_' || *c == b'$') {
            return false;
        }
        parts += 1;
    }
    parts >= 2
}

fn is_type_name(v: &str) -> bool {
    // Rust references: `&str`, `&mut String`, `&'a [u8]`, `&dyn Trait`.
    if let Some(rest) = v.strip_prefix('&') {
        let rest = rest.trim_start_matches("mut");
        return rest.is_empty() || is_type_name(rest) || rest.starts_with('[') || rest.starts_with('\'') || rest.starts_with("dyn");
    }
    matches!(
        v,
        "str" | "string" | "String" | "bool" | "boolean" | "Boolean" | "int" | "integer" | "Integer" | "number" | "Number"
            | "float" | "double" | "any" | "Any" | "None" | "Optional" | "object" | "Object" | "bytes" | "SecretStr"
            | "Secret" | "SecretString" | "char" | "unknown" | "void" | "Text" | "text" | "varchar" | "VARCHAR" | "u8"
            | "Vec" | "Zeroizing" | "SecretBox" | "Password" | "Token"
    ) || v.starts_with("Optional[")
        || v.starts_with("Option<")
}

fn looks_like_path(v: &str, l: &str) -> bool {
    v.starts_with('/')
        || v.starts_with("./")
        || v.starts_with("../")
        || v.starts_with("~/")
        || (v.len() > 2 && v.as_bytes()[1] == b':' && v.as_bytes()[0].is_ascii_alphabetic())
        || [".pem", ".key", ".json", ".txt", ".p12", ".pfx", ".crt", ".cer", ".env", ".yaml", ".yml", ".toml", ".ini", ".conf"]
            .iter()
            .any(|x| l.ends_with(x))
}

fn looks_like_url(l: &str) -> bool {
    (l.starts_with("http://") || l.starts_with("https://") || l.starts_with("file://")) && !l.contains('@')
}

/// `0-9a-f` with optional dashes (hashes, UUIDs).
fn is_hexish(v: &str) -> bool {
    v.bytes().all(|c| c.is_ascii_hexdigit() || c == b'-') && v.bytes().filter(|c| c.is_ascii_hexdigit()).count() >= 16
}

/// Key names that hold non-secrets (`max_tokens`, `token_type`, `public_key`, `secret_name`…).
fn key_denied(key: &str) -> bool {
    // camelCase → snake_case words.
    let mut s = String::with_capacity(key.len() + 8);
    let mut prev_lower = false;
    for c in key.chars() {
        if c.is_ascii_uppercase() && prev_lower {
            s.push('_');
        }
        prev_lower = c.is_ascii_lowercase() || c.is_ascii_digit();
        s.push(c.to_ascii_lowercase());
    }
    s.split(|c: char| !c.is_ascii_alphanumeric()).any(denied_word)
}

fn denied_word(w: &str) -> bool {
    matches!(
        w,
        "public"
            | "pub"
            | "csrf"
            | "xsrf"
            | "integrity"
            | "checksum"
            | "digest"
            | "etag"
            | "hash"
            | "hashed"
            | "sha"
            | "sha1"
            | "sha256"
            | "sha512"
            | "md5"
            | "nonce"
            | "commit"
            | "revision"
            | "resolved"
            | "url"
            | "uri"
            | "endpoint"
            | "path"
            | "file"
            | "filename"
            | "dir"
            | "directory"
            | "name"
            | "id"
            | "ids"
            | "length"
            | "len"
            | "size"
            | "count"
            | "limit"
            | "min"
            | "max"
            | "type"
            | "types"
            | "policy"
            | "field"
            | "label"
            | "placeholder"
            | "input"
            | "prompt"
            | "hint"
            | "pattern"
            | "regex"
            | "rule"
            | "rules"
            | "reset"
            | "expiry"
            | "expires"
            | "expiration"
            | "ttl"
            | "usage"
            | "budget"
            | "header"
            | "param"
            | "version"
            | "format"
            | "algorithm"
            | "alg"
            | "strength"
            | "enabled"
            | "required"
            | "provider"
            | "location"
            | "ref"
            | "arn"
            | "tokenizer"
            | "keyboard"
            | "keycode"
            | "keypress"
            | "keystroke"
            | "keyword"
            | "keywords"
            | "kind"
            | "mode"
            | "style"
            | "prefix"
            | "suffix"
            | "less"
            | "manager"
            | "store"
            | "storage"
            | "env"
            | "var"
            | "source"
            | "server"
            | "address"
            | "host"
            | "port"
            | "user"
            | "username"
            | "login"
            | "email"
            | "visible"
            | "show"
            | "toggle"
            | "icon"
            | "button"
            | "error"
            | "errors"
            | "message"
            | "text"
            | "title"
            | "description"
            | "help"
            | "validation"
            | "valid"
            | "invalid"
            | "confirm"
            | "match"
            | "matches"
            | "changed"
            | "change"
            | "forgot"
            | "new"
            | "old"
            | "current"
    )
}

/// A short, value-free description of the key (`DB_PASSWORD`), or `None`.
fn clean_key(key: &str) -> Option<String> {
    let k = key.trim_matches(|c: char| !c.is_ascii_alphanumeric());
    if k.is_empty() || k.len() > 48 || !k.bytes().all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-' | b'.')) {
        return None;
    }
    Some(k.to_string())
}

/// Length-adjusted entropy floor for keyword-anchored values (detection report §4):
/// `min(4.8, 0.8·log2(n) + 0.3) − 0.6`, capped at `cfg_min`.
fn entropy_floor(n: usize, cfg_min: f32) -> f32 {
    let t = (0.8 * (n.max(1) as f32).log2() + 0.3).min(4.8) - 0.6;
    t.min(cfg_min)
}

fn high_entropy_ok(v: &str) -> bool {
    let n = v.len();
    let has_digit = v.bytes().any(|c| c.is_ascii_digit());
    let has_upper = v.bytes().any(|c| c.is_ascii_uppercase());
    let has_lower = v.bytes().any(|c| c.is_ascii_lowercase());
    if !(has_digit && has_upper && has_lower) || is_hexish(v) || v.starts_with('/') || v.contains("//") {
        return false;
    }
    let l = v.to_ascii_lowercase();
    if l.starts_with("sha1-") || l.starts_with("sha256-") || l.starts_with("sha384-") || l.starts_with("sha512-") {
        return false;
    }
    let t = (0.8 * (n as f32).log2() + 0.3).min(4.8);
    shannon_entropy(v) >= t
}

/// The host part of `host[:port][/path]…`.
fn host_of(s: &str) -> &str {
    let end = s.find(|c: char| matches!(c, '/' | ':' | '?' | '#' | ',' | ' ' | '"' | '\'' | ')')).unwrap_or(s.len());
    &s[..end]
}

/// Context word present in the `window` chars before `start` on the same line.
fn has_context(text: &str, start: usize, words: &[&str], window: usize) -> bool {
    let lo = text[..start].char_indices().rev().take(window).last().map_or(start, |(i, _)| i);
    let mut w = &text[lo..start];
    if let Some(p) = w.rfind('\n') {
        w = &w[p + 1..];
    }
    let l = normalize_bengali(&w.to_lowercase());
    words.iter().any(|k| contains_word(&l, k))
}

/// Composes the Bengali nukta letters and two-part vowel signs that keyboards emit
/// decomposed, so context words match either form.
fn normalize_bengali(s: &str) -> String {
    if !s.bytes().any(|b| b == 0xE0) {
        return s.to_string();
    }
    s.replace("\u{9AF}\u{9BC}", "\u{9DF}")
        .replace("\u{9A1}\u{9BC}", "\u{9DC}")
        .replace("\u{9A2}\u{9BC}", "\u{9DD}")
        .replace("\u{9C7}\u{9BE}", "\u{9CB}")
        .replace("\u{9C7}\u{9D7}", "\u{9CC}")
}

/// The decomposed spelling of a Bengali word (inverse of [`normalize_bengali`]).
fn decompose_bengali(s: &str) -> String {
    s.replace('\u{9DF}', "\u{9AF}\u{9BC}")
        .replace('\u{9DC}', "\u{9A1}\u{9BC}")
        .replace('\u{9DD}', "\u{9A2}\u{9BC}")
        .replace('\u{9CB}', "\u{9C7}\u{9BE}")
        .replace('\u{9CC}', "\u{9C7}\u{9D7}")
}

/// `hay` contains `word`; ASCII-alphanumeric words must not touch other ASCII letters.
fn contains_word(hay: &str, word: &str) -> bool {
    let ascii = word.is_ascii();
    let hb = hay.as_bytes();
    let mut from = 0;
    while let Some(p) = hay[from..].find(word) {
        let s = from + p;
        let e = s + word.len();
        if !ascii {
            return true;
        }
        let before_ok = s == 0 || !hb[s - 1].is_ascii_alphanumeric();
        let after_ok = e >= hb.len() || !hb[e].is_ascii_alphanumeric() || !word.as_bytes()[word.len() - 1].is_ascii_alphanumeric();
        if before_ok && after_ok {
            return true;
        }
        from = s + 1;
        while from < hay.len() && !hay.is_char_boundary(from) {
            from += 1;
        }
    }
    false
}

const FILE_EXTS: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "svg", "webp", "ico", "bmp", "avif", "js", "mjs", "cjs", "ts", "tsx", "jsx", "css", "scss",
    "less", "json", "md", "txt", "yml", "yaml", "toml", "lock", "rs", "py", "go", "java", "kt", "rb", "php", "html", "htm",
    "xml", "map", "vue", "svelte", "wasm", "zip", "gz", "tar", "pdf", "mp3", "mp4", "woff", "woff2", "ttf", "otf",
];

// ---------------------------------------------------------------------------------------
// PII helpers
// ---------------------------------------------------------------------------------------

fn pii_id(p: Pii) -> &'static str {
    match p {
        Pii::Email => "email",
        Pii::PhoneBd => "phone-bd",
        Pii::PhoneIntl => "phone-intl",
        Pii::PhoneNational => "phone-national",
        Pii::Card => "credit-card",
        Pii::Iban => "iban",
        Pii::Ipv4 => "ipv4",
        Pii::Ipv6 => "ipv6",
        Pii::Ssn => "us-ssn",
        Pii::Nid => "national-id",
        Pii::Passport => "passport",
        Pii::Dob => "date-of-birth",
    }
}

fn pii_kind(p: Pii) -> &'static str {
    match p {
        Pii::Email => EMAIL,
        Pii::PhoneBd | Pii::PhoneIntl | Pii::PhoneNational => PHONE,
        Pii::Card => CARD,
        Pii::Iban => IBAN,
        Pii::Ipv4 | Pii::Ipv6 => IP,
        Pii::Ssn | Pii::Nid | Pii::Passport => NID,
        Pii::Dob => DOB,
    }
}

fn pii_label(p: Pii) -> &'static str {
    match p {
        Pii::Email => "Email address",
        Pii::PhoneBd => "Bangladesh mobile number",
        Pii::PhoneIntl | Pii::PhoneNational => "Phone number",
        Pii::Card => "Payment card number",
        Pii::Iban => "IBAN",
        Pii::Ipv4 | Pii::Ipv6 => "IP address",
        Pii::Ssn => "US Social Security number",
        Pii::Nid => "National ID number",
        Pii::Passport => "Passport number",
        Pii::Dob => "Date of birth",
    }
}

fn email_hint(domain: &str) -> String {
    let provider = match domain {
        "gmail.com" | "googlemail.com" => Some("Gmail"),
        "outlook.com" | "hotmail.com" | "live.com" | "msn.com" => Some("Outlook/Hotmail"),
        "icloud.com" | "me.com" | "mac.com" => Some("iCloud"),
        "proton.me" | "protonmail.com" | "pm.me" => Some("Proton Mail"),
        "aol.com" => Some("AOL"),
        "gmx.com" | "gmx.de" | "gmx.net" => Some("GMX"),
        "zoho.com" => Some("Zoho Mail"),
        "yandex.com" | "yandex.ru" => Some("Yandex"),
        d if d.starts_with("yahoo.") || d.starts_with("ymail.") => Some("Yahoo Mail"),
        _ => None,
    };
    if let Some(p) = provider {
        return format!("{p} address");
    }
    let parts: Vec<&str> = domain.split('.').collect();
    let academic = parts.iter().any(|p| *p == "edu" || *p == "ac") || domain.ends_with(".edu");
    if academic {
        return "Academic (university) email address".into();
    }
    if parts.iter().any(|p| *p == "gov" || *p == "gob" || *p == "gouv") {
        return "Government email address".into();
    }
    "Work or custom-domain email address".into()
}

/// Mobile operator for the 10-digit Bangladeshi national number `local` (starts with `1`).
pub(crate) fn bd_operator(local: &str) -> Option<&'static str> {
    match local.as_bytes().get(1) {
        Some(b'3') | Some(b'7') => Some("Grameenphone"),
        Some(b'4') | Some(b'9') => Some("Banglalink"),
        Some(b'5') => Some("Teletalk"),
        Some(b'6') => Some("Airtel"),
        Some(b'8') => Some("Robi"),
        _ => None,
    }
}

fn bd_operator_hint(local: &str) -> String {
    match bd_operator(local) {
        Some(o) => format!("Bangladesh mobile number ({o})"),
        None => "Bangladesh mobile number".into(),
    }
}

pub(crate) const CALLING_CODES: &[(&str, &str)] = &[
    ("880", "Bangladesh"), ("852", "Hong Kong"), ("886", "Taiwan"), ("966", "Saudi Arabia"), ("971", "United Arab Emirates"),
    ("974", "Qatar"), ("977", "Nepal"), ("965", "Kuwait"), ("968", "Oman"), ("973", "Bahrain"), ("960", "Maldives"),
    ("975", "Bhutan"), ("234", "Nigeria"), ("254", "Kenya"), ("353", "Ireland"), ("351", "Portugal"), ("358", "Finland"),
    ("380", "Ukraine"), ("20", "Egypt"), ("27", "South Africa"), ("30", "Greece"), ("31", "Netherlands"), ("32", "Belgium"),
    ("33", "France"), ("34", "Spain"), ("39", "Italy"), ("41", "Switzerland"), ("43", "Austria"), ("44", "United Kingdom"),
    ("45", "Denmark"), ("46", "Sweden"), ("47", "Norway"), ("48", "Poland"), ("49", "Germany"), ("52", "Mexico"),
    ("55", "Brazil"), ("60", "Malaysia"), ("61", "Australia"), ("62", "Indonesia"), ("63", "Philippines"),
    ("64", "New Zealand"), ("65", "Singapore"), ("66", "Thailand"), ("81", "Japan"), ("82", "South Korea"),
    ("84", "Vietnam"), ("86", "China"), ("90", "Turkey"), ("91", "India"), ("92", "Pakistan"), ("93", "Afghanistan"),
    ("94", "Sri Lanka"), ("95", "Myanmar"), ("98", "Iran"), ("1", "US/Canada"), ("7", "Russia/Kazakhstan"),
];

fn phone_country_hint(digits: &str) -> String {
    for (code, name) in CALLING_CODES {
        if digits.starts_with(code) {
            return format!("Phone number ({name}, +{code})");
        }
    }
    "International phone number".into()
}

/// Brand + last 4 if `s` (digits with optional consistent separators) is a valid card.
fn card_check(s: &str) -> Option<String> {
    let groups: Vec<&str> = s.split(|c| c == ' ' || c == '-').collect();
    let seps: Vec<char> = s.chars().filter(|c| *c == ' ' || *c == '-').collect();
    if seps.windows(2).any(|w| w[0] != w[1]) || groups.iter().any(|g| g.is_empty()) {
        return None;
    }
    let digits: String = groups.concat();
    let n = digits.len();
    if !(13..=19).contains(&n) {
        return None;
    }
    if groups.len() > 1 {
        let lens: Vec<usize> = groups.iter().map(|g| g.len()).collect();
        let fours = lens[..lens.len() - 1].iter().all(|&l| l == 4) && (1..=4).contains(lens.last().unwrap());
        let amex = lens == [4, 6, 5] || lens == [4, 6, 4];
        if !(fours || amex) {
            return None;
        }
    }
    let d = digits.as_bytes();
    if d.iter().all(|&c| c == d[0]) || !luhn_valid(&digits) {
        return None;
    }
    let p2: u32 = digits[..2].parse().ok()?;
    let p3: u32 = digits[..3].parse().ok()?;
    let p4: u32 = digits[..4].parse().ok()?;
    let brand = match () {
        _ if d[0] == b'4' && matches!(n, 13 | 16 | 19) => "Visa",
        _ if ((51..=55).contains(&p2) || (2221..=2720).contains(&p4)) && n == 16 => "Mastercard",
        _ if (p2 == 34 || p2 == 37) && n == 15 => "American Express",
        _ if (p4 == 6011 || (644..=649).contains(&p3) || p2 == 65) && (16..=19).contains(&n) => "Discover",
        _ if (3528..=3589).contains(&p4) && (16..=19).contains(&n) => "JCB",
        _ if p2 == 62 && (16..=19).contains(&n) => "UnionPay",
        _ if (p2 == 36 || p2 == 38 || p2 == 39 || (300..=305).contains(&p3)) && (14..=19).contains(&n) => "Diners Club",
        _ => return None,
    };
    Some(format!("{brand} card ending {}", &digits[n - 4..]))
}

const IBAN_LENGTHS: &[(&str, usize)] = &[
    ("AD", 24), ("AE", 23), ("AL", 28), ("AT", 20), ("AZ", 28), ("BA", 20), ("BE", 16), ("BG", 22), ("BH", 22), ("BR", 29),
    ("BY", 28), ("CH", 21), ("CR", 22), ("CY", 28), ("CZ", 24), ("DE", 22), ("DK", 18), ("DO", 28), ("EE", 20), ("EG", 29),
    ("ES", 24), ("FI", 18), ("FO", 18), ("FR", 27), ("GB", 22), ("GE", 22), ("GI", 23), ("GL", 18), ("GR", 27), ("GT", 28),
    ("HR", 21), ("HU", 28), ("IE", 22), ("IL", 23), ("IQ", 23), ("IS", 26), ("IT", 27), ("JO", 30), ("KW", 30), ("KZ", 20),
    ("LB", 28), ("LC", 32), ("LI", 21), ("LT", 20), ("LU", 20), ("LV", 21), ("MC", 27), ("MD", 24), ("ME", 22), ("MK", 19),
    ("MR", 27), ("MT", 31), ("MU", 30), ("NL", 18), ("NO", 15), ("PK", 24), ("PL", 28), ("PS", 29), ("PT", 25), ("QA", 29),
    ("RO", 24), ("RS", 22), ("SA", 24), ("SC", 31), ("SE", 24), ("SI", 19), ("SK", 24), ("SM", 27), ("ST", 25), ("SV", 28),
    ("TL", 23), ("TN", 24), ("TR", 26), ("UA", 29), ("VA", 22), ("VG", 24), ("XK", 20),
];

pub(crate) fn iban_length(country: &str) -> Option<usize> {
    IBAN_LENGTHS.iter().find(|(c, _)| *c == country).map(|(_, l)| *l)
}

pub(crate) fn country_name(code: &str) -> Option<&'static str> {
    Some(match code {
        "DE" => "Germany", "GB" => "United Kingdom", "FR" => "France", "ES" => "Spain", "IT" => "Italy",
        "NL" => "Netherlands", "BE" => "Belgium", "CH" => "Switzerland", "AT" => "Austria", "IE" => "Ireland",
        "PT" => "Portugal", "PL" => "Poland", "SE" => "Sweden", "NO" => "Norway", "DK" => "Denmark", "FI" => "Finland",
        "AE" => "United Arab Emirates", "SA" => "Saudi Arabia", "TR" => "Turkey", "PK" => "Pakistan", "QA" => "Qatar",
        "KW" => "Kuwait", "BH" => "Bahrain", "EG" => "Egypt", "GR" => "Greece", "CZ" => "Czechia", "HU" => "Hungary",
        "RO" => "Romania", "LU" => "Luxembourg", "MT" => "Malta", "CY" => "Cyprus", "BR" => "Brazil", "UA" => "Ukraine",
        "IL" => "Israel", "JO" => "Jordan", "LB" => "Lebanon", "HR" => "Croatia", "SI" => "Slovenia", "SK" => "Slovakia",
        "LT" => "Lithuania", "LV" => "Latvia", "EE" => "Estonia", "BG" => "Bulgaria", "RS" => "Serbia", "IS" => "Iceland",
        _ => return None,
    })
}

pub(crate) fn ipv4_public(ip: std::net::Ipv4Addr) -> bool {
    let o = ip.octets();
    !(ip.is_private()
        || ip.is_loopback()
        || ip.is_link_local()
        || ip.is_broadcast()
        || ip.is_multicast()
        || ip.is_unspecified()
        || ip.is_documentation()
        || o[0] == 0
        || o[0] >= 240
        || (o[0] == 100 && (64..128).contains(&o[1]))
        || (o[0] == 198 && (o[1] == 18 || o[1] == 19))
        || (o[0] == 192 && o[1] == 0 && o[2] == 0))
}

pub(crate) fn ipv6_public(ip: std::net::Ipv6Addr) -> bool {
    if let Some(v4) = ip.to_ipv4_mapped() {
        return ipv4_public(v4);
    }
    let s = ip.segments();
    // Global unicast is 2000::/3; exclude documentation 2001:db8::/32.
    (0x2000..0x4000).contains(&s[0]) && !(s[0] == 0x2001 && s[1] == 0x0db8)
}

// ---------------------------------------------------------------------------------------
// Secret hints
// ---------------------------------------------------------------------------------------

pub(crate) fn scheme_name(scheme: &str) -> &'static str {
    let s = scheme.trim_start_matches("jdbc:");
    match s {
        "postgres" | "postgresql" => "PostgreSQL",
        "mysql" => "MySQL",
        "mariadb" => "MariaDB",
        "mongodb" | "mongodb+srv" => "MongoDB",
        "redis" | "rediss" => "Redis",
        "amqp" | "amqps" => "AMQP (RabbitMQ)",
        "mssql" | "sqlserver" => "SQL Server",
        "clickhouse" => "ClickHouse",
        "snowflake" => "Snowflake",
        _ => "Database",
    }
}

fn jwt_hint(v: &str) -> Option<String> {
    use base64::Engine;
    let header = v.split('.').next()?;
    let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(header.trim_end_matches('='))
        .ok()?;
    let json: serde_json::Value = serde_json::from_slice(&raw).ok()?;
    let obj = json.as_object()?;
    if !obj.contains_key("alg") && !obj.contains_key("typ") {
        return None;
    }
    Some(match obj.get("alg").and_then(|a| a.as_str()) {
        Some(alg) if alg.len() <= 12 && alg.bytes().all(|c| c.is_ascii_alphanumeric()) => format!("JSON Web Token ({alg})"),
        _ => "JSON Web Token".to_string(),
    })
}

fn secret_hint(rule: &str, value: &str, whole: &[u8]) -> Option<String> {
    let h = match rule {
        "private-key" | "private-key-truncated" => {
            let head = value.get(..value.len().min(64)).unwrap_or("").to_ascii_uppercase();
            if head.contains("OPENSSH") {
                "OpenSSH private key"
            } else if head.contains("RSA") {
                "RSA private key"
            } else if head.contains("EC PRIVATE") {
                "EC private key"
            } else if head.contains("DSA") {
                "DSA private key"
            } else if head.contains("PGP") {
                "PGP private key block"
            } else if head.contains("ENCRYPTED") {
                "Encrypted private key (PEM)"
            } else {
                "Private key (PEM)"
            }
        }
        "anthropic-api-key" => {
            if value.starts_with("sk-ant-oat") {
                "Anthropic OAuth token (Claude Code login)"
            } else if value.starts_with("sk-ant-admin") {
                "Anthropic admin API key"
            } else {
                "Anthropic API key"
            }
        }
        "openai-api-key" => {
            if value.starts_with("sk-proj-") {
                "OpenAI API key (project-scoped)"
            } else if value.starts_with("sk-svcacct-") {
                "OpenAI API key (service account)"
            } else if value.starts_with("sk-admin-") {
                "OpenAI admin API key"
            } else {
                "OpenAI API key"
            }
        }
        "aws-access-key" => {
            if value.starts_with("ASIA") {
                "AWS access key ID (temporary credentials)"
            } else {
                "AWS access key ID"
            }
        }
        "aws-secret-key" => {
            if whole.windows(3).any(|w| w.eq_ignore_ascii_case(b"aws")) {
                "AWS secret access key"
            } else {
                "40-character secret key (AWS-style)"
            }
        }
        "github-token" => match &value[..4] {
            "ghp_" => "GitHub personal access token (classic)",
            "gho_" => "GitHub OAuth access token",
            "ghu_" => "GitHub user-to-server token",
            "ghs_" => "GitHub server-to-server token",
            _ => "GitHub refresh token",
        },
        "github-fine-grained-pat" => "GitHub fine-grained personal access token",
        "stripe-key" => {
            let live = value.contains("_live_") || value.contains("_prod_");
            match (value.starts_with("rk_"), live) {
                (false, true) => "Stripe live secret key",
                (false, false) => "Stripe test secret key",
                (true, true) => "Stripe live restricted key",
                (true, false) => "Stripe test restricted key",
            }
        }
        "slack-token" => {
            let v = value.trim_start_matches("xoxe.");
            match v.get(..4) {
                Some("xoxb") => "Slack bot token",
                Some("xoxp") => "Slack user token",
                Some("xoxa") => "Slack app access token",
                Some("xoxr") => "Slack refresh token",
                Some("xoxe") => "Slack refresh/rotation token",
                _ => "Slack token",
            }
        }
        "webhook-url" => {
            if value.contains("slack.com") {
                "Slack incoming webhook URL"
            } else {
                "Discord webhook URL"
            }
        }
        _ => return None,
    };
    Some(h.to_string())
}

// ---------------------------------------------------------------------------------------
// Validators (public)
// ---------------------------------------------------------------------------------------

/// Shannon entropy of `s` in bits per character.
pub fn shannon_entropy(s: &str) -> f32 {
    if s.is_empty() {
        return 0.0;
    }
    if s.is_ascii() {
        let mut counts = [0u32; 128];
        for b in s.bytes() {
            counts[b as usize] += 1;
        }
        let n = s.len() as f32;
        return counts
            .iter()
            .filter(|&&c| c > 0)
            .map(|&c| {
                let p = c as f32 / n;
                -p * p.log2()
            })
            .sum();
    }
    let mut counts = std::collections::HashMap::new();
    let mut n = 0usize;
    for c in s.chars() {
        *counts.entry(c).or_insert(0usize) += 1;
        n += 1;
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
                if x > 9 {
                    x - 9
                } else {
                    x
                }
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

/// Brand and last four digits of a payment card number (`"Visa card ending 4242"`) if
/// it passes the length, IIN and Luhn checks; separators must be consistent.
pub fn card_hint(number: &str) -> Option<String> {
    card_check(number)
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
        assert_eq!(card_hint("4242 4242 4242 4242").as_deref(), Some("Visa card ending 4242"));
        assert_eq!(card_hint("3782 822463 10005").as_deref(), Some("American Express card ending 0005"));
        assert!(card_hint("4242 4242-4242 4242").is_none());
    }

    #[test]
    fn context_words_stand_alone() {
        assert!(contains_word("my nid: ", "nid"));
        assert!(!contains_word("unidentified ", "nid"));
        assert!(contains_word("জাতীয় পরিচয়পত্র নং", "জাতীয় পরিচয়"));
    }

    #[test]
    fn key_denylist() {
        for k in ["max_tokens", "tokenType", "token_url", "public_key", "SECRET_NAME", "password_hash", "passwordField"] {
            assert!(key_denied(k), "{k}");
        }
        for k in ["password", "DB_PASSWORD", "api_key", "SECRET_KEY_BASE", "GITHUB_TOKEN", "clientSecret"] {
            assert!(!key_denied(k), "{k}");
        }
    }
}

