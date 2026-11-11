//! zuko-hook — the relay Claude Code runs on every hook event.
//!
//! Reads the hook JSON on stdin, adds a little context, and hands the **whole**
//! payload to Zuko over the named pipe `\\.\pipe\zuko-<sid>` (Windows) or the Unix
//! socket `$XDG_RUNTIME_DIR/zuko.sock` (Linux). The wire format is CONTRACTS.md §1:
//! one JSON line out, and for the events that wait, one `{"stdout": …}` line back
//! whose `stdout` object is printed verbatim.
//!
//! Hard rule: **never block Claude Code.**
//! * Every step runs on a worker thread under a deadline enforced by the main
//!   thread, so a pipe that accepts the connection and then stops reading cannot
//!   wedge the session: we abandon the worker and exit.
//! * Firewall events (`PreToolUse`, `PostToolUse`, `UserPromptSubmit`,
//!   `SessionStart`) get [`REPLY_BUDGET`]; `PermissionRequest` waits for a human
//!   for at most [`DECISION_BUDGET`]; everything else is fire-and-forget.
//! * When the app is closed, crashed or too slow, the relay does not simply go
//!   quiet: [`fallback`] runs the same `zuko-core` guard with the saved policy, so
//!   blocked domains and paths stay blocked and risky actions are still asked
//!   about. It never auto-approves on its own.
//!
//! Usage: `zuko-hook [--agent <name>] <EventName>` (the name is also read from the
//! JSON).

use std::io::{Read, Write};
use std::sync::mpsc;
use std::time::Duration;

use serde_json::{Map, Value};

mod fallback;

/// Budget for getting a pipe connection. Beyond this Claude Code wins, always.
const CONNECT_TIMEOUT: Duration = Duration::from_millis(300);
/// Whole-run budget for an event nobody waits on: connect and write, no more.
const FIRE_AND_FORGET_BUDGET: Duration = Duration::from_secs(2);
/// How long a firewall event waits for the app before the local fallback decides.
const REPLY_BUDGET: Duration = Duration::from_millis(1500);
/// How long a permission prompt may stay on screen before the terminal takes over.
/// The hook entry's own timeout is 120 s, so we always finish first.
const DECISION_BUDGET: Duration = Duration::from_secs(110);
/// Largest request line we send (CONTRACTS.md §1), and largest reply we accept.
const MAX_PAYLOAD: usize = 16 << 20;
/// Field cap applied only when a payload would otherwise exceed [`MAX_PAYLOAD`].
const OVERSIZE_FIELD_CAPS: &[usize] = &[1 << 20, 64 << 10, 2_000];

#[cfg(windows)]
mod win;
#[cfg(windows)]
use win::connect;

#[cfg(target_os = "linux")]
mod unix;
#[cfg(target_os = "linux")]
use unix::connect;

/// How long the relay waits for the app, by event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Wait {
    /// Send and exit.
    None,
    /// A firewall decision: [`REPLY_BUDGET`], then the local fallback.
    Reply,
    /// A human at the island: [`DECISION_BUDGET`], then the terminal asks.
    Decision,
}

fn wait_for(event: &str) -> Wait {
    match event {
        "PreToolUse" | "PostToolUse" | "UserPromptSubmit" | "SessionStart" => Wait::Reply,
        "PermissionRequest" => Wait::Decision,
        _ => Wait::None,
    }
}

/// What came back over the pipe.
#[derive(Debug, PartialEq)]
enum Answer {
    /// Nobody is listening (no pipe, or not ours).
    Unreachable,
    /// Delivered; nothing expected back.
    Sent,
    /// The app answered: the object to print, or `None` for "no opinion".
    Reply(Option<Value>),
    /// Delivered, but the app hung up without a usable reply.
    NoReply,
}

