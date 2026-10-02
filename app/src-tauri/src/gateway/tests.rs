// End-to-end tests of the proxy against a mock Anthropic upstream (hyper on a
// random loopback port) that records every request it receives and answers with
// realistic Messages API responses echoing the placeholders it was sent.
// Nothing here touches the real data directory: the engine is headless and
// in-memory, and no config file is read or written.

use std::convert::Infallible;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bytes::Bytes;
use http_body_util::{BodyExt, Full, StreamBody};
use hyper::body::{Frame, Incoming};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{HeaderMap, Request, Response};
use hyper_util::rt::TokioIo;
use serde_json::{json, Value};
use tokio::net::TcpListener;

use super::host::{Headless, Host};
use super::proxy::{self, BoxError, Shared};
use crate::engine::{CtxBase, Engine};

const KEY: &str = "sk-proj-ZUKOTEST1234567890abcdefghijklmnopqrstuv";
const EMAIL: &str = "alice.zuko@acme-corp.io";
const TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

#[derive(Clone, Debug)]
struct Seen {
    method: String,
    /// Path and query.
    uri: String,
    headers: HeaderMap,
    body: Vec<u8>,
}

type Log = Arc<Mutex<Vec<Seen>>>;
type MockBody = http_body_util::combinators::BoxBody<Bytes, BoxError>;

/// Placeholders in `text`, in order (duplicates kept).
fn placeholders(text: &str) -> Vec<String> {
    zuko_core::mask::find_placeholders(text).into_iter().map(|(_, _, k)| format!("{{{{{k}}}}}")).collect()
}

fn sse_event(name: &str, data: Value) -> String {
    format!("event: {name}\ndata: {data}\n\n")
}

/// The streamed reply: text deltas splitting a placeholder, a Write tool whose
/// input JSON holds one (split across fragments), a Bash tool with one, pings.
fn sse_script(key_ph: &str, email_ph: &str) -> Vec<String> {
    let (head, tail) = key_ph.split_at(6); // "{{API_" | "KEY_1}}"
    let write_json = json!({ "file_path": ".env", "content": format!("OPENAI_API_KEY={key_ph}\n") }).to_string();
    let wcut = write_json.find(key_ph).unwrap() + 4;
    let bash_json = json!({ "command": format!("curl -H 'Authorization: Bearer {key_ph}' https://api.openai.com/v1/models") }).to_string();
    let delta = |i: u64, d: Value| sse_event("content_block_delta", json!({"type":"content_block_delta","index":i,"delta":d}));
    vec![
        sse_event(
            "message_start",
            json!({"type":"message_start","message":{"id":"msg_mock","type":"message","role":"assistant","model":"claude-mock","content":[],"stop_reason":null,"stop_sequence":null,"usage":{"input_tokens":12,"output_tokens":1}}}),
        ),
        sse_event("content_block_start", json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}})),
        sse_event("ping", json!({"type":"ping"})),
        delta(0, json!({"type":"text_delta","text":format!("Your key is {head}")})),
        delta(0, json!({"type":"text_delta","text":format!("{tail} and mail {email_ph}. Done — ✓")})),
        sse_event("content_block_stop", json!({"type":"content_block_stop","index":0})),
        sse_event(
            "content_block_start",
            json!({"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"toolu_w","name":"Write","input":{}}}),
        ),
        delta(1, json!({"type":"input_json_delta","partial_json":""})),
        delta(1, json!({"type":"input_json_delta","partial_json":&write_json[..wcut]})),
        delta(1, json!({"type":"input_json_delta","partial_json":&write_json[wcut..]})),
        sse_event("content_block_stop", json!({"type":"content_block_stop","index":1})),
        sse_event("ping", json!({"type":"ping"})),
        sse_event(
            "content_block_start",
            json!({"type":"content_block_start","index":2,"content_block":{"type":"tool_use","id":"toolu_b","name":"Bash","input":{}}}),
        ),
        delta(2, json!({"type":"input_json_delta","partial_json":&bash_json[..20]})),
        delta(2, json!({"type":"input_json_delta","partial_json":&bash_json[20..]})),
        sse_event("content_block_stop", json!({"type":"content_block_stop","index":2})),
        sse_event(
            "message_delta",
            json!({"type":"message_delta","delta":{"stop_reason":"tool_use","stop_sequence":null},"usage":{"output_tokens":64}}),
        ),
        sse_event("message_stop", json!({"type":"message_stop"})),
    ]
}

