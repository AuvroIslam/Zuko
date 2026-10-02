// Relay server for zuko-hook (CONTRACTS.md §1).
//
// Windows: the named pipe `\\.\pipe\zuko-<sid>`, one instance per connection.
// Linux: the Unix socket `$XDG_RUNTIME_DIR/zuko.sock`.
//
// One connection carries one request line: the full hook payload plus the relay's
// fields (`zuko_wants_reply`, `zuko_env`, terminal context). What happens next:
// * Firewall events (PreToolUse, PostToolUse, UserPromptSubmit, SessionStart) are
//   decided by firewall.rs and answered with one line, `{"stdout": <object|null>}`,
//   which the relay prints verbatim. The relay gives us 1.5 s; past that it decides
//   on its own with the stateless fallback, so a slow answer is a lost answer.
// * PermissionRequest waits for a human on the island and is answered with the
//   documented PermissionRequest output (or `{"stdout": null}`: the terminal asks).
// * `ZukoExtension` (the browser extension's native host) gets `{"reply": …}`.
// * Everything else is fire-and-forget: we read it and hang up.
// Every event also reaches the island as a `hook` event: a UI copy with
// `tool_response` removed and strings cut to 2000 chars, plus `zuko` risk info.
//
// Claude Code is never blocked by us. Three things guarantee it:
//   * zuko-hook gives the connection 300 ms, and every wait a hard deadline after
//     which it exits on its own — we can be slow, never fatal;
//   * we only wait for a human once the island has *confirmed* the card is on
//     screen, so a paused island or a webview that is not listening costs
//     800 ms, not two minutes;
//   * whatever happens we answer before the relay's own 110 s decision budget,
//     and the terminal takes over.
//
// The relay is answered before audit receipts are written and UI events emitted:
// that work is real I/O and Claude Code has no reason to wait for it.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
#[cfg(windows)]
use tokio::net::windows::named_pipe::{NamedPipeServer, ServerOptions};
use tokio::sync::mpsc;

use crate::firewall;
use crate::island::WINDOW_LABEL;
use crate::log;

/// Slightly under zuko-hook's own 110 s wait, so we always answer first.
const DECISION_TIMEOUT: Duration = Duration::from_secs(108);
/// How long the island gets to say "the card is up". Without it, an island that
/// is paused, hidden behind a crashed webview or simply not listening would leave
/// Claude Code staring at a prompt nobody can see for nearly two minutes.
const ACK_TIMEOUT: Duration = Duration::from_millis(800);
/// Largest request line accepted (CONTRACTS.md §1).
pub const MAX_PAYLOAD: usize = 16 << 20;
/// Strings in the island's copy of a payload are cut to this many characters.
const UI_STRING_MAX: usize = 2_000;

/// What the island can say about a permission request.
pub enum Reply {
    /// The card is on screen and a human can act on it.
    Ack,
    /// A human clicked Allow (`true`) or Deny, after `elapsed_ms` on screen.
    Decision { allow: bool, elapsed_ms: Option<u64> },
    /// Nobody can act on it — paused, or another request already holds the card.
    Decline,
}

/// Permission requests the island has been told about.
#[derive(Default)]
pub struct Pending(pub Mutex<HashMap<String, mpsc::Sender<Reply>>>);

static COUNTER: AtomicU64 = AtomicU64::new(1);

/// `\\.\pipe\zuko-<sid>` — must match zuko-hook's `pipe_path()` exactly.
#[cfg(windows)]
pub fn pipe_name() -> String {
    let key = crate::platform::current_user_sid()
        .unwrap_or_else(|| std::env::var("USERNAME").unwrap_or_else(|_| "user".into()));
    format!(r"\\.\pipe\zuko-{key}")
}

#[cfg(windows)]
pub fn start(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let name = pipe_name();
        // first_pipe_instance also means we refuse to join a pipe somebody else
        // already owns under our name, rather than serving on top of it.
        let mut server = match ServerOptions::new().first_pipe_instance(true).create(&name) {
            Ok(s) => s,
            Err(err) => {
                log::line(format!("cannot open the relay pipe: {err}"));
                return;
            }
        };
        loop {
            if server.connect().await.is_err() {
                tokio::time::sleep(Duration::from_millis(200)).await;
                continue;
            }
            // Hand the connected instance to a task and listen on a fresh one.
            let next = match ServerOptions::new().create(&name) {
                Ok(s) => s,
                Err(err) => {
                    log::line(format!("cannot reopen the relay pipe: {err}"));
                    return;
                }
            };
            let connected = std::mem::replace(&mut server, next);
            let app = app.clone();
            tauri::async_runtime::spawn(async move { handle(app, connected).await });
        }
    });
}