fn main() {
    let Some((payload, event)) = read_event() else { std::process::exit(0) };
    let wait = wait_for(&event);
    let line = request_line(&payload, wait != Wait::None);

    let budget = match wait {
        Wait::None => FIRE_AND_FORGET_BUDGET,
        Wait::Reply => REPLY_BUDGET,
        Wait::Decision => DECISION_BUDGET,
    };

    // The worker owns every blocking call. If it overruns the budget we stop
    // listening; exiting the process takes the pipe handle with it. (No
    // catch_unwind: the release profile is panic = "abort", and `talk` is written
    // to have nothing to panic on.)
    let (tx, rx) = mpsc::channel::<Answer>();
    std::thread::spawn(move || {
        let _ = tx.send(talk(&line, wait != Wait::None));
    });
    let answer = rx.recv_timeout(budget).ok();

    let out = match answer {
        Some(Answer::Reply(stdout)) => stdout,
        // The app decided nothing (or is not there): for firewall events the
        // stateless guard has the last word. A PermissionRequest nobody answered
        // prints nothing, so Claude Code asks in the terminal.
        _ if wait == Wait::Reply => fallback::decide(&event, &payload),
        _ => None,
    };
    if let Some(obj) = out {
        let mut stdout = std::io::stdout();
        let _ = writeln!(stdout, "{obj}");
        let _ = stdout.flush();
    }
    std::process::exit(0);
}

/// Reads stdin and returns the enriched payload plus the event name.
fn read_event() -> Option<(Value, String)> {
    let mut raw = Vec::new();
    if std::io::stdin().read_to_end(&mut raw).is_err() || raw.is_empty() {
        return None;
    }
    let args: Vec<String> = std::env::args().skip(1).collect();
    enrich(&raw, &args, &|var| std::env::var(var).ok())
}

/// Parses the hook JSON and adds the relay's fields (CONTRACTS.md §1). `env` reads
/// an environment variable; it is a parameter so tests never depend on ours.
fn enrich(raw: &[u8], args: &[String], env: &dyn Fn(&str) -> Option<String>) -> Option<(Value, String)> {
    // Some shells hand us a UTF-8 BOM; serde_json would choke on it.
    let raw = raw.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(raw);
    let mut payload = serde_json::from_slice::<Value>(raw).ok()?;
    let map = payload.as_object_mut()?;

    // argv: "zuko-hook [--agent <name>] [<EventName>]". --agent tags the payload
    // so the app routes it to the right pill; the app validates the name.
    let mut agent = String::new();
    let mut arg_event = String::new();
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        if arg == "--agent" {
            agent = it.next().cloned().unwrap_or_default();
        } else if arg_event.is_empty() {
            arg_event = arg.clone();
        }
    }
    if !agent.is_empty() {
        map.insert("zuko_agent".into(), Value::String(agent));
    }
    let event = map
        .get("hook_event_name")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .unwrap_or(arg_event);
    map.insert("hook_event_name".into(), Value::String(event.clone()));

    let cwd_missing = map.get("cwd").and_then(Value::as_str).map(str::is_empty).unwrap_or(true);
    if cwd_missing {
        if let Ok(cwd) = std::env::current_dir() {
            map.insert("cwd".into(), Value::String(cwd.to_string_lossy().to_string()));
        }
    }

    // Claude Code passes its environment to hooks, so this is the session's own
    // ANTHROPIC_BASE_URL: the app uses it to tell gateway sessions from sessions
    // that route around the gateway.
    let mut zuko_env = Map::new();
    zuko_env.insert(
        "anthropicBaseUrl".into(),
        env("ANTHROPIC_BASE_URL").filter(|s| !s.is_empty()).map(Value::String).unwrap_or(Value::Null),
    );
    zuko_env.insert("home".into(), Value::String(env(fallback::HOME_VAR).unwrap_or_default()));
    map.insert("zuko_env".into(), Value::Object(zuko_env));

    // Which terminal the session runs in: context only, never a filter.
    for (key, var) in [
        ("term_program", "TERM_PROGRAM"),
        ("wt_session", "WT_SESSION"),
        ("term_session_id", "TERM_SESSION_ID"),
        ("vscode_pid", "VSCODE_PID"),
        ("session_pid", "CLAUDE_CODE_SSE_PORT"),
    ] {
        if !map.contains_key(key) {
            map.insert(key.into(), Value::String(env(var).unwrap_or_default()));
        }
    }
    Some((payload, event))
}