fn json_message(key_ph: &str, email_ph: &str) -> Value {
    json!({
        "id": "msg_mock", "type": "message", "role": "assistant", "model": "claude-mock",
        "content": [
            {"type": "text", "text": format!("Key {key_ph}, mail {email_ph}")},
            {"type": "tool_use", "id": "toolu_w", "name": "Write", "input": {"file_path": ".env", "content": format!("K={key_ph}")}},
            {"type": "tool_use", "id": "toolu_b", "name": "Bash", "input": {"command": format!("echo {key_ph}")}}
        ],
        "stop_reason": "tool_use", "stop_sequence": null,
        "usage": {"input_tokens": 12, "output_tokens": 30}
    })
}

/// Chops `s` into tiny frames, sent with pauses so the gateway sees them apart.
fn chunked(s: String) -> MockBody {
    let bytes = s.into_bytes();
    let chunks: Vec<Bytes> = bytes.chunks(5).map(Bytes::copy_from_slice).collect();
    let stream = futures_util::stream::unfold((chunks.into_iter(), 0usize), |(mut it, n)| async move {
        let chunk = it.next()?;
        if n % 20 == 19 {
            tokio::time::sleep(Duration::from_millis(2)).await;
        } else {
            tokio::task::yield_now().await;
        }
        Some((Ok::<_, BoxError>(Frame::data(chunk)), (it, n + 1)))
    });
    StreamBody::new(stream).boxed()
}

fn full(status: u16, headers: &[(&str, &str)], body: impl Into<Bytes>) -> Response<MockBody> {
    let mut b = Response::builder().status(status);
    for (k, v) in headers {
        b = b.header(*k, *v);
    }
    b.body(Full::new(body.into()).map_err(|n| match n {}).boxed()).unwrap()
}

async fn mock(req: Request<Incoming>, log: Log) -> Response<MockBody> {
    let method = req.method().to_string();
    let uri = req.uri().path_and_query().map(|p| p.to_string()).unwrap_or_default();
    let headers = req.headers().clone();
    let body = req.into_body().collect().await.unwrap().to_bytes().to_vec();
    log.lock().unwrap().push(Seen { method: method.clone(), uri: uri.clone(), headers, body: body.clone() });

    let text = String::from_utf8_lossy(&body).to_string();
    let phs = placeholders(&text);
    let key_ph = phs.iter().find(|p| p.starts_with("{{API_KEY")).cloned().unwrap_or("{{API_KEY_9}}".into());
    let email_ph = phs.iter().find(|p| p.starts_with("{{EMAIL")).cloned().unwrap_or("{{EMAIL_9}}".into());
    let path = uri.split('?').next().unwrap_or("").to_string();

    if uri.contains("status=429") {
        return full(
            429,
            &[
                ("content-type", "application/json"),
                ("retry-after", "7"),
                ("x-should-retry", "true"),
                ("anthropic-ratelimit-unified-status", "rejected"),
                ("request-id", "req_429"),
            ],
            r#"{"type":"error","error":{"type":"rate_limit_error","message":"Number of requests has exceeded your rate limit"}}"#,
        );
    }
    match (method.as_str(), path.as_str()) {
        ("POST", "/v1/messages") => {
            let req: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
            if req["stream"] == json!(true) {
                let mut r = Response::new(chunked(sse_script(&key_ph, &email_ph).concat()));
                r.headers_mut().insert("content-type", "text/event-stream; charset=utf-8".parse().unwrap());
                r.headers_mut().insert("request-id", "req_sse".parse().unwrap());
                r.headers_mut().insert("anthropic-ratelimit-unified-5h-utilization", "0.25".parse().unwrap());
                r
            } else {
                full(200, &[("content-type", "application/json"), ("request-id", "req_json")], json_message(&key_ph, &email_ph).to_string())
            }
        }
        ("POST", "/v1/messages/count_tokens") => full(200, &[("content-type", "application/json")], r#"{"input_tokens":42}"#),
        _ => {
            let echo = json!({ "method": method, "uri": uri, "body": text });
            full(200, &[("content-type", "application/json"), ("x-mock", "yes")], echo.to_string())
        }
    }
}

struct Harness {
    base: String,
    upstream_log: Log,
    shared: Arc<Shared>,
    client: reqwest::Client,
}

async fn start_mock(log: Log) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else { continue };
            let log = log.clone();
            tokio::spawn(async move {
                let svc = service_fn(move |req| {
                    let log = log.clone();
                    async move { Ok::<_, Infallible>(mock(req, log).await) }
                });
                let _ = http1::Builder::new().serve_connection(TokioIo::new(stream), svc).await;
            });
        }
    });
    format!("http://{addr}")
}

