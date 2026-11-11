//! The message layer between a browser frame and the app's pipe line (CONTRACTS.md §4).
//!
//! Browser → app: the frame's JSON is wrapped as
//! `{"hook_event_name":"ZukoExtension","zuko_wants_reply":true,"message":<msg>}`, one line.
//! App → browser: the app answers one line `{"reply":<message>}`; we unwrap it.
//! Failures never go silent: every frame in gets exactly one frame out, and an unreachable
//! app is reported as `{"ok":false,"error":"Zuko desktop app is not running"}`.
//!
//! Extra: if the browser's message carries an `"id"`, it is echoed on the reply (the app
//! does not need to know about it), so the extension can match answers to requests even
//! after a timeout.

use serde_json::{json, Map, Value};

use crate::frame::MAX_OUTBOUND;

pub const APP_NOT_RUNNING: &str = "Zuko desktop app is not running";

/// Why a round trip to the app produced no reply.
#[derive(Debug, PartialEq, Eq)]
pub enum TransportError {
    /// Nothing is serving the pipe (or it is somebody else's): the app is closed.
    Unreachable,
    /// Connected, but no answer arrived within the budget.
    Timeout,
    /// Connected, wrote, but the reply was empty, cut short or unreadable.
    BadReply(String),
}

/// One request/response exchange with the app: send a line, get a line back.
pub trait Transport {
    /// `line` has no trailing newline; the reply must not either.
    fn exchange(&self, line: &str) -> Result<String, TransportError>;
}

/// The id of a browser message, if it is an object that has one.
pub fn message_id(msg: &Value) -> Option<Value> {
    msg.get("id").filter(|v| !v.is_null()).cloned()
}

/// The one line sent to the app.
pub fn request_line(msg: &Value) -> String {
    json!({ "hook_event_name": "ZukoExtension", "zuko_wants_reply": true, "message": msg }).to_string()
}

/// `{"ok":false,"error":…}`, carrying the request id when there is one.
pub fn error_reply(error: &str, id: Option<&Value>) -> Value {
    let mut m = Map::new();
    m.insert("ok".into(), Value::Bool(false));
    m.insert("error".into(), Value::String(error.to_string()));
    if let Some(id) = id {
        m.insert("id".into(), id.clone());
    }
    Value::Object(m)
}

/// Turns the app's reply line into the message for the browser.
fn unwrap_reply(line: &str, id: Option<&Value>) -> Value {
    let Ok(parsed) = serde_json::from_str::<Value>(line.trim()) else {
        return error_reply("Zuko sent an unreadable reply", id);
    };
    let Some(mut reply) = parsed.get("reply").cloned() else {
        return error_reply("Zuko sent an unexpected reply", id);
    };
    if let (Some(id), Some(obj)) = (id, reply.as_object_mut()) {
        obj.entry("id").or_insert_with(|| id.clone());
    }
    reply
}

/// Handles one browser message end to end and returns the bytes of the answer frame.
/// The answer is guaranteed to fit the 1 MiB host → browser cap.
pub fn handle(transport: &dyn Transport, raw: &[u8]) -> Vec<u8> {
    let msg: Value = match serde_json::from_slice(raw) {
        Ok(v) => v,
        Err(_) => return finish(error_reply("message from the browser is not valid JSON", None), None),
    };
    let id = message_id(&msg);
    let reply = match transport.exchange(&request_line(&msg)) {
        Ok(line) => unwrap_reply(&line, id.as_ref()),
        Err(TransportError::Unreachable) => error_reply(APP_NOT_RUNNING, id.as_ref()),
        Err(TransportError::Timeout) => error_reply("Zuko desktop app did not answer in time", id.as_ref()),
        Err(TransportError::BadReply(why)) => error_reply(&format!("Zuko desktop app sent a bad reply: {why}"), id.as_ref()),
    };
    finish(reply, id.as_ref())
}

