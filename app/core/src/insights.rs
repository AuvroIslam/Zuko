//! Vault insights: a few plain facts about a stored value, for the person looking at
//! the Vault table ("Visa, ends 4242, Luhn passes", "Bangladesh, Grameenphone").
//!
//! Display only and computed locally. The facts are deliberately non-sensitive:
//!
//! * never the value, a preview of it, or any run of more than four of its characters
//!   (a card's last four digits are the most that ever appears);
//! * vendor, brand, country and operator names come from fixed tables in this crate,
//!   not from the value's text;
//! * a JWT is decoded for its header's `alg` and the payload's `exp` only; no other
//!   claim is read, so names, emails or ids inside a token never surface.
//!
//! Nothing here is stored, logged or sent anywhere.

use crate::detect::{self, bd_operator, country_name, iban_length, ipv4_public, ipv6_public, scheme_name, CALLING_CODES};
use crate::vault::Entry;
use base64::Engine as _;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Insight {
    pub label: String,
    pub text: String,
}

fn fact(label: &str, text: impl Into<String>) -> Insight {
    Insight { label: label.into(), text: text.into() }
}

/// Non-sensitive facts about `entry`'s value, most useful first. `now` is unix seconds
/// (used only for token expiry). Always starts with the character count.
pub fn insights(entry: &Entry, now: u64) -> Vec<Insight> {
    let v = entry.value.as_str();
    let n = v.chars().count();
    let mut out = vec![fact("Length", format!("{n} character{}", if n == 1 { "" } else { "s" }))];
    match entry.kind.as_str() {
        "CARD" => card(v, &mut out),
        "PHONE" => phone(v, &mut out),
        "EMAIL" => email(v, &mut out),
        "IBAN" => iban(v, &mut out),
        "JWT" => jwt(v, now, &mut out),
        "IP" => ip(v, &mut out),
        "CONN_STRING" => conn_string(v, &mut out),
        "PRIVATE_KEY" => private_key(v, &mut out),
        "NID" => {
            let digits = v.chars().filter(char::is_ascii_digit).count();
            out.push(fact("Digits", format!("{digits} digit{}", if digits == 1 { "" } else { "s" })));
        }
        "API_KEY" | "TOKEN" | "SECRET" | "PASSWORD" => {
            // A hand-added value may be a JWT or a card whatever its kind says.
            if looks_like_jwt(v) {
                jwt(v, now, &mut out);
            } else {
                if let Some((vendor, what)) = key_vendor(v) {
                    out.push(fact("Vendor", vendor));
                    out.push(fact("Key type", what));
                }
                strength(v, &mut out);
            }
        }
        _ => {
            let words = v.split_whitespace().count();
            if words > 1 {
                out.push(fact("Words", words.to_string()));
            }
        }
    }
    out
}

// ── Cards, phones, email, IBAN ────────────────────────────────────────────────

fn last_four(v: &str) -> String {
    let d: Vec<char> = v.chars().filter(char::is_ascii_digit).collect();
    d[d.len().saturating_sub(4)..].iter().collect()
}

fn card(v: &str, out: &mut Vec<Insight>) {
    let brand = detect::card_hint(v).and_then(|h| h.split(" card ending ").next().map(str::to_string));
    out.push(fact("Brand", brand.unwrap_or_else(|| "Not a recognised card brand".into())));
    out.push(fact("Last four", last_four(v)));
    let luhn = detect::luhn_valid(v);
    out.push(fact("Luhn check", if luhn { "Passes (a plausible card number)" } else { "Fails (probably mistyped or not a card)" }));
}

/// The calling code and country for a number written `+880…`, `00880…` or in
/// Bangladesh's national form `01…`.
fn phone(v: &str, out: &mut Vec<Insight>) {
    let digits: String = v.chars().filter(char::is_ascii_digit).collect();
    let intl = if v.trim_start().starts_with('+') {
        digits.clone()
    } else if let Some(rest) = digits.strip_prefix("00") {
        rest.to_string()
    } else if digits.len() == 11 && digits.starts_with("01") {
        format!("880{}", &digits[1..])
    } else {
        digits.clone()
    };
    match CALLING_CODES.iter().find(|(code, _)| intl.starts_with(code)) {
        Some((code, country)) => {
            out.push(fact("Country", format!("{country} (+{code})")));
            if *code == "880" {
                if let Some(op) = bd_operator(&intl[3..]) {
                    out.push(fact("Operator", format!("{op} (mobile)")));
                }
            }
        }
        None => out.push(fact("Country", "Unknown calling code")),
    }
    out.push(fact("Last four", last_four(v)));
}