async fn harness() -> Harness {
    harness_with(zuko_core::policy::Policy::default()).await
}

async fn harness_with(policy: zuko_core::policy::Policy) -> Harness {
    let upstream_log: Log = Arc::default();
    let upstream = start_mock(upstream_log.clone()).await;
    let engine = Engine::with_parts(policy, zuko_core::vault::Vault::new(), CtxBase::default());
    let host = Host::Headless(Headless { engine, verbose: false, dump: None });
    let shared = Arc::new(Shared::new(host, TOKEN.into(), upstream).unwrap());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}/t/{TOKEN}", listener.local_addr().unwrap());
    proxy::spawn(listener, shared.clone());
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    Harness { base, upstream_log, shared, client }
}

impl Harness {
    fn seen(&self) -> Vec<Seen> {
        self.upstream_log.lock().unwrap().clone()
    }

    fn last_body(&self) -> String {
        String::from_utf8(self.seen().last().unwrap().body.clone()).unwrap()
    }

    async fn post(&self, path: &str, body: &Value) -> reqwest::Response {
        self.client
            .post(format!("{}{path}", self.base))
            .header("anthropic-version", "2023-06-01")
            .header("anthropic-beta", "oauth-2025-04-20,claude-code-20250219")
            .header("authorization", "Bearer sk-ant-oat01-fake")
            .header("x-claude-code-session-id", "sess-1")
            .json(body)
            .send()
            .await
            .unwrap()
    }
}

fn prompt_request(stream: bool) -> Value {
    json!({
        "model": "claude-haiku-mock",
        "max_tokens": 1024,
        "stream": stream,
        "system": [{"type": "text", "text": "x-anthropic-billing-header: cc_version=1"}, {"type": "text", "text": "You are Claude Code.", "cache_control": {"type": "ephemeral"}}],
        "messages": [{"role": "user", "content": [{"type": "text", "text": format!("My OpenAI key is {KEY} and my email is {EMAIL}. Put the key in .env")}]}]
    })
}

/// Splits an SSE byte stream into events (each ending with its blank line).
fn events(s: &str) -> Vec<String> {
    s.split_inclusive("\n\n").map(str::to_string).collect()
}

fn data(event: &str) -> Value {
    let line = event.lines().find_map(|l| l.strip_prefix("data: ")).unwrap_or("null");
    serde_json::from_str(line).unwrap()
}

fn is_delta(event: &str, index: u64, kind: &str) -> bool {
    let d = data(event);
    d["type"] == "content_block_delta" && d["index"] == index && d["delta"]["type"] == kind
}

