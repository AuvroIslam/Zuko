//! Messages API request masking and response rehydration (JSON and SSE).

use serde_json::{json, Value};
use std::sync::LazyLock;
use zuko_core::anthropic::{mask_request, rehydrate_response, RequestOutcome, SinkPolicy, SseRehydrator};
use zuko_core::detect::{Category, Detector, DetectorConfig, Finding};
use zuko_core::mask::{legend, rehydrate_json, rehydrate_text, MaskCtx};
use zuko_core::vault::Vault;

// The fixtures store each fake key "defanged" (a U+00A6 marker after its third
// character) so secret scanners such as GitHub push protection do not flag them.
// The text the tests check is the original, with the markers removed.
static REQUEST: LazyLock<String> =
    LazyLock::new(|| include_str!("fixtures/anthropic_request.json").replace('\u{a6}', ""));
const STREAM: &str = include_str!("fixtures/anthropic_stream.sse");

const OPENAI: &str = "sk-proj-ZukoFake0123456789abcdefghijklmnopqrstuvwx";
const EMAIL: &str = "rahim.uddin@gmail.com";
const DB_PW: &str = "S3cr3t\"Pa\\ss-01";

fn ctx() -> MaskCtx {
    MaskCtx { source: "gateway".into(), now: 1_700_000_000 }
}

fn det() -> Detector {
    Detector::new(&DetectorConfig::default())
}

fn finding(value: &str, kind: &str) -> Finding {
    Finding {
        start: 0,
        end: value.len(),
        value: value.into(),
        kind: kind.into(),
        rule: "t".into(),
        label: format!("{kind} label"),
        category: Category::Secret,
        hint: None,
        confidence: 1.0,
    }
}

/// Vault as the gateway would have it after masking the user's first prompt.
fn stream_vault() -> Vault {
    let mut v = Vault::new();
    assert_eq!(v.intern(&finding(OPENAI, "API_KEY"), "t", 1).as_deref(), Some("API_KEY_1"));
    assert_eq!(v.intern(&finding(EMAIL, "EMAIL"), "t", 1).as_deref(), Some("EMAIL_1"));
    assert_eq!(v.intern(&finding(DB_PW, "PASSWORD"), "t", 1).as_deref(), Some("PASSWORD_1"));
    v
}

// ------------------------------------------------------------------------------------
// Requests
// ------------------------------------------------------------------------------------

