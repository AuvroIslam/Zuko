//! The placeholder format: `{{KIND_N}}`, e.g. `{{API_KEY_1}}`, `{{EMAIL_12}}`.
//!
//! Chosen because models copy it verbatim, it survives markdown (no `_x_` / `__x__`
//! emphasis, no `<tag>` eaten as HTML), it is JSON-safe (no quotes or backslashes) and
//! it reads as "a value goes here". `KIND` is `[A-Z][A-Z0-9_]*` and never ends with `_`;
//! `N` is a decimal number ≥ 1.

/// Opening delimiter.
pub const OPEN: &str = "{{";
/// Closing delimiter.
pub const CLOSE: &str = "}}";
/// Longest placeholder we ever produce or recognize, delimiters included. Streaming
/// rehydration holds back at most this many bytes.
pub const MAX_LEN: usize = 48;

/// `("API_KEY", 1)` → `"API_KEY_1"`.
pub fn key(kind: &str, n: u32) -> String {
    format!("{kind}_{n}")
}

/// `("API_KEY", 1)` → `"{{API_KEY_1}}"`.
pub fn format(kind: &str, n: u32) -> String {
    format!("{OPEN}{kind}_{n}{CLOSE}")
}

/// `"API_KEY_1"` → `"{{API_KEY_1}}"`.
pub fn wrap(key: &str) -> String {
    format!("{OPEN}{key}{CLOSE}")
}

/// Splits a key (`"API_KEY_1"`, with or without braces) into `("API_KEY", 1)`.
pub fn parse(s: &str) -> Option<(String, u32)> {
    let inner = s
        .strip_prefix(OPEN)
        .and_then(|r| r.strip_suffix(CLOSE))
        .unwrap_or(s);
    if inner.len() + OPEN.len() + CLOSE.len() > MAX_LEN {
        return None;
    }
    let (kind, n) = inner.rsplit_once('_')?;
    if !valid_kind(kind) || n.is_empty() || !n.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    if n.starts_with('0') {
        return None;
    }
    let n: u32 = n.parse().ok()?;
    Some((kind.to_string(), n))
}

/// True for `[A-Z][A-Z0-9_]*` not ending in `_`.
pub fn valid_kind(kind: &str) -> bool {
    let b = kind.as_bytes();
    !b.is_empty()
        && b[0].is_ascii_uppercase()
        && b.iter().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || *c == b'_')
        && *b.last().unwrap() != b'_'
}

/// Every well-formed placeholder in `text`, as `(byte_start, byte_end, key)` with
/// `key` lacking the braces. Non-overlapping, in order. Placeholders that tolerate
/// inner spaces (`{{ API_KEY_1 }}`) are reported too, since models sometimes add them.
pub fn find_all(text: &str) -> Vec<(usize, usize, String)> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(off) = text[i..].find(OPEN) {
        let start = i + off;
        // Scan to the closing delimiter, bounded by MAX_LEN.
        let limit = (start + MAX_LEN + 2).min(bytes.len());
        let mut matched = None;
        if let Some(rel) = text[start + 2..limit].find(CLOSE) {
            let inner = &text[start + 2..start + 2 + rel];
            let trimmed = inner.trim_matches(' ');
            if parse(trimmed).is_some() {
                matched = Some((start, start + 2 + rel + 2, trimmed.to_string()));
            }
        }
        match matched {
            Some(m) => {
                i = m.1;
                out.push(m);
            }
            None => i = start + 1,
        }
        if i >= bytes.len() {
            break;
        }
    }
    out
}

/// True if `tail` (the end of a chunk) could be the beginning of a placeholder that the
/// next chunk completes: `"{"`, `"{{"`, `"{{API_K"`, `"{{ API_KEY_1 "`, `"{{API_KEY_1}"`.
/// Used by streaming rehydration to decide how much to hold back.
pub fn could_be_prefix(tail: &str) -> bool {
    if tail.is_empty() || tail.len() > MAX_LEN {
        return false;
    }
    if tail == "{" {
        return true;
    }
    let Some(rest) = tail.strip_prefix(OPEN) else {
        return false;
    };
    // A complete placeholder is not a prefix of anything longer that matters.
    if rest.contains(CLOSE) {
        return false;
    }
    let rest = rest.strip_suffix('}').unwrap_or(rest);
    let body = rest.trim_start_matches(' ');
    // Allow a trailing space run (the `{{ KEY }}` variant) after a complete key.
    let core = body.trim_end_matches(' ');
    if core.len() != body.len() && !core.is_empty() {
        return parse(core).is_some();
    }
    core.bytes()
        .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == b'_')
        && core.bytes().next().map_or(true, |c| c.is_ascii_uppercase())
}

/// Length of the longest suffix of `text` that [`could_be_prefix`] accepts (0 if none).
/// Always on a char boundary.
pub fn holdback_len(text: &str) -> usize {
    let start = text.len().saturating_sub(MAX_LEN);
    // Candidates must begin with '{'; check each '{' in the window from the left so the
    // longest viable suffix wins.
    let mut idx = start;
    while idx < text.len() {
        if !text.is_char_boundary(idx) {
            idx += 1;
            continue;
        }
        if text.as_bytes()[idx] == b'{' && could_be_prefix(&text[idx..]) {
            return text.len() - idx;
        }
        idx += 1;
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_and_parse_round_trip() {
        assert_eq!(format("API_KEY", 1), "{{API_KEY_1}}");
        assert_eq!(parse("{{API_KEY_1}}"), Some(("API_KEY".into(), 1)));
        assert_eq!(parse("EMAIL_12"), Some(("EMAIL".into(), 12)));
        assert_eq!(parse("{{api_key_1}}"), None);
        assert_eq!(parse("{{API_KEY_}}"), None);
        assert_eq!(parse("{{API_KEY_01}}"), None);
        assert_eq!(parse("{{_KEY_1}}"), None);
        assert_eq!(parse("{{KEY}}"), None);
    }

    #[test]
    fn finds_placeholders_and_spaced_variants() {
        let t = "a {{API_KEY_1}} b {{ EMAIL_2 }} c {{nope}} {{X_1}}";
        let got: Vec<_> = find_all(t).into_iter().map(|(_, _, k)| k).collect();
        assert_eq!(got, vec!["API_KEY_1", "EMAIL_2", "X_1"]);
        let (s, e, _) = find_all(t)[0];
        assert_eq!(&t[s..e], "{{API_KEY_1}}");
    }

    #[test]
    fn prefixes() {
        for p in ["{", "{{", "{{A", "{{API_KEY_", "{{API_KEY_1", "{{API_KEY_1}", "{{ API", "{{ API_KEY_1 "] {
            assert!(could_be_prefix(p), "{p}");
        }
        for p in ["", "x", "{a", "{{a", "{{API_KEY_1}}", "{{1"] {
            assert!(!could_be_prefix(p), "{p}");
        }
        assert_eq!(holdback_len("hello {{API_"), 6);
        assert_eq!(holdback_len("hello {"), 1);
        assert_eq!(holdback_len("hello {{API_KEY_1}} done"), 0);
        assert_eq!(holdback_len("é{{"), 2);
    }
}