/// The request line: the payload plus `zuko_wants_reply`, newline-terminated,
/// never longer than [`MAX_PAYLOAD`].
///
/// The payload goes over untruncated — the firewall needs the whole command, the
/// whole file being written and the whole tool result. Only a payload that would
/// exceed the cap loses detail: `tool_response` first, then ever-shorter strings,
/// and `zuko_truncated` tells the app it is not looking at everything.
fn request_line(payload: &Value, wants_reply: bool) -> String {
    let mut payload = payload.clone();
    if let Some(map) = payload.as_object_mut() {
        map.insert("zuko_wants_reply".into(), Value::Bool(wants_reply));
    }
    let mut line = payload.to_string();
    if line.len() >= MAX_PAYLOAD {
        if let Some(map) = payload.as_object_mut() {
            map.remove("tool_response");
            map.insert("zuko_truncated".into(), Value::Bool(true));
        }
        line = payload.to_string();
        for cap in OVERSIZE_FIELD_CAPS {
            if line.len() < MAX_PAYLOAD {
                break;
            }
            truncate_strings(&mut payload, *cap);
            line = payload.to_string();
        }
    }
    line.push('\n');
    line
}

/// Caps every string in `value` at `max` bytes, cut on a char boundary.
fn truncate_strings(value: &mut Value, max: usize) {
    match value {
        Value::String(s) => {
            if s.len() > max {
                let mut end = max;
                while end > 0 && !s.is_char_boundary(end) {
                    end -= 1;
                }
                s.truncate(end);
                s.push('…');
            }
        }
        Value::Array(items) => items.iter_mut().for_each(|v| truncate_strings(v, max)),
        Value::Object(map) => map.values_mut().for_each(|v| truncate_strings(v, max)),
        _ => {}
    }
}

/// Connect, send, and — when the event waits — read the app's one-line reply.
fn talk(line: &str, wants_reply: bool) -> Answer {
    let Some(pipe) = connect() else { return Answer::Unreachable };
    exchange(pipe, line, wants_reply)
}

/// The conversation itself, over any stream (split out for tests).
fn exchange(mut pipe: impl Read + Write, line: &str, wants_reply: bool) -> Answer {
    if pipe.write_all(line.as_bytes()).is_err() {
        return Answer::NoReply;
    }
    let _ = pipe.flush();
    if !wants_reply {
        return Answer::Sent;
    }

    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        match pipe.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                buf.extend_from_slice(&chunk[..n]);
                if buf.contains(&b'\n') || buf.len() > MAX_PAYLOAD {
                    break;
                }
            }
            Err(_) => break,
        }
    }
    parse_reply(&buf)
}

