//! Builders for the JSON that Claude Code hooks print on stdout (exact shapes from
//! https://code.claude.com/docs/en/hooks). `None` means "print nothing" (no opinion).

use crate::guard::{Decision, Verdict};
use serde_json::{json, Map, Value};

/// `PreToolUse` output for a decision:
/// * Allow → `{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"allow","permissionDecisionReason":<reason_user>,"updatedInput"?:…}}`
/// * Ask   → same with `"ask"` and `reason_user` (shown to the user, not the model)
/// * Deny  → same with `"deny"` and `reason_model` (shown to the model)
/// * Defer → `None`, unless `updated_input` is set (then it is treated as Ask, see
///   [`crate::guard`]).
pub fn pre_tool_use(d: &Decision) -> Option<Value> {
    let verdict = match (d.verdict, &d.updated_input) {
        (Verdict::Defer, None) => return None,
        (Verdict::Defer, Some(_)) => Verdict::Ask,
        (v, _) => v,
    };
    let reason = match verdict {
        Verdict::Deny => &d.reason_model,
        _ => &d.reason_user,
    };
    let mut out = Map::new();
    out.insert("hookEventName".into(), json!("PreToolUse"));
    out.insert("permissionDecision".into(), json!(verdict.as_str()));
    out.insert("permissionDecisionReason".into(), json!(reason));
    // A denied call never runs, so a rewritten input would only leak values into logs.
    if verdict != Verdict::Deny {
        if let Some(u) = &d.updated_input {
            out.insert("updatedInput".into(), u.clone());
        }
    }
    Some(json!({ "hookSpecificOutput": Value::Object(out) }))
}

/// `PermissionRequest` output.
/// `{"hookSpecificOutput":{"hookEventName":"PermissionRequest","decision":{"behavior":"allow"|"deny","updatedInput"?:…,"message"?:…}}}`
/// `message` is only emitted for deny; `updated_input` only for allow.
pub fn permission_request(allow: bool, message: Option<&str>, updated_input: Option<Value>) -> Value {
    let mut decision = Map::new();
    decision.insert("behavior".into(), json!(if allow { "allow" } else { "deny" }));
    if allow {
        if let Some(u) = updated_input {
            decision.insert("updatedInput".into(), u);
        }
    } else if let Some(m) = message {
        decision.insert("message".into(), json!(m));
    }
    json!({
        "hookSpecificOutput": {
            "hookEventName": "PermissionRequest",
            "decision": Value::Object(decision),
        }
    })
}

/// Blocks a prompt: `{"decision":"block","reason":…,"hookSpecificOutput":{"hookEventName":"UserPromptSubmit","suppressOriginalPrompt":<suppress>}}`.
pub fn user_prompt_block(reason: &str, suppress_original: bool) -> Value {
    json!({
        "decision": "block",
        "reason": reason,
        "hookSpecificOutput": {
            "hookEventName": "UserPromptSubmit",
            "suppressOriginalPrompt": suppress_original,
        }
    })
}

/// Adds context to a prompt: `{"hookSpecificOutput":{"hookEventName":"UserPromptSubmit","additionalContext":…}}`.
pub fn user_prompt_context(context: &str) -> Value {
    json!({
        "hookSpecificOutput": {
            "hookEventName": "UserPromptSubmit",
            "additionalContext": context,
        }
    })
}

/// `{"hookSpecificOutput":{"hookEventName":"SessionStart","additionalContext":…}}`.
pub fn session_start_context(context: &str) -> Value {
    json!({
        "hookSpecificOutput": {
            "hookEventName": "SessionStart",
            "additionalContext": context,
        }
    })
}

/// Replaces what the model sees from a tool:
/// `{"hookSpecificOutput":{"hookEventName":"PostToolUse","updatedToolOutput":<output>,"additionalContext"?:…}}`.
pub fn post_tool_use_output(output: Value, additional_context: Option<&str>) -> Value {
    let mut out = Map::new();
    out.insert("hookEventName".into(), json!("PostToolUse"));
    out.insert("updatedToolOutput".into(), output);
    if let Some(c) = additional_context {
        out.insert("additionalContext".into(), json!(c));
    }
    json!({ "hookSpecificOutput": Value::Object(out) })
}

