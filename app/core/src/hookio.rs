//! Builders for the JSON that Claude Code hooks print on stdout (exact shapes from
//! https://code.claude.com/docs/en/hooks). `None` means "print nothing" (no opinion).

use crate::guard::{Decision, Verdict};
use serde_json::Value;

/// `PreToolUse` output for a decision:
/// * Allow → `{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"allow","permissionDecisionReason":<reason_user>,"updatedInput"?:…}}`
/// * Ask   → same with `"ask"` and `reason_user` (shown to the user, not the model)
/// * Deny  → same with `"deny"` and `reason_model` (shown to the model)
/// * Defer → `None`, unless `updated_input` is set (then it is treated as Ask, see
///   [`crate::guard`]).
pub fn pre_tool_use(d: &Decision) -> Option<Value> {
    let _ = (d, Verdict::Defer);
    todo!()
}

/// `PermissionRequest` output.
/// `{"hookSpecificOutput":{"hookEventName":"PermissionRequest","decision":{"behavior":"allow"|"deny","updatedInput"?:…,"message"?:…}}}`
/// `message` is only emitted for deny; `updated_input` only for allow.
pub fn permission_request(allow: bool, message: Option<&str>, updated_input: Option<Value>) -> Value {
    let _ = (allow, message, updated_input);
    todo!()
}

/// Blocks a prompt: `{"decision":"block","reason":…,"hookSpecificOutput":{"hookEventName":"UserPromptSubmit","suppressOriginalPrompt":<suppress>}}`.
pub fn user_prompt_block(reason: &str, suppress_original: bool) -> Value {
    let _ = (reason, suppress_original);
    todo!()
}

/// Adds context to a prompt: `{"hookSpecificOutput":{"hookEventName":"UserPromptSubmit","additionalContext":…}}`.
pub fn user_prompt_context(context: &str) -> Value {
    let _ = context;
    todo!()
}

/// `{"hookSpecificOutput":{"hookEventName":"SessionStart","additionalContext":…}}`.
pub fn session_start_context(context: &str) -> Value {
    let _ = context;
    todo!()
}

/// Replaces what the model sees from a tool:
/// `{"hookSpecificOutput":{"hookEventName":"PostToolUse","updatedToolOutput":<output>,"additionalContext"?:…}}`.
pub fn post_tool_use_output(output: Value, additional_context: Option<&str>) -> Value {
    let _ = (output, additional_context);
    todo!()
}

/// Adds context after a tool ran, without replacing the output.
pub fn post_tool_use_context(context: &str) -> Value {
    let _ = context;
    todo!()
}