#[cfg(target_os = "linux")]
pub fn start(app: AppHandle) {
    use std::os::unix::fs::PermissionsExt;
    use tokio::net::UnixListener;

    tauri::async_runtime::spawn(async move {
        let Some(path) = crate::platform::relay_socket_path() else {
            log::line("no private runtime directory ($XDG_RUNTIME_DIR) — Claude Code hooks are inactive");
            return;
        };
        // A socket file left behind by a crash answers nothing and can go. One
        // that answers belongs to a Zuko that is still running: like
        // first_pipe_instance on Windows, we refuse to serve on top of it.
        if path.exists() {
            if std::os::unix::net::UnixStream::connect(&path).is_ok() {
                log::line("another Zuko already serves the relay socket");
                return;
            }
            let _ = std::fs::remove_file(&path);
        }
        let listener = match UnixListener::bind(&path) {
            Ok(l) => l,
            Err(err) => {
                log::line(format!("cannot open the relay socket: {err}"));
                return;
            }
        };
        // The runtime directory is already 0700; this is belt and braces.
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
        let uid = unsafe { libc::getuid() };
        loop {
            let stream = match listener.accept().await {
                Ok((stream, _)) => stream,
                Err(_) => {
                    tokio::time::sleep(Duration::from_millis(200)).await;
                    continue;
                }
            };
            // Only the relay run by our own user may drive the island.
            if !matches!(stream.peer_cred(), Ok(c) if c.uid() == uid) {
                log::line("refused a relay connection from another user");
                continue;
            }
            let app = app.clone();
            tauri::async_runtime::spawn(async move { handle(app, stream).await });
        }
    });
}

/// One accepted relay connection, whatever carries it.
trait Relay: AsyncRead + AsyncWrite + Unpin {
    /// Ends the conversation once everything has been written.
    fn finish(&mut self) {}
}

#[cfg(windows)]
impl Relay for NamedPipeServer {
    fn finish(&mut self) {
        let _ = self.disconnect();
    }
}

/// Dropping the stream closes it; the relay reads up to our newline first.
#[cfg(target_os = "linux")]
impl Relay for tokio::net::UnixStream {}

async fn handle(app: AppHandle, mut pipe: impl Relay) {
    let Some(payload) = read_request(&mut pipe).await else {
        pipe.finish();
        return;
    };
    let event = payload
        .get("hook_event_name")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let wants_reply = payload.get("zuko_wants_reply").and_then(Value::as_bool).unwrap_or(false);

    match event.as_str() {
        "ZukoExtension" => {
            let reply = firewall::handle_extension(&app, &payload).await;
            write_line(&mut pipe, &json!({ "reply": reply })).await;
            linger(&mut pipe).await;
        }
        "PermissionRequest" => permission_request(&app, &mut pipe, payload, wants_reply).await,
        _ => {
            let started = Instant::now();
            let outcome = firewall::evaluate(&app, &payload).await;
            if wants_reply {
                write_line(&mut pipe, &reply_object(outcome.stdout.clone())).await;
                linger(&mut pipe).await;
            }
            pipe.finish();
            let verdict = outcome
                .stdout
                .as_ref()
                .and_then(|o| o.pointer("/hookSpecificOutput/permissionDecision").or_else(|| o.get("decision")))
                .and_then(Value::as_str)
                .unwrap_or("-");
            log::line(format!("hook {event} → {verdict} in {} ms", started.elapsed().as_millis()));

            let mut ui = ui_copy(&payload);
            if let Some(zuko) = &outcome.zuko {
                ui["zuko"] = zuko.clone();
            }
            if let Some(masked) = &outcome.ui_prompt {
                ui["prompt"] = Value::String(clip(masked, UI_STRING_MAX));
            }
            let _ = app.emit_to(WINDOW_LABEL, "hook", ui);
            outcome.apply(&app);
            return;
        }
    }
    pipe.finish();
}

