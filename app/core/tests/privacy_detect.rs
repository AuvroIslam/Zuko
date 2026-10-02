//! Detector recall/precision fixtures and behaviour tests.

use zuko_core::detect::{Category, Detector, DetectorConfig, Finding};

const POSITIVE: &str = include_str!("fixtures/detect_positive.txt");
const NEGATIVE: &str = include_str!("fixtures/detect_negative.txt");
const CLEAN_CODE: &str = include_str!("fixtures/clean_code.txt");

struct Expect {
    start: usize,
    end: usize,
    kind: String,
    rule: String,
}

/// Strips `«KIND/rule:value»` markers, returning the plain text and the expected spans.
fn parse_case(line: &str) -> (String, Vec<Expect>) {
    let line = line.replace('␤', "\n");
    let mut text = String::new();
    let mut exp = Vec::new();
    let mut rest = line.as_str();
    while let Some(open) = rest.find('«') {
        text.push_str(&rest[..open]);
        let after = &rest[open + '«'.len_utf8()..];
        let close = after.find('»').expect("unclosed marker");
        let inner = &after[..close];
        let (kr, value) = inner.split_once(':').expect("marker needs KIND:value");
        let (kind, rule) = kr.split_once('/').unwrap_or((kr, ""));
        let start = text.len();
        text.push_str(value);
        exp.push(Expect { start, end: text.len(), kind: kind.into(), rule: rule.into() });
        rest = &after[close + '»'.len_utf8()..];
    }
    text.push_str(rest);
    (text, exp)
}

fn describe(fs: &[Finding]) -> String {
    fs.iter()
        .map(|f| format!("[{} {} {:?} {:?}]", f.kind, f.rule, f.value, f.hint))
        .collect::<Vec<_>>()
        .join(" ")
}

fn ips_on() -> DetectorConfig {
    DetectorConfig { ips: true, ..DetectorConfig::default() }
}

#[test]
fn recall_fixtures_all_detected_exactly() {
    let default = Detector::new(&DetectorConfig::default());
    let with_ips = Detector::new(&ips_on());
    let mut det = &default;
    let mut failures = Vec::new();
    let mut cases = 0;
    let mut expected = 0;
    for line in POSITIVE.lines() {
        if line.starts_with('#') || line.trim().is_empty() {
            continue;
        }
        if line.trim() == "@config ips" {
            det = &with_ips;
            continue;
        }
        cases += 1;
        let (text, exp) = parse_case(line);
        expected += exp.len();
        let got = det.scan(&text);
        for e in &exp {
            let hit = got.iter().find(|f| f.start == e.start && f.end == e.end);
            match hit {
                Some(f) if f.kind == e.kind && (e.rule.is_empty() || f.rule == e.rule) => {
                    assert_eq!(f.value, text[e.start..e.end]);
                }
                _ => failures.push(format!(
                    "MISSED {}/{} {:?}\n   in: {:?}\n  got: {}",
                    e.kind,
                    e.rule,
                    &text[e.start..e.end],
                    text,
                    describe(&got)
                )),
            }
        }
        for f in &got {
            if !exp.iter().any(|e| e.start == f.start && e.end == f.end) {
                failures.push(format!("EXTRA {} {} {:?}\n   in: {:?}", f.kind, f.rule, f.value, text));
            }
        }
    }
    assert!(cases >= 100, "fixture has {cases} cases");
    assert!(failures.is_empty(), "{} of {expected} expectations failed:\n{}", failures.len(), failures.join("\n"));
}

#[test]
fn precision_fixtures_have_no_findings() {
    let dets = [Detector::new(&DetectorConfig::default()), Detector::new(&ips_on())];
    let mut failures = Vec::new();
    let mut cases = 0;
    for line in NEGATIVE.lines() {
        if line.starts_with('#') || line.trim().is_empty() {
            continue;
        }
        cases += 1;
        let text = line.replace('␤', "\n");
        for d in &dets {
            let got = d.scan(&text);
            if !got.is_empty() {
                failures.push(format!("{:?}\n  got: {}", text, describe(&got)));
                break;
            }
        }
    }
    assert!(cases >= 100, "fixture has {cases} cases");
    assert!(failures.is_empty(), "{} false positives:\n{}", failures.len(), failures.join("\n"));
}

#[test]
fn realistic_code_corpus_is_clean() {
    for cfg in [DetectorConfig::default(), ips_on()] {
        let got = Detector::new(&cfg).scan(CLEAN_CODE);
        assert!(got.is_empty(), "false positives in clean_code.txt: {}", describe(&got));
    }
}

#[test]
fn at_least_thirty_secret_rules() {
    let d = Detector::new(&DetectorConfig::default());
    let ids = d.rule_ids();
    let secret_rules = ids.iter().filter(|id| !matches!(**id, "email" | "phone-bd" | "phone-intl" | "phone-national" | "credit-card" | "iban" | "us-ssn" | "national-id" | "passport" | "date-of-birth")).count();
    assert!(secret_rules >= 30, "{secret_rules} secret rules: {ids:?}");
    let mut sorted = ids.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(sorted.len(), ids.len(), "duplicate rule ids");
}

