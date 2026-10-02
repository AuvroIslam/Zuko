// The island chat: multi-turn conversation with Claude (with web search) that always
// goes through Zuko's masker.
//
// Everything happens here rather than in the island: the API key never leaves the
// Credential Manager, and file bytes never cross the IPC boundary.
//
// Privacy rules for every turn:
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
// * Nothing here is logged with message content.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use zuko_core::mask::{self, MaskCtx, MaskReport};

use crate::engine::{self, Engine};
use crate::{files, sanitize, secrets};

const ENDPOINT: &str = "https://api.anthropic.com/v1/messages";
const ANTHROPIC_VERSION: &str = "2023-06-01";
/// Server-side fallback: on a policy decline the API retries the same request on
/// a fallback model inside the same call, so the island never shows a dead end.
const FALLBACK_BETA: &str = "server-side-fallback-2026-07-01";
const MAX_TOKENS: u32 = 4096;
/// A file's text is inlined up to this many characters; a bigger one is refused.
const MAX_INLINE_TEXT: usize = 200_000;

pub const DEFAULT_MODEL: &str = "claude-opus-5";

const PERSONA: &str = "You are Zuko, a friendly privacy guardian who lives at the top of the user's screen. \
You keep the user's secrets and personal data on their machine and still help with absolutely anything — research, coding, finding places, recommendations, tasks, questions. \
You have web search access. Respond in the user's language. Be thorough and complete — use as much detail as the task requires. \
No markdown formatting (no **, no ##, no bullet dashes). Use plain text with line breaks.";

#[derive(Default)]
pub struct Chat {
    /// Full multi-turn history in the form the model saw it (masked), including
    /// tool_use / tool_result blocks.
    messages: Mutex<Vec<Value>>,
}

impl Chat {
    pub fn reset(&self) {
        self.messages.lock().unwrap().clear();
    }

    fn is_empty(&self) -> bool {
        self.messages.lock().unwrap().is_empty()
    }

    fn push(&self, message: Value) {
        self.messages.lock().unwrap().push(message);
    }

    fn pop(&self) {
        self.messages.lock().unwrap().pop();
    }