/// Providers by name from a fixed table; for any other domain only its kind and top-level
/// part, because the full domain is part of the value.
fn email(v: &str, out: &mut Vec<Insight>) {
    let Some(domain) = v.rsplit_once('@').map(|(_, d)| d.to_ascii_lowercase()) else { return };
    let provider = match domain.as_str() {
        "gmail.com" | "googlemail.com" => Some("Gmail"),
        "outlook.com" | "hotmail.com" | "live.com" | "msn.com" => Some("Outlook"),
        "icloud.com" | "me.com" | "mac.com" => Some("iCloud"),
        "proton.me" | "protonmail.com" | "pm.me" => Some("Proton"),
        d if d.starts_with("yahoo.") || d.starts_with("ymail.") => Some("Yahoo"),
        _ => None,
    };
    match provider {
        Some(p) => out.push(fact("Mail provider", p)),
        None => {
            out.push(fact("Mail provider", "Custom or work domain"));
            if let Some(tld) = domain.rsplit('.').next().filter(|t| t.len() <= 4 && *t != domain) {
                out.push(fact("Top-level domain", format!(".{tld}")));
            }
        }
    }
    let local = v.rsplit_once('@').map(|(l, _)| l.chars().count()).unwrap_or(0);
    out.push(fact("Name part", format!("{local} characters before the @")));
}

fn iban(v: &str, out: &mut Vec<Insight>) {
    let compact: String = v.chars().filter(|c| !c.is_whitespace()).collect::<String>().to_ascii_uppercase();
    let cc = compact.get(..2).unwrap_or("");
    match country_name(cc) {
        Some(name) => out.push(fact("Country", format!("{name} ({cc})"))),
        None => out.push(fact("Country", "Unrecognised country code")),
    }
    if let Some(expected) = iban_length(cc) {
        let ok = compact.len() == expected;
        out.push(fact("Length for country", if ok { format!("Correct ({expected})") } else { format!("Expected {expected}") }));
    }
    let ok = detect::iban_valid(v);
    out.push(fact("Mod-97 check", if ok { "Passes" } else { "Fails (probably mistyped)" }));
    out.push(fact("Last four", last_four(&compact)));
}

// ── Keys and tokens ───────────────────────────────────────────────────────────

/// Vendor and key type implied by a well-known prefix. Names come from this table.
fn key_vendor(v: &str) -> Option<(&'static str, &'static str)> {
    const TABLE: &[(&str, &str, &str)] = &[
        ("sk-ant-oat", "Anthropic", "OAuth token (Claude Code login)"),
        ("sk-ant-admin", "Anthropic", "Admin API key"),
        ("sk-ant-", "Anthropic", "API key"),
        ("sk-proj-", "OpenAI", "Project-scoped API key"),
        ("sk-svcacct-", "OpenAI", "Service-account API key"),
        ("sk-admin-", "OpenAI", "Admin API key"),
        ("sk-", "OpenAI", "Secret API key"),
        ("github_pat_", "GitHub", "Fine-grained personal access token"),
        ("ghp_", "GitHub", "Personal access token (classic)"),
        ("gho_", "GitHub", "OAuth access token"),
        ("ghu_", "GitHub", "User-to-server token"),
        ("ghs_", "GitHub", "Server-to-server token"),
        ("ghr_", "GitHub", "Refresh token"),
        ("glpat-", "GitLab", "Personal access token"),
        ("AKIA", "AWS", "Access key ID"),
        ("ASIA", "AWS", "Temporary access key ID"),
        ("sk_live_", "Stripe", "Live secret key"),
        ("sk_test_", "Stripe", "Test secret key"),
        ("rk_live_", "Stripe", "Live restricted key"),
        ("rk_test_", "Stripe", "Test restricted key"),
        ("pk_live_", "Stripe", "Live publishable key"),
        ("pk_test_", "Stripe", "Test publishable key"),
        ("xoxb-", "Slack", "Bot token"),
        ("xoxp-", "Slack", "User token"),
        ("xoxa-", "Slack", "App token"),
        ("xoxr-", "Slack", "Refresh token"),
        ("xapp-", "Slack", "App-level token"),
        ("AIza", "Google", "API key"),
        ("ya29.", "Google", "OAuth access token"),
        ("npm_", "npm", "Access token"),
        ("hf_", "Hugging Face", "Access token"),
        ("SG.", "SendGrid", "API key"),
        ("dop_v1_", "DigitalOcean", "Personal access token"),
        ("shpat_", "Shopify", "Admin API access token"),
        ("pypi-", "PyPI", "Upload token"),
        ("Bearer ", "HTTP", "Bearer authorization header value"),
    ];
    TABLE.iter().find(|(p, _, _)| v.starts_with(p)).map(|(_, vendor, what)| (*vendor, *what))
}