/// `{"stdout": {…}}` → print it; `{"stdout": null}` → print nothing. Anything
/// else (empty, not JSON, a non-object `stdout`) is not a reply: the caller falls
/// back rather than guess.
fn parse_reply(buf: &[u8]) -> Answer {
    let line = match buf.iter().position(|b| *b == b'\n') {
        Some(i) => &buf[..i],
        None => buf,
    };
    let Ok(Value::Object(mut reply)) = serde_json::from_slice::<Value>(line) else {
        return Answer::NoReply;
    };
    match reply.remove("stdout") {
        Some(obj @ Value::Object(_)) => Answer::Reply(Some(obj)),
        Some(Value::Null) => Answer::Reply(None),
        _ => Answer::NoReply,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn env_of(pairs: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
        move |k| pairs.iter().find(|(n, _)| *n == k).map(|(_, v)| v.to_string())
    }

    #[test]
    fn the_full_payload_is_forwarded_with_the_relays_fields() {
        let big = "x".repeat(50_000);
        let raw = json!({
            "hook_event_name": "PostToolUse",
            "session_id": "s1",
            "cwd": "C:\\p",
            "transcript_path": "C:\\t.jsonl",
            "tool_name": "Bash",
            "tool_input": {"command": "cat big.txt"},
            "tool_response": {"stdout": big, "stderr": "", "interrupted": false, "isImage": false},
        })
        .to_string();
        let env = env_of(&[("ANTHROPIC_BASE_URL", "http://127.0.0.1:47821/t/abc"), (fallback::HOME_VAR, "C:\\Users\\a")]);
        let (p, event) = enrich(raw.as_bytes(), &["PostToolUse".into()], &env).unwrap();
        assert_eq!(event, "PostToolUse");
        // Nothing dropped, nothing truncated.
        assert_eq!(p["transcript_path"], "C:\\t.jsonl");
        assert_eq!(p["tool_response"]["stdout"].as_str().unwrap().len(), 50_000);
        assert_eq!(p["zuko_env"]["anthropicBaseUrl"], "http://127.0.0.1:47821/t/abc");
        assert_eq!(p["zuko_env"]["home"], "C:\\Users\\a");

        let line = request_line(&p, true);
        assert!(line.ends_with('\n'));
        let sent: Value = serde_json::from_str(line.trim_end()).unwrap();
        assert_eq!(sent["zuko_wants_reply"], true);
        assert_eq!(sent["tool_response"]["stdout"].as_str().unwrap().len(), 50_000);
        assert!(sent.get("zuko_truncated").is_none());
    }

    #[test]
    fn argv_supplies_the_event_and_agent_and_a_bom_is_fine() {
        let mut raw = vec![0xEF, 0xBB, 0xBF];
        raw.extend_from_slice(br#"{"session_id":"s","cwd":"/p"}"#);
        let env = env_of(&[]);
        let (p, event) = enrich(&raw, &["--agent".into(), "codex".into(), "Stop".into()], &env).unwrap();
        assert_eq!(event, "Stop");
        assert_eq!(p["hook_event_name"], "Stop");
        assert_eq!(p["zuko_agent"], "codex");
        assert_eq!(p["zuko_env"]["anthropicBaseUrl"], Value::Null);
        assert!(enrich(b"[1,2]", &[], &env).is_none());
        assert!(enrich(b"not json", &[], &env).is_none());
    }

    #[test]
    fn oversized_payloads_shed_the_tool_response_first() {
        let p = json!({
            "hook_event_name": "PostToolUse",
            "tool_input": {"command": "x"},
            "tool_response": {"stdout": "y".repeat(MAX_PAYLOAD + 10)},
        });
        let line = request_line(&p, true);
        assert!(line.len() <= MAX_PAYLOAD);
        let sent: Value = serde_json::from_str(line.trim_end()).unwrap();
        assert!(sent.get("tool_response").is_none());
        assert_eq!(sent["zuko_truncated"], true);
        assert_eq!(sent["tool_input"]["command"], "x");

        // Huge inputs get their strings capped.
        let p = json!({"hook_event_name": "PreToolUse", "tool_input": {"content": "é".repeat(MAX_PAYLOAD)}});
        let line = request_line(&p, true);
        assert!(line.len() <= MAX_PAYLOAD);
    }

    #[test]
    fn waits_match_the_contract() {
        for e in ["PreToolUse", "PostToolUse", "UserPromptSubmit", "SessionStart"] {
            assert_eq!(wait_for(e), Wait::Reply, "{e}");
        }
        assert_eq!(wait_for("PermissionRequest"), Wait::Decision);
        for e in ["Stop", "SessionEnd", "Notification", "SubagentStop", ""] {
            assert_eq!(wait_for(e), Wait::None, "{e}");
        }
    }

    #[test]
    fn replies_are_parsed_strictly() {
        assert_eq!(
            parse_reply(b"{\"stdout\":{\"a\":1}}\n"),
            Answer::Reply(Some(json!({"a": 1})))
        );
        assert_eq!(parse_reply(b"{\"stdout\":null}\n"), Answer::Reply(None));
        // Not a reply: fall back rather than guess.
        assert_eq!(parse_reply(b""), Answer::NoReply);
        assert_eq!(parse_reply(b"allow\n"), Answer::NoReply);
        assert_eq!(parse_reply(b"{\"stdout\":\"allow\"}\n"), Answer::NoReply);
        assert_eq!(parse_reply(b"{\"other\":1}\n"), Answer::NoReply);
    }

    /// An in-memory pipe: what we wrote, and a canned reply to read.
    struct Fake {
        written: Vec<u8>,
        reply: std::io::Cursor<Vec<u8>>,
    }
    impl Read for Fake {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            self.reply.read(buf)
        }
    }
    impl Write for Fake {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.written.extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn the_exchange_sends_one_line_and_reads_one_line() {
        let fake = Fake { written: Vec::new(), reply: std::io::Cursor::new(b"{\"stdout\":{\"x\":true}}\n".to_vec()) };
        assert_eq!(exchange(fake, "{}\n", true), Answer::Reply(Some(json!({"x": true}))));
        let fake = Fake { written: Vec::new(), reply: std::io::Cursor::new(Vec::new()) };
        assert_eq!(exchange(fake, "{}\n", false), Answer::Sent);
        let fake = Fake { written: Vec::new(), reply: std::io::Cursor::new(Vec::new()) };
        assert_eq!(exchange(fake, "{}\n", true), Answer::NoReply);
    }
}