#[tokio::test]
async fn streamed_messages_are_masked_and_rehydrated() {
    let h = harness().await;
    let resp = h.post("/v1/messages?beta=true", &prompt_request(true)).await;
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.headers()["content-type"], "text/event-stream; charset=utf-8");
    assert_eq!(resp.headers()["request-id"], "req_sse");
    assert_eq!(resp.headers()["anthropic-ratelimit-unified-5h-utilization"], "0.25");
    let out = resp.text().await.unwrap();

    // What left the machine: placeholders only, query and auth headers intact.
    let seen = h.seen();
    assert_eq!(seen.len(), 1);
    let up = &seen[0];
    assert_eq!(up.uri, "/v1/messages?beta=true");
    let sent = String::from_utf8(up.body.clone()).unwrap();
    assert!(!sent.contains(KEY) && !sent.contains(EMAIL), "raw secret reached upstream: {sent}");
    assert!(!sent.contains("ZUKOTEST"));
    assert!(sent.contains("{{API_KEY_1}}") && sent.contains("{{EMAIL_1}}"));
    assert!(!sent.contains("\n  "), "re-serialized compactly");
    assert_eq!(up.headers["anthropic-beta"], "oauth-2025-04-20,claude-code-20250219");
    assert_eq!(up.headers["authorization"], "Bearer sk-ant-oat01-fake");
    assert_eq!(up.headers["anthropic-version"], "2023-06-01");
    assert_eq!(up.headers["accept-encoding"], "identity");
    assert_eq!(up.headers["content-length"], up.body.len().to_string().as_str());
    let sent_json: Value = serde_json::from_str(&sent).unwrap();
    assert_eq!(sent_json["system"][0]["text"], "x-anthropic-billing-header: cc_version=1", "attribution block stays first");
    assert!(sent_json["system"].as_array().unwrap().len() == 3, "legend appended as the last system block");

    // What the client got.
    let up_events: Vec<String> = sse_script("{{API_KEY_1}}", "{{EMAIL_1}}");
    let got = events(&out);
    let text: String = got.iter().filter(|e| is_delta(e, 0, "text_delta")).map(|e| data(e)["delta"]["text"].as_str().unwrap().to_string()).collect();
    assert_eq!(text, format!("Your key is {KEY} and mail {EMAIL}. Done — ✓"));

    let write: Vec<&String> = got.iter().filter(|e| is_delta(e, 1, "input_json_delta")).collect();
    assert_eq!(write.len(), 1, "Write input is buffered into one delta");
    let input: Value = serde_json::from_str(data(write[0])["delta"]["partial_json"].as_str().unwrap()).unwrap();
    assert_eq!(input, json!({"file_path": ".env", "content": format!("OPENAI_API_KEY={KEY}\n")}));

    let bash: String = got.iter().filter(|e| is_delta(e, 2, "input_json_delta")).map(|e| data(e)["delta"]["partial_json"].as_str().unwrap().to_string()).collect();
    assert!(bash.contains("Bearer {{API_KEY_1}}") && !bash.contains(KEY), "Bash input keeps the placeholder: {bash}");

    // Every other event, pings included, byte-identical and in order.
    let others = |v: &[String]| -> Vec<String> {
        v.iter().filter(|e| !is_delta(e, 0, "text_delta") && !is_delta(e, 1, "input_json_delta")).cloned().collect()
    };
    assert_eq!(others(&got), others(&up_events));
    assert_eq!(got.iter().filter(|e| e.starts_with("event: ping")).count(), 2);
    assert!(out.ends_with(&up_events[up_events.len() - 1]));
}

