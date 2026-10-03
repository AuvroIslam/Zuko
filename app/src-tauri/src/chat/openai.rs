// OpenAI, through the Chat Completions API (POST {API_BASE}/chat/completions). The
// key is the user's own, kept in the OS keyring as "openai-api-key"; it is sent only
// to OpenAI's API, as a bearer token, and is never logged, shown or put in an error.
//
// Only masked text reaches this file (see chat.rs). There are no tools: a request is
// the system prompt (persona + placeholder legend) followed by the text of each
// message, and the answer is the first choice's text.

use std::time::Duration;

use reqwest::StatusCode;
use serde_json::{json, Value};

pub const API_BASE: &str = "https://api.openai.com/v1";
pub const DEFAULT_MODEL: &str = "gpt-5-mini";
/// When the saved model is not offered any more, the first of these the key can use
/// is selected (then simply the first chat model in the list).
const PREFERRED: &[&str] = &["gpt-5-mini", "gpt-4.1-mini", "gpt-4o-mini", "gpt-5", "gpt-4.1", "gpt-4o"];
/// Leaves room for reasoning models, which count their hidden reasoning against it.
const MAX_COMPLETION_TOKENS: u32 = 8192;
const TIMEOUT: Duration = Duration::from_secs(120);
const LIST_TIMEOUT: Duration = Duration::from_secs(15);

pub(super) fn request(model: &str, system: &str, history: &[Value]) -> Value {
    let mut messages = vec![json!({ "role": "system", "content": system })];
    messages.extend(history.iter().map(super::plain_message));
    json!({
        "model": model,
        "messages": messages,
        "max_completion_tokens": MAX_COMPLETION_TOKENS,
    })
}

pub(super) fn reply(response: &Value) -> Result<Vec<Value>, String> {
    let choice = response.pointer("/choices/0").ok_or_else(|| "Unexpected answer from OpenAI.".to_string())?;
    // A refusal is its own field, next to an empty content.
    if let Some(refusal) = choice.pointer("/message/refusal").and_then(Value::as_str).filter(|r| !r.trim().is_empty()) {
        return Err(refusal.trim().to_string());
    }
    let text = choice.pointer("/message/content").and_then(Value::as_str).unwrap_or("").trim();
    if text.is_empty() {
        return Err(match choice.get("finish_reason").and_then(Value::as_str) {
            Some("length") => "OpenAI ran out of tokens before writing an answer. Try a shorter question or another model.".into(),
            Some("content_filter") => "OpenAI's content filter withheld this answer.".into(),
            _ => "No response text.".into(),
        });
    }
    Ok(vec![json!({ "type": "text", "text": text })])
}

pub(super) async fn call(base: &str, key: &str, body: &Value) -> Result<Value, String> {
    let response = super::cloud_client(TIMEOUT)?
        .post(format!("{base}/chat/completions"))
        .bearer_auth(key)
        .json(body)
        .send()
        .await
        .map_err(|e| network_error(&e, TIMEOUT))?;
    let status = response.status();
    let text = response.text().await.map_err(|e| network_error(&e, TIMEOUT))?;
    if !status.is_success() {
        return Err(api_error(status, &text));
    }
    serde_json::from_str(&text).map_err(|_| "OpenAI sent an answer Zuko could not read.".to_string())
}

/// The chat models the key can use (GET {base}/models), sorted and without repeats.
pub(super) async fn list_models(base: &str, key: &str) -> Result<Vec<String>, String> {
    let response = super::cloud_client(LIST_TIMEOUT)?
        .get(format!("{base}/models"))
        .bearer_auth(key)
        .send()
        .await
        .map_err(|e| network_error(&e, LIST_TIMEOUT))?;
    let status = response.status();
    let text = response.text().await.map_err(|e| network_error(&e, LIST_TIMEOUT))?;
    if !status.is_success() {
        return Err(api_error(status, &text));
    }
    let v: Value = serde_json::from_str(&text).map_err(|_| "OpenAI sent a model list Zuko could not read.".to_string())?;
    let mut ids: Vec<String> = v
        .get("data")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|m| m.get("id").and_then(Value::as_str)).filter(|id| chat_capable(id)).map(str::to_string).collect())
        .unwrap_or_default();
    ids.sort();
    ids.dedup();
    if ids.is_empty() {
        return Err("OpenAI lists no chat models for this key.".into());
    }
    Ok(ids)
}

