//! Streaming rehydration: for every split of many strings,
//! `concat(push(c)) + finish() == rehydrate_text(concat(c))`.

use zuko_core::detect::{Category, Finding};
use zuko_core::mask::rehydrate_text;
use zuko_core::placeholder::MAX_LEN;
use zuko_core::stream::StreamRehydrator;
use zuko_core::vault::Vault;

fn vault() -> Vault {
    let mut v = Vault::new();
    let mut add = |value: &str, kind: &str| {
        v.intern(
            &Finding {
                start: 0,
                end: value.len(),
                value: value.into(),
                kind: kind.into(),
                rule: "t".into(),
                label: "Test".into(),
                category: Category::Secret,
                hint: None,
                confidence: 1.0,
            },
            "t",
            0,
        )
    };
    assert!(add("sk-ünïcode\"\\value{}", "API_KEY").is_some()); // API_KEY_1: quotes, backslash, braces
    assert!(add("first@acme.io", "EMAIL").is_some()); // EMAIL_1
    assert!(add("রহিম@acme.io", "EMAIL").is_some()); // EMAIL_2
    assert!(add("{{API_KEY_1}} lookalike", "SECRET").is_none()); // refused: contains a placeholder
    v
}

/// Deterministic pseudo-random generator (no deps).
struct Lcg(u64);
impl Lcg {
    fn next(&mut self) -> usize {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (self.0 >> 33) as usize
    }
}

const FRAGMENTS: &[&str] = &[
    "{", "}", "{{", "}}", " ", "x", "_", "1", "é", "ঢাকা", "🎉", "\n", "API", "API_KEY_1", "{{API_KEY_1}}",
    "{{ API_KEY_1 }}", "{{  EMAIL_2  }}", "{{EMAIL_1}}", "{{EMAIL_2}}", "{{API_KEY_10}}", "{{NOPE_1}}", "{{{",
    "{{API_", "KEY_1}}", "{{EMAIL_", "2}", "{{ ", "{{#each items}}", "{{ color: 'red' }}", "{{API_KEY_01}}",
    "{{api_key_1}}", "}}}", "{{EMAIL_1}", "{{EMAIL_1}}}",
];

fn corpus() -> Vec<String> {
    let mut out: Vec<String> = vec![
        String::new(),
        "plain text, no braces".into(),
        "a {{API_KEY_1}} b {{ API_KEY_1 }} {{NOPE_1}} {{API_KEY_1".into(),
        "{{{{API_KEY_1}}}}".into(),
        "ends with {".into(),
        "ends with {{".into(),
        "ends with {{EMAIL_2}".into(),
        format!("{{{{{}}}}}", "A".repeat(60)),
        format!("long spaced {{{{{}EMAIL_1}}}}", " ".repeat(40)),
        "JSX style={{ color: 'red' }} and {{#if x}}{{EMAIL_1}}{{/if}}".into(),
    ];
    let mut rng = Lcg(0x5eed);
    for _ in 0..400 {
        let n = 1 + rng.next() % 12;
        let s: String = (0..n).map(|_| FRAGMENTS[rng.next() % FRAGMENTS.len()]).collect();
        out.push(s);
    }
    out
}

fn run(v: &Vault, chunks: &[&str]) -> (String, usize) {
    let mut r = StreamRehydrator::new();
    let mut got = String::new();
    for c in chunks {
        got.push_str(&r.push(v, c));
    }
    got.push_str(&r.finish(v));
    (got, r.keys().len())
}

fn boundaries(s: &str) -> Vec<usize> {
    (0..=s.len()).filter(|&i| s.is_char_boundary(i)).collect()
}

#[test]
fn every_two_way_split_of_many_strings() {
    let v = vault();
    let mut checks = 0;
    for s in corpus() {
        let (want, keys) = rehydrate_text(&v, &s);
        for i in boundaries(&s) {
            let (got, n) = run(&v, &[&s[..i], &s[i..]]);
            assert_eq!(got, want, "split {i} of {s:?}");
            assert_eq!(n, keys.len(), "keys for split {i} of {s:?}");
            checks += 1;
        }
    }
    assert!(checks > 5000, "{checks}");
}

#[test]
fn every_three_way_split_of_short_strings() {
    let v = vault();
    for s in corpus().into_iter().filter(|s| s.len() <= 48) {
        let want = rehydrate_text(&v, &s).0;
        let b = boundaries(&s);
        for &i in &b {
            for &j in b.iter().filter(|&&j| j >= i) {
                let (got, _) = run(&v, &[&s[..i], &s[i..j], &s[j..]]);
                assert_eq!(got, want, "splits {i},{j} of {s:?}");
            }
        }
    }
}

#[test]
fn char_by_char_and_random_chunkings() {
    let v = vault();
    let mut rng = Lcg(42);
    for s in corpus() {
        let want = rehydrate_text(&v, &s).0;
        // One char at a time.
        let chars: Vec<String> = s.chars().map(String::from).collect();
        let refs: Vec<&str> = chars.iter().map(String::as_str).collect();
        assert_eq!(run(&v, &refs).0, want, "char-by-char {s:?}");
        // Random chunk sizes, including empty chunks.
        for _ in 0..5 {
            let b = boundaries(&s);
            let mut cuts: Vec<usize> = (0..rng.next() % 6).map(|_| b[rng.next() % b.len()]).collect();
            cuts.push(0);
            cuts.push(s.len());
            cuts.sort();
            let parts: Vec<&str> = cuts.windows(2).map(|w| &s[w[0]..w[1]]).collect();
            assert_eq!(run(&v, &parts).0, want, "chunks {cuts:?} of {s:?}");
        }
    }
}

#[test]
fn holds_back_only_possible_placeholder_prefixes() {
    let v = vault();
    let mut r = StreamRehydrator::new();
    assert_eq!(r.push(&v, "hello {{API_"), "hello ");
    assert_eq!(r.push(&v, "KEY_1}} world {"), "sk-ünïcode\"\\value{} world ");
    assert_eq!(r.push(&v, "x"), "{x");
    assert_eq!(r.push(&v, "{{#each}} and {{ color }}"), "{{#each}} and {{ color }}");
    assert_eq!(r.push(&v, "{{EMAIL_2}"), "");
    assert_eq!(r.push(&v, "}"), "রহিম@acme.io");
    assert_eq!(r.finish(&v), "");
    assert_eq!(r.keys(), &["API_KEY_1".to_string(), "EMAIL_2".to_string()]);
    // Never more than MAX_LEN bytes held.
    let mut r = StreamRehydrator::new();
    let mut emitted = 0;
    let s = format!("{{{{{}", "A".repeat(200));
    for c in s.chars() {
        emitted += r.push(&v, &c.to_string()).len();
        assert!(s[..emitted].len() + MAX_LEN >= s.len().min(emitted + MAX_LEN));
    }
    emitted += r.finish(&v).len();
    assert_eq!(emitted, s.len());
    // An unfinished placeholder at the end is flushed verbatim.
    let mut r = StreamRehydrator::new();
    assert_eq!(r.push(&v, "tail {{EMAIL_1"), "tail ");
    assert_eq!(r.finish(&v), "{{EMAIL_1");
}
