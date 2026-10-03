// Ollama on this machine (POST {localAi.endpoint}/api/chat, not streamed), through the
// local AI's own client in localai.rs rather than a second one, so the chat gets the
// same guarantees as the scans: the endpoint is validated as loopback before every
// call, the client ignores proxies and refuses redirects, and calls share the same
// concurrency limit. No key is needed and nothing leaves the PC. The turn is still
// masked like every other provider's (chat.rs), so the history never holds a raw
// secret either, and switching to a cloud provider later starts a new conversation.
//
// The endpoint comes from the policy's `localAi` section, but this does not depend on
// the "Use local AI" switch: that one turns the scans and explanations on, while this
// is the user choosing Ollama to chat with.

use std::time::Duration;

use serde_json::{json, Value};

use crate::localai::{AiError, LocalAi, KEEP_ALIVE};

pub const DEFAULT_MODEL: &str = zuko_core::localai::DEFAULT_MODEL;
/// A small model on a CPU can take a while for a long answer, plus loading the model
/// the first time.
const TIMEOUT: Duration = Duration::from_secs(180);

pub(super) fn request(model: &str, system: &str, history: &[Value]) -> Value {
    let mut messages = vec![json!({ "role": "system", "content": system })];
    messages.extend(history.iter().map(super::plain_message));
    // The client sets `"stream": false` and the validated model name again.
    json!({ "model": model, "messages": messages, "keep_alive": KEEP_ALIVE })
}

pub(super) fn reply(response: &Value) -> Result<Vec<Value>, String> {
    let raw = response.pointer("/message/content").and_then(Value::as_str).unwrap_or("");
    let text = without_thinking(raw).trim();
    if text.is_empty() {
        return Err("Ollama sent an empty answer.".into());
    }
    Ok(vec![json!({ "type": "text", "text": text })])
}

/// Reasoning models (deepseek-r1, qwen3…) may start with a `<think>…</think>` block;
/// only the answer after it is kept.
fn without_thinking(text: &str) -> &str {
    let t = text.trim_start();
    if t.starts_with("<think>") {
        if let Some(end) = t.find("</think>") {
            return &t[end + "</think>".len()..];
        }
    }
    text
}

pub(super) async fn call(ai: &LocalAi, endpoint: &str, model: &str, body: Value) -> Result<Value, String> {
    ai.chat_request(endpoint, model, body, TIMEOUT).await.map_err(|e| explain(e, model))
}

