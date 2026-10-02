//! Vault + masking + rehydration properties.

use serde_json::json;
use std::sync::LazyLock;
use zuko_core::detect::{Category, Detector, DetectorConfig, Finding};
use zuko_core::mask::{
    find_placeholders, keys_in_json, keys_in_text, legend, mask_json, mask_known, mask_known_json, mask_text,
    rehydrate_json, rehydrate_json_text, rehydrate_text, MaskCtx, LEGEND_INTRO,
};
use zuko_core::placeholder;
use zuko_core::vault::Vault;

// The fixtures store each fake key "defanged" (a U+00A6 marker after its third
// character) so secret scanners such as GitHub push protection do not flag them.
// The text the tests check is the original, with the markers removed.
static POSITIVE: LazyLock<String> =
    LazyLock::new(|| include_str!("fixtures/detect_positive.txt").replace('\u{a6}', ""));
const CLEAN_CODE: &str = include_str!("fixtures/clean_code.txt");
const MASK_RS: &str = include_str!("../src/mask.rs");

fn ctx() -> MaskCtx {
    MaskCtx { source: "test".into(), now: 7 }
}

fn finding(value: &str, kind: &str, hint: Option<&str>) -> Finding {
    Finding {
        start: 0,
        end: value.len(),
        value: value.into(),
        kind: kind.into(),
        rule: "t".into(),
        label: format!("{kind} label"),
        category: Category::Secret,
        hint: hint.map(Into::into),
        confidence: 1.0,
    }
}

/// Fixture lines with markers removed.
fn fixture_texts() -> Vec<String> {
    POSITIVE
        .lines()
        .filter(|l| !l.starts_with('#') && !l.starts_with('@') && !l.trim().is_empty())
        .map(|l| {
            let mut out = String::new();
            let mut rest = l;
            while let Some(i) = rest.find('«') {
                out.push_str(&rest[..i]);
                let after = &rest[i + '«'.len_utf8()..];
                let colon = after.find(':').unwrap();
                let close = after.find('»').unwrap();
                out.push_str(&after[colon + 1..close]);
                rest = &after[close + '»'.len_utf8()..];
            }
            out.push_str(rest);
            out.replace('␤', "\n")
        })
        .collect()
}

#[test]
fn every_fixture_round_trips_and_is_idempotent() {
    let det = Detector::new(&DetectorConfig { ips: true, ..DetectorConfig::default() });
    let mut v = Vault::new();
    let texts = fixture_texts();
    let mut masked_any = 0;
    for t in &texts {
        let (m, r) = mask_text(&det, &mut v, t, &ctx());
        if r.count > 0 {
            masked_any += 1;
        }
        // No value survives.
        for k in &r.keys {
            let e = v.get(k).unwrap();
            assert!(!m.contains(&e.value), "{k} value still present in {m:?}");
        }
        assert_eq!(rehydrate_text(&v, &m).0, *t, "round trip");
        assert_eq!(mask_text(&det, &mut v, &m, &ctx()).0, m, "idempotent");
        assert_eq!(mask_known(&v, t).0, m, "known-only masking equals full masking once interned");
        assert_eq!(keys_in_text(&v, &m), r.keys);
    }
    assert!(masked_any >= 100);
    // The whole corpus at once gives the same keys (deterministic vault).
    let all = texts.join("\n");
    let (m, r) = mask_text(&det, &mut v, &all, &ctx());
    assert!(r.new_keys.is_empty(), "no new keys on second pass: {:?}", r.new_keys);
    assert_eq!(rehydrate_text(&v, &m).0, all);
    // Clean code is untouched.
    assert_eq!(mask_text(&det, &mut v, CLEAN_CODE, &ctx()).0, CLEAN_CODE);
}

#[test]
fn deterministic_keys_across_vault_round_trip() {
    let det = Detector::new(&DetectorConfig::default());
    let mut v = Vault::new();
    let t = "a@acme.io b@acme.io a@acme.io card 4242424242424242";
    let (m, r) = mask_text(&det, &mut v, t, &ctx());
    assert_eq!(m, "{{EMAIL_1}} {{EMAIL_2}} {{EMAIL_1}} card {{CARD_1}}");
    assert_eq!(r.count, 4);
    assert_eq!(r.keys, vec!["EMAIL_1", "EMAIL_2", "CARD_1"]);
    assert_eq!(r.new_keys, vec!["EMAIL_1", "EMAIL_2", "CARD_1"]);
    let mut back = Vault::from_json(&v.to_json()).unwrap();
    let (m2, r2) = mask_text(&det, &mut back, t, &ctx());
    assert_eq!(m2, m);
    assert!(r2.new_keys.is_empty());
    assert_eq!(back.get("EMAIL_1").unwrap().hits, 4, "interned twice, matched twice");
}

