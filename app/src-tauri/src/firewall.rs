// The hook path: turns Claude Code hook events (relayed by zuko-hook over the
// pipe) into decisions, using the shared Engine.
//
// Per event (see CONTRACTS.md §1 and plan.md §5):
// * SessionStart       → open the session ledger; return additionalContext explaining
//                        the placeholder convention when the vault is non-empty.
// * UserPromptSubmit   → gateway active: count what will be masked, emit a `privacy`
//                        event, no output. Gateway expected but this session's
//                        ANTHROPIC_BASE_URL differs: warn via additionalContext + UI.
//                        Hooks-only: if the prompt contains secrets and the policy says
//                        so, block with suppressOriginalPrompt and emit a
//                        `blocked_prompt` privacy event carrying the masked prompt.
// * PreToolUse         → zuko_core::guard::decide with the session ledger and vault;
//                        record the step; emit `activity`; append an audit receipt;
//                        return hookio::pre_tool_use(&decision).
// * PostToolUse        → ledger.record_post with the tool response; in hooks-only mode
//                        mask Bash/PowerShell output via updatedToolOutput.
// * PermissionRequest  → handled in pipe.rs (human decision); this module supplies
//                        the `zuko` risk info attached to the UI payload.
// * Stop / SessionEnd  → activity + ledger cleanup.
// * ZukoExtension      → browser extension messages via the native host (CONTRACTS §4).
//
// OWNER: hook path (wave 2). Stub until then: no opinion on anything.

use serde_json::Value;
use tauri::AppHandle;

/// Handles one relayed event. Returns the object the relay must print on stdout,
/// or None for "no opinion". Must answer well within the relay's 1.5 s budget.
pub async fn handle_event(app: &AppHandle, payload: &Value) -> Option<Value> {
    let _ = (app, payload);
    None
}

/// The `zuko` risk info (CONTRACTS.md `ZukoHookInfo`) for a PermissionRequest
/// payload, attached before it is shown on the island.
pub fn permission_info(app: &AppHandle, payload: &Value) -> Option<Value> {
    let _ = (app, payload);
    None
}

/// Handles a browser-extension message (CONTRACTS.md §4) and returns the reply.
pub async fn handle_extension(app: &AppHandle, message: &Value) -> Value {
    let _ = (app, message);
    serde_json::json!({ "ok": false, "error": "not implemented" })
}
