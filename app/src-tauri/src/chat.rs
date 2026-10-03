// The island chat: a multi-turn conversation that always goes through Zuko's masker,
// answered by the provider picked in Settings → Chat:
// * Claude (Anthropic Messages API, with web search): masked text goes to Anthropic.
// * OpenAI (Chat Completions API): masked text goes to OpenAI.
// * Ollama on this machine (`/api/chat` at the local AI endpoint, loopback only):
//   nothing leaves the PC.
//
// Everything happens here rather than in the island: API keys never leave the OS
// keyring, and file bytes never cross the IPC boundary.
//
// Privacy rules for every turn, whatever the provider (they all run here, before any
// provider code sees the turn):
// * The user's message, the window context (title and URL) and any attached file are
//   masked with the engine (source "chat") before they join the history. The history
//   only ever holds the masked form, so every request replays exactly what the model
//   has already seen.
// * Text and code files are inlined as masked text. A PDF is never sent as a base64
//   document block: its text layer is extracted and masked like any other text (a
//   PDF without one is refused, since a picture of text cannot be checked). Images are
//   refused for the same reason.
// * Files must come from the inbox (where `ingest_file` copies dropped files), so the
//   webview cannot ask for an arbitrary path.
// * When placeholders appear anywhere in the conversation, `mask::legend` is appended to
//   the system prompt so the model knows what each one stands for, without the values.
// * The reply is rehydrated locally before it is returned to the island.
// * A conversation belongs to one provider. Switching providers starts a new one, so
//   what was said to the local model is never replayed to a cloud provider later.
// * Nothing here is logged with message content, and no key is ever logged.
// * Local AI (optional, `policy.localAi.deepScanPrompts`): the chat is interactive, so
//   it waits (bounded by timeoutMs) for a deep scan of the message and the window
//   title before masking. Values the model finds (and Zuko verifies) are interned and
//   masked with everything else; a timeout or a bad answer changes nothing.
//
// The provider-specific parts are small and live in chat/: how the masked history
// becomes a request body, how a response becomes assistant text, and the HTTP call.
// The history is kept in one neutral shape (Anthropic-style content blocks, which is
// what Claude needs to replay its web-search blocks); OpenAI and Ollama get the text
// of each message.

mod anthropic;
mod ollama;
mod openai;

use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{json, Value};
use zuko_core::localai::LocalAiConfig;
use zuko_core::mask::{self, MaskCtx, MaskReport};

use crate::engine::{self, Engine};
use crate::{files, sanitize, secrets};

/// A file's text is inlined up to this many characters; a bigger one is refused.
const MAX_INLINE_TEXT: usize = 200_000;

const PERSONA: &str = "You are Zuko, a friendly privacy guardian who lives at the top of the user's screen. \
You keep the user's secrets and personal data on their machine and still help with absolutely anything — research, coding, finding places, recommendations, tasks, questions.";

const STYLE: &str = "Respond in the user's language. Be thorough and complete — use as much detail as the task requires. \
No markdown formatting (no **, no ##, no bullet dashes). Use plain text with line breaks.";

// ── Providers ─────────────────────────────────────────────────────────────────

/// Who answers the island chat. Stored in settings.json as "anthropic" (the default,
/// the original behaviour), "openai" or "ollama".
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    #[default]
    Anthropic,
    OpenAi,
    Ollama,
}

impl<'de> Deserialize<'de> for Provider {
    /// Lenient, and failing safe: a name this build does not know (one written by a
    /// newer build, or a typo) reads as Ollama, the provider that keeps everything on
    /// this machine. Failing the parse instead would reset every other preference in
    /// settings.json, and falling back to a cloud provider would send text somewhere
    /// the user did not choose.
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let v = Value::deserialize(d)?;
        Ok(v.as_str().and_then(Provider::parse).unwrap_or(Provider::Ollama))
    }
}

impl Provider {
    /// The settings/command spelling, case-insensitive.
    pub fn parse(name: &str) -> Option<Provider> {
        match name.trim().to_ascii_lowercase().as_str() {
            "anthropic" => Some(Provider::Anthropic),
            "openai" => Some(Provider::OpenAi),
            "ollama" => Some(Provider::Ollama),
            _ => None,
        }
    }

