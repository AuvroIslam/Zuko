//! Incremental rehydration for text that arrives in chunks (SSE `text_delta`s, a
//! browser stream). A placeholder may be split across chunks — `"{{API_"` + `"KEY_1}} …"`
//! — so the rehydrator holds back any tail that could still become a placeholder
//! (at most [`crate::placeholder::MAX_LEN`] bytes) and emits it once it is resolved.
//!
//! Invariant (tested for every split position of every test string):
//! `concat(push(c) for c in chunks) + finish() == rehydrate_text(concat(chunks))`.

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
        let _ = (vault, chunk, &self.pending);
        todo!()
    }

    /// Flushes whatever is held back, rehydrated if it completes a placeholder.
    pub fn finish(&mut self, vault: &Vault) -> String {
        let _ = vault;
        todo!()
    }

    /// Keys rehydrated so far (with repeats).
    pub fn keys(&self) -> &[String] {
        &self.keys
    }
}