#[tokio::test]
async fn json_messages_and_history_round_trip() {
    let h = harness().await;
    let resp = h.post("/v1/messages", &prompt_request(false)).await;
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.headers()["request-id"], "req_json");
    let reply: Value = resp.json().await.unwrap();
    let first_sent: Value = serde_json::from_str(&h.last_body()).unwrap();
    assert!(!h.last_body().contains(KEY));

    let content = reply["content"].clone();
    assert_eq!(content[0]["text"], format!("Key {KEY}, mail {EMAIL}"));
    assert_eq!(content[1]["input"]["content"], format!("K={KEY}"));
    assert_eq!(content[2]["input"]["command"], "echo {{API_KEY_1}}", "Bash input is not rehydrated");

    // Next turn: Claude Code sends the assistant reply (with real values) back.
    let mut next = prompt_request(false);
    next["messages"].as_array_mut().unwrap().extend([
        json!({"role": "assistant", "content": content}),
        json!({"role": "user", "content": [
            {"type": "tool_result", "tool_use_id": "toolu_w", "content": "File written"},
            {"type": "tool_result", "tool_use_id": "toolu_b", "content": format!("{KEY}\n")}
        ]}),
    ]);
    let resp = h.post("/v1/messages", &next).await;
    assert_eq!(resp.status(), 200);
    let sent = h.last_body();
    assert!(!sent.contains(KEY) && !sent.contains(EMAIL), "history leaked: {sent}");
    let sent: Value = serde_json::from_str(&sent).unwrap();
    // The re-masked history is identical to what the upstream produced and was sent first.
    assert_eq!(sent["messages"][0], first_sent["messages"][0]);
    assert_eq!(sent["messages"][1]["content"], json_message("{{API_KEY_1}}", "{{EMAIL_1}}")["content"]);
    assert_eq!(sent["messages"][2]["content"][1]["content"], "{{API_KEY_1}}\n");
    assert_eq!(placeholders(&sent.to_string()).iter().filter(|p| p.contains("_2")).count(), 0, "no new keys for known values");
}

#[tokio::test]
async fn count_tokens_is_masked() {
    let h = harness().await;
    let mut body = prompt_request(false);
    body.as_object_mut().unwrap().remove("max_tokens");
    let resp = h.post("/v1/messages/count_tokens?beta=true", &body).await;
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.text().await.unwrap(), r#"{"input_tokens":42}"#);
    let seen = h.seen();
    assert_eq!(seen[0].uri, "/v1/messages/count_tokens?beta=true");
    let sent = h.last_body();
    assert!(!sent.contains(KEY) && sent.contains("{{API_KEY_1}}"));
}

#[tokio::test]
async fn path_tricks_cannot_skip_masking() {
    let h = harness().await;
    for path in ["/v1/./messages", "/v1//messages/", "/v1/%6Dessages", "/x/../V1/messages"] {
        let resp = h.post(&format!("{path}?beta=true"), &prompt_request(false)).await;
        assert_eq!(resp.status(), 200, "{path}");
        let seen = h.seen();
        assert_eq!(seen.last().unwrap().uri, "/v1/messages?beta=true", "{path}");
        assert!(!h.last_body().contains(KEY), "{path}");
    }
}

#[tokio::test]
async fn rejects_bad_token_and_browsers() {
    let h = harness().await;
    let root = h.base.trim_end_matches(TOKEN).trim_end_matches("/t/").to_string();
    let wrong = format!("{root}/t/{}", "f".repeat(64));
    for url in [format!("{root}/v1/messages"), format!("{wrong}/v1/messages"), format!("{root}/t/"), format!("{root}/t/{TOKEN}x/v1/messages")] {
        let resp = h.client.post(&url).json(&prompt_request(false)).send().await.unwrap();
        assert_eq!(resp.status(), 403, "{url}");
    }
    let resp = h
        .client
        .post(format!("{}/v1/messages", h.base))
        .header("origin", "https://evil.example")
        .json(&prompt_request(false))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 403);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["type"], "error");
    assert!(h.seen().is_empty(), "nothing reached the upstream");
}