#[test]
fn realistic_claude_code_request_is_masked_per_spec() {
    let original: Value = serde_json::from_str(&REQUEST).unwrap();
    let mut body = original.clone();
    let mut vault = Vault::new();
    let out = mask_request(&det(), &mut vault, &mut body, &ctx());

    // Nothing sensitive leaves the machine (thinking only ever held placeholders).
    let wire = serde_json::to_string(&body).unwrap();
    for secret in [OPENAI, EMAIL, "S3cr3tPassw0rd", "4242 4242 4242 4242", "+8801712345678", "maintainer.fake@acme-corp.io"] {
        assert!(!wire.contains(secret), "{secret} leaked");
    }

    // System: earlier blocks byte-identical except masking inside text; legend appended last.
    let sys = body["system"].as_array().unwrap();
    let osys = original["system"].as_array().unwrap();
    assert_eq!(sys.len(), osys.len() + 1);
    assert_eq!(sys[0], osys[0], "attribution block untouched");
    assert_eq!(sys[1], osys[1], "cache_control block untouched");
    assert_eq!(sys[2]["cache_control"], osys[2]["cache_control"]);
    assert!(sys[2]["text"].as_str().unwrap().ends_with("Contact for this repo: {{EMAIL_1}}"));
    assert_eq!(sys[3]["type"], "text");
    assert!(sys[3].get("cache_control").is_none());
    let keys = vec!["API_KEY_1".to_string(), "EMAIL_1".into(), "EMAIL_2".into(), "CONN_STRING_1".into(), "CARD_1".into(), "PHONE_1".into()];
    assert_eq!(sys[3]["text"], legend(&vault, &keys));
    assert!(out.legend_added);

    // tools, tool_choice, metadata, thinking config: untouched.
    for f in ["tools", "tool_choice", "metadata", "thinking", "model", "max_tokens", "stream", "temperature"] {
        assert_eq!(body[f], original[f], "{f}");
    }

    let msgs = body["messages"].as_array().unwrap();
    // User text: full masking; system-reminder untouched.
    assert_eq!(msgs[0]["content"][0], original["messages"][0]["content"][0]);
    assert_eq!(msgs[0]["content"][1]["text"], "Add my OpenAI key {{API_KEY_1}} to .env and email the report to {{EMAIL_2}}");
    // Assistant: thinking + redacted_thinking byte-identical; text and tool_use input known-masked.
    assert_eq!(msgs[1]["content"][0], original["messages"][1]["content"][0]);
    assert_eq!(msgs[1]["content"][1], original["messages"][1]["content"][1]);
    assert_eq!(msgs[1]["content"][2]["text"], "I'll add {{API_KEY_1}} to .env now.");
    assert_eq!(msgs[1]["content"][3]["input"]["content"], "OPENAI_API_KEY={{API_KEY_1}}\n");
    assert_eq!(msgs[1]["content"][3]["input"]["file_path"], "C:\\Users\\rahim\\proj\\.env");
    assert_eq!(msgs[1]["content"][3]["id"], "toolu_01ZukoFake");
    // tool_result: string and nested text blocks masked; images counted as opaque.
    assert_eq!(msgs[2], original["messages"][2], "no secrets in this tool result");
    assert_eq!(msgs[3], original["messages"][3]);
    let tr = &msgs[4]["content"][0]["content"];
    assert_eq!(tr[0]["text"], "OPENAI_API_KEY={{API_KEY_1}}\n{\"db\": \"{{CONN_STRING_1}}\"}");
    assert_eq!(tr[1], original["messages"][4]["content"][0]["content"][1], "image untouched");
    // Text documents masked, base64 documents untouched.
    assert_eq!(msgs[4]["content"][1]["source"]["data"], "Customer card {{CARD_1}}, phone {{PHONE_1}}");
    assert_eq!(msgs[4]["content"][1]["title"], "customer.txt");
    assert_eq!(msgs[4]["content"][2], original["messages"][4]["content"][2]);
    assert_eq!(msgs[4]["content"][3]["text"], "Now commit it. My backup mail is {{EMAIL_2}}");
    assert_eq!(msgs[4]["content"][3]["cache_control"], json!({"type": "ephemeral"}));

    assert_eq!(out.opaque_blocks, 2);
    assert_eq!(out.known_replaced, 2);
    assert_eq!(out.masked.keys.len(), 6, "{:?}", out.masked.keys);
    assert_eq!(out.masked.new_keys.len(), 6);

    // Deterministic: the next turn re-sends the same history and gets identical bytes.
    let mut again = original.clone();
    let out2 = mask_request(&det(), &mut vault, &mut again, &ctx());
    assert_eq!(serde_json::to_string(&again).unwrap(), wire);
    assert!(out2.masked.new_keys.is_empty());

    // The masked body is a fixed point (idempotent).
    let mut twice = body.clone();
    twice["system"].as_array_mut().unwrap().pop();
    let out3 = mask_request(&det(), &mut vault, &mut twice, &ctx());
    assert_eq!(out3.masked.count, 0);
    assert_eq!(out3.known_replaced, 0);
    assert_eq!(serde_json::to_string(&twice).unwrap(), wire);
}

#[test]
fn string_system_and_string_content() {
    let mut vault = Vault::new();
    let mut body = json!({
        "model": "claude-haiku-4-5",
        "system": "Ops contact: ops.fake@acme-corp.io",
        "messages": [
            {"role": "user", "content": "card 4242424242424242"},
            {"role": "assistant", "content": "Noted 4242424242424242."},
        ]
    });
    let out = mask_request(&det(), &mut vault, &mut body, &ctx());
    assert_eq!(body["system"][0], json!({"type": "text", "text": "Ops contact: {{EMAIL_1}}"}));
    assert_eq!(body["system"][1]["type"], "text");
    assert!(body["system"][1]["text"].as_str().unwrap().contains("- {{CARD_1}}: Visa card ending 4242"));
    assert_eq!(body["messages"][0]["content"], "card {{CARD_1}}");
    assert_eq!(body["messages"][1]["content"], "Noted {{CARD_1}}.");
    assert_eq!(out.known_replaced, 1);
    assert!(out.legend_added);
}