/// True for model ids that hold a conversation through Chat Completions: the `gpt-*`,
/// `chatgpt-*` and `o<digit>…` families, minus image, audio, realtime, speech,
/// embedding, moderation and search models, completion-only (`-instruct`) models and
/// the ones only the Responses API serves (`-pro`, `codex`, deep research).
pub fn chat_capable(id: &str) -> bool {
    const NOT_CHAT: &[&str] = &[
        "image", "realtime", "audio", "transcribe", "tts", "instruct", "embedding", "moderation", "search", "codex", "-pro",
        "deep-research", "computer-use",
    ];
    let id = id.trim().to_ascii_lowercase();
    let family = id.starts_with("gpt-")
        || id.starts_with("chatgpt-")
        || (id.starts_with('o') && id[1..].starts_with(|c: char| c.is_ascii_digit()));
    family && !NOT_CHAT.iter().any(|w| id.contains(w))
}

/// The model to select: the saved one when offered, else a sensible default.
pub fn pick(models: &[String], saved: &str) -> String {
    if models.is_empty() || models.iter().any(|m| m == saved) {
        return saved.to_string();
    }
    PREFERRED
        .iter()
        .find(|p| models.iter().any(|m| m == *p))
        .map(|p| p.to_string())
        .unwrap_or_else(|| models[0].clone())
}

fn network_error(e: &reqwest::Error, limit: Duration) -> String {
    if e.is_timeout() {
        format!("OpenAI did not answer within {} seconds. Try again in a moment.", limit.as_secs())
    } else {
        format!("Can't reach OpenAI. Check your internet connection. ({})", super::root_cause(e))
    }
}

/// A plain-words error for a failed call. A 401 deliberately leaves out OpenAI's own
/// message, which quotes part of the key.
fn api_error(status: StatusCode, body: &str) -> String {
    let v = serde_json::from_str::<Value>(body).ok();
    let field = |name: &str| v.as_ref().and_then(|v| v.pointer(&format!("/error/{name}"))).and_then(Value::as_str).unwrap_or("").to_string();
    let detail = match field("message") {
        m if m.is_empty() => body.chars().take(200).collect(),
        m => m,
    };
    match status.as_u16() {
        401 => "OpenAI rejected the API key (401). Check or replace it in Settings → Chat.".into(),
        429 if field("code") == "insufficient_quota" || field("type") == "insufficient_quota" => {
            format!("OpenAI says this key has no credit left (429): {detail}")
        }
        429 => format!("OpenAI's rate limit was reached (429). Wait a moment and try again. {detail}").trim().to_string(),
        _ => format!("OpenAI API {status}: {detail}"),
    }
}

#[cfg(test)]
pub(crate) mod mock {
    //! A fake OpenAI API on a random loopback port: `/v1/chat/completions` and
    //! `/v1/models` with scripted status and body, recording every request.

    use std::convert::Infallible;
    use std::sync::{Arc, Mutex};

    use bytes::Bytes;
    use http_body_util::{BodyExt, Full};
    use hyper::server::conn::http1;
    use hyper::service::service_fn;
    use hyper::{Request, Response, StatusCode};
    use hyper_util::rt::TokioIo;
    use tokio::net::TcpListener;

    #[derive(Clone, Debug)]
    pub struct Seen {
        pub path: String,
        pub authorization: String,
        pub body: String,
    }

    #[derive(Clone)]
    pub struct Script {
        pub chat: (u16, String),
        pub models: (u16, String),
    }

    #[derive(Clone)]
    pub struct Mock {
        /// The API base, like `https://api.openai.com/v1`.
        pub url: String,
        pub seen: Arc<Mutex<Vec<Seen>>>,
        pub script: Arc<Mutex<Script>>,
    }

    impl Mock {
        pub fn seen(&self) -> Vec<Seen> {
            self.seen.lock().unwrap().clone()
        }

        pub fn answer_chat(&self, status: u16, body: impl Into<String>) {
            self.script.lock().unwrap().chat = (status, body.into());
        }
    }

    pub fn completion(text: &str) -> String {
        serde_json::json!({
            "id": "chatcmpl-test", "object": "chat.completion", "model": "gpt-test",
            "choices": [{ "index": 0, "message": { "role": "assistant", "content": text, "refusal": null }, "finish_reason": "stop" }],
        })
        .to_string()
    }

