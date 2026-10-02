//! Incremental rehydration for text that arrives in chunks (SSE `text_delta`s, a
//! browser stream). A placeholder may be split across chunks — `"{{API_"` + `"KEY_1}} …"`
//! — so the rehydrator holds back any tail that could still become a placeholder
//! (at most [`crate::placeholder::MAX_LEN`] bytes) and emits it once it is resolved.
//!
//! Invariant (tested for every split position of every test string):
//! `concat(push(c) for c in chunks) + finish() == rehydrate_text(concat(chunks))`.

use crate::mask::rehydrate_text;
use crate::placeholder;
use crate::vault::Vault;

#[derive(Clone, Debug, Default)]
pub struct StreamRehydrator {
    pending: String,
    /// Keys rehydrated so far (with repeats).
    keys: Vec<String>,
}

impl StreamRehydrator {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feeds a chunk; returns the text that is now safe to emit (possibly empty).
    pub fn push(&mut self, vault: &Vault, chunk: &str) -> String {
        self.pending.push_str(chunk);
        let hold = placeholder::holdback_len(&self.pending);
        let cut = self.pending.len() - hold;
        let ready: String = self.pending[..cut].to_string();
        self.pending.drain(..cut);
        let (out, keys) = rehydrate_text(vault, &ready);
        self.keys.extend(keys);
        out
    }

    /// Flushes whatever is held back, rehydrated if it completes a placeholder.
    pub fn finish(&mut self, vault: &Vault) -> String {
        let rest = std::mem::take(&mut self.pending);
        let (out, keys) = rehydrate_text(vault, &rest);
        self.keys.extend(keys);
        out
    }

    /// Keys rehydrated so far (with repeats).
    pub fn keys(&self) -> &[String] {
        &self.keys
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detect::{Category, Finding};

    #[test]
    fn every_split_position_matches_whole_text() {
        let mut v = Vault::new();
        v.intern(
            &Finding {
                start: 0,
                end: 9,
                value: "s3cr3t-value".into(),
                kind: "API_KEY".into(),
                rule: "t".into(),
                label: "Test".into(),
                category: Category::Secret,
                hint: None,
                confidence: 1.0,
            },
            "t",
            0,
        );
        let text = "a {{API_KEY_1}} b {{ API_KEY_1 }} {{NOPE_1}} {{API_KEY_1";
        let want = rehydrate_text(&v, text).0;
        for i in 0..=text.len() {
            if !text.is_char_boundary(i) {
                continue;
            }
            let mut r = StreamRehydrator::new();
            let mut got = r.push(&v, &text[..i]);
            got.push_str(&r.push(&v, &text[i..]));
            got.push_str(&r.finish(&v));
            assert_eq!(got, want, "split at {i}");
        }
    }
}