/// The island decides, within the ack and decision deadlines; the decision is
/// answered in Claude Code's documented shape and recorded.
async fn permission_request(app: &AppHandle, pipe: &mut impl Relay, payload: Value, wants_reply: bool) {
    let info = firewall::permission_info(app, &payload);
    let id = format!("{}-{}", std::process::id(), COUNTER.fetch_add(1, Ordering::Relaxed));
    let (tx, mut rx) = mpsc::channel::<Reply>(4);
    app.state::<Pending>().0.lock().unwrap().insert(id.clone(), tx);

    let mut ui = ui_copy(&payload);
    ui["request_id"] = json!(id);
    if let Some(info) = &info {
        ui["zuko"] = info.clone();
    }
    log::line(format!("hook PermissionRequest id={id}"));
    let _ = app.emit_to(WINDOW_LABEL, "hook", ui);

    let decision = wait_for_decision(&id, &mut rx).await;
    app.state::<Pending>().0.lock().unwrap().remove(&id);

    // No decision: `{"stdout": null}`. zuko-hook prints nothing and Claude Code
    // asks in the terminal, exactly as if Zuko were closed.
    let stdout = decision.map(|(allow, _)| {
        zuko_core::hookio::permission_request(allow, (!allow).then_some(firewall::DENIED_IN_ZUKO), None)
    });
    if wants_reply {
        write_line(pipe, &reply_object(stdout)).await;
        linger(pipe).await;
    }
    if let Some((allow, elapsed_ms)) = decision {
        firewall::record_permission(app, &payload, info.as_ref(), allow, elapsed_ms);
    }
}

/// Reads one request line (at most [`MAX_PAYLOAD`] bytes) and parses it. A line
/// that is too long, not JSON or not an object is no request at all.
async fn read_request(pipe: &mut (impl AsyncRead + Unpin)) -> Option<Value> {
    let mut buf = Vec::new();
    let mut chunk = vec![0u8; 64 * 1024];
    loop {
        match pipe.read(&mut chunk).await {
            Ok(0) => break,
            Ok(n) => {
                let start = buf.len();
                buf.extend_from_slice(&chunk[..n]);
                if buf[start..].contains(&b'\n') {
                    break;
                }
                if buf.len() > MAX_PAYLOAD {
                    log::line("relay request over 16 MiB — ignored");
                    return None;
                }
            }
            Err(_) => return None,
        }
    }
    let line = match buf.iter().position(|b| *b == b'\n') {
        Some(i) => &buf[..i],
        None => &buf[..],
    };
    serde_json::from_slice::<Value>(line).ok().filter(Value::is_object)
}

/// `{"stdout": …}`: an object to print, or null for "no opinion".
fn reply_object(stdout: Option<Value>) -> Value {
    json!({ "stdout": stdout.filter(Value::is_object) })
}

async fn write_line(pipe: &mut (impl AsyncWrite + Unpin), value: &Value) {
    let mut line = value.to_string();
    line.push('\n');
    let _ = pipe.write_all(line.as_bytes()).await;
    let _ = pipe.flush().await;
}

/// Waits (briefly) for the relay to hang up after reading our reply.
///
/// Disconnecting a Windows pipe instance discards whatever the client has not
/// read yet, and a reply carrying a rewritten file or a masked command output is
/// far bigger than the pipe's buffer. The relay closes its end as soon as it has
/// our newline, so this costs nothing when things work, and two seconds at worst.
async fn linger(pipe: &mut (impl AsyncRead + Unpin)) {
    let mut sink = [0u8; 256];
    let _ = tokio::time::timeout(Duration::from_secs(2), async {
        while let Ok(n) = pipe.read(&mut sink).await {
            if n == 0 {
                break;
            }
        }
    })
    .await;
}