/// A rough, honest strength note from length, character mix and entropy. Not a
/// guarantee: a short random key is still weaker than a long one.
fn strength(v: &str, out: &mut Vec<Insight>) {
    let n = v.chars().count();
    let classes = [
        v.chars().any(|c| c.is_ascii_lowercase()),
        v.chars().any(|c| c.is_ascii_uppercase()),
        v.chars().any(|c| c.is_ascii_digit()),
        v.chars().any(|c| !c.is_alphanumeric()),
    ]
    .iter()
    .filter(|b| **b)
    .count();
    let bits = detect::shannon_entropy(v);
    let note = if n < 12 || bits < 2.5 {
        "Weak: short or repetitive"
    } else if n >= 20 && bits >= 3.8 && classes >= 2 {
        "Strong: long and random-looking"
    } else {
        "Moderate"
    };
    out.push(fact("Strength", format!("{note} ({classes} of 4 character types, about {bits:.1} bits per character)")));
}

fn private_key(v: &str, out: &mut Vec<Insight>) {
    let head = v.get(..v.len().min(64)).unwrap_or("").to_ascii_uppercase();
    let what = if head.contains("OPENSSH") {
        "OpenSSH private key"
    } else if head.contains("RSA") {
        "RSA private key"
    } else if head.contains("EC PRIVATE") {
        "EC private key"
    } else if head.contains("PGP") {
        "PGP private key block"
    } else if head.contains("ENCRYPTED") {
        "Encrypted private key (PEM)"
    } else {
        "Private key (PEM)"
    };
    out.push(fact("Key type", what));
    out.push(fact("Lines", v.lines().count().to_string()));
}

fn conn_string(v: &str, out: &mut Vec<Insight>) {
    let Some((scheme, rest)) = v.split_once("://") else { return };
    out.push(fact("Service", scheme_name(&scheme.to_ascii_lowercase())));
    let authority = rest.split(['/', '?']).next().unwrap_or("");
    let has_password = authority.rsplit_once('@').is_some_and(|(cred, _)| cred.contains(':'));
    out.push(fact("Credentials", if has_password { "Contains a password" } else { "No password in the address" }));
    let host = authority.rsplit('@').next().unwrap_or("");
    let host = host.rsplit_once(':').map_or(host, |(h, _)| h).trim_matches(['[', ']']).to_ascii_lowercase();
    let local = host == "localhost" || host.parse::<std::net::IpAddr>().is_ok_and(|ip| ip.is_loopback());
    out.push(fact("Host", if local { "This computer" } else { "Another machine" }));
}

fn ip(v: &str, out: &mut Vec<Insight>) {
    let ip = v.trim().trim_matches(['[', ']']).parse::<std::net::IpAddr>();
    match ip {
        Ok(std::net::IpAddr::V4(a)) => {
            out.push(fact("Version", "IPv4"));
            out.push(fact("Scope", if ipv4_public(a) { "Public internet" } else { "Private, loopback or reserved" }));
        }
        Ok(std::net::IpAddr::V6(a)) => {
            out.push(fact("Version", "IPv6"));
            out.push(fact("Scope", if ipv6_public(a) { "Public internet" } else { "Private, loopback or reserved" }));
        }
        Err(_) => {}
    }
}

// ── JWT ───────────────────────────────────────────────────────────────────────