#[test]
fn missing_or_empty_system_gets_only_the_legend() {
    for system in [None, Some(json!("")), Some(json!([]))] {
        let mut vault = Vault::new();
        let mut body = json!({"messages": [{"role": "user", "content": "mail bob.fake@acme-corp.io"}]});
        if let Some(s) = system {
            body["system"] = s;
        }
        mask_request(&det(), &mut vault, &mut body, &ctx());
        let sys = body["system"].as_array().unwrap();
        assert_eq!(sys.len(), 1);
        assert!(sys[0]["text"].as_str().unwrap().starts_with("Privacy note from Zuko"));
    }
}

#[test]
fn clean_request_is_unchanged_and_non_objects_are_ignored() {
    let mut vault = Vault::new();
    let original = json!({
        "system": [{"type": "text", "text": "You are helpful.", "cache_control": {"type": "ephemeral"}}],
        "messages": [
            {"role": "user", "content": [{"type": "text", "text": "Refactor fn load_config() in src/config.rs"}]},
            {"role": "assistant", "content": [{"type": "text", "text": "Sure."}]}
        ]
    });
    let mut body = original.clone();
    let out = mask_request(&det(), &mut vault, &mut body, &ctx());
    assert_eq!(body, original);
    assert_eq!(out, RequestOutcome::default());
    let mut not_obj = json!(["x"]);
    assert_eq!(mask_request(&det(), &mut vault, &mut not_obj, &ctx()), RequestOutcome::default());
    // count_tokens bodies have the same shape.
    let mut ct = json!({"model": "m", "messages": [{"role": "user", "content": "key sk-proj-ZukoFake0123456789abcdefghijklmnopqrstuvwx"}]});
    let out = mask_request(&det(), &mut vault, &mut ct, &ctx());
    assert_eq!(ct["messages"][0]["content"], "key {{API_KEY_1}}");
    assert!(out.legend_added);
}

#[test]
fn mid_conversation_system_messages_are_fully_masked() {
    let mut vault = Vault::new();
    let mut body = json!({"messages": [
        {"role": "user", "content": "hi"},
        {"role": "system", "content": [{"type": "text", "text": "Reminder: mail ops.fake@acme-corp.io", "cache_control": {"type": "ephemeral"}}]}
    ]});
    mask_request(&det(), &mut vault, &mut body, &ctx());
    assert_eq!(body["messages"][1]["content"][0]["text"], "Reminder: mail {{EMAIL_1}}");
    assert_eq!(body["messages"][1]["content"][0]["cache_control"]["type"], "ephemeral");
}

// ------------------------------------------------------------------------------------
// Non-streaming responses
// ------------------------------------------------------------------------------------

#[test]
fn non_streaming_response_rehydration_respects_sinks() {
    let vault = stream_vault();
    let original = json!({
        "id": "msg_01", "type": "message", "role": "assistant",
        "content": [
            {"type": "thinking", "thinking": "use {{API_KEY_1}}", "signature": "sig"},
            {"type": "text", "text": "Saved {{API_KEY_1}} and {{ EMAIL_1 }}; {{NOPE_1}} stays."},
            {"type": "tool_use", "id": "t1", "name": "Write", "input": {"file_path": ".env", "content": "K={{API_KEY_1}}\nP={{PASSWORD_1}}"}},
            {"type": "tool_use", "id": "t2", "name": "Bash", "input": {"command": "echo {{API_KEY_1}}"}}
        ],
        "stop_reason": "tool_use"
    });
    let mut body = original.clone();
    let keys = rehydrate_response(&vault, &mut body, &SinkPolicy::default());
    assert_eq!(keys, vec!["API_KEY_1", "EMAIL_1", "API_KEY_1", "PASSWORD_1"]);
    assert_eq!(body["content"][0], original["content"][0], "thinking untouched");
    assert_eq!(body["content"][1]["text"], format!("Saved {OPENAI} and {EMAIL}; {{{{NOPE_1}}}} stays."));
    assert_eq!(body["content"][2]["input"]["content"], format!("K={OPENAI}\nP={DB_PW}"));
    assert_eq!(body["content"][3], original["content"][3], "Bash is not a rehydration sink");
    // Text sink off.
    let mut body = original.clone();
    let keys = rehydrate_response(&vault, &mut body, &SinkPolicy { text: false, tools: vec![] });
    assert!(keys.is_empty());
    assert_eq!(body, original);
}