    /// The name the user reads.
    pub fn label(self) -> &'static str {
        match self {
            Provider::Anthropic => "Claude",
            Provider::OpenAi => "OpenAI",
            Provider::Ollama => "Ollama",
        }
    }

    /// True when the (masked) conversation leaves this machine.
    pub fn is_cloud(self) -> bool {
        self != Provider::Ollama
    }

    pub fn default_model(self) -> &'static str {
        match self {
            Provider::Anthropic => anthropic::DEFAULT_MODEL,
            Provider::OpenAi => openai::DEFAULT_MODEL,
            Provider::Ollama => ollama::DEFAULT_MODEL,
        }
    }

    /// The keyring entry holding this provider's API key (Ollama needs none).
    pub fn key_name(self) -> Option<&'static str> {
        match self {
            Provider::Anthropic => Some("anthropic-api-key"),
            Provider::OpenAi => Some("openai-api-key"),
            Provider::Ollama => None,
        }
    }

    /// Zuko's persona. Only Claude gets a web search tool, so only Claude is told it has one.
    fn persona(self) -> String {
        let tools = match self {
            Provider::Anthropic => "You have web search access.",
            Provider::OpenAi | Provider::Ollama => {
                "You have no web access: when a question needs current information, say so instead of guessing."
            }
        };
        format!("{PERSONA} {tools} {STYLE}")
    }

    /// The request body for this provider, from the (already masked) history.
    fn request(self, model: &str, system: &str, history: &[Value]) -> Value {
        match self {
            Provider::Anthropic => anthropic::request(model, system, history),
            Provider::OpenAi => openai::request(model, system, history),
            Provider::Ollama => ollama::request(model, system, history),
        }
    }

    /// The assistant's content blocks (neutral shape) from this provider's response,
    /// or the message to show when there is no usable answer (a refusal, an empty one).
    fn reply(self, response: &Value) -> Result<Vec<Value>, String> {
        match self {
            Provider::Anthropic => anthropic::reply(response),
            Provider::OpenAi => openai::reply(response),
            Provider::Ollama => ollama::reply(response),
        }
    }
}

/// Which provider and model answer a turn.
#[derive(Clone, Debug, PartialEq)]
pub struct Target {
    pub provider: Provider,
    pub model: String,
}

/// Where the cloud providers live. Tests point these at local mock servers.
struct Upstream<'a> {
    openai: &'a str,
}

const UPSTREAM: Upstream<'static> = Upstream { openai: openai::API_BASE };

// ── Conversation state ────────────────────────────────────────────────────────

#[derive(Default)]
pub struct Chat {
    inner: Mutex<History>,
}

#[derive(Default)]
struct History {
    /// The provider this conversation is with (None until the first turn).
    provider: Option<Provider>,
    /// Full multi-turn history in the form the model saw it (masked), in the neutral
    /// shape: `{role, content: [blocks]}`, Claude's tool_use / tool_result blocks included.
    messages: Vec<Value>,
}

impl Chat {
    pub fn reset(&self) {
        let mut h = self.inner.lock().unwrap();
        h.messages.clear();
        h.provider = None;
    }

    /// Starts a turn with `provider`. A conversation held with another provider is
    /// dropped first (see the privacy rules above).
    fn begin(&self, provider: Provider) {
        let mut h = self.inner.lock().unwrap();
        if h.provider != Some(provider) {
            h.messages.clear();
            h.provider = Some(provider);
        }
    }

    fn is_empty(&self) -> bool {
        self.inner.lock().unwrap().messages.is_empty()
    }

    fn push(&self, message: Value) {
        self.inner.lock().unwrap().messages.push(message);
    }

    fn pop(&self) {
        self.inner.lock().unwrap().messages.pop();
    }

