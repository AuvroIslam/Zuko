// The Zuko gateway: a local Anthropic API proxy on 127.0.0.1 that Claude Code
// reaches through ANTHROPIC_BASE_URL=http://127.0.0.1:<port>/t/<token>.
//
// * POST …/v1/messages and …/v1/messages/count_tokens: the JSON body is masked
//   with zuko_core::anthropic::mask_request (shared Engine vault + detector), then
//   forwarded to the upstream; responses are rehydrated (SseRehydrator for
//   `text/event-stream`, rehydrate_response for JSON) with the default SinkPolicy.
//   Streaming is relayed chunk by chunk without buffering; pings are forwarded.
// * Every other path and method is passed through untouched.
// * Headers: everything is forwarded verbatim (anthropic-version, anthropic-beta,
//   authorization, x-api-key, x-claude-code-*), except hop-by-hop headers, Host,
//   Content-Length (recomputed) and Accept-Encoding (forced to identity so the
//   stream can be read). Upstream status codes, error bodies and rate-limit /
//   retry headers are returned unchanged.
// * Requests without the path token, or carrying an Origin header (browsers), are
//   rejected with 403. Only loopback peers are served.
// * Config in %LOCALAPPDATA%\Zuko\gateway.json: { port, token, upstream }. Port
//   defaults to 47821 (next free port if taken), token is 32 random bytes hex,
//   upstream defaults to https://api.anthropic.com (or the user's previous
//   ANTHROPIC_BASE_URL captured at install time).
// * Each masked request emits a `privacy` event (source "gateway") when anything
//   was masked, and an audit receipt (event "Gateway", verdict "masked").
//
// OWNER: gateway (wave 2). Stub until then: never starts.

use serde::Serialize;
use tauri::AppHandle;

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GatewayStatus {
    pub running: bool,
    pub port: u16,
    /// Full base URL including the path token.
    pub url: Option<String>,
    pub upstream: String,
}

/// Starts the gateway on Tauri's async runtime. Logs and returns on failure.
pub fn start(app: AppHandle) {
    let _ = app;
}

pub fn status() -> GatewayStatus {
    GatewayStatus { upstream: "https://api.anthropic.com".into(), ..Default::default() }
}

/// The base URL Claude Code should use (`http://127.0.0.1:<port>/t/<token>`),
/// creating the config on first call. Used by the installer.
pub fn base_url() -> String {
    String::new()
}

/// Records the user's previous ANTHROPIC_BASE_URL as the upstream (installer).
pub fn set_upstream(url: &str) {
    let _ = url;
}