// ------------------------------------------------------------------------------------
// SSE
// ------------------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct Event {
    raw: String,
    data: Value,
}

/// Minimal SSE parser for assertions (handles \n, \r\n, comments, multi-line data).
fn parse_sse(s: &str) -> Vec<Event> {
    let norm = s.replace("\r\n", "\n");
    let mut out = Vec::new();
    for raw in norm.split_inclusive("\n\n") {
        let mut data = Vec::new();
        for line in raw.lines() {
            if let Some(d) = line.strip_prefix("data:") {
                data.push(d.strip_prefix(' ').unwrap_or(d).to_string());
            }
        }
        if data.is_empty() {
            continue;
        }
        let data: Value = serde_json::from_str(&data.join("\n")).unwrap_or_else(|e| panic!("bad JSON in {raw:?}: {e}"));
        out.push(Event { raw: raw.to_string(), data });
    }
    out
}

fn lf(s: &str) -> String {
    s.replace("\r\n", "\n")
}

fn run(vault: &Vault, sinks: &SinkPolicy, chunks: &[&[u8]]) -> (Vec<u8>, Vec<String>) {
    let mut r = SseRehydrator::new(sinks.clone());
    let mut out = Vec::new();
    for c in chunks {
        out.extend(r.push(vault, c));
    }
    out.extend(r.finish(vault));
    (out, r.keys())
}

fn is_delta_of(e: &Event, index: u64) -> bool {
    e.data["type"] == "content_block_delta" && e.data["index"] == index
}

/// Checks the semantic contract of a rehydrated stream against its input.
fn check_stream(vault: &Vault, input: &str, output: &str) {
    let ins = parse_sse(input);
    let outs = parse_sse(output);
    // Every event that is not a text/Write delta passes through byte-for-byte, in order.
    let rewritten = |e: &Event| is_delta_of(e, 1) || is_delta_of(e, 2);
    let a: Vec<String> = ins.iter().filter(|e| !rewritten(e)).map(|e| lf(&e.raw)).collect();
    let b: Vec<String> = outs.iter().filter(|e| !rewritten(e)).map(|e| lf(&e.raw)).collect();
    assert_eq!(a, b);
    // Text block 1: concatenated deltas equal the rehydrated text.
    let text_of = |evs: &[Event]| -> String {
        evs.iter().filter(|e| is_delta_of(e, 1)).map(|e| e.data["delta"]["text"].as_str().unwrap().to_string()).collect()
    };
    let want = rehydrate_text(vault, &text_of(&ins)).0;
    assert_eq!(text_of(&outs), want);
    assert!(want.contains(OPENAI) && want.contains(EMAIL) && want.contains("{{NOPE_1}}"));
    // The held-back tail ("{") is flushed right before the text block's stop.
    let stop1 = outs.iter().position(|e| e.data["type"] == "content_block_stop" && e.data["index"] == 1).unwrap();
    assert!(is_delta_of(&outs[stop1 - 1], 1));
    assert!(outs[stop1 - 1].data["delta"]["text"].as_str().unwrap().ends_with('{'));
    // Write block 2: exactly one input_json_delta, right before its stop, rehydrated.
    let w: Vec<usize> = outs.iter().enumerate().filter(|(_, e)| is_delta_of(e, 2)).map(|(i, _)| i).collect();
    assert_eq!(w.len(), 1, "one input_json_delta for the Write block");
    assert_eq!(outs[w[0] + 1].data["type"], "content_block_stop");
    assert_eq!(outs[w[0] + 1].data["index"], 2);
    let json_of = |evs: &[Event], i: u64| -> String {
        evs.iter().filter(|e| is_delta_of(e, i)).map(|e| e.data["delta"]["partial_json"].as_str().unwrap().to_string()).collect()
    };
    let mut want_input: Value = serde_json::from_str(&json_of(&ins, 2)).unwrap();
    rehydrate_json(vault, &mut want_input);
    let got_input: Value = serde_json::from_str(&json_of(&outs, 2)).unwrap();
    assert_eq!(got_input, want_input);
    assert_eq!(got_input["content"], format!("OPENAI_API_KEY={OPENAI}\nDB_PASSWORD={DB_PW}\nOWNER={EMAIL}\n"));
    assert_eq!(got_input["file_path"], "C:\\proj\\.env");
    // Key order of the tool input is preserved.
    let raw_input = json_of(&outs, 2);
    assert!(raw_input.find("file_path").unwrap() < raw_input.find("content").unwrap());
    // Bash block 3 keeps its placeholders; thinking untouched.
    assert!(json_of(&outs, 3).contains("Bearer {{API_KEY_1}}"));
    assert!(output.contains("Write {{API_KEY_1}} into .env, keep {{EMAIL_1}} private."));
}