    fn snapshot(&self) -> Vec<Value> {
        self.inner.lock().unwrap().messages.clone()
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ChatContext {
    File { name: String, path: String },
    Window { app_name: String, title: String, url: Option<String> },
}

/// A value Zuko masked in the outgoing message (never the value itself).
#[derive(Debug, Clone, Serialize)]
pub struct MaskedKey {
    pub key: String,
    pub label: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatReply {
    /// The assistant's text with placeholders restored.
    pub text: String,
    /// What was masked before this turn left the machine.
    pub masked: Vec<MaskedKey>,
    /// The engine's report for the outgoing message, for the privacy notice.
    #[serde(skip)]
    pub report: MaskReport,
}

/// Debug output never shows the text: it holds the restored, real values.
impl std::fmt::Debug for ChatReply {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChatReply")
            .field("text", &format_args!("<{} chars>", self.text.chars().count()))
            .field("masked", &self.masked)
            .finish()
    }
}

// ── One turn ──────────────────────────────────────────────────────────────────

/// One chat turn with `target`. Returns the assistant's text, or a message the island
/// shows in the note view.
pub async fn send(
    engine: &Engine,
    chat: &Chat,
    target: &Target,
    query: String,
    context: Option<ChatContext>,
) -> Result<ChatReply, String> {
    send_to(engine, chat, target, query, context, &UPSTREAM).await
}

/// [`send`] against explicit upstream URLs.
async fn send_to(
    engine: &Engine,
    chat: &Chat,
    target: &Target,
    query: String,
    context: Option<ChatContext>,
    upstream: &Upstream<'_>,
) -> Result<ChatReply, String> {
    match target.provider {
        Provider::Anthropic => {
            let key = api_key(Provider::Anthropic)?;
            send_via(engine, chat, target, query, context, |body| async move { anthropic::call(&key, &body).await }).await
        }
        Provider::OpenAi => {
            let key = api_key(Provider::OpenAi)?;
            let base = upstream.openai.to_string();
            send_via(engine, chat, target, query, context, |body| async move { openai::call(&base, &key, &body).await }).await
        }
        Provider::Ollama => {
            // The local AI's endpoint, whether or not the local AI scans are on: the chat
            // is a separate use of the same Ollama. The client validates it as loopback
            // before every call.
            let endpoint = engine.policy().local_ai.endpoint.clone();
            let model = target.model.clone();
            let ai = engine.localai();
            send_via(engine, chat, target, query, context, |body| async move { ollama::call(ai, &endpoint, &model, body).await })
                .await
        }
    }
}

/// The stored key for a cloud provider, or a message that says where to add it.
fn api_key(provider: Provider) -> Result<String, String> {
    provider
        .key_name()
        .and_then(secrets::get)
        .ok_or_else(|| format!("{} API key missing. Add it in Settings → Chat.", provider.label()))
}

/// [`send`] with the network call injected, so tests can see exactly what would go out.
async fn send_via<F, Fut>(
    engine: &Engine,
    chat: &Chat,
    target: &Target,
    query: String,
    context: Option<ChatContext>,
    transport: F,
) -> Result<ChatReply, String>
where
    F: FnOnce(Value) -> Fut,
    Fut: Future<Output = Result<Value, String>>,
{
    let provider = target.provider;
    chat.begin(provider);
    if engine.policy().local_ai.scans_prompts() {
        let mut text = query.clone();
        if let Some(ChatContext::Window { title, .. }) = &context {
            text = format!("{title}\n{text}");
        }
        // Stricter-only: this can only add vault entries before the masking below.
        let _ = crate::localai::learn(engine, &text, false).await;
    }
    let turn = prepare_turn(engine, chat.is_empty(), &query, context.as_ref())?;
    chat.push(json!({ "role": "user", "content": turn.content }));
    let history = chat.snapshot();

    let system = system_prompt(engine, provider, &history);
    let body = provider.request(&target.model, &system, &history);

    let response = match transport(body).await {
        Ok(v) => v,
        Err(err) => {
            chat.pop(); // keep the history consistent with what the model saw
            return Err(err);
        }
    };
    let blocks = match provider.reply(&response) {
        Ok(blocks) => blocks,
        Err(err) => {
            chat.pop();
            return Err(err);
        }
    };
    let text = text_of(&blocks);
    if text.is_empty() {
        chat.pop();
        return Err("No response text.".into());
    }

    // Store the whole content — Claude's tool_use / tool_result blocks included — so
    // the next turn has the right context. It stays as the model wrote it (placeholders).
    chat.push(json!({ "role": "assistant", "content": blocks }));

    // Real values come back only here, on this machine.
    let (text, _restored) = engine.with_vault(|v| mask::rehydrate_text(v, &text));
    Ok(ChatReply { text, masked: turn.masked, report: turn.report })
}

/// The text blocks of an assistant message, joined.
fn text_of(blocks: &[Value]) -> String {
    blocks
        .iter()
        .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
        .filter_map(|b| b.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

/// A history message as `{role, content: "<its text>"}`, for providers that take
/// plain text (OpenAI, Ollama). Non-text blocks (Claude's web search, if the
/// conversation ever held any) are left out; everything here is already masked.
fn plain_message(message: &Value) -> Value {
    let role = message.get("role").and_then(Value::as_str).unwrap_or("user");
    let content = match message.get("content") {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(blocks)) => blocks
            .iter()
            .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
            .filter_map(|b| b.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n\n"),
        _ => String::new(),
    };
    json!({ "role": role, "content": content })
}

/// The persona, plus the legend of every placeholder in the conversation so far.
fn system_prompt(engine: &Engine, provider: Provider, history: &[Value]) -> String {
    let persona = provider.persona();
    let legend = engine.with_vault(|v| {
        let keys = mask::keys_in_json(v, &Value::Array(history.to_vec()));
        mask::legend(v, &keys)
    });
    if legend.is_empty() {
        persona
    } else {
        format!("{persona}\n\n{legend}")
    }
}

/// The masked content of one user turn.
struct Turn {
    content: Vec<Value>,
    masked: Vec<MaskedKey>,
    report: MaskReport,
}

fn prepare_turn(engine: &Engine, first_message: bool, query: &str, context: Option<&ChatContext>) -> Result<Turn, String> {
    let det = engine.detector();
    let ctx = MaskCtx { source: "chat".into(), now: engine::now() };
    let mut report = MaskReport::default();
    let mut mask_text = |text: &str| -> String {
        engine.with_vault(|v| {
            let (out, r) = mask::mask_text(&det, v, text, &ctx);
            report.absorb(r);
            out
        })
    };

    let mut content: Vec<Value> = Vec::new();

    // File / window context rides along with the first message only.
    if first_message {
        match context {
            Some(ChatContext::File { name, path }) => {
                let (kind, body) = file_text(path)?;
                content.push(json!({ "type": "text", "text": format!("File contents ({kind}):\n{}", mask_text(&body)) }));
                content.push(json!({ "type": "text", "text": format!("File: {}", mask_text(name)) }));
            }
            Some(ChatContext::Window { app_name, title, url }) => {
                let mut text = format!("Context — App: {app_name}, Window: {title}");
                if let Some(url) = url {
                    text.push_str(&format!(", URL: {url}"));
                }
                content.push(json!({ "type": "text", "text": mask_text(&text) }));
            }
            None => {}
        }
    }
    content.push(json!({ "type": "text", "text": mask_text(query) }));

    if report.count > 0 {
        engine.persist_vault();
    }
    let masked = engine.with_vault(|v| {
        report
            .keys
            .iter()
            .map(|k| MaskedKey { key: k.clone(), label: v.get(k).map(|e| e.label.clone()).unwrap_or_default() })
            .collect()
    });
    Ok(Turn { content, masked, report })
}

/// The text of a dropped file, by its inbox path: ("text" | "markdown" | "code" |
/// "pdf", contents). Refuses what cannot be masked or is too big.
fn file_text(path: &str) -> Result<(String, String), String> {
    let path = inbox_file(path)?;
    let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    if matches!(ext.as_str(), "jpg" | "jpeg" | "png" | "gif" | "webp" | "bmp" | "heic" | "tif" | "tiff") {
        return Err("Zuko can't check pictures for secrets yet, so images aren't sent to the chat. Paste the text instead.".into());
    }
    let extracted = sanitize::extract(&path)?;
    if extracted.kind == sanitize::Kind::Pdf && extracted.warnings.iter().any(|w| w.starts_with("No text layer")) {
        return Err("This PDF has no text layer (a scan?). Zuko can't check pictures of text for secrets, so it isn't sent.".into());
    }
    if extracted.body.chars().count() > MAX_INLINE_TEXT {
        return Err(format!(
            "{} is too long for the chat (limit {} characters of text). Sanitize it in Settings > Documents and paste what you need.",
            extracted.name, MAX_INLINE_TEXT
        ));
    }
    let kind = format!("{:?}", extracted.kind).to_lowercase();
    Ok((kind, extracted.body))
}

/// `path` canonicalized, if it lies inside the inbox.
fn inbox_file(path: &str) -> Result<PathBuf, String> {
    const MSG: &str = "Drop the file onto Zuko first, so it can be checked before it is sent.";
    let inbox = std::fs::canonicalize(files::inbox_dir()).map_err(|_| MSG.to_string())?;
    let real = std::fs::canonicalize(Path::new(path)).map_err(|e| format!("Can't open the file: {e}"))?;
    if real.starts_with(&inbox) {
        Ok(real)
    } else {
        Err(MSG.into())
    }
}

// ── HTTP helpers shared by the cloud providers ────────────────────────────────

/// A client for a cloud API: bounded by `timeout`, and never following a redirect
/// (an API never needs one, and the key must only ever go to the host it is for).
fn cloud_client(timeout: Duration) -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .timeout(timeout)
        .connect_timeout(Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| format!("Couldn't start the HTTP client ({e})"))
}

/// The innermost cause of a request error ("connection refused", "dns error: …").
/// reqwest's own message is only "error sending request".
fn root_cause(e: &reqwest::Error) -> String {
    let mut cause: &dyn std::error::Error = e;
    while let Some(next) = cause.source() {
        cause = next;
    }
    cause.to_string()
}

// ── Settings window: models and status ────────────────────────────────────────

/// The model dropdown for one provider (Settings → Chat).
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatModels {
    pub provider: Provider,
    /// Models to offer, sorted. Claude: Zuko's list. OpenAI: the chat models the stored
    /// key can use (GET /v1/models). Ollama: the installed models (/api/tags).
    pub models: Vec<String>,
    /// The model to show as selected: the saved one, or for OpenAI a sensible default
    /// when the saved one is not offered any more.
    pub selected: String,
    /// Why the list could not be fetched (no key, offline, Ollama not running…).
    pub error: Option<String>,
}

/// Whether the chat can work with a provider, as far as Zuko can tell without sending
/// anything to a cloud provider.
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatStatus {
    pub provider: Provider,
    /// "Claude", "OpenAI" or "Ollama".
    pub label: String,
    pub model: String,
    /// True when the masked conversation leaves this machine.
    pub cloud: bool,
    pub ready: bool,
    /// Cloud providers: whether a key is stored (never the key). None for Ollama.
    pub key_present: Option<bool>,
    /// Ollama: the local AI endpoint the chat uses.
    pub endpoint: Option<String>,
    /// Ollama answered /api/tags.
    pub reachable: Option<bool>,
    /// Ollama has the model installed.
    pub model_present: Option<bool>,
    pub error: Option<String>,
    /// What to do about it, e.g. `ollama pull gemma3:4b`.
    pub hint: Option<String>,
}

/// The local AI config with the chat's model, for the shared Ollama client's status probe.
fn ollama_config(engine: &Engine, model: &str) -> LocalAiConfig {
    LocalAiConfig { model: model.trim().to_string(), ..engine.policy().local_ai.clone() }
}

pub async fn models(engine: &Engine, provider: Provider, saved: &str) -> ChatModels {
    models_from(engine, provider, saved, &UPSTREAM).await
}

async fn models_from(engine: &Engine, provider: Provider, saved: &str, upstream: &Upstream<'_>) -> ChatModels {
    let saved = if saved.trim().is_empty() { provider.default_model() } else { saved.trim() };
    let mut out = ChatModels { provider, selected: saved.to_string(), ..Default::default() };
    match provider {
        Provider::Anthropic => out.models = anthropic::MODELS.iter().map(|m| m.to_string()).collect(),
        Provider::OpenAi => match api_key(Provider::OpenAi) {
            Err(why) => out.error = Some(why),
            Ok(key) => match openai::list_models(upstream.openai, &key).await {
                Ok(list) => {
                    out.selected = openai::pick(&list, saved);
                    out.models = list;
                }
                Err(why) => out.error = Some(why),
            },
        },
        Provider::Ollama => {
            let s = engine.localai().status(&ollama_config(engine, saved)).await;
            out.models = s.models;
            out.models.sort();
            // A missing model is reported by the status line, with the pull command;
            // the reachable-but-missing case is not an error for the list itself.
            if !s.reachable {
                out.error = s.error;
            }
        }
    }
    out
}

pub async fn status(engine: &Engine, provider: Provider, model: &str) -> ChatStatus {
    let model = if model.trim().is_empty() { provider.default_model() } else { model.trim() };
    let mut s = ChatStatus {
        provider,
        label: provider.label().into(),
        model: model.to_string(),
        cloud: provider.is_cloud(),
        ..Default::default()
    };
    match provider {
        Provider::Anthropic | Provider::OpenAi => {
            let present = provider.key_name().is_some_and(secrets::present);
            s.key_present = Some(present);
            s.ready = present;
            if !present {
                s.error = Some(format!("No {} API key stored yet.", provider.label()));
                s.hint = Some("Paste your key below and press Save key.".into());
            }
        }
        Provider::Ollama => {
            let local = engine.localai().status(&ollama_config(engine, model)).await;
            s.endpoint = Some(local.endpoint.clone());
            s.reachable = Some(local.reachable);
            s.model_present = Some(local.model_present);
            s.ready = local.endpoint_ok && local.reachable && local.model_present;
            s.error = local.error;
            s.hint = local.hint;
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::CtxBase;
    use std::sync::Arc;
    use zuko_core::policy::Policy;
    use zuko_core::vault::Vault;

    const KEY: &str = "sk-proj-abcdefghijklmnopqrstuvwx1234";

    fn engine() -> Engine {
        Engine::with_parts(Policy::default(), Vault::new(), CtxBase::default())
    }

    fn claude() -> Target {
        Target { provider: Provider::Anthropic, model: "test-model".into() }
    }

    /// Puts `content` into the (test) inbox and returns its path.
    fn inbox_file_with(name: &str, content: &[u8]) -> String {
        let dir = files::inbox_dir();
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join(format!("{}-{name}", std::process::id()));
        std::fs::write(&p, content).unwrap();
        p.to_string_lossy().to_string()
    }

    /// Runs one Claude turn against a canned reply; returns (result, the request body that would have gone out).
    async fn turn(e: &Engine, chat: &Chat, query: &str, ctx: Option<ChatContext>, reply: Value) -> (Result<ChatReply, String>, Value) {
        turn_with(e, chat, &claude(), query, ctx, reply).await
    }

    async fn turn_with(
        e: &Engine,
        chat: &Chat,
        target: &Target,
        query: &str,
        ctx: Option<ChatContext>,
        reply: Value,
    ) -> (Result<ChatReply, String>, Value) {
        let sent = Arc::new(Mutex::new(Value::Null));
        let sent2 = sent.clone();
        let result = send_via(e, chat, target, query.into(), ctx, move |body| async move {
            *sent2.lock().unwrap() = body;
            Ok(reply)
        })
        .await;
        let body = sent.lock().unwrap().clone();
        (result, body)
    }

    fn reply(text: &str) -> Value {
        json!({ "stop_reason": "end_turn", "content": [{ "type": "text", "text": text }] })
    }

    #[tokio::test]
    async fn masks_the_message_and_restores_the_reply() {
        let e = engine();
        let chat = Chat::default();
        let (r, body) = turn(
            &e,
            &chat,
            &format!("My key is {KEY}, please put it in .env and mail bob@acme.io"),
            None,
            reply("Done: OPENAI_API_KEY={{API_KEY_1}} is in .env. I wrote to {{EMAIL_1}}."),
        )
        .await;
        let r = r.unwrap();

        // Nothing sensitive in the request, in any field.
        let wire = body.to_string();
        assert!(!wire.contains(KEY) && !wire.contains("bob@acme.io"), "leaked: {wire}");
        assert!(wire.contains("{{API_KEY_1}}") && wire.contains("{{EMAIL_1}}"));
        // The persona is Zuko, and the legend explains the placeholders without values.
        let system = body["system"].as_str().unwrap();
        assert!(system.contains("You are Zuko") && system.contains("privacy guardian"));
        assert!(system.contains("web search access"));
        assert!(system.contains("- {{API_KEY_1}}:") && system.contains("- {{EMAIL_1}}:"), "{system}");
        assert!(!system.contains(KEY));
        // Claude keeps its web search tool and the server-side fallback.
        assert_eq!(body["tools"][0]["name"], "web_search");
        assert_eq!(body["model"], "test-model");

        // The island gets real values back, plus what was masked.
        assert_eq!(r.text, format!("Done: OPENAI_API_KEY={KEY} is in .env. I wrote to bob@acme.io."));
        assert_eq!(r.masked.len(), 2);
        assert_eq!(r.masked[0].key, "API_KEY_1");
        assert!(!r.masked[0].label.is_empty());
        assert_eq!(r.report.count, 2);
        assert_eq!(r.report.new_keys.len(), 2);

        // The history keeps the wire form: the next turn replays placeholders only.
        let (_, body2) = turn(&e, &chat, "and now?", None, reply("ok")).await;
        let wire2 = body2.to_string();
        assert!(!wire2.contains(KEY) && !wire2.contains("bob@acme.io"));
        assert!(wire2.contains("OPENAI_API_KEY={{API_KEY_1}} is in .env"));
        assert_eq!(body2["messages"].as_array().unwrap().len(), 3);
        let (_, body3) = turn(&e, &chat, "and now?", None, reply("ok")).await;
        assert_eq!(body3["messages"].as_array().unwrap().len(), 5);
    }

    #[tokio::test]
    async fn no_secrets_means_no_legend() {
        let e = engine();
        let chat = Chat::default();
        let (r, body) = turn(&e, &chat, "what is the capital of France?", None, reply("Paris.")).await;
        assert_eq!(r.unwrap().text, "Paris.");
        assert_eq!(body["system"].as_str().unwrap(), Provider::Anthropic.persona());
    }

    #[tokio::test]
    async fn failures_leave_the_history_as_the_model_saw_it() {
        let e = engine();
        let chat = Chat::default();
        let res = send_via(&e, &chat, &claude(), "hello".into(), None, |_| async { Err("Network error: down".to_string()) }).await;
        assert!(res.is_err());
        assert!(chat.is_empty());
        let (r, _) = turn(&e, &chat, "hi", None, json!({ "stop_reason": "refusal", "stop_details": { "explanation": "nope" } })).await;
        assert_eq!(r.err().as_deref(), Some("nope"));
        assert!(chat.is_empty());
        // An answer without text is a failure too, and is not kept.
        let (r, _) = turn(&e, &chat, "hi", None, json!({ "stop_reason": "end_turn", "content": [] })).await;
        assert_eq!(r.err().as_deref(), Some("No response text."));
        assert!(chat.is_empty());
    }

    #[tokio::test]
    async fn window_context_is_masked_and_only_sent_once() {
        let e = engine();
        let chat = Chat::default();
        let ctx = ChatContext::Window {
            app_name: "Chrome".into(),
            title: "Inbox - carol@acme.io".into(),
            url: Some(format!("https://example.test/cb?token={KEY}")),
        };
        let (r, body) = turn(&e, &chat, "summarize", Some(ctx.clone()), reply("ok")).await;
        assert!(r.is_ok());
        let wire = body.to_string();
        assert!(!wire.contains("carol@acme.io") && !wire.contains(KEY), "{wire}");
        assert!(wire.contains("Context — App: Chrome"));
        let (_, body2) = turn(&e, &chat, "more", Some(ctx), reply("ok")).await;
        assert_eq!(body2.to_string().matches("Context — App").count(), 1);
    }

    #[tokio::test]
    async fn attached_text_files_are_masked_and_pdfs_become_text() {
        let e = engine();
        let chat = Chat::default();
        let path = inbox_file_with("notes.txt", format!("db password in config: key={KEY}\nowner dave@acme.io").as_bytes());
        let ctx = ChatContext::File { name: "notes.txt".into(), path };
        let (r, body) = turn(&e, &chat, "review this", Some(ctx), reply("fine")).await;
        assert!(r.is_ok(), "{:?}", r.err());
        let wire = body.to_string();
        assert!(!wire.contains(KEY) && !wire.contains("dave@acme.io"), "{wire}");
        assert!(wire.contains("File contents (text):"));

        // A PDF is sent as masked text, never as a base64 document block.
        let chat = Chat::default();
        let pdf = crate::sanitize::tests_support::tiny_pdf(&[&format!("Contract. Contact erin@acme.io, key {KEY}")]);
        let path = inbox_file_with("contract.pdf", &pdf);
        let ctx = ChatContext::File { name: "contract.pdf".into(), path };
        let (r, body) = turn(&e, &chat, "summarize", Some(ctx), reply("ok")).await;
        assert!(r.is_ok(), "{:?}", r.err());
        let wire = body.to_string();
        assert!(!wire.contains("base64") && !wire.contains("\"document\""), "{wire}");
        assert!(!wire.contains("erin@acme.io") && !wire.contains(KEY));
        assert!(wire.contains("## Page 1") && wire.contains("Contract. Contact {{EMAIL_"), "{wire}");

        // A scan has nothing to check, so it is not sent at all.
        let chat = Chat::default();
        let path = inbox_file_with("scan.pdf", &crate::sanitize::tests_support::tiny_pdf(&[""]));
        let (r, _) = turn(&e, &chat, "read it", Some(ChatContext::File { name: "scan.pdf".into(), path }), reply("ok")).await;
        assert!(r.err().unwrap().contains("no text layer"));
        assert!(chat.is_empty());
    }

    #[tokio::test]
    async fn images_outside_files_and_huge_files_are_refused() {
        let e = engine();
        let chat = Chat::default();
        let path = inbox_file_with("shot.png", b"\x89PNG....");
        let (r, _) = turn(&e, &chat, "what is this", Some(ChatContext::File { name: "shot.png".into(), path }), reply("ok")).await;
        assert!(r.err().unwrap().contains("pictures"));

        // Only the inbox is readable from the webview's side.
        let outside = std::env::temp_dir().join(format!("zuko-outside-{}.txt", std::process::id()));
        std::fs::write(&outside, "hello").unwrap();
        let ctx = ChatContext::File { name: "x.txt".into(), path: outside.to_string_lossy().to_string() };
        let (r, _) = turn(&e, &chat, "read", Some(ctx), reply("ok")).await;
        assert!(r.err().unwrap().contains("Drop the file onto Zuko"));
        let _ = std::fs::remove_file(&outside);

        let big = "a ".repeat(MAX_INLINE_TEXT);
        let path = inbox_file_with("big.txt", big.as_bytes());
        let (r, _) = turn(&e, &chat, "read", Some(ChatContext::File { name: "big.txt".into(), path }), reply("ok")).await;
        assert!(r.err().unwrap().contains("too long"));
        assert!(chat.is_empty());
    }

    #[tokio::test]
    async fn a_conversation_belongs_to_one_provider() {
        let e = engine();
        let chat = Chat::default();
        let local = Target { provider: Provider::Ollama, model: "gemma3:4b".into() };
        let ollama_reply = |t: &str| json!({ "message": { "role": "assistant", "content": t }, "done": true });
        let (r, _) = turn_with(&e, &chat, &local, "my private diary entry", None, ollama_reply("noted")).await;
        assert_eq!(r.unwrap().text, "noted");
        let (_, body) = turn_with(&e, &chat, &local, "and more", None, ollama_reply("ok")).await;
        assert_eq!(body["messages"].as_array().unwrap().len(), 4, "system + 3 turns");

        // Switching to a cloud provider starts over: the local conversation is not replayed.
        let (r, body) = turn(&e, &chat, "hello Claude", None, reply("hi")).await;
        assert!(r.is_ok());
        assert!(!body.to_string().contains("diary"), "{body}");
        assert_eq!(body["messages"].as_array().unwrap().len(), 1);
        // A reset forgets the provider too.
        chat.reset();
        assert!(chat.is_empty());
    }

    #[tokio::test]
    async fn every_provider_gets_the_same_masking_and_legend() {
        for (target, answer) in [
            (
                Target { provider: Provider::OpenAi, model: "gpt-test".into() },
                json!({ "choices": [{ "message": { "role": "assistant", "content": "Use {{API_KEY_1}}." }, "finish_reason": "stop" }] }),
            ),
            (
                Target { provider: Provider::Ollama, model: "gemma3:4b".into() },
                json!({ "message": { "role": "assistant", "content": "Use {{API_KEY_1}}." }, "done": true }),
            ),
        ] {
            let e = engine();
            let chat = Chat::default();
            let ctx = ChatContext::Window { app_name: "Code".into(), title: "frank@acme.io - notes".into(), url: None };
            let (r, body) = turn_with(&e, &chat, &target, &format!("where does {KEY} go?"), Some(ctx), answer).await;
            let r = r.unwrap();
            let wire = body.to_string();
            assert!(!wire.contains(KEY) && !wire.contains("frank@acme.io"), "{wire}");
            let messages = body["messages"].as_array().unwrap();
            // A system message with the persona and the legend, then the masked user text.
            assert_eq!(messages[0]["role"], "system");
            let system = messages[0]["content"].as_str().unwrap();
            assert!(system.contains("You are Zuko") && system.contains("no web access"), "{system}");
            assert!(system.contains("- {{API_KEY_1}}:") && system.contains("- {{EMAIL_1}}:"), "{system}");
            assert_eq!(messages[1]["role"], "user");
            let user = messages[1]["content"].as_str().unwrap();
            assert!(user.contains("Context — App: Code, Window: {{EMAIL_1}} - notes"), "{user}");
            assert!(user.contains("where does {{API_KEY_1}} go?"), "{user}");
            assert!(body.get("tools").is_none(), "no web search tool outside Claude");
            assert_eq!(r.text, format!("Use {KEY}."));
            assert_eq!(r.report.count, 2);

            // Attachments: a PDF goes as masked text, a picture is not sent at all.
            let tag = format!("{:?}", target.provider).to_lowercase();
            let pdf = crate::sanitize::tests_support::tiny_pdf(&[&format!("Invoice for gina@acme.io, key {KEY}")]);
            let path = inbox_file_with(&format!("{tag}-invoice.pdf"), &pdf);
            let ctx = ChatContext::File { name: "invoice.pdf".into(), path };
            let ok = json!({ "choices": [{ "message": { "content": "ok" } }], "message": { "content": "ok" } });
            let (r, body) = turn_with(&e, &Chat::default(), &target, "summarize", Some(ctx), ok.clone()).await;
            assert!(r.is_ok(), "{:?}", r.err());
            let wire = body.to_string();
            assert!(!wire.contains("gina@acme.io") && !wire.contains(KEY) && !wire.contains("base64"), "{wire}");
            assert!(wire.contains("File contents (pdf):") && wire.contains("Invoice for {{EMAIL_"), "{wire}");
            let path = inbox_file_with(&format!("{tag}-shot.png"), b"\x89PNG....");
            let ctx = ChatContext::File { name: "shot.png".into(), path };
            let (r, body) = turn_with(&e, &Chat::default(), &target, "what is this", Some(ctx), ok).await;
            assert!(r.unwrap_err().contains("pictures"));
            assert_eq!(body, Value::Null, "nothing was sent");
        }
    }

    #[test]
    fn provider_names_round_trip_and_unknown_ones_stay_local() {
        for p in [Provider::Anthropic, Provider::OpenAi, Provider::Ollama] {
            let json = serde_json::to_value(p).unwrap();
            assert_eq!(serde_json::from_value::<Provider>(json.clone()).unwrap(), p);
            assert_eq!(Provider::parse(json.as_str().unwrap()), Some(p));
        }
        assert_eq!(serde_json::to_value(Provider::OpenAi).unwrap(), "openai");
        assert_eq!(serde_json::from_value::<Provider>(json!("gemini")).unwrap(), Provider::Ollama);
        assert_eq!(serde_json::from_value::<Provider>(json!(3)).unwrap(), Provider::Ollama);
        assert_eq!(Provider::parse("Local"), None);
        assert!(!Provider::Ollama.is_cloud() && Provider::OpenAi.is_cloud());
        assert_eq!(Provider::Ollama.key_name(), None);
    }
}