    pub async fn start(script: Script) -> Mock {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mock = Mock {
            url: format!("http://{}/v1", listener.local_addr().unwrap()),
            seen: Arc::default(),
            script: Arc::new(Mutex::new(script)),
        };
        let m = mock.clone();
        tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else { continue };
                let m = m.clone();
                tokio::spawn(async move {
                    let svc = service_fn(move |req: Request<hyper::body::Incoming>| {
                        let m = m.clone();
                        async move {
                            let path = req.uri().path().to_string();
                            let authorization =
                                req.headers().get("authorization").and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
                            let body = String::from_utf8_lossy(&req.into_body().collect().await.unwrap().to_bytes()).to_string();
                            m.seen.lock().unwrap().push(Seen { path: path.clone(), authorization, body });
                            let script = m.script.lock().unwrap().clone();
                            let (status, out) = match path.as_str() {
                                "/v1/chat/completions" => script.chat,
                                "/v1/models" => script.models,
                                _ => (404, "{}".into()),
                            };
                            let mut resp = Response::new(Full::new(Bytes::from(out)));
                            *resp.status_mut() = StatusCode::from_u16(status).unwrap();
                            resp.headers_mut().insert("content-type", "application/json".parse().unwrap());
                            Ok::<_, Infallible>(resp)
                        }
                    });
                    let _ = http1::Builder::new().serve_connection(TokioIo::new(stream), svc).await;
                });
            }
        });
        mock
    }
}

#[cfg(test)]
mod tests {
    use super::super::{models_from, send_to, send_via, status, Chat, ChatReply, Provider, Target, Upstream};
    use super::*;
    use crate::engine::{CtxBase, Engine};
    use zuko_core::policy::Policy;
    use zuko_core::vault::Vault;

    /// The user's secret that must never reach OpenAI.
    const SECRET: &str = "sk-proj-ZUKOFAKE0123456789abcdefghijklmnop";
    /// The (fake) OpenAI key Zuko authenticates with.
    const AUTH: &str = "sk-test-openai-auth-0000000000000000";

    fn engine() -> Engine {
        Engine::with_parts(Policy::default(), Vault::new(), CtxBase::default())
    }

    fn gpt() -> Target {
        Target { provider: Provider::OpenAi, model: "gpt-test".into() }
    }

    fn models_json(ids: &[&str]) -> String {
        json!({ "object": "list", "data": ids.iter().map(|id| json!({ "id": id, "object": "model", "owned_by": "openai" })).collect::<Vec<_>>() })
            .to_string()
    }

    fn script() -> mock::Script {
        mock::Script {
            chat: (200, mock::completion("Put OPENAI_API_KEY={{API_KEY_1}} in .env, then tell {{EMAIL_1}}.")),
            models: (200, models_json(&["gpt-4o", "gpt-5-mini", "whisper-1"])),
        }
    }

    async fn turn(e: &Engine, chat: &Chat, m: &mock::Mock, query: &str) -> Result<ChatReply, String> {
        let base = m.url.clone();
        send_via(e, chat, &gpt(), query.into(), None, |body| async move { call(&base, AUTH, &body).await }).await
    }