#[test]
fn hints_are_informative_and_value_free() {
    let d = Detector::new(&DetectorConfig::default());
    let cases = [
        ("card 4242 4242 4242 4242", "Visa card ending 4242"),
        ("mc 5555555555554444", "Mastercard card ending 4444"),
        ("mail rahim@gmail.com", "Gmail address"),
        ("mail someone@cse.univ.ac.bd", "Academic (university) email address"),
        ("call +8801712345678", "Bangladesh mobile number (Grameenphone)"),
        ("call 01912345678", "Bangladesh mobile number (Banglalink)"),
        ("uk +44 20 7946 0958", "Phone number (United Kingdom, +44)"),
        ("iban DE89370400440532013000", "IBAN (Germany)"),
        ("DOB: 15/03/1990", "Date of birth (year 1990)"),
        ("DATABASE_URL=postgres://app:S3cr3tPassw0rd@db.internal:5432/orders", "PostgreSQL connection string with password"),
        ("DB_PASSWORD=letmein99", "value of DB_PASSWORD"),
    ];
    for (text, hint) in cases {
        let fs = d.scan(text);
        assert_eq!(fs.len(), 1, "{text}: {}", describe(&fs));
        assert_eq!(fs[0].hint.as_deref(), Some(hint), "{text}");
        let h = fs[0].hint.as_deref().unwrap();
        // The hint never contains the value (beyond the last 4 card digits).
        assert!(!h.contains(&fs[0].value), "{h}");
    }
    let key = "sk-proj-ZukoFake0123456789abcdefghijklmnopqrstuv";
    let fs = d.scan(&format!("key {key}"));
    assert_eq!(fs[0].hint.as_deref(), Some("OpenAI API key (project-scoped)"));
    let fs = d.scan("Authorization: Bearer eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiJ6dWtvLWZha2UifQ.c2lnbmF0dXJlLWZha2UtenVrbw");
    assert_eq!(fs.len(), 1);
    assert_eq!(fs[0].kind, "JWT");
    assert_eq!(fs[0].hint.as_deref(), Some("JSON Web Token (HS256)"));
}

#[test]
fn custom_terms_whole_words_case_insensitive() {
    let cfg = DetectorConfig {
        custom_terms: vec!["Project Falcon".into(), "Acme".into(), "ঢাকা ব্যাংক".into(), "  ".into()],
        ..DetectorConfig::default()
    };
    let d = Detector::new(&cfg);
    let t = "project falcon ships for ACME; not Acmeville or xacme. ঢাকা ব্যাংক loan";
    let fs = d.scan(t);
    let vals: Vec<&str> = fs.iter().map(|f| f.value.as_str()).collect();
    assert_eq!(vals, vec!["project falcon", "ACME", "ঢাকা ব্যাংক"]);
    assert!(fs.iter().all(|f| f.kind == "TERM" && f.category == Category::Custom && f.rule == "custom-term"));
    // Stronger rules win over custom terms on overlap.
    let fs = d.scan("mail ops@acme.io");
    assert_eq!(fs.len(), 1);
    assert_eq!(fs[0].kind, "EMAIL");
}

#[test]
fn allowlist_values_and_domains() {
    let cfg = DetectorConfig {
        allowlist: vec!["acme.io".into(), "Hunter2!x".into()],
        ..DetectorConfig::default()
    };
    let d = Detector::new(&cfg);
    assert!(d.scan("mail bob@acme.io and bob@mail.acme.io").is_empty());
    assert!(d.scan("password = \"Hunter2!x\"").is_empty());
    assert_eq!(d.scan("mail bob@acme.com").len(), 1);
    // Without the default allowlist, example.com is masked.
    let d = Detector::new(&DetectorConfig { allowlist: vec![], ..DetectorConfig::default() });
    assert_eq!(d.scan("mail someone@example.com").len(), 1);
}

#[test]
fn config_toggles() {
    let text = "key sk-proj-ZukoFake0123456789abcdefghijklmnopqrstuv mail bob@acme.io card 4242424242424242 phone +8801712345678 iban DE89370400440532013000 ip 52.95.110.1";
    let kinds = |cfg: DetectorConfig| -> Vec<String> { Detector::new(&cfg).scan(text).into_iter().map(|f| f.kind).collect() };
    assert_eq!(kinds(DetectorConfig::default()), vec!["API_KEY", "EMAIL", "CARD", "PHONE", "IBAN"]);
    assert_eq!(kinds(ips_on()), vec!["API_KEY", "EMAIL", "CARD", "PHONE", "IBAN", "IP"]);
    assert_eq!(kinds(DetectorConfig { secrets: false, ..DetectorConfig::default() }), vec!["EMAIL", "CARD", "PHONE", "IBAN"]);
    assert_eq!(kinds(DetectorConfig { pii: false, ips: true, ..DetectorConfig::default() }), vec!["API_KEY"]);
    assert_eq!(
        kinds(DetectorConfig { emails: false, cards: false, phones: false, ibans: false, ..DetectorConfig::default() }),
        vec!["API_KEY"]
    );
    let d = Detector::new(&DetectorConfig { national_ids: false, ..DetectorConfig::default() });
    assert!(d.scan("SSN: 123-45-6789, DOB: 15/03/1990").is_empty());
}

