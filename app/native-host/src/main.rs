//! zuko-native-host — the bridge between the Zuko browser extension and the desktop app.
//!
//! Chrome / Edge start this process when the extension calls
//! `chrome.runtime.connectNative("app.zuko.host")`. Frames (4-byte little-endian length +
//! UTF-8 JSON) arrive on stdin. Each one is forwarded to the app as a single JSON line on
//! `\\.\pipe\zuko-<SID>` (Windows) or `$XDG_RUNTIME_DIR/zuko.sock` (Linux), wrapped as
//! `{"hook_event_name":"ZukoExtension","message":<msg>}`; the app's one-line reply
//! `{"reply":…}` goes back to the browser as a frame. See CONTRACTS.md §4.
//!
//! * One frame in, exactly one frame out, in order. An unreachable app is an error frame,
//!   not silence: `{"ok":false,"error":"Zuko desktop app is not running"}`.
//! * The host never listens on a socket and only talks to a pipe served by the same user
//!   (checked in `win.rs` / `unix.rs`). Chrome starts it only for the extension origin
//!   named in the host manifest.
//! * Replies are capped at 1 MiB (the browser's limit for host → extension messages).
//!   A larger one is replaced by an error frame.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::sync::mpsc;
use std::time::Duration;

mod frame;
mod relay;

#[cfg(windows)]
mod win;
#[cfg(windows)]
use win::connect;

#[cfg(target_os = "linux")]
mod unix;
#[cfg(target_os = "linux")]
use unix::connect;

use frame::{read_frame, write_frame, Inbound, WriteError, MAX_INBOUND, MAX_OUTBOUND};
use relay::{error_reply, finish, Transport, TransportError};

/// Budget for getting a pipe connection while the app is busy serving someone else.
pub(crate) const CONNECT_TIMEOUT: Duration = Duration::from_millis(1000);
/// How long one request may wait for the app's answer (masking a large document, or a waited
/// local-AI deep scan, itself bounded by the configured timeout).
const REPLY_BUDGET: Duration = Duration::from_secs(45);
/// Largest reply line we will buffer from the app.
const MAX_REPLY_LINE: u64 = 64 * 1024 * 1024;

fn main() {
    let stdin = io::stdin();
    let stdout = io::stdout();
    let code = match serve(&mut stdin.lock(), &mut stdout.lock(), &PipeTransport) {
        Ok(()) => 0,
        Err(_) => 1,
    };
    std::process::exit(code);
}

/// The frame loop: returns `Ok` when the browser closes stdin, `Err` when a stream breaks.
fn serve<R: Read, W: Write>(input: &mut R, output: &mut W, transport: &dyn Transport) -> io::Result<()> {
    loop {
        let reply = match read_frame(input)? {
            Inbound::Eof => return Ok(()),
            Inbound::TooLarge(n) => finish(
                error_reply(&format!("message of {n} bytes exceeds the {} byte limit", MAX_INBOUND), None),
                None,
            ),
            Inbound::Message(raw) => relay::handle(transport, &raw),
        };
        match write_frame(output, &reply) {
            Ok(()) => {}
            Err(WriteError::Io(e)) => return Err(e),
            // `finish` already guarantees the cap; this is belt and braces.
            Err(WriteError::TooLarge(n)) => {
                let msg = format!("reply of {n} bytes exceeds the {MAX_OUTBOUND} byte limit");
                let fallback = error_reply(&msg, None).to_string();
                if let Err(WriteError::Io(e)) = write_frame(output, fallback.as_bytes()) {
                    return Err(e);
                }
            }
        }
    }
}

/// The real transport: a fresh pipe connection per message (the app serves one request
/// per connection, like it does for the hook relay), so an app restart never leaves us
/// holding a dead handle.
struct PipeTransport;

impl Transport for PipeTransport {
    fn exchange(&self, line: &str) -> Result<String, TransportError> {
        // The worker owns every blocking call. If it overruns the budget we stop
        // listening; it ends when the app closes the pipe or this process exits.
        let (tx, rx) = mpsc::channel();
        let line = line.to_string();
        std::thread::spawn(move || {
            let _ = tx.send(talk(&line));
        });
        match rx.recv_timeout(REPLY_BUDGET) {
            Ok(result) => result,
            Err(_) => Err(TransportError::Timeout),
        }
    }
}