    fn snapshot(&self) -> Vec<Value> {
        self.messages.lock().unwrap().clone()
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

/// One chat turn. Returns the assistant's text, or a message the island shows
/// in the note view.
pub async fn send(
    engine: &Engine,
    chat: &Chat,
    model: &str,
    query: String,
    context: Option<ChatContext>,
) -> Result<ChatReply, String> {
    let key = secrets::get("anthropic-api-key").ok_or_else(|| "API key missing. Open settings.".to_string())?;
    send_via(engine, chat, model, query, context, |body| async move { call(&key, &body).await }).await
}

/// [`send`] with the network call injected, so tests can see exactly what would go out.
async fn send_via<F, Fut>(
    engine: &Engine,
    chat: &Chat,
    model: &str,
    query: String,
    context: Option<ChatContext>,
    transport: F,
) -> Result<ChatReply, String>
where
    F: FnOnce(Value) -> Fut,
    Fut: Future<Output = Result<Value, String>>,
{
    let turn = prepare_turn(engine, chat.is_empty(), &query, context.as_ref())?;
    chat.push(json!({ "role": "user", "content": turn.content }));
    let history = chat.snapshot();

    let body = json!({
        "model": model,
        "max_tokens": MAX_TOKENS,
        "system": system_prompt(engine, &history),
        "tools": [{ "type": "web_search_20260209", "name": "web_search", "max_uses": 5 }],
        "fallbacks": "default",
        "messages": history,
    });

    let response = match transport(body).await {
        Ok(v) => v,
        Err(err) => {
            chat.pop(); // keep the history consistent with what the model saw
            return Err(err);
        }
    };

    // A policy decline comes back as HTTP 200 with stop_reason "refusal".
    if response.get("stop_reason").and_then(Value::as_str) == Some("refusal") {
        chat.pop();
        let why = response
            .get("stop_details")
            .and_then(|d| d.get("explanation"))
            .and_then(Value::as_str)
            .unwrap_or("Claude declined this one.");
        return Err(why.to_string());
    }

    let Some(blocks) = response.get("content").and_then(Value::as_array).cloned() else {
        chat.pop();
        return Err("Unexpected API response.".into());
    };

    // Store the whole content — tool_use / tool_result blocks included — so the
    // next turn has the right context. It stays as the model wrote it (placeholders).
    chat.push(json!({ "role": "assistant", "content": blocks.clone() }));

    let text = blocks
        .iter()
        .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
        .filter_map(|b| b.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string();

    if text.is_empty() {
        return Err("No response text.".into());
    }
    // Real values come back only here, on this machine.
    let (text, _restored) = engine.with_vault(|v| mask::rehydrate_text(v, &text));
    Ok(ChatReply { text, masked: turn.masked, report: turn.report })
}

/// The persona, plus the legend of every placeholder in the conversation so far.
fn system_prompt(engine: &Engine, history: &[Value]) -> String {
    let legend = engine.with_vault(|v| {
        let keys = mask::keys_in_json(v, &Value::Array(history.to_vec()));
        mask::legend(v, &keys)
    });
    if legend.is_empty() {
        PERSONA.to_string()
    } else {
        format!("{PERSONA}\n\n{legend}")
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

async fn call(key: &str, body: &Value) -> Result<Value, String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(90))
        .build()
        .map_err(|e| e.to_string())?;

    let response = client
        .post(ENDPOINT)
        .header("x-api-key", key)
        .header("anthropic-version", ANTHROPIC_VERSION)
        .header("anthropic-beta", FALLBACK_BETA)
        .header("content-type", "application/json")
        .json(body)
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;

    let status = response.status();
    let text = response.text().await.map_err(|e| e.to_string())?;
    if !status.is_success() {
        // Surface the API's own message, which is what makes a bad key obvious.
        let detail = serde_json::from_str::<Value>(&text)
            .ok()
            .and_then(|v| {
                v.get("error")
                    .and_then(|e| e.get("message"))
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .unwrap_or_else(|| text.chars().take(200).collect());
        return Err(format!("Claude API {status}: {detail}"));
    }
    serde_json::from_str(&text).map_err(|e| format!("Bad API response: {e}"))
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

    /// Puts `content` into the (test) inbox and returns its path.
    fn inbox_file_with(name: &str, content: &[u8]) -> String {
        let dir = files::inbox_dir();
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join(format!("{}-{name}", std::process::id()));
        std::fs::write(&p, content).unwrap();
        p.to_string_lossy().to_string()
    }

    /// Runs one turn against a canned reply; returns (result, the request body that would have gone out).
    async fn turn(e: &Engine, chat: &Chat, query: &str, ctx: Option<ChatContext>, reply: Value) -> (Result<ChatReply, String>, Value) {
        let sent = Arc::new(Mutex::new(Value::Null));
        let sent2 = sent.clone();
        let result = send_via(e, chat, "test-model", query.into(), ctx, move |body| async move {
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
        assert!(system.contains("- {{API_KEY_1}}:") && system.contains("- {{EMAIL_1}}:"), "{system}");
        assert!(!system.contains(KEY));

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
        assert_eq!(body["system"].as_str().unwrap(), PERSONA);
    }

    #[tokio::test]
    async fn failures_leave_the_history_as_the_model_saw_it() {
        let e = engine();
        let chat = Chat::default();
        let res = send_via(&e, &chat, "m", "hello".into(), None, |_| async { Err("Network error: down".to_string()) }).await;
        assert!(res.is_err());
        assert!(chat.is_empty());
        let (r, _) = turn(&e, &chat, "hi", None, json!({ "stop_reason": "refusal", "stop_details": { "explanation": "nope" } })).await;
        assert_eq!(r.err().as_deref(), Some("nope"));
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
}