fn explain(e: AiError, model: &str) -> String {
    let model = model.trim();
    match e {
        AiError::Timeout => format!("Ollama did not answer within {} seconds. A smaller model answers faster.", TIMEOUT.as_secs()),
        AiError::Http(why) if why.contains("not found") => {
            format!("The model {model} is not installed in Ollama. Run: ollama pull {model}")
        }
        AiError::Http(why) if why.contains("not reachable") => format!("{why}. Is Ollama running? Start it and try again."),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::super::{models_from, send, status, Chat, Provider, Target, UPSTREAM};
    use super::*;
    use crate::engine::{CtxBase, Engine};
    use crate::localai::mock;
    use std::time::Instant;
    use zuko_core::localai::LocalAiConfig;
    use zuko_core::policy::Policy;
    use zuko_core::vault::Vault;

    const SECRET: &str = "sk-proj-ZUKOFAKE0123456789abcdefghijklmnop";

    /// An engine whose local AI endpoint is `endpoint`, with the local AI itself OFF:
    /// the chat must work without it.
    fn engine_at(endpoint: &str) -> Engine {
        let mut p = Policy::default();
        p.local_ai = LocalAiConfig { enabled: false, endpoint: endpoint.into(), ..Default::default() };
        Engine::with_parts(p, Vault::new(), CtxBase::default())
    }

    fn local(model: &str) -> Target {
        Target { provider: Provider::Ollama, model: model.into() }
    }

    #[tokio::test]
    async fn ollama_turns_are_masked_and_restored_on_this_pc() {
        let m = mock::start(mock::Script { content: Some("Saved {{API_KEY_1}} for {{EMAIL_1}}.".into()), ..Default::default() }).await;
        let e = engine_at(&m.url);
        assert!(!e.policy().local_ai.enabled);
        let chat = Chat::default();
        let r = send(&e, &chat, &local("gemma3:4b"), format!("Remember {SECRET} for lena@acme.io"), None).await.unwrap();

        let bodies = m.chat_bodies();
        assert_eq!(bodies.len(), 1, "one chat call, no scan (the local AI is off)");
        let wire = &bodies[0];
        assert!(!wire.contains(SECRET) && !wire.contains("lena@acme.io"), "leaked: {wire}");
        let body: Value = serde_json::from_str(wire).unwrap();
        assert_eq!(body["model"], "gemma3:4b");
        assert_eq!(body["stream"], false);
        assert!(body.get("format").is_none(), "a conversation, not the scans' JSON mode");
        let system = body["messages"][0]["content"].as_str().unwrap();
        assert!(system.contains("You are Zuko") && system.contains("- {{API_KEY_1}}:") && system.contains("- {{EMAIL_1}}:"), "{system}");
        assert_eq!(body["messages"][1]["content"], "Remember {{API_KEY_1}} for {{EMAIL_1}}");

        assert_eq!(r.text, format!("Saved {SECRET} for lena@acme.io."));
        assert_eq!(r.report.count, 2);
        assert_eq!(e.localai().requests(), 1);

        // Turn two replays the masked history.
        send(&e, &chat, &local("gemma3:4b"), "thanks".into(), None).await.unwrap();
        let body: Value = serde_json::from_str(&m.chat_bodies()[1]).unwrap();
        assert_eq!(body["messages"].as_array().unwrap().len(), 4);
        assert_eq!(body["messages"][2]["content"], "Saved {{API_KEY_1}} for {{EMAIL_1}}.");
    }

    #[tokio::test]
    async fn non_loopback_endpoints_are_refused_with_zero_requests() {
        for url in ["http://10.255.255.1:11434", "http://localhost.evil.com:11434", "http://127.0.0.1@example.com", "https://api.example.com"] {
            let e = engine_at(url);
            let chat = Chat::default();
            let started = Instant::now();
            let err = send(&e, &chat, &local("gemma3:4b"), format!("key {SECRET}"), None).await.unwrap_err();
            assert!(err.contains("must be"), "{url}: {err}");
            assert!(started.elapsed() < Duration::from_millis(200));
            assert_eq!(e.localai().requests(), 0, "{url}: nothing may be sent");
            assert!(chat.is_empty());
            let s = status(&e, Provider::Ollama, "gemma3:4b").await;
            assert!(!s.ready && s.reachable == Some(false) && !s.cloud && s.error.is_some(), "{s:?}");
            let listed = models_from(&e, Provider::Ollama, "gemma3:4b", &UPSTREAM).await;
            assert!(listed.models.is_empty() && listed.error.is_some());
        }
        // A model name that is not one is refused before sending too.
        let m = mock::start(mock::Script::default()).await;
        let e = engine_at(&m.url);
        let err = send(&e, &Chat::default(), &local("gemma3:4b; rm -rf"), "hi".into(), None).await.unwrap_err();
        assert!(err.contains("not a valid Ollama model name"), "{err}");
        assert_eq!(e.localai().requests(), 0);
    }

    #[tokio::test]
    async fn a_missing_model_or_ollama_says_what_to_do() {
        let m = mock::start(mock::Script {
            status: 404,
            content: Some("model \"llama9:1b\" not found, try pulling it first".into()),
            models: vec!["qwen2.5:7b".into(), "gemma3:4b".into()],
            ..Default::default()
        })
        .await;
        let e = engine_at(&m.url);
        let chat = Chat::default();
        let err = send(&e, &chat, &local("llama9:1b"), "hi".into(), None).await.unwrap_err();
        assert_eq!(err, "The model llama9:1b is not installed in Ollama. Run: ollama pull llama9:1b");
        assert!(chat.is_empty());

        let s = status(&e, Provider::Ollama, "llama9:1b").await;
        assert_eq!((s.ready, s.reachable, s.model_present), (false, Some(true), Some(false)));
        assert_eq!(s.hint.as_deref(), Some("ollama pull llama9:1b"));
        assert_eq!(s.endpoint.as_deref(), Some(m.url.as_str()));
        let s = status(&e, Provider::Ollama, "gemma3:4b").await;
        assert!(s.ready && s.error.is_none(), "{s:?}");
        let listed = models_from(&e, Provider::Ollama, "llama9:1b", &UPSTREAM).await;
        assert_eq!(listed.models, ["gemma3:4b", "qwen2.5:7b"]);
        assert_eq!(listed.selected, "llama9:1b", "a missing model stays selected; the status says how to pull it");
        assert!(listed.error.is_none());

        // Nothing listening.
        let e = engine_at("http://127.0.0.1:1");
        let err = send(&e, &Chat::default(), &local("gemma3:4b"), "hi".into(), None).await.unwrap_err();
        assert!(err.contains("not reachable") && err.contains("Is Ollama running?"), "{err}");
    }

    #[test]
    fn answers_lose_a_leading_think_block() {
        let r = reply(&json!({ "message": { "content": "<think>the user wants a greeting</think>\n\nHello!" } })).unwrap();
        assert_eq!(r[0]["text"], "Hello!");
        assert_eq!(reply(&json!({ "message": { "content": "A <think> in the middle stays." } })).unwrap()[0]["text"], "A <think> in the middle stays.");
        assert!(reply(&json!({ "message": { "content": "  " } })).is_err());
        assert!(reply(&json!({})).is_err());
    }

    /// The real thing: `cargo test -p zuko --lib real_ollama_chat -- --ignored --nocapture`
    /// with Ollama running at 127.0.0.1:11434 and `gemma3:4b` pulled.
    #[tokio::test]
    #[ignore]
    async fn real_ollama_chat_smoke_test() {
        let e = engine_at(zuko_core::localai::DEFAULT_ENDPOINT);
        let s = status(&e, Provider::Ollama, DEFAULT_MODEL).await;
        println!("status: ready={} reachable={:?} model_present={:?} error={:?}", s.ready, s.reachable, s.model_present, s.error);
        let chat = Chat::default();
        let started = Instant::now();
        let r = send(
            &e,
            &chat,
            &local(DEFAULT_MODEL),
            format!("My test API key is {SECRET}. In one short sentence, tell me which environment variable name I should store it in, and write the key itself back exactly as you were given it."),
            None,
        )
        .await;
        let ms = started.elapsed().as_millis();
        match &r {
            Ok(reply) => println!("reply ({ms} ms, masked {:?}): {}", reply.masked.iter().map(|k| &k.key).collect::<Vec<_>>(), reply.text),
            Err(err) => println!("error ({ms} ms): {err}"),
        }
        let r = r.unwrap();
        assert!(!r.text.is_empty());
        assert_eq!(r.report.count, 1, "the fake key was masked before it reached the model");
    }
}