/// Adds context after a tool ran, without replacing the output.
pub fn post_tool_use_context(context: &str) -> Value {
    json!({
        "hookSpecificOutput": {
            "hookEventName": "PostToolUse",
            "additionalContext": context,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::guard::Decision;

    fn decision(verdict: Verdict, updated: Option<Value>) -> Decision {
        Decision {
            verdict,
            reason_user: "⚠ HIGH RISK — DELETES build/".into(),
            reason_model: "Zuko blocked this.".into(),
            updated_input: updated,
            ..Default::default()
        }
    }

    #[test]
    fn pre_tool_use_allow_ask_deny() {
        let allow = pre_tool_use(&decision(Verdict::Allow, None)).unwrap();
        assert_eq!(
            allow,
            json!({"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"allow","permissionDecisionReason":"⚠ HIGH RISK — DELETES build/"}})
        );
        let ask = pre_tool_use(&decision(Verdict::Ask, None)).unwrap();
        assert_eq!(ask["hookSpecificOutput"]["permissionDecision"], "ask");
        assert_eq!(ask["hookSpecificOutput"]["permissionDecisionReason"], "⚠ HIGH RISK — DELETES build/");
        let deny = pre_tool_use(&decision(Verdict::Deny, Some(json!({"command": "x"})))).unwrap();
        assert_eq!(
            deny,
            json!({"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"deny","permissionDecisionReason":"Zuko blocked this."}})
        );
    }

    #[test]
    fn pre_tool_use_defer_and_updated_input() {
        assert!(pre_tool_use(&decision(Verdict::Defer, None)).is_none());
        let up = json!({"file_path": "C:\\p\\.env", "content": "K=real"});
        let d = pre_tool_use(&decision(Verdict::Defer, Some(up.clone()))).unwrap();
        assert_eq!(d["hookSpecificOutput"]["permissionDecision"], "ask");
        assert_eq!(d["hookSpecificOutput"]["updatedInput"], up);
        let a = pre_tool_use(&decision(Verdict::Allow, Some(up.clone()))).unwrap();
        assert_eq!(a["hookSpecificOutput"]["permissionDecision"], "allow");
        assert_eq!(a["hookSpecificOutput"]["updatedInput"], up);
    }

    #[test]
    fn permission_request_shapes() {
        assert_eq!(
            permission_request(true, Some("ignored"), Some(json!({"command": "npm test"}))),
            json!({"hookSpecificOutput":{"hookEventName":"PermissionRequest","decision":{"behavior":"allow","updatedInput":{"command":"npm test"}}}})
        );
        assert_eq!(
            permission_request(true, None, None),
            json!({"hookSpecificOutput":{"hookEventName":"PermissionRequest","decision":{"behavior":"allow"}}})
        );
        assert_eq!(
            permission_request(false, Some("Denied in Zuko"), Some(json!({"x": 1}))),
            json!({"hookSpecificOutput":{"hookEventName":"PermissionRequest","decision":{"behavior":"deny","message":"Denied in Zuko"}}})
        );
        assert_eq!(
            permission_request(false, None, None),
            json!({"hookSpecificOutput":{"hookEventName":"PermissionRequest","decision":{"behavior":"deny"}}})
        );
    }

    #[test]
    fn prompt_and_session_shapes() {
        assert_eq!(
            user_prompt_block("Secret found", true),
            json!({"decision":"block","reason":"Secret found","hookSpecificOutput":{"hookEventName":"UserPromptSubmit","suppressOriginalPrompt":true}})
        );
        assert_eq!(
            user_prompt_context("ctx"),
            json!({"hookSpecificOutput":{"hookEventName":"UserPromptSubmit","additionalContext":"ctx"}})
        );
        assert_eq!(
            session_start_context("hello"),
            json!({"hookSpecificOutput":{"hookEventName":"SessionStart","additionalContext":"hello"}})
        );
    }

    #[test]
    fn post_tool_use_shapes() {
        let out = json!({"stdout": "{{API_KEY_1}}", "stderr": "", "interrupted": false, "isImage": false});
        assert_eq!(
            post_tool_use_output(out.clone(), None),
            json!({"hookSpecificOutput":{"hookEventName":"PostToolUse","updatedToolOutput":out}})
        );
        assert_eq!(
            post_tool_use_output(out.clone(), Some("masked 1 value")),
            json!({"hookSpecificOutput":{"hookEventName":"PostToolUse","updatedToolOutput":out,"additionalContext":"masked 1 value"}})
        );
        assert_eq!(
            post_tool_use_context("note"),
            json!({"hookSpecificOutput":{"hookEventName":"PostToolUse","additionalContext":"note"}})
        );
    }
}