/// Connect, send one line, read one line.
fn talk(line: &str) -> Result<String, TransportError> {
    let mut pipe = connect().ok_or(TransportError::Unreachable)?;
    let io_err = |e: io::Error| TransportError::BadReply(e.to_string());
    pipe.write_all(line.as_bytes()).map_err(io_err)?;
    pipe.write_all(b"\n").map_err(io_err)?;
    pipe.flush().map_err(io_err)?;

    let mut reply = String::new();
    let mut reader = BufReader::new(pipe).take(MAX_REPLY_LINE);
    reader.read_line(&mut reply).map_err(io_err)?;
    let reply = reply.trim();
    if reply.is_empty() {
        return Err(TransportError::BadReply("empty reply".into()));
    }
    Ok(reply.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};
    use std::cell::RefCell;
    use std::io::Cursor;

    struct Echo(RefCell<Vec<String>>);
    impl Transport for Echo {
        fn exchange(&self, line: &str) -> Result<String, TransportError> {
            self.0.borrow_mut().push(line.to_string());
            let req: Value = serde_json::from_str(line).unwrap();
            Ok(json!({ "reply": { "ok": true, "echo": req["message"] } }).to_string())
        }
    }

    struct Down;
    impl Transport for Down {
        fn exchange(&self, _: &str) -> Result<String, TransportError> {
            Err(TransportError::Unreachable)
        }
    }

    fn framed(v: &Value) -> Vec<u8> {
        let b = v.to_string().into_bytes();
        let mut out = (b.len() as u32).to_le_bytes().to_vec();
        out.extend(b);
        out
    }

    fn frames(mut wire: &[u8]) -> Vec<Value> {
        let mut out = Vec::new();
        while let Inbound::Message(m) = read_frame(&mut wire).unwrap() {
            out.push(serde_json::from_slice(&m).unwrap());
        }
        out
    }

    #[test]
    fn one_frame_out_per_frame_in_in_order() {
        let mut input = framed(&json!({"op":"hello","id":1}));
        input.extend(framed(&json!({"op":"vault","id":2})));
        let mut out = Vec::new();
        let t = Echo(RefCell::new(Vec::new()));
        serve(&mut Cursor::new(input), &mut out, &t).unwrap();
        let got = frames(&out);
        assert_eq!(got.len(), 2);
        assert_eq!(got[0]["echo"]["op"], "hello");
        assert_eq!(got[0]["id"], 1);
        assert_eq!(got[1]["echo"]["op"], "vault");
        assert_eq!(t.0.borrow().len(), 2);
    }

    #[test]
    fn unreachable_app_gets_the_documented_error() {
        let input = framed(&json!({"op":"hello"}));
        let mut out = Vec::new();
        serve(&mut Cursor::new(input), &mut out, &Down).unwrap();
        assert_eq!(frames(&out), vec![json!({"ok":false,"error":"Zuko desktop app is not running"})]);
    }

    #[test]
    fn oversized_app_reply_is_replaced_by_an_error_frame() {
        struct Huge;
        impl Transport for Huge {
            fn exchange(&self, _: &str) -> Result<String, TransportError> {
                Ok(json!({"reply":{"ok":true,"vault":"v".repeat(MAX_OUTBOUND)}}).to_string())
            }
        }
        let mut out = Vec::new();
        serve(&mut Cursor::new(framed(&json!({"op":"vault"}))), &mut out, &Huge).unwrap();
        assert!(out.len() <= 4 + MAX_OUTBOUND);
        let got = frames(&out);
        assert_eq!(got[0]["ok"], false);
        assert!(got[0]["error"].as_str().unwrap().contains("1 MB"));
    }

    #[test]
    fn oversized_inbound_gets_an_error_and_the_loop_continues() {
        let big = MAX_INBOUND + 1;
        let mut input = (big as u32).to_le_bytes().to_vec();
        input.resize(4 + big, b' ');
        input.extend(framed(&json!({"op":"hello"})));
        let mut out = Vec::new();
        serve(&mut Cursor::new(input), &mut out, &Down).unwrap();
        let got = frames(&out);
        assert_eq!(got.len(), 2);
        assert_eq!(got[0]["ok"], false);
        assert!(got[0]["error"].as_str().unwrap().contains("exceeds"));
    }

    #[test]
    fn broken_stream_is_an_error_and_clean_eof_is_not() {
        let mut out = Vec::new();
        assert!(serve(&mut Cursor::new(vec![9, 0, 0]), &mut out, &Down).is_err());
        assert!(serve(&mut Cursor::new(Vec::new()), &mut out, &Down).is_ok());
    }
}