fn variants() -> Vec<(&'static str, String)> {
    let base = lf(STREAM);
    let crlf = base.replace('\n', "\r\n");
    // Comments between and inside events, plus a keepalive comment-only event.
    let commented = base
        .replace("event: ping\n", ": keepalive\n\nevent: ping\n")
        .replace("event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":1}", "event: content_block_stop\n: comment inside an event\ndata: {\"type\":\"content_block_stop\",\"index\":1}");
    // Multi-line data on a text delta and on message_delta.
    let multiline = base
        .replace("data: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"text_delta\",\"text\":\"Saving {{API_\"}}", "data: {\"type\":\"content_block_delta\",\"index\":1,\ndata: \"delta\":{\"type\":\"text_delta\",\"text\":\"Saving {{API_\"}}")
        .replace("data: {\"type\":\"message_delta\",", "data: {\"type\":\"message_delta\",\ndata:");
    assert_ne!(commented, base);
    assert_ne!(multiline, base);
    vec![("lf", base), ("crlf", crlf), ("comments", commented), ("multiline", multiline)]
}

#[test]
fn sse_stream_rehydration_matches_spec() {
    let vault = stream_vault();
    for (name, input) in variants() {
        let (out, keys) = run(&vault, &SinkPolicy::default(), &[input.as_bytes()]);
        let out = String::from_utf8(out).unwrap();
        check_stream(&vault, &input, &out);
        assert_eq!(keys, vec!["API_KEY_1", "EMAIL_1", "API_KEY_1", "PASSWORD_1", "EMAIL_1"], "{name}");
        if name == "crlf" {
            assert!(!out.replace("\r\n", "").contains('\n'), "synthesized events keep CRLF");
        }
    }
}

#[test]
fn sse_split_at_every_byte_position_is_identical() {
    let vault = stream_vault();
    let sinks = SinkPolicy::default();
    for (name, input) in variants() {
        let bytes = input.as_bytes();
        let (whole, _) = run(&vault, &sinks, &[bytes]);
        for i in 0..=bytes.len() {
            let (got, _) = run(&vault, &sinks, &[&bytes[..i], &bytes[i..]]);
            assert!(got == whole, "{name}: split at byte {i} differs");
        }
        // One byte at a time (splits inside every UTF-8 sequence and every \r\n).
        let singles: Vec<&[u8]> = bytes.chunks(1).collect();
        assert!(run(&vault, &sinks, &singles).0 == whole, "{name}: byte-by-byte differs");
        // Three-way splits at a stride.
        for i in (0..bytes.len()).step_by(97) {
            for j in (i..bytes.len()).step_by(89) {
                let (got, _) = run(&vault, &sinks, &[&bytes[..i], &bytes[i..j], &bytes[j..]]);
                assert!(got == whole, "{name}: splits {i},{j} differ");
            }
        }
    }
}