#[test]
fn generic_entropy_is_opt_in() {
    let token = "Zk3pQ9vL2mN8xR4tY7wB1cD6fG5hJ0kS";
    let text = format!("value {token} here");
    assert!(Detector::new(&DetectorConfig::default()).scan(&text).is_empty());
    let d = Detector::new(&DetectorConfig { generic_entropy: true, ..DetectorConfig::default() });
    let fs = d.scan(&text);
    assert_eq!(fs.len(), 1);
    assert_eq!(fs[0].value, token);
    assert_eq!(fs[0].rule, "high-entropy-token");
    // Even when on: no git SHAs, UUIDs, identifiers, integrity hashes or data URIs.
    for t in [
        "commit 3f786850e387550fdab836ed7e6dc881de23001b",
        "id 123e4567-e89b-12d3-a456-426614174000",
        "getUserAccountSettingsHandlerForTheWin",
        "sha512-z4PhNX7vuL3xVChQ1m2AB9Yg5AULVxXcg/SpIdNs6c5H0NE8XYXysP+DGNKHfuwvY7kxvUdBeoGlODJ6+SfaPg==",
        "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==",
    ] {
        assert!(d.scan(t).is_empty(), "{t}: {}", describe(&d.scan(t)));
    }
}

#[test]
fn findings_are_sorted_non_overlapping_and_on_char_boundaries() {
    let d = Detector::new(&ips_on());
    let mut text = String::new();
    for line in POSITIVE.lines().filter(|l| !l.starts_with('#') && !l.starts_with('@')) {
        text.push_str(&parse_case(line).0);
        text.push_str(" ঢাকা\n");
    }
    let fs = d.scan(&text);
    assert!(fs.len() >= 100, "{}", fs.len());
    for w in fs.windows(2) {
        assert!(w[0].end <= w[1].start, "overlap {:?} {:?}", w[0].value, w[1].value);
    }
    for f in &fs {
        assert!(text.is_char_boundary(f.start) && text.is_char_boundary(f.end));
        assert_eq!(&text[f.start..f.end], f.value);
        assert!(f.confidence >= 0.5);
    }
}

#[test]
fn never_overlaps_placeholders_and_is_idempotent_on_masked_text() {
    let d = Detector::new(&DetectorConfig::default());
    assert!(d.scan("postgres://app:{{PASSWORD_1}}@db:5432/x and {{EMAIL_1}}").is_empty());
    assert!(d.scan("password = \"{{PASSWORD_3}}\"; card {{CARD_1}}").is_empty());
    assert!(d.scan("Authorization: Bearer {{TOKEN_1}}").is_empty());
}

#[test]
fn adjacent_values_are_both_found() {
    let d = Detector::new(&DetectorConfig::default());
    let fs = d.scan("01712345678,01812345678");
    assert_eq!(fs.len(), 2, "{}", describe(&fs));
    let fs = d.scan("a@acme.io,b@acme.io");
    assert_eq!(fs.len(), 2, "{}", describe(&fs));
    let fs = d.scan("4242424242424242\n5555555555554444");
    assert_eq!(fs.len(), 2, "{}", describe(&fs));
}

#[test]
fn unicode_neighbours_do_not_break_detection() {
    let d = Detector::new(&DetectorConfig::default());
    let key = "ghp_ZukoFake0123456789abcdefghijklmnopqr";
    for t in [format!("টোকেন:{key}।"), format!("«{key}»"), format!("—{key}—"), format!("日本{key}語")] {
        let fs = d.scan(&t);
        assert_eq!(fs.len(), 1, "{t}: {}", describe(&fs));
        assert_eq!(fs[0].value, key);
    }
    // A negated value class that hits non-ASCII bytes is snapped to a char boundary.
    let fs = d.scan("password = \"pässwörd99\"");
    assert_eq!(fs.len(), 1);
    assert_eq!(fs[0].value, "pässwörd99");
}

#[test]
fn empty_and_tiny_inputs() {
    let d = Detector::new(&DetectorConfig::default());
    for t in ["", " ", "{", "{{", "@", "+", "sk-", "-----BEGIN", "é"] {
        assert!(d.scan(t).is_empty(), "{t}");
    }
}