/// Serializes `reply`, replacing it with an error if it would exceed the 1 MiB cap.
pub fn finish(reply: Value, id: Option<&Value>) -> Vec<u8> {
    let bytes = reply.to_string().into_bytes();
    if bytes.len() <= MAX_OUTBOUND {
        return bytes;
    }
    error_reply(
        &format!("reply of {} bytes exceeds the 1 MB native messaging limit", bytes.len()),
        id,
    )
    .to_string()
    .into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    /// Records the lines it is given and plays back a canned result.
    struct Fake {
        result: Result<String, TransportError>,
        seen: RefCell<Vec<String>>,
    }
    impl Fake {
        fn new(result: Result<String, TransportError>) -> Self {
            Fake { result, seen: RefCell::new(Vec::new()) }
        }
    }
    impl Transport for Fake {
        fn exchange(&self, line: &str) -> Result<String, TransportError> {
            self.seen.borrow_mut().push(line.to_string());
            match &self.result {
                Ok(s) => Ok(s.clone()),
                Err(TransportError::Unreachable) => Err(TransportError::Unreachable),
                Err(TransportError::Timeout) => Err(TransportError::Timeout),
                Err(TransportError::BadReply(s)) => Err(TransportError::BadReply(s.clone())),
            }
        }
    }

    fn parse(bytes: &[u8]) -> Value {
        serde_json::from_slice(bytes).unwrap()
    }

    #[test]
    fn wraps_the_request_for_the_app() {
        let t = Fake::new(Ok(r#"{"reply":{"ok":true}}"#.into()));
        handle(&t, br#"{"op":"hello","version":"0.1.0"}"#);
        let sent: Value = serde_json::from_str(&t.seen.borrow()[0]).unwrap();
        assert_eq!(sent["hook_event_name"], "ZukoExtension");
        assert_eq!(sent["message"], json!({"op":"hello","version":"0.1.0"}));
        assert!(!t.seen.borrow()[0].contains('\n'), "the request must be a single line");
    }

    #[test]
    fn unwraps_the_reply() {
        let t = Fake::new(Ok(r#"{"reply":{"ok":true,"app":"zuko","version":"0.1.1"}}"#.into()));
        let out = parse(&handle(&t, br#"{"op":"hello"}"#));
        assert_eq!(out, json!({"ok":true,"app":"zuko","version":"0.1.1"}));
    }

    #[test]
    fn echoes_the_request_id() {
        let t = Fake::new(Ok(r#"{"reply":{"ok":true,"text":"x"}}"#.into()));
        let out = parse(&handle(&t, br#"{"op":"mask","text":"y","id":7}"#));
        assert_eq!(out["id"], 7);
        assert_eq!(out["text"], "x");
    }

    #[test]
    fn app_unreachable_is_a_clean_error() {
        let t = Fake::new(Err(TransportError::Unreachable));
        let out = parse(&handle(&t, br#"{"op":"hello","id":"a1"}"#));
        assert_eq!(out, json!({"ok":false,"error":"Zuko desktop app is not running","id":"a1"}));
        let out = parse(&handle(&t, br#"{"op":"hello"}"#));
        assert_eq!(out, json!({"ok":false,"error":"Zuko desktop app is not running"}));
    }

    #[test]
    fn timeouts_and_bad_replies_are_errors_not_silence() {
        let t = Fake::new(Err(TransportError::Timeout));
        assert_eq!(parse(&handle(&t, br#"{"op":"vault"}"#))["ok"], false);
        let t = Fake::new(Ok("not json".into()));
        assert_eq!(parse(&handle(&t, br#"{"op":"vault"}"#))["ok"], false);
        let t = Fake::new(Ok(r#"{"stdout":null}"#.into()));
        assert_eq!(parse(&handle(&t, br#"{"op":"vault"}"#))["ok"], false);
        let t = Fake::new(Err(TransportError::BadReply("eof".into())));
        assert!(parse(&handle(&t, br#"{"op":"vault"}"#))["error"].as_str().unwrap().contains("eof"));
    }

    #[test]
    fn invalid_json_from_the_browser_never_reaches_the_app() {
        let t = Fake::new(Ok(r#"{"reply":{"ok":true}}"#.into()));
        let out = parse(&handle(&t, b"{nope"));
        assert_eq!(out["ok"], false);
        assert!(t.seen.borrow().is_empty());
    }

    #[test]
    fn oversized_reply_becomes_an_error() {
        let big = "a".repeat(MAX_OUTBOUND);
        let line = json!({"reply":{"ok":true,"vault":big}}).to_string();
        let t = Fake::new(Ok(line));
        let bytes = handle(&t, br#"{"op":"vault","id":3}"#);
        assert!(bytes.len() <= MAX_OUTBOUND);
        let out = parse(&bytes);
        assert_eq!(out["ok"], false);
        assert_eq!(out["id"], 3);
        assert!(out["error"].as_str().unwrap().contains("1 MB"));
    }

    #[test]
    fn a_reply_just_under_the_cap_passes() {
        // {"ok":true,"t":""} is 18 bytes of JSON around the string.
        let body = "a".repeat(MAX_OUTBOUND - 18);
        let reply = json!({"ok":true,"t":body});
        assert_eq!(reply.to_string().len(), MAX_OUTBOUND);
        assert_eq!(finish(reply, None).len(), MAX_OUTBOUND);
    }
}