#[test]
fn placeholders_are_never_altered_by_vault_values() {
    let mut v = Vault::new();
    // Custom terms are exempt from the minimum length, so short uppercase words that
    // appear inside placeholder kinds must not break placeholders.
    for term in ["KEY", "API", "TOKEN", "_1", "EMAIL"] {
        let mut f = finding(term, "TERM", None);
        f.category = Category::Custom;
        v.intern(&f, "t", 1);
    }
    let det = Detector::new(&DetectorConfig::default());
    let t = "use {{API_KEY_1}} and {{ TOKEN_2 }} but mask KEY and TOKEN";
    let (m, _) = mask_text(&det, &mut v, t, &ctx());
    assert!(m.starts_with("use {{API_KEY_1}} and {{ TOKEN_2 }} but mask {{TERM_"), "{m}");
    let (k, _) = mask_known(&v, t);
    assert!(k.starts_with("use {{API_KEY_1}} and {{ TOKEN_2 }}"), "{k}");
    // A value that itself contains a placeholder is refused.
    assert!(v.add_manual("pre {{API_KEY_1}} post", "SECRET", "x", 1).is_none());
}

#[test]
fn json_escaped_values_are_found_and_rehydrated_at_value_level() {
    let mut v = Vault::new();
    let pw = r#"pa"ss\wörd-01"#;
    let key = v.intern(&finding(pw, "PASSWORD", None), "t", 1).unwrap();
    // Raw form.
    assert_eq!(mask_known(&v, &format!("pw={pw}")).0, format!("pw={{{{{key}}}}}"));
    // JSON-escaped form inside a JSON text (e.g. a tool result showing a config file):
    // masked under its own key, so the round trip is exact; known-only masking skips it.
    let det = Detector::new(&DetectorConfig { secrets: false, pii: false, ..DetectorConfig::default() });
    let doc = json!({ "password": pw }).to_string();
    assert_eq!(mask_known(&v, &doc).1, 0);
    let (m, r) = mask_text(&det, &mut v, &doc, &ctx());
    assert_eq!(r.count, 1);
    assert_eq!(r.new_keys, vec!["PASSWORD_2"]);
    assert_eq!(m, r#"{"password":"{{PASSWORD_2}}"}"#);
    assert_eq!(rehydrate_text(&v, &m).0, doc);
    assert_eq!(v.get("PASSWORD_2").unwrap().value, r#"pa\"ss\\wörd-01"#);
    assert_eq!(mask_text(&det, &mut v, &doc, &ctx()).0, m, "stable on repeat");
    assert_eq!(mask_known(&v, &doc).0, m, "now a raw value of its own entry");
    // ASCII-escaped form (Python json.dumps default).
    let ascii = r#"{"password": "pa\"ss\\w\u00f6rd-01"}"#;
    let (m, r) = mask_text(&det, &mut v, ascii, &ctx());
    assert_eq!(r.count, 1, "{ascii}");
    assert_eq!(rehydrate_text(&v, &m).0, ascii);
    // Value-level rehydration re-escapes correctly.
    let mut body = json!({ "content": format!("{{\"password\": \"{{{{{key}}}}}\"}}") });
    rehydrate_json(&v, &mut body);
    assert_eq!(body["content"], format!("{{\"password\": \"{pw}\"}}"));
    let text = serde_json::to_string(&json!({"file_path": "a.json", "content": format!("x={{{{{key}}}}}")})).unwrap();
    let (out, keys) = rehydrate_json_text(&v, &text).unwrap();
    assert_eq!(keys, vec![key.clone()]);
    let parsed: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(parsed["content"], format!("x={pw}"));
}

#[test]
fn rehydrate_json_text_preserves_layout() {
    let mut v = Vault::new();
    let key = v.intern(&finding("s3cr3t-value", "API_KEY", None), "t", 1).unwrap();
    let ph = placeholder::wrap(&key);
    let src = format!("{{ \"z\": 1.50, \"{ph}\": \"{ph}\",\n  \"a\" : [\"x {ph} y\", true, null, \"\\u00e9\"] }}");
    let (out, keys) = rehydrate_json_text(&v, &src).unwrap();
    assert_eq!(out, src.replace(&format!("\"{ph}\",\n"), "\"s3cr3t-value\",\n").replace(&format!("x {ph} y"), "x s3cr3t-value y"));
    assert_eq!(keys.len(), 2, "object keys are not rehydrated");
    assert!(out.contains("1.50") && out.contains(&format!("\"{ph}\":")));
    assert!(rehydrate_json_text(&v, "{\"a\": \"{{API_KEY_1}}\"").is_none(), "unparsable input");
    assert_eq!(rehydrate_json_text(&v, "{}").unwrap().0, "{}");
}

#[test]
fn json_tree_helpers() {
    let det = Detector::new(&DetectorConfig::default());
    let mut v = Vault::new();
    let mut body = json!({
        "bob@acme.io": "mail bob@acme.io",
        "list": ["card 4242424242424242", 5, {"deep": "again bob@acme.io"}],
    });
    let r = mask_json(&det, &mut v, &mut body, &ctx());
    assert_eq!(r.count, 3);
    assert_eq!(body["list"][2]["deep"], "again {{EMAIL_1}}");
    assert!(body.get("bob@acme.io").is_some(), "object keys untouched");
    assert_eq!(keys_in_json(&v, &body), vec!["EMAIL_1", "CARD_1"]);
    let keys = rehydrate_json(&v, &mut body);
    assert_eq!(keys.len(), 3);
    assert_eq!(body["list"][0], "card 4242424242424242");
    assert_eq!(mask_known_json(&v, &mut body), 3);
    assert_eq!(body["bob@acme.io"], "mail {{EMAIL_1}}");
}

#[test]
fn multibyte_text_everywhere() {
    let cfg = DetectorConfig { custom_terms: vec!["ঢাকা ব্যাংক".into(), "Zürich AG".into()], ..DetectorConfig::default() };
    let det = Detector::new(&cfg);
    let mut v = Vault::new();
    let t = "আমার ঢাকা ব্যাংক অ্যাকাউন্ট, ইমেইল রহিম@? no: rahim@gmail.com, ফোন ০১৭ / 01712345678, Zürich AG 🎉 {{ক";
    let (m, r) = mask_text(&det, &mut v, t, &ctx());
    assert_eq!(r.count, 4, "{m}");
    assert!(m.contains("{{TERM_1}}") && m.contains("{{TERM_2}}") && m.contains("{{EMAIL_1}}") && m.contains("{{PHONE_1}}"));
    assert_eq!(rehydrate_text(&v, &m).0, t);
    assert_eq!(mask_text(&det, &mut v, &m, &ctx()).0, m);
    // Placeholder scanning near multibyte chars never panics and matches the spec.
    for i in 0..60 {
        let s = format!("{}{{{{{}é{}", "x".repeat(i), "A".repeat(i % 7), "ü".repeat(30));
        assert!(find_placeholders(&s).is_empty());
        let s = format!("{}{{{{EMAIL_1}}}}{}", "ব".repeat(i), "é".repeat(i));
        assert_eq!(find_placeholders(&s).len(), 1);
        assert_eq!(rehydrate_text(&v, &s).0, s.replace("{{EMAIL_1}}", "rahim@gmail.com"));
    }
}

/// The documented legend block, extracted from the doc comment of `mask::legend`.
fn documented_legend() -> String {
    let start = MASK_RS.find("/// ```text\n").expect("doc block") + "/// ```text\n".len();
    let end = start + MASK_RS[start..].find("/// ```\n").unwrap();
    MASK_RS[start..end]
        .lines()
        .map(|l| l.strip_prefix("/// ").unwrap_or(l.trim_start_matches("///")))
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn legend_is_exactly_as_documented() {
    let mut v = Vault::new();
    let k1 = v.intern(&finding("sk-proj-abcdefghijklmnopqrstuvwx1234", "API_KEY", Some("OpenAI API key")), "t", 1).unwrap();
    let k2 = v.intern(&finding("4242424242424242", "CARD", Some("Visa card ending 4242")), "t", 1).unwrap();
    let doc = documented_legend();
    assert_eq!(legend(&v, &[k2.clone(), k1.clone(), "NOPE_1".into(), k1.clone()]), doc);
    assert!(doc.starts_with(LEGEND_INTRO));
    assert_eq!(legend(&v, &[]), "");
    assert_eq!(legend(&v, &["NOPE_1".into()]), "");
}

#[test]
fn legend_sorting_fallbacks_and_sanitizing() {
    let mut v = Vault::new();
    let mut keys = Vec::new();
    for i in 0..12 {
        keys.push(v.intern(&finding(&format!("value-number-{i:02}"), "TOKEN", None), "t", 1).unwrap());
    }
    keys.push(v.add_manual("manual-secret-x", "SECRET", "Line one\nline two", 1).unwrap());
    keys.push(v.intern(&finding("abcdefgh@acme.io", "EMAIL", Some("Work email")), "t", 1).unwrap());
    keys.reverse();
    let l = legend(&v, &keys);
    let lines: Vec<&str> = l.lines().skip(LEGEND_INTRO.lines().count()).collect();
    assert_eq!(lines[0], "- {{EMAIL_1}}: Work email");
    assert_eq!(lines[1], "- {{SECRET_1}}: Line one line two");
    assert_eq!(lines[2], "- {{TOKEN_1}}: TOKEN label");
    assert_eq!(lines[3], "- {{TOKEN_2}}: TOKEN label");
    assert_eq!(lines[13], "- {{TOKEN_12}}: TOKEN label", "numeric, not lexicographic, order");
    assert_eq!(lines.len(), 14);
    // Deterministic.
    keys.rotate_left(5);
    assert_eq!(legend(&v, &keys), l);
}

#[test]
fn vault_hardening() {
    let mut v = Vault::new();
    assert!(v.intern(&finding("short", "API_KEY", None), "t", 1).is_none(), "below MIN_VALUE_LEN");
    assert_eq!(v.intern(&finding("ঢাকা১২৩৪", "SECRET", None), "t", 1).as_deref(), Some("SECRET_1"), "length counts chars");
    assert_eq!(v.intern(&finding("abcdefgh", "lower-case kind!", None), "t", 1).as_deref(), Some("SECRET_2"), "invalid kind → SECRET");
    assert_eq!(v.get("{{SECRET_2}}").unwrap().value, "abcdefgh");
    assert_eq!(v.get("{{ SECRET_2 }}").unwrap().value, "abcdefgh");
    // Clones share the index until mutated (copy-on-write), and stay independent.
    let snapshot = v.clone();
    v.intern(&finding("zzzzzzzz", "SECRET", None), "t", 2);
    assert!(snapshot.key_for_value("zzzzzzzz").is_none());
    assert_eq!(v.key_for_value("zzzzzzzz"), Some("SECRET_3"));
    assert_eq!(snapshot.find_values("abcdefgh zzzzzzzz").len(), 1);
    assert_eq!(v.find_values("abcdefgh zzzzzzzz").len(), 2);
    // Counters survive removal and clear.
    assert!(v.remove("SECRET_3"));
    assert!(!v.remove("SECRET_3"));
    v.clear();
    assert_eq!(v.intern(&finding("yyyyyyyy", "SECRET", None), "t", 3).as_deref(), Some("SECRET_4"));
    // Hand-edited files: duplicates dropped, counters repaired, bad keys ignored.
    let edited = r#"{"entries":[
        {"key":"API_KEY_7","value":"dup-value-1","kind":"API_KEY","label":"x","category":"secret","hint":null,"source":"manual","created":1,"lastUsed":1,"hits":1},
        {"key":"API_KEY_8","value":"dup-value-1","kind":"API_KEY","label":"x","category":"secret","hint":null,"source":"manual","created":1,"lastUsed":1,"hits":1},
        {"key":"not a key","value":"other-value","kind":"API_KEY","label":"x","category":"secret","hint":null,"source":"manual","created":1,"lastUsed":1,"hits":1}
    ],"counters":{}}"#;
    let mut e = Vault::from_json(edited).unwrap();
    assert_eq!(e.len(), 1);
    assert_eq!(e.intern(&finding("fresh-value", "API_KEY", None), "t", 1).as_deref(), Some("API_KEY_8"));
    assert!(Vault::from_json("not json").is_err());
    // Merge keeps existing keys and remaps the other vault's.
    let mut a = Vault::new();
    a.intern(&finding("shared-value", "TOKEN", None), "t", 1);
    let mut b = Vault::new();
    b.intern(&finding("only-in-b-1", "TOKEN", None), "t", 1);
    b.intern(&finding("shared-value", "TOKEN", None), "t", 1);
    let remap = a.merge(&b, 5);
    assert_eq!(remap, vec![("TOKEN_1".to_string(), "TOKEN_2".to_string()), ("TOKEN_2".to_string(), "TOKEN_1".to_string())]);
    // Views never contain the value.
    for view in a.views() {
        let value = &a.get(&view.key).unwrap().value;
        assert!(!serde_json::to_string(&view).unwrap().contains(value.as_str()));
    }
}

#[test]
fn find_values_offsets_are_char_boundaries() {
    let mut v = Vault::new();
    v.intern(&finding("পাসওয়ার্ড১", "PASSWORD", None), "t", 1);
    v.intern(&finding("naïve-sëcret", "SECRET", None), "t", 1);
    let t = "x পাসওয়ার্ড১ y naïve-sëcret z পাসওয়ার্ড১";
    let ms = v.find_values(t);
    assert_eq!(ms.len(), 3);
    for m in ms {
        assert!(t.is_char_boundary(m.start) && t.is_char_boundary(m.end));
    }
    assert_eq!(rehydrate_text(&v, &mask_known(&v, t).0).0, t);
}