/// The island's copy of a payload: no `tool_response` (it can be megabytes and the
/// island never shows it), every string cut to [`UI_STRING_MAX`] characters.
fn ui_copy(payload: &Value) -> Value {
    fn cut(v: &mut Value) {
        match v {
            Value::String(s) => {
                if s.chars().count() > UI_STRING_MAX {
                    *s = clip(s, UI_STRING_MAX);
                }
            }
            Value::Array(a) => a.iter_mut().for_each(cut),
            Value::Object(o) => o.values_mut().for_each(cut),
            _ => {}
        }
    }
    let mut ui = payload.clone();
    if let Some(map) = ui.as_object_mut() {
        map.remove("tool_response");
    }
    cut(&mut ui);
    ui
}

/// The first `max` characters, with an ellipsis when cut.
fn clip(s: &str, max: usize) -> String {
    match s.char_indices().nth(max) {
        Some((i, _)) => format!("{}…", &s[..i]),
        None => s.to_string(),
    }
}

/// Two waits: a short one for "the card is up", then the long one for a human.
/// Returns (allow, elapsed_ms) when a human decided.
async fn wait_for_decision(id: &str, rx: &mut mpsc::Receiver<Reply>) -> Option<(bool, Option<u64>)> {
    match tokio::time::timeout(ACK_TIMEOUT, rx.recv()).await {
        Ok(Some(Reply::Ack)) => {}
        // A click that beats the ack is still a click.
        Ok(Some(Reply::Decision { allow, elapsed_ms })) => {
            log::line(format!("hook id={id} answered {}", word(allow)));
            return Some((allow, elapsed_ms));
        }
        Ok(Some(Reply::Decline)) => {
            log::line(format!("hook id={id} not shown — terminal takes over"));
            return None;
        }
        Ok(None) => return None,
        Err(_) => {
            log::line(format!("hook id={id} island never acknowledged — terminal takes over"));
            return None;
        }
    }

    match tokio::time::timeout(DECISION_TIMEOUT, rx.recv()).await {
        Ok(Some(Reply::Decision { allow, elapsed_ms })) => {
            log::line(format!("hook id={id} answered {}", word(allow)));
            Some((allow, elapsed_ms))
        }
        Ok(Some(Reply::Decline)) => {
            log::line(format!("hook id={id} released without a decision"));
            None
        }
        _ => {
            log::line(format!("hook id={id} timed out — terminal takes over"));
            None
        }
    }
}

fn word(allow: bool) -> &'static str {
    if allow { "allow" } else { "deny" }
}

fn send(app: &AppHandle, request_id: &str, reply: Reply, keep: bool) {
    let sender = {
        let pending = app.state::<Pending>();
        let mut map = pending.0.lock().unwrap();
        if keep { map.get(request_id).cloned() } else { map.remove(request_id) }
    };
    match sender {
        Some(tx) => {
            let _ = tx.try_send(reply);
        }
        None => log::line(format!("reply for id={request_id} — no pending request")),
    }
}

/// The island has the card on screen; the long wait may begin.
pub fn acknowledge(app: &AppHandle, request_id: &str) {
    send(app, request_id, Reply::Ack, true);
}

/// Nobody can act on this one — paused, or another card already holds the view.
pub fn decline(app: &AppHandle, request_id: &str) {
    log::line(format!("decline id={request_id}"));
    send(app, request_id, Reply::Decline, false);
}