#[tokio::test]
async fn other_paths_pass_through() {
    let h = harness().await;
    let resp = h.client.get(format!("{}/v1/models?limit=5", h.base)).header("x-api-key", "k").send().await.unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.headers()["x-mock"], "yes");
    let echo: Value = resp.json().await.unwrap();
    assert_eq!(echo["method"], "GET");
    assert_eq!(echo["uri"], "/v1/models?limit=5");
    assert_eq!(h.seen()[0].headers["x-api-key"], "k");

    // A POST elsewhere is not inspected: the body arrives byte for byte.
    let raw = format!(r#"{{"note": "{KEY}"}}"#);
    let resp = h.client.post(format!("{}/v1/files", h.base)).body(raw.clone()).send().await.unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(h.last_body(), raw);

    // HEAD probe (Claude Code's connection warm-up).
    let resp = h.client.head(format!("{}/api/hello", h.base)).send().await.unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(h.seen().last().unwrap().method, "HEAD");
}

#[tokio::test]
async fn upstream_errors_pass_through() {
    let h = harness().await;
    let resp = h.post("/v1/messages?status=429", &prompt_request(true)).await;
    assert_eq!(resp.status(), 429);
    assert_eq!(resp.headers()["retry-after"], "7");
    assert_eq!(resp.headers()["x-should-retry"], "true");
    assert_eq!(resp.headers()["anthropic-ratelimit-unified-status"], "rejected");
    assert_eq!(resp.headers()["request-id"], "req_429");
    assert_eq!(
        resp.text().await.unwrap(),
        r#"{"type":"error","error":{"type":"rate_limit_error","message":"Number of requests has exceeded your rate limit"}}"#
    );
    assert!(!h.last_body().contains(KEY), "masked even when the upstream refuses");
}

#[tokio::test]
async fn unreachable_upstream_is_a_502() {
    let h = harness().await;
    // A port nothing listens on.
    let dead = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap();
    let upstream = format!("http://{dead}");
    h.shared.set_upstream(upstream.clone());
    let resp = h.post("/v1/messages", &prompt_request(false)).await;
    assert_eq!(resp.status(), 502);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["type"], "error");
    assert_eq!(body["error"]["type"], "api_error");
    let message = body["error"]["message"].as_str().unwrap();
    assert!(message.starts_with(&format!("Zuko gateway could not reach {upstream}: ")), "{message}");
    assert!(!message.contains(KEY));
}

#[tokio::test]
async fn upstream_disconnect_mid_stream_is_propagated() {
    // An upstream that sends half a stream and then drops the connection.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let (mut s, _) = listener.accept().await.unwrap();
        let mut buf = vec![0u8; 65536];
        let _ = s.read(&mut buf).await;
        let part = sse_script("{{API_KEY_1}}", "{{EMAIL_1}}")[..3].concat();
        let head = format!("HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\n\r\n", part.len() + 1000);
        s.write_all(head.as_bytes()).await.unwrap();
        s.write_all(part.as_bytes()).await.unwrap();
        s.flush().await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        drop(s);
    });
    let h = harness().await;
    h.shared.set_upstream(format!("http://{addr}"));
    let resp = h.post("/v1/messages", &prompt_request(true)).await;
    assert_eq!(resp.status(), 200);
    assert!(resp.text().await.is_err(), "the client must see a broken stream, not a clean end");
}

// ── Local AI (mock Ollama) ────────────────────────────────────────────────────

const NAME_PROMPT: &str = "Please email the contract to Rahim Uddin at House 12, Road 5, Dhanmondi, Dhaka";

fn name_request() -> Value {
    json!({
        "model": "claude-haiku-mock", "max_tokens": 64, "stream": false,
        "messages": [{"role": "user", "content": [
            {"type": "text", "text": "<system-reminder>context</system-reminder>"},
            {"type": "text", "text": NAME_PROMPT}
        ]}]
    })
}

fn ai_policy(ollama: &str, wait: bool, timeout_ms: u64) -> zuko_core::policy::Policy {
    let mut p = zuko_core::policy::Policy::default();
    p.local_ai = zuko_core::localai::LocalAiConfig {
        enabled: true,
        endpoint: ollama.into(),
        wait_for_prompt_scan: wait,
        timeout_ms,
        ..Default::default()
    };
    p
}