    #[tokio::test]
    async fn masked_turns_reach_openai_and_the_reply_is_restored() {
        let m = mock::start(script()).await;
        let e = engine();
        let chat = Chat::default();
        let r = turn(&e, &chat, &m, &format!("My key is {SECRET}; put it in .env and email ana@acme.io")).await.unwrap();

        let seen = m.seen();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].path, "/v1/chat/completions");
        assert_eq!(seen[0].authorization, format!("Bearer {AUTH}"));
        let wire = &seen[0].body;
        assert!(!wire.contains(SECRET) && !wire.contains("ana@acme.io"), "leaked: {wire}");
        let body: Value = serde_json::from_str(wire).unwrap();
        assert_eq!(body["model"], "gpt-test");
        assert_eq!(body["max_completion_tokens"], MAX_COMPLETION_TOKENS);
        assert!(body.get("tools").is_none() && body.get("stream").is_none());
        let messages = body["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0]["role"], "system");
        let system = messages[0]["content"].as_str().unwrap();
        assert!(system.contains("You are Zuko") && system.contains("- {{API_KEY_1}}:") && system.contains("- {{EMAIL_1}}:"), "{system}");
        assert_eq!(messages[1], json!({ "role": "user", "content": "My key is {{API_KEY_1}}; put it in .env and email {{EMAIL_1}}" }));

        // Real values come back on this machine only.
        assert_eq!(r.text, format!("Put OPENAI_API_KEY={SECRET} in .env, then tell ana@acme.io."));
        assert_eq!(r.report.count, 2);

        // The next turn replays the masked history as plain text messages.
        m.answer_chat(200, mock::completion("Done."));
        let r = turn(&e, &chat, &m, &format!("and {SECRET} again?")).await.unwrap();
        assert_eq!(r.text, "Done.");
        let wire = &m.seen()[1].body;
        assert!(!wire.contains(SECRET) && !wire.contains("ana@acme.io"), "leaked: {wire}");
        let body: Value = serde_json::from_str(wire).unwrap();
        let roles: Vec<&str> = body["messages"].as_array().unwrap().iter().map(|m| m["role"].as_str().unwrap()).collect();
        assert_eq!(roles, ["system", "user", "assistant", "user"]);
        assert_eq!(body["messages"][2]["content"], "Put OPENAI_API_KEY={{API_KEY_1}} in .env, then tell {{EMAIL_1}}.");
        assert_eq!(body["messages"][3]["content"], "and {{API_KEY_1}} again?");
    }

    #[tokio::test]
    async fn errors_are_explained_and_leave_the_history_alone() {
        let m = mock::start(script()).await;
        let e = engine();
        let chat = Chat::default();
        let cases = [
            (401, json!({ "error": { "message": "Incorrect API key provided: sk-test-****0000.", "type": "invalid_request_error", "code": "invalid_api_key" } }), "rejected the API key"),
            (429, json!({ "error": { "message": "Rate limit reached for gpt-test.", "type": "requests", "code": "rate_limit_exceeded" } }), "rate limit was reached"),
            (429, json!({ "error": { "message": "You exceeded your current quota.", "type": "insufficient_quota", "code": "insufficient_quota" } }), "no credit left"),
            (500, json!({ "error": { "message": "The server had an error." } }), "OpenAI API 500"),
            (404, json!({ "error": { "message": "The model `gpt-test` does not exist." } }), "does not exist"),
        ];
        for (code, body, expect) in cases {
            m.answer_chat(code, body.to_string());
            let err = turn(&e, &chat, &m, &format!("key {SECRET}")).await.unwrap_err();
            assert!(err.contains(expect), "{code}: {err}");
            assert!(!err.contains("sk-test"), "an error never quotes the key: {err}");
            assert!(chat.is_empty(), "{code}: a failed turn is not kept");
        }

        // A refusal and an answer with no text.
        m.answer_chat(200, json!({ "choices": [{ "message": { "role": "assistant", "content": null, "refusal": "I can't help with that." }, "finish_reason": "stop" }] }).to_string());
        assert_eq!(turn(&e, &chat, &m, "hm").await.unwrap_err(), "I can't help with that.");
        m.answer_chat(200, json!({ "choices": [{ "message": { "role": "assistant", "content": "" }, "finish_reason": "length" }] }).to_string());
        assert!(turn(&e, &chat, &m, "hm").await.unwrap_err().contains("ran out of tokens"));
        m.answer_chat(200, "<html>bad gateway</html>");
        assert!(turn(&e, &chat, &m, "hm").await.unwrap_err().contains("could not read"));
        assert!(chat.is_empty());

        // Nothing listening: a network error in plain words.
        let err = send_via(&e, &chat, &gpt(), "hi".into(), None, |body| async move { call("http://127.0.0.1:1/v1", AUTH, &body).await })
            .await
            .unwrap_err();
        assert!(err.starts_with("Can't reach OpenAI"), "{err}");
        assert!(chat.is_empty());
    }

    #[test]
    fn only_chat_models_are_offered() {
        for id in ["gpt-4o", "gpt-4o-mini", "gpt-4.1", "gpt-5-mini", "chatgpt-4o-latest", "o1", "o3-mini", "o4-mini", "gpt-3.5-turbo"] {
            assert!(chat_capable(id), "{id}");
        }
        for id in [
            "whisper-1", "dall-e-3", "tts-1", "text-embedding-3-small", "omni-moderation-latest", "babbage-002", "davinci-002",
            "gpt-image-1", "gpt-4o-realtime-preview", "gpt-4o-audio-preview", "gpt-4o-mini-tts", "gpt-4o-transcribe",
            "gpt-4o-search-preview", "gpt-3.5-turbo-instruct", "o1-pro", "o3-pro", "codex-mini-latest", "o3-deep-research",
            "computer-use-preview", "omni", "o",
        ] {
            assert!(!chat_capable(id), "{id}");
        }
        let list: Vec<String> = ["gpt-4.1", "gpt-4o", "o3-mini"].iter().map(|s| s.to_string()).collect();
        assert_eq!(pick(&list, "gpt-4o"), "gpt-4o");
        assert_eq!(pick(&list, "gpt-3.5-turbo"), "gpt-4.1", "the preferred fallback the key has");
        assert_eq!(pick(&["o3-mini".to_string()], "gone"), "o3-mini");
        assert_eq!(pick(&[], "gpt-4o"), "gpt-4o", "nothing listed: keep the saved one");
    }

    #[tokio::test]
    async fn the_model_list_is_filtered_and_sorted() {
        let ids = [
            "gpt-4o", "whisper-1", "dall-e-3", "gpt-4o-mini", "text-embedding-3-small", "o3-mini", "gpt-4o-realtime-preview",
            "gpt-image-1", "gpt-4o-mini-tts", "o1-pro", "gpt-3.5-turbo-instruct", "gpt-4.1", "chatgpt-4o-latest", "o4-mini",
            "gpt-4o",
        ];
        let m = mock::start(mock::Script { models: (200, models_json(&ids)), ..script() }).await;
        let list = list_models(&m.url, AUTH).await.unwrap();
        assert_eq!(list, ["chatgpt-4o-latest", "gpt-4.1", "gpt-4o", "gpt-4o-mini", "o3-mini", "o4-mini"]);
        assert_eq!(m.seen()[0].path, "/v1/models");
        assert_eq!(m.seen()[0].authorization, format!("Bearer {AUTH}"));

        let m = mock::start(mock::Script { models: (401, json!({ "error": { "message": "bad key sk-test-****0000" } }).to_string()), ..script() }).await;
        let err = list_models(&m.url, AUTH).await.unwrap_err();
        assert!(err.contains("rejected the API key") && !err.contains("sk-test"), "{err}");
        let m = mock::start(mock::Script { models: (200, models_json(&["whisper-1", "tts-1"])), ..script() }).await;
        assert!(list_models(&m.url, AUTH).await.unwrap_err().contains("no chat models"));
    }

    /// The only test that touches the (in-memory) "openai-api-key" entry, so the
    /// missing-key checks cannot race another test.
    #[tokio::test]
    async fn the_stored_key_is_used_and_its_absence_explained() {
        let m = mock::start(script()).await;
        let up = Upstream { openai: &m.url };
        let e = engine();
        let chat = Chat::default();
        crate::secrets::clear("openai-api-key").unwrap();

        let err = send_to(&e, &chat, &gpt(), "hello".into(), None, &up).await.unwrap_err();
        assert_eq!(err, "OpenAI API key missing. Add it in Settings → Chat.");
        let listed = models_from(&e, Provider::OpenAi, "gpt-4o", &up).await;
        assert!(listed.models.is_empty() && listed.error.unwrap().contains("missing"));
        assert_eq!(listed.selected, "gpt-4o");
        let s = status(&e, Provider::OpenAi, "").await;
        assert_eq!((s.ready, s.key_present, s.cloud), (false, Some(false), true));
        assert_eq!(s.model, DEFAULT_MODEL);
        assert!(m.seen().is_empty(), "nothing is sent without a key");

        crate::secrets::set("openai-api-key", AUTH).unwrap();
        let r = send_to(&e, &chat, &gpt(), format!("store {SECRET}"), None, &up).await.unwrap();
        assert!(r.text.contains(SECRET));
        assert_eq!(m.seen()[0].authorization, format!("Bearer {AUTH}"));
        let listed = models_from(&e, Provider::OpenAi, "gpt-3.5-turbo", &up).await;
        assert_eq!(listed.models, ["gpt-4o", "gpt-5-mini"]);
        assert_eq!(listed.selected, "gpt-5-mini", "the saved model is gone: a sensible default");
        let s = status(&e, Provider::OpenAi, "gpt-4o").await;
        assert_eq!((s.ready, s.key_present), (true, Some(true)));
        let json = serde_json::to_string(&s).unwrap() + &serde_json::to_string(&listed).unwrap();
        assert!(!json.contains(AUTH), "status and model list never carry the key");
        crate::secrets::clear("openai-api-key").unwrap();
    }
}