/// Called by the island's Allow / Deny buttons. `elapsed_ms` is how long the card
/// was on screen before the click (rubber-stamp detection and the audit receipt).
pub fn answer(app: &AppHandle, request_id: &str, decision: &str, elapsed_ms: Option<u64>) {
    // "always" still answers a plain allow; remembering it is the island's business.
    let allow = matches!(decision, "allow" | "always");
    log::line(format!("decision id={request_id} {} after {elapsed_ms:?} ms", word(allow)));
    send(app, request_id, Reply::Decision { allow, elapsed_ms }, false);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ui_copy_drops_the_tool_response_and_cuts_strings() {
        let p = json!({
            "hook_event_name": "PostToolUse",
            "tool_input": {"content": "é".repeat(5000)},
            "tool_response": {"stdout": "x".repeat(10)},
            "n": 3,
        });
        let ui = ui_copy(&p);
        assert!(ui.get("tool_response").is_none());
        let s = ui["tool_input"]["content"].as_str().unwrap();
        assert_eq!(s.chars().count(), UI_STRING_MAX + 1);
        assert!(s.ends_with('…'));
        assert_eq!(ui["n"], 3);
        // The original is untouched: the firewall sees everything.
        assert!(p.get("tool_response").is_some());
    }

    #[test]
    fn replies_are_one_object_or_null() {
        assert_eq!(reply_object(Some(json!({"a": 1}))), json!({"stdout": {"a": 1}}));
        assert_eq!(reply_object(None), json!({"stdout": null}));
        assert_eq!(reply_object(Some(json!("allow"))), json!({"stdout": null}));
    }

    #[tokio::test]
    async fn requests_are_one_bounded_json_line() {
        let mut ok: &[u8] = b"{\"hook_event_name\":\"Stop\"}\n{\"next\":1}\n";
        assert_eq!(read_request(&mut ok).await.unwrap()["hook_event_name"], "Stop");
        let mut no_newline: &[u8] = b"{\"a\":1}";
        assert!(read_request(&mut no_newline).await.is_some());
        let mut array: &[u8] = b"[1]\n";
        assert!(read_request(&mut array).await.is_none());
        let mut junk: &[u8] = b"allow\n";
        assert!(read_request(&mut junk).await.is_none());
        let big = vec![b' '; MAX_PAYLOAD + 70_000];
        let mut too_big: &[u8] = &big;
        assert!(read_request(&mut too_big).await.is_none());
    }

    /// The real relay binary against this module's protocol and the real firewall,
    /// over the real pipe name. Skipped when the release relay has not been built,
    /// or when a running Zuko already owns the pipe.
    #[cfg(windows)]
    #[tokio::test]
    async fn the_relay_round_trips_through_the_pipe() {
        use crate::engine::{CtxBase, Engine};
        use std::process::Stdio;

        let relay = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../target/release/zuko-hook.exe");
        if !relay.exists() {
            eprintln!("skipped: build the relay first (cargo build --release -p zuko-hook)");
            return;
        }
        let Ok(mut server) = ServerOptions::new().first_pipe_instance(true).create(pipe_name()) else {
            eprintln!("skipped: another process owns the relay pipe");
            return;
        };
        let scratch = std::env::temp_dir().join(format!("zuko-pipe-test-{}", std::process::id()));
        std::fs::create_dir_all(&scratch).unwrap();

        let base = CtxBase {
            home: "C:\\Users\\a".into(),
            protected_paths: vec!["C:\\Users\\a\\.claude\\settings.json".into()],
            protected_processes: vec!["zuko.exe".into()],
            windows: true,
        };
        let engine = std::sync::Arc::new(Engine::with_parts(Default::default(), Default::default(), base));
        let facts = firewall::Facts::default();

        // Serve exactly as `handle` does, minus the window: read, decide, reply.
        let served = engine.clone();
        let server_task = tokio::spawn(async move {
            let mut seen = Vec::new();
            loop {
                if server.connect().await.is_err() {
                    break;
                }
                let next = ServerOptions::new().create(pipe_name()).unwrap();
                let mut conn = std::mem::replace(&mut server, next);
                let Some(payload) = read_request(&mut conn).await else { continue };
                let event = payload["hook_event_name"].as_str().unwrap_or_default().to_string();
                if event == "Shutdown" {
                    break;
                }
                let out = firewall::process(&served, &facts, &payload);
                if payload["zuko_wants_reply"] == true {
                    write_line(&mut conn, &reply_object(out.stdout.clone())).await;
                    linger(&mut conn).await;
                }
                let _ = conn.disconnect();
                seen.push((event, payload));
            }
            seen
        });

        let key = "sk-proj-abcdefghijklmnopqrstuvwx1234";
        let cwd = "C:\\Users\\a\\proj";
        let run = |event: &str, body: Value| {
            let relay = relay.clone();
            let scratch = scratch.clone();
            let event = event.to_string();
            // The relay is a plain blocking child process; the server above keeps
            // running on the test's runtime meanwhile.
            tokio::task::spawn_blocking(move || {
                use std::io::Write as _;
                let started = Instant::now();
                let mut child = std::process::Command::new(&relay)
                    .arg(&event)
                    .env("ZUKO_CONFIG_DIR", &scratch)
                    .env("ZUKO_DATA_DIR", &scratch)
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .spawn()
                    .unwrap();
                let mut stdin = child.stdin.take().unwrap();
                stdin.write_all(body.to_string().as_bytes()).unwrap();
                drop(stdin);
                let out = child.wait_with_output().unwrap();
                (String::from_utf8_lossy(&out.stdout).trim().to_string(), started.elapsed())
            })
        };

        let (deny, t1) = run("PreToolUse", json!({"hook_event_name": "PreToolUse", "session_id": "e2e", "cwd": cwd,
            "tool_name": "WebFetch", "tool_input": {"url": "https://pastebin.com/raw/1", "prompt": "x"}})).await.unwrap();
        let (allow, t2) = run("PreToolUse", json!({"hook_event_name": "PreToolUse", "session_id": "e2e", "cwd": cwd,
            "tool_name": "Bash", "tool_input": {"command": "ls"}})).await.unwrap();
        let (blocked, _) = run("UserPromptSubmit", json!({"hook_event_name": "UserPromptSubmit", "session_id": "e2e", "cwd": cwd,
            "prompt": format!("my key is {key}")})).await.unwrap();
        // Far past the old 2000-char truncation; small enough for a debug-build scan
        // to stay inside the relay's 1.5 s budget.
        let big = "y".repeat(1 << 20);
        let (masked, t4) = run("PostToolUse", json!({"hook_event_name": "PostToolUse", "session_id": "e2e", "cwd": cwd,
            "transcript_path": "C:\\t.jsonl",
            "tool_name": "Bash", "tool_input": {"command": "cat .env"},
            "tool_response": {"stdout": format!("KEY={key}\n{big}"), "stderr": "", "interrupted": false, "isImage": false}})).await.unwrap();
        let (write, _) = run("PreToolUse", json!({"hook_event_name": "PreToolUse", "session_id": "e2e", "cwd": cwd,
            "tool_name": "Write", "tool_input": {"file_path": format!("{cwd}\\.env"), "content": "K={{API_KEY_1}}"}})).await.unwrap();
        let (stop, _) = run("Stop", json!({"hook_event_name": "Stop", "session_id": "e2e"})).await.unwrap();
        let _ = run("Shutdown", json!({"hook_event_name": "Shutdown"})).await.unwrap();
        let seen = server_task.await.unwrap();
        let _ = std::fs::remove_dir_all(&scratch);

        let deny: Value = serde_json::from_str(&deny).unwrap();
        assert_eq!(deny["hookSpecificOutput"]["permissionDecision"], "deny");
        let allow: Value = serde_json::from_str(&allow).unwrap();
        assert_eq!(allow["hookSpecificOutput"]["permissionDecision"], "allow", "the app may allow; the fallback never does");
        let blocked: Value = serde_json::from_str(&blocked).unwrap();
        assert_eq!(blocked["decision"], "block");
        assert!(!blocked.to_string().contains(key));
        let masked: Value = serde_json::from_str(&masked).unwrap();
        let stdout = masked["hookSpecificOutput"]["updatedToolOutput"]["stdout"].as_str().unwrap();
        assert!(stdout.starts_with("KEY={{API_KEY_1}}\n"));
        assert_eq!(stdout.len(), "KEY={{API_KEY_1}}\n".len() + big.len(), "the whole output came through");
        let write: Value = serde_json::from_str(&write).unwrap();
        assert_eq!(write["hookSpecificOutput"]["updatedInput"]["content"], format!("K={key}"));
        assert!(stop.is_empty());

        // The full payload arrived: nothing dropped, the relay's fields added.
        let post = &seen.iter().find(|(e, _)| e == "PostToolUse").unwrap().1;
        assert_eq!(post["transcript_path"], "C:\\t.jsonl");
        assert_eq!(post["zuko_wants_reply"], true);
        assert!(post["zuko_env"].is_object());
        let stop_payload = &seen.iter().find(|(e, _)| e == "Stop").unwrap().1;
        assert_eq!(stop_payload["zuko_wants_reply"], false);

        eprintln!("relay round trips: deny {t1:?}, allow {t2:?}, 1 MiB PostToolUse {t4:?}");
        assert!(t1 < Duration::from_millis(1500) && t2 < Duration::from_millis(1500));
    }
}