#[test]
fn sse_unchanged_events_are_byte_identical_and_clean_streams_pass_through() {
    let empty = Vault::new();
    let input = lf(STREAM);
    let (out, keys) = run(&empty, &SinkPolicy::default(), &[input.as_bytes()]);
    // With an empty vault nothing can be rehydrated: only the Write input is re-emitted
    // as one delta and the text tail held back is flushed separately.
    let out = String::from_utf8(out).unwrap();
    assert!(keys.is_empty());
    let text: String = parse_sse(&out).iter().filter(|e| is_delta_of(e, 1)).map(|e| e.data["delta"]["text"].as_str().unwrap().to_string()).collect();
    assert!(text.contains("{{API_KEY_1}}"));
    // A stream without text or allowed tools is passed through byte-for-byte.
    let vault = stream_vault();
    let only_bash: String = input
        .split_inclusive("\n\n")
        .filter(|e| !e.contains("\"index\":1") && !e.contains("\"index\":2"))
        .collect();
    let (out, _) = run(&vault, &SinkPolicy::default(), &[only_bash.as_bytes()]);
    assert_eq!(String::from_utf8(out).unwrap(), only_bash);
    // Text sink off: text deltas pass through too.
    let (out, _) = run(&vault, &SinkPolicy { text: false, tools: vec![] }, &[input.as_bytes()]);
    assert_eq!(String::from_utf8(out).unwrap(), input);
}

#[test]
fn sse_error_and_truncated_streams() {
    let vault = stream_vault();
    let start = "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n";
    let delta = "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"mail {{EMAIL_\"}}\n\n";
    let error = "event: error\ndata: {\"type\":\"error\",\"error\":{\"type\":\"overloaded_error\",\"message\":\"Overloaded\"}}\n\n";
    let input = format!("{start}{delta}{error}");
    let (out, _) = run(&vault, &SinkPolicy::default(), &[input.as_bytes()]);
    let out = String::from_utf8(out).unwrap();
    let evs = parse_sse(&out);
    // Held text ("{{EMAIL_") is released before the error event, which passes unchanged.
    assert_eq!(evs.len(), 4);
    assert_eq!(evs[1].data["delta"]["text"], "mail ");
    assert_eq!(evs[2].data["delta"]["text"], "{{EMAIL_");
    assert_eq!(evs[3].raw, error);
    // Unterminated final event: passed through as received, after held content.
    let tail = "event: message_stop\ndata: {\"type\":\"message_stop\"}";
    let input = format!("{start}{delta}{tail}");
    let (out, _) = run(&vault, &SinkPolicy::default(), &[input.as_bytes()]);
    let out = String::from_utf8(out).unwrap();
    assert!(out.ends_with(&format!("{{{{EMAIL_\"}}}}\n\n{tail}")), "{out:?}");
    // Non-JSON data and unknown event types pass through.
    let odd = "event: mystery\ndata: not json\n\nevent: future_event\ndata: {\"type\":\"future_event\",\"index\":9}\n\n";
    let (out, _) = run(&vault, &SinkPolicy::default(), &[odd.as_bytes()]);
    assert_eq!(String::from_utf8(out).unwrap(), odd);
}

#[test]
fn sse_unparsable_tool_input_is_emitted_unchanged() {
    let vault = stream_vault();
    let s = concat!(
        "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"tool_use\",\"id\":\"t\",\"name\":\"Edit\",\"input\":{}}}\n\n",
        "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"old_string\\\": \\\"{{API_KEY_1}}\"}}\n\n",
        "event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
    );
    let (out, keys) = run(&vault, &SinkPolicy::default(), &[s.as_bytes()]);
    let evs = parse_sse(&String::from_utf8(out).unwrap());
    assert_eq!(evs.len(), 3);
    assert_eq!(evs[1].data["delta"]["partial_json"], "{\"old_string\": \"{{API_KEY_1}}");
    assert!(keys.is_empty());
}

#[test]
fn sse_rehydrator_is_send() {
    fn assert_send<T: Send>() {}
    assert_send::<SseRehydrator>();
}