fn looks_like_jwt(v: &str) -> bool {
    let parts: Vec<&str> = v.split('.').collect();
    parts.len() == 3 && parts[0].starts_with("eyJ") && parts[1].starts_with("eyJ")
}

fn decode_part(part: &str) -> Option<serde_json::Value> {
    let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(part.trim_end_matches('=')).ok()?;
    serde_json::from_slice(&raw).ok()
}

fn span(secs: u64) -> String {
    match secs {
        0..=59 => "under a minute".into(),
        60..=3599 => format!("{} min", secs / 60),
        3600..=86_399 => format!("{} h", secs / 3600),
        _ => format!("{} days", secs / 86_400),
    }
}

/// Header `alg` and payload `exp` only. Every other claim is ignored on purpose.
fn jwt(v: &str, now: u64, out: &mut Vec<Insight>) {
    let mut parts = v.split('.');
    let header = parts.next().and_then(decode_part);
    let payload = parts.next().and_then(decode_part);
    if let Some(alg) = header.as_ref().and_then(|h| h.get("alg")).and_then(|a| a.as_str()) {
        // Only well-known algorithm names are echoed; anything else is just "custom".
        const KNOWN: &[&str] = &[
            "HS256", "HS384", "HS512", "RS256", "RS384", "RS512", "ES256", "ES384", "ES512", "PS256", "PS384", "PS512", "EdDSA", "none",
        ];
        let name = KNOWN.iter().find(|k| k.eq_ignore_ascii_case(alg)).copied().unwrap_or("custom");
        let note = if name == "none" { " (unsigned: anyone could have written it)" } else { "" };
        out.push(fact("Algorithm", format!("{name}{note}")));
    }
    match payload.as_ref().and_then(|p| p.get("exp")).and_then(|e| e.as_f64()) {
        Some(exp) if exp.is_finite() && exp >= 0.0 => {
            let exp = exp as u64;
            let text = if exp <= now { format!("Expired {} ago", span(now - exp)) } else { format!("Valid for another {}", span(exp - now)) };
            out.push(fact("Expiry", text));
        }
        _ => out.push(fact("Expiry", "No expiry set")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detect::Category;

    fn entry(kind: &str, value: &str) -> Entry {
        Entry {
            key: format!("{kind}_1"),
            value: value.into(),
            kind: kind.into(),
            label: "t".into(),
            category: Category::Secret,
            hint: None,
            source: "t".into(),
            created: 0,
            last_used: 0,
            hits: 1,
        }
    }

    fn get<'a>(i: &'a [Insight], label: &str) -> &'a str {
        i.iter().find(|x| x.label == label).map(|x| x.text.as_str()).unwrap_or_else(|| panic!("no {label} in {i:?}"))
    }

    /// The privacy invariant: no insight carries the value or any 5-character run of it.
    fn assert_clean(e: &Entry, now: u64) {
        let out = insights(e, now);
        let chars: Vec<char> = e.value.chars().collect();
        for i in &out {
            for hay in [i.label.as_str(), i.text.as_str()] {
                assert!(!hay.contains(&e.value), "{} leaks the value: {hay}", e.kind);
                for w in chars.windows(5) {
                    let s: String = w.iter().collect();
                    assert!(!hay.contains(&s), "{} leaks {s:?}: {hay}", e.kind);
                }
            }
        }
    }

    fn b64(s: &str) -> String {
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(s)
    }

    fn jwt_with(header: &str, payload: &str) -> String {
        format!("{}.{}.c2lnbmF0dXJlLWJ5dGVz", b64(header), b64(payload))
    }

    #[test]
    fn length_is_always_first() {
        let i = insights(&entry("TERM", "Project Falcon"), 0);
        assert_eq!(i[0], fact("Length", "14 characters"));
        assert_eq!(get(&i, "Words"), "2");
    }

    #[test]
    fn card_brand_last_four_and_luhn() {
        let i = insights(&entry("CARD", "4242 4242 4242 4242"), 0);
        assert_eq!(get(&i, "Brand"), "Visa");
        assert_eq!(get(&i, "Last four"), "4242");
        assert!(get(&i, "Luhn check").starts_with("Passes"));
        let bad = insights(&entry("CARD", "4242 4242 4242 4241"), 0);
        assert!(get(&bad, "Luhn check").starts_with("Fails"));
        assert_eq!(get(&bad, "Last four"), "4241");
        assert_eq!(get(&insights(&entry("CARD", "378282246310005"), 0), "Brand"), "American Express");
    }

    #[test]
    fn phone_country_and_operator() {
        let i = insights(&entry("PHONE", "+8801712344471"), 0);
        assert_eq!(get(&i, "Country"), "Bangladesh (+880)");
        assert_eq!(get(&i, "Operator"), "Grameenphone (mobile)");
        assert_eq!(get(&i, "Last four"), "4471");
        // National form and 00 prefix.
        assert_eq!(get(&insights(&entry("PHONE", "01812344471"), 0), "Operator"), "Robi (mobile)");
        assert_eq!(get(&insights(&entry("PHONE", "00447911123456"), 0), "Country"), "United Kingdom (+44)");
        assert_eq!(get(&insights(&entry("PHONE", "+4915123456789"), 0), "Country"), "Germany (+49)");
        assert_eq!(get(&insights(&entry("PHONE", "+9999999999"), 0), "Country"), "Unknown calling code");
    }

    #[test]
    fn email_shows_provider_not_domain() {
        let i = insights(&entry("EMAIL", "jamie.rahman@gmail.com"), 0);
        assert_eq!(get(&i, "Mail provider"), "Gmail");
        let w = insights(&entry("EMAIL", "jamie.rahman@acme.io"), 0);
        assert_eq!(get(&w, "Mail provider"), "Custom or work domain");
        assert_eq!(get(&w, "Top-level domain"), ".io");
    }

    #[test]
    fn api_key_vendor_type_and_strength() {
        let i = insights(&entry("API_KEY", "sk-proj-Xk29fLm0aQ7rT1vB3nZ8yW4uE6iO2pS5dG9fQa"), 0);
        assert_eq!(get(&i, "Vendor"), "OpenAI");
        assert_eq!(get(&i, "Key type"), "Project-scoped API key");
        assert!(get(&i, "Strength").starts_with("Strong"));
        let a = insights(&entry("API_KEY", "sk-ant-oat01-Qw8eR4tY6uI0oP2aS4dF"), 0);
        assert_eq!(get(&a, "Vendor"), "Anthropic");
        assert!(get(&a, "Key type").starts_with("OAuth"));
        let t = insights(&entry("TOKEN", "ghp_7hT3kLmN9pQ2rS5tU8vW1xY4zA6bC0X0aB"), 0);
        assert_eq!(get(&t, "Vendor"), "GitHub");
        let s = insights(&entry("TOKEN", "sk_test_abcdefghijklmnop"), 0);
        assert_eq!(get(&s, "Key type"), "Test secret key");
        let weak = insights(&entry("PASSWORD", "aaaaaaaa"), 0);
        assert!(get(&weak, "Strength").starts_with("Weak"));
        assert!(!weak.iter().any(|x| x.label == "Vendor"));
    }

    #[test]
    fn iban_country_and_mod97() {
        let i = insights(&entry("IBAN", "DE89 3704 0044 0532 0130 00"), 0);
        assert_eq!(get(&i, "Country"), "Germany (DE)");
        assert_eq!(get(&i, "Mod-97 check"), "Passes");
        assert!(get(&i, "Length for country").starts_with("Correct"));
        assert_eq!(get(&i, "Last four"), "3000");
        let bad = insights(&entry("IBAN", "DE89 3704 0044 0532 0130 01"), 0);
        assert!(get(&bad, "Mod-97 check").starts_with("Fails"));
    }

    #[test]
    fn jwt_algorithm_and_expiry_only() {
        let expired = jwt_with(r#"{"alg":"HS256","typ":"JWT"}"#, r#"{"sub":"jamie@example.org","name":"Jamie Rahman","exp":1000}"#);
        let i = insights(&entry("JWT", &expired), 1000 + 3 * 86_400);
        assert_eq!(get(&i, "Algorithm"), "HS256");
        assert_eq!(get(&i, "Expiry"), "Expired 3 days ago");
        // No claim other than exp ever shows.
        let all: String = i.iter().map(|x| format!("{} {}", x.label, x.text)).collect();
        assert!(!all.contains("Jamie") && !all.contains("example"));
        let live = jwt_with(r#"{"alg":"RS256"}"#, r#"{"exp":10000}"#);
        assert_eq!(get(&insights(&entry("JWT", &live), 10000 - 7200), "Expiry"), "Valid for another 2 h");
        let none = jwt_with(r#"{"alg":"none"}"#, r#"{"a":1}"#);
        let n = insights(&entry("JWT", &none), 0);
        assert!(get(&n, "Algorithm").starts_with("none"));
        assert_eq!(get(&n, "Expiry"), "No expiry set");
        let odd = jwt_with(r#"{"alg":"Secret-Algo-Name"}"#, r#"{}"#);
        assert_eq!(get(&insights(&entry("JWT", &odd), 0), "Algorithm"), "custom");
    }

    #[test]
    fn a_jwt_stored_as_a_token_is_still_decoded() {
        let t = jwt_with(r#"{"alg":"ES256"}"#, r#"{"exp":10}"#);
        assert_eq!(get(&insights(&entry("TOKEN", &t), 20), "Expiry"), "Expired under a minute ago");
    }

    #[test]
    fn other_kinds() {
        let c = insights(&entry("CONN_STRING", "postgres://app:hunter2@localhost:5432/app"), 0);
        assert_eq!(get(&c, "Service"), "PostgreSQL");
        assert_eq!(get(&c, "Credentials"), "Contains a password");
        assert_eq!(get(&c, "Host"), "This computer");
        let r = insights(&entry("CONN_STRING", "mysql://u:p@db.internal.example.com/x"), 0);
        assert_eq!(get(&r, "Host"), "Another machine");
        assert_eq!(get(&insights(&entry("IP", "8.8.8.8"), 0), "Scope"), "Public internet");
        assert!(get(&insights(&entry("IP", "192.168.1.20"), 0), "Scope").starts_with("Private"));
        let k = insights(&entry("PRIVATE_KEY", "-----BEGIN OPENSSH PRIVATE KEY-----\nabc\n-----END OPENSSH PRIVATE KEY-----"), 0);
        assert_eq!(get(&k, "Key type"), "OpenSSH private key");
        assert_eq!(get(&insights(&entry("NID", "1234567890"), 0), "Digits"), "10 digits");
    }

    #[test]
    fn no_insight_ever_contains_the_value_or_a_five_char_run_of_it() {
        let jwt = jwt_with(r#"{"alg":"HS256"}"#, r#"{"exp":1000,"sub":"someone"}"#);
        let samples: Vec<(&str, String)> = vec![
            ("CARD", "4242 4242 4242 4242".into()),
            ("CARD", "5555555555554444".into()),
            ("PHONE", "+8801712344471".into()),
            ("PHONE", "+14155552671".into()),
            ("EMAIL", "jamie.rahman@acme.io".into()),
            ("EMAIL", "jamie.rahman@gmail.com".into()),
            ("API_KEY", "sk-proj-Xk29fLm0aQ7rT1vB3nZ8yW4uE6iO2pS5dG9fQa".into()),
            ("API_KEY", "sk-ant-api03-Qw8eR4tY6uI0oP2aS4dF6gH8jK0lZ7Kp2".into()),
            ("TOKEN", "ghp_7hT3kLmN9pQ2rS5tU8vW1xY4zA6bC0X0aB".into()),
            ("TOKEN", "AKIAIOSFODNN7EXAMPLE".into()),
            ("SECRET", "correct horse battery staple".into()),
            ("PASSWORD", "hunter2hunter2".into()),
            ("IBAN", "GB82 WEST 1234 5698 7654 32".into()),
            ("IBAN", "DE89370400440532013000".into()),
            ("JWT", jwt.clone()),
            // (A fixed product name such as "PostgreSQL" may share letters with the scheme in the
            // value; that is public vocabulary, not a copy, so the sample avoids the overlap.)
            ("CONN_STRING", "redis://default:hunter2@localhost:6379/0".into()),
            ("IP", "203.0.113.77".into()),
            ("NID", "1990123456789".into()),
            ("TERM", "Project Falcon".into()),
            ("PRIVATE_KEY", "-----BEGIN RSA PRIVATE KEY-----\nMIIEowIBAAKCAQEA\n-----END RSA PRIVATE KEY-----".into()),
        ];
        for (kind, value) in samples {
            assert_clean(&entry(kind, &value), 2000);
        }
    }
}