#[tokio::test]
async fn waiting_for_the_ai_scan_masks_a_name_on_its_first_send() {
    let ollama = crate::localai::mock::start(Default::default()).await;
    let h = harness_with(ai_policy(&ollama.url, true, 3000)).await;
    assert_eq!(h.post("/v1/messages", &name_request()).await.status(), 200);
    let sent = h.last_body();
    assert!(!sent.contains("Rahim Uddin") && !sent.contains("Dhanmondi"), "raw PII reached upstream: {sent}");
    assert!(sent.contains("{{NAME_1}}") && sent.contains("{{ADDRESS_1}}"));
    // The model saw only the user's text, not the system reminder.
    assert!(!ollama.chat_bodies().concat().contains("system-reminder>context"));
}

#[tokio::test]
async fn background_ai_scans_mask_later_requests_and_never_delay_this_one() {
    let ollama = crate::localai::mock::start(crate::localai::mock::Script { delay: Duration::from_millis(300), ..Default::default() }).await;
    let h = harness_with(ai_policy(&ollama.url, false, 3000)).await;
    let started = std::time::Instant::now();
    assert_eq!(h.post("/v1/messages", &name_request()).await.status(), 200);
    assert!(started.elapsed() < Duration::from_millis(280), "the request waited for the model");
    // The known limitation: the first send of a new name goes out as typed.
    assert!(h.last_body().contains("Rahim Uddin"));
    // Once the background scan is done, every later request masks it.
    for _ in 0..100 {
        if h.shared.host.engine().unwrap().vault_snapshot().len() >= 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(h.post("/v1/messages", &name_request()).await.status(), 200);
    let sent = h.last_body();
    assert!(!sent.contains("Rahim Uddin") && sent.contains("{{NAME_1}}"), "{sent}");
}

#[tokio::test]
async fn a_dead_or_lying_model_leaves_the_gateway_exactly_as_before() {
    // An invented finding, wait mode: forwarded with deterministic masking only.
    let liar = crate::localai::mock::start(crate::localai::mock::Script {
        content: Some(r#"{"findings":[{"kind":"NAME","value":"Not In The Text"}]}"#.into()),
        ..Default::default()
    })
    .await;
    let h = harness_with(ai_policy(&liar.url, true, 3000)).await;
    assert_eq!(h.post("/v1/messages", &prompt_request(false)).await.status(), 200);
    let sent = h.last_body();
    assert!(sent.contains("{{API_KEY_1}}") && sent.contains("{{EMAIL_1}}") && !sent.contains(KEY));
    assert_eq!(h.shared.host.engine().unwrap().vault_snapshot().len(), 2, "nothing invented was added");
    // The model never saw the key or the email: it got the masked text.
    let shown = liar.chat_bodies().concat();
    assert!(!shown.contains(KEY) && !shown.contains(EMAIL) && shown.contains("{{API_KEY_1}}"));

    // A model that never answers: the request waits at most the timeout, then goes.
    let slow = crate::localai::mock::start(crate::localai::mock::Script { delay: Duration::from_secs(10), ..Default::default() }).await;
    let h = harness_with(ai_policy(&slow.url, true, 500)).await;
    let started = std::time::Instant::now();
    assert_eq!(h.post("/v1/messages", &prompt_request(false)).await.status(), 200);
    assert!(started.elapsed() < Duration::from_millis(2500), "took {:?}", started.elapsed());
    assert!(!h.last_body().contains(KEY));

    // Nothing listening at all.
    let h = harness_with(ai_policy("http://127.0.0.1:1", true, 500)).await;
    assert_eq!(h.post("/v1/messages", &prompt_request(false)).await.status(), 200);
    assert!(!h.last_body().contains(KEY));
}
