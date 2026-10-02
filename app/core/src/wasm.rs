//! C-ABI exports for the browser extension (wasm32-unknown-unknown, no wasm-bindgen).
//!
//! Protocol: JSON in, JSON out, through linear memory.
//! * `zuko_alloc(len) -> ptr` / `zuko_free(ptr, len)` manage buffers.
//! * `zuko_call(ptr, len) -> u64` takes a UTF-8 JSON request and returns
//!   `(out_ptr << 32) | out_len` pointing at a UTF-8 JSON response the caller must free.
//!
//! Requests (`{"op": …}`):
//! * `{"op":"configure","detector":DetectorConfig}` → `{"ok":true}`
//! * `{"op":"loadVault","vault":<Vault JSON>}` → `{"ok":true,"size":n}`
//! * `{"op":"exportVault"}` → `{"ok":true,"vault":<Vault JSON>}`
//! * `{"op":"scan","text":…}` → `{"ok":true,"findings":[Finding…]}`
//! * `{"op":"mask","text":…,"source":"browser","now":…}` → `{"ok":true,"text":…,"report":MaskReport}`
//! * `{"op":"maskKnown","text":…}` → `{"ok":true,"text":…,"count":n}`
//! * `{"op":"rehydrate","text":…}` → `{"ok":true,"text":…,"keys":[…]}`
//! * `{"op":"legend","keys":[…]}` → `{"ok":true,"text":…}`
//! * `{"op":"views"}` → `{"ok":true,"entries":[EntryView…]}`
//! * errors → `{"ok":false,"error":…}`
//!
//! State (detector + vault) lives in a single global, since WASM here is single-threaded.
//! It starts as `DetectorConfig::default()` and an empty vault. `loadVault` accepts the
//! vault as a JSON object or as a string holding that JSON; `source` defaults to
//! `"browser"` and `now` to 0. Buffers are boxed slices, so `zuko_free` must be given
//! the exact length that was allocated (or returned).

use crate::detect::{Detector, DetectorConfig};
use crate::mask::{legend, mask_known, mask_text, rehydrate_text, MaskCtx};
use crate::vault::Vault;
use serde_json::{json, Value};
use std::cell::RefCell;

struct State {
    det: Detector,
    vault: Vault,
}

thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
}

fn with_state<R>(f: impl FnOnce(&mut State) -> R) -> R {
    STATE.with(|cell| {
        let mut slot = cell.borrow_mut();
        let state = slot.get_or_insert_with(|| State { det: Detector::new(&DetectorConfig::default()), vault: Vault::new() });
        f(state)
    })
}

#[no_mangle]
pub extern "C" fn zuko_alloc(len: usize) -> *mut u8 {
    Box::into_raw(vec![0u8; len].into_boxed_slice()) as *mut u8
}

/// # Safety
/// `ptr`/`len` must come from `zuko_alloc` or a `zuko_call` result.
#[no_mangle]
pub unsafe extern "C" fn zuko_free(ptr: *mut u8, len: usize) {
    if ptr.is_null() {
        return;
    }
    drop(Box::from_raw(std::ptr::slice_from_raw_parts_mut(ptr, len)));
}

/// # Safety
/// `ptr`/`len` must describe a live buffer from `zuko_alloc` holding UTF-8 JSON.
#[no_mangle]
pub unsafe extern "C" fn zuko_call(ptr: *const u8, len: usize) -> u64 {
    let input: &[u8] = if ptr.is_null() || len == 0 { &[] } else { std::slice::from_raw_parts(ptr, len) };
    let out = handle(input).into_bytes().into_boxed_slice();
    let out_len = out.len();
    let out_ptr = Box::into_raw(out) as *mut u8;
    ((out_ptr as usize as u64) << 32) | (out_len as u64 & 0xFFFF_FFFF)
}

fn error(msg: impl std::fmt::Display) -> String {
    json!({ "ok": false, "error": msg.to_string() }).to_string()
}

fn text_arg(req: &Value) -> Result<&str, String> {
    req.get("text").and_then(Value::as_str).ok_or_else(|| "missing string field \"text\"".to_string())
}

fn handle(input: &[u8]) -> String {
    let Ok(s) = std::str::from_utf8(input) else { return error("request is not UTF-8") };
    let req: Value = match serde_json::from_str(s) {
        Ok(v) => v,
        Err(e) => return error(format!("invalid JSON: {e}")),
    };
    let Some(op) = req.get("op").and_then(Value::as_str) else { return error("missing string field \"op\"") };
    match dispatch(op, &req) {
        Ok(v) => v.to_string(),
        Err(e) => error(e),
    }
}

fn dispatch(op: &str, req: &Value) -> Result<Value, String> {
    match op {
        "configure" => {
            let cfg: DetectorConfig = match req.get("detector") {
                Some(v) => serde_json::from_value(v.clone()).map_err(|e| format!("invalid detector config: {e}"))?,
                None => DetectorConfig::default(),
            };
            let det = Detector::new(&cfg);
            with_state(|st| st.det = det);
            Ok(json!({ "ok": true }))
        }
        "loadVault" => {
            let raw = match req.get("vault") {
                Some(Value::String(s)) => s.clone(),
                Some(v @ Value::Object(_)) => v.to_string(),
                Some(Value::Null) | None => "{}".to_string(),
                Some(_) => return Err("\"vault\" must be an object or a JSON string".into()),
            };
            let vault = Vault::from_json(&raw).map_err(|e| format!("invalid vault: {e}"))?;
            let size = vault.len();
            with_state(|st| st.vault = vault);
            Ok(json!({ "ok": true, "size": size }))
        }
        "exportVault" => {
            let vault: Value = with_state(|st| serde_json::from_str(&st.vault.to_json())).map_err(|e| e.to_string())?;
            Ok(json!({ "ok": true, "vault": vault }))
        }
        "scan" => {
            let text = text_arg(req)?;
            let findings = with_state(|st| st.det.scan(text));
            Ok(json!({ "ok": true, "findings": findings }))
        }
        "mask" => {
            let text = text_arg(req)?;
            let ctx = MaskCtx {
                source: req.get("source").and_then(Value::as_str).unwrap_or("browser").to_string(),
                now: req.get("now").and_then(Value::as_u64).unwrap_or(0),
            };
            let (masked, report) = with_state(|st| mask_text(&st.det, &mut st.vault, text, &ctx));
            Ok(json!({ "ok": true, "text": masked, "report": report }))
        }
        "maskKnown" => {
            let text = text_arg(req)?;
            let (masked, count) = with_state(|st| mask_known(&st.vault, text));
            Ok(json!({ "ok": true, "text": masked, "count": count }))
        }
        "rehydrate" => {
            let text = text_arg(req)?;
            let (out, keys) = with_state(|st| rehydrate_text(&st.vault, text));
            Ok(json!({ "ok": true, "text": out, "keys": keys }))
        }
        "legend" => {
            let keys: Vec<String> = match req.get("keys") {
                Some(v) => serde_json::from_value(v.clone()).map_err(|_| "\"keys\" must be an array of strings".to_string())?,
                None => Vec::new(),
            };
            let text = with_state(|st| legend(&st.vault, &keys));
            Ok(json!({ "ok": true, "text": text }))
        }
        "views" => {
            let entries = with_state(|st| st.vault.views());
            Ok(json!({ "ok": true, "entries": entries }))
        }
        other => Err(format!("unknown op \"{other}\"")),
    }
}
