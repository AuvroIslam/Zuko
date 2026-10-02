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

#[no_mangle]
pub extern "C" fn zuko_alloc(len: usize) -> *mut u8 {
    let _ = len;
    todo!()
}

/// # Safety
/// `ptr`/`len` must come from `zuko_alloc` or a `zuko_call` result.
#[no_mangle]
pub unsafe extern "C" fn zuko_free(ptr: *mut u8, len: usize) {
    let _ = (ptr, len);
    todo!()
}

/// # Safety
/// `ptr`/`len` must describe a live buffer from `zuko_alloc` holding UTF-8 JSON.
#[no_mangle]
pub unsafe extern "C" fn zuko_call(ptr: *const u8, len: usize) -> u64 {
    let _ = (ptr, len);
    todo!()
}
