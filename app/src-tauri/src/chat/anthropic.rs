// Claude, through the Anthropic Messages API, with server-side web search. The
// original (and default) provider of the island chat. The history is already in the
// shape this API takes, so the request is the history as it is.
//
// Only masked text reaches this file (see chat.rs).

use std::time::Duration;

use serde_json::{json, Value};

const ENDPOINT: &str = "https://api.anthropic.com/v1/messages";
const ANTHROPIC_VERSION: &str = "2023-06-01";
/// Server-side fallback: on a policy decline the API retries the same request on
/// a fallback model inside the same call, so the island never shows a dead end.
const FALLBACK_BETA: &str = "server-side-fallback-2026-07-01";
const MAX_TOKENS: u32 = 4096;
const TIMEOUT: Duration = Duration::from_secs(90);

pub const DEFAULT_MODEL: &str = "claude-opus-5";

/// The models the settings window offers (labels live in the UI).
pub const MODELS: &[&str] = &["claude-opus-5", "claude-sonnet-5", "claude-haiku-4-5"];

pub(super) fn request(model: &str, system: &str, history: &[Value]) -> Value {
    json!({
        "model": model,
        "max_tokens": MAX_TOKENS,
        "system": system,
        "tools": [{ "type": "web_search_20260209", "name": "web_search", "max_uses": 5 }],
        "fallbacks": "default",
        "messages": history,
    })
}

/// The whole content — tool_use / tool_result blocks included — so the next turn has
/// the right context.
pub(super) fn reply(response: &Value) -> Result<Vec<Value>, String> {
    // A policy decline comes back as HTTP 200 with stop_reason "refusal".
    if response.get("stop_reason").and_then(Value::as_str) == Some("refusal") {
        let why = response
            .get("stop_details")
            .and_then(|d| d.get("explanation"))
            .and_then(Value::as_str)
            .unwrap_or("Claude declined this one.");
        return Err(why.to_string());
    }
    response
        .get("content")
        .and_then(Value::as_array)
        .cloned()
        .ok_or_else(|| "Unexpected API response.".to_string())
}

pub(super) async fn call(key: &str, body: &Value) -> Result<Value, String> {
    let response = super::cloud_client(TIMEOUT)?
        .post(ENDPOINT)
        .header("x-api-key", key)
        .header("anthropic-version", ANTHROPIC_VERSION)
        .header("anthropic-beta", FALLBACK_BETA)
        .header("content-type", "application/json")
        .json(body)
        .send()
        .await
        .map_err(|e| format!("Network error: {}", super::root_cause(&e)))?;

    let status = response.status();
    let text = response.text().await.map_err(|e| format!("Network error: {}", super::root_cause(&e)))?;
    if !status.is_success() {
        // Surface the API's own message, which is what makes a bad key obvious.
        let detail = serde_json::from_str::<Value>(&text)
            .ok()
            .and_then(|v| {
                v.get("error")
                    .and_then(|e| e.get("message"))
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .unwrap_or_else(|| text.chars().take(200).collect());
        return Err(format!("Claude API {status}: {detail}"));
    }
    serde_json::from_str(&text).map_err(|e| format!("Bad API response: {e}"))
}
