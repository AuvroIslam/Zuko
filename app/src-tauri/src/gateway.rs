// The Zuko gateway: a local Anthropic API proxy on 127.0.0.1 that Claude Code
// reaches through ANTHROPIC_BASE_URL=http://127.0.0.1:<port>/t/<token>.
//
// * POST …/v1/messages and …/v1/messages/count_tokens (any query string, e.g.
//   `?beta=true`): the JSON body is masked with zuko_core::anthropic::mask_request
//   (shared Engine vault + detector; the vault lock is held only while masking) and
//   re-serialized compactly, then forwarded to the upstream. A body that is not JSON
//   is forwarded unchanged (and logged). Responses to /v1/messages are rehydrated
//   with the default SinkPolicy, using a vault snapshot taken right after masking:
//   SseRehydrator for 2xx `text/event-stream` (chunk by chunk, no extra buffering;
//   pings forwarded; an upstream disconnect ends our stream with an error too),
//   rehydrate_response for 2xx `application/json`. Error statuses pass unchanged.
// * Every other path and method is passed through untouched (body streamed both ways).
// * Headers: everything is forwarded verbatim (anthropic-version, anthropic-beta,
//   authorization, x-api-key, x-claude-code-*), except hop-by-hop headers, Host,
//   Content-Length (recomputed on masked routes) and Accept-Encoding (forced to
//   identity so the stream can be read). Upstream status codes, error bodies and
//   rate-limit / retry / request-id headers are returned unchanged. An unreachable
//   upstream yields a 502 Anthropic-style error naming it.
// * Requests without the path token, or carrying an Origin header (browsers), are
//   rejected with 403. Only loopback peers are served.
// * Config in %LOCALAPPDATA%\Zuko\gateway.json (or $ZUKO_DATA_DIR): { port, token,
//   upstream }. Port defaults to 47821 (next free port if taken), token is 32 random
//   bytes hex, upstream defaults to https://api.anthropic.com (or the user's previous
//   ANTHROPIC_BASE_URL captured at install time, see set_upstream).
// * When masking added placeholders to the newest message (what the user just typed
//   or a tool just returned) or created vault entries, the request emits a `privacy`
//   event (source "gateway"), an activity item and an audit receipt (event "Gateway",
//   verdict "masked", request body SHA-256), which counts on today's "masked" counter.
//   History is re-masked on every request, so it is not reported again. Only keys
//   and labels are ever reported or logged, never values.
//
// Layout: config.rs (gateway.json), host.rs (app vs headless: engine access,
// events, logging), proxy.rs (the HTTP server and the transforms).

mod config;
mod host;
mod proxy;
#[cfg(test)]
mod tests;

use std::sync::{Arc, Mutex};

use serde::Serialize;
use tauri::AppHandle;

use self::host::{Headless, Host};
use self::proxy::Shared;

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GatewayStatus {
    pub running: bool,
    pub port: u16,
    /// Full base URL including the path token.
    pub url: Option<String>,
    pub upstream: String,
}

/// The instance serving in this process, if any.
struct Running {
    shared: Arc<Shared>,
    config: config::Config,
}

static RUNNING: Mutex<Option<Running>> = Mutex::new(None);

fn running() -> std::sync::MutexGuard<'static, Option<Running>> {
    RUNNING.lock().unwrap_or_else(|e| e.into_inner())
}

/// Starts the gateway on Tauri's async runtime. Logs and returns on failure.
pub fn start(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        match launch(Host::App(app)).await {
            Ok((cfg, server)) => {
                crate::log::line(format!("gateway: listening on 127.0.0.1:{} → {}", cfg.port, cfg.upstream));
                server.await;
                crate::log::line("gateway: stopped");
            }
            Err(e) => crate::log::line(format!("gateway: not started: {e}")),
        }
    });
}

/// Binds the configured port (or the next free one, saved back), registers the
/// instance and returns the server future.
async fn launch(host: Host) -> Result<(config::Config, impl std::future::Future<Output = ()>), String> {
    if running().is_some() {
        return Err("already running".into());
    }
    let mut cfg = config::load_or_create();
    let listener = match tokio::net::TcpListener::bind(("127.0.0.1", cfg.port)).await {
        Ok(l) => l,
        Err(first) => {
            // Taken (another program, or a second Zuko). Move to a free port and
            // remember it; the installer compares the configured URL with ours.
            let port = config::first_free_port(cfg.port.saturating_add(1))
                .ok_or_else(|| format!("port {} is unavailable ({first}) and no free port was found", cfg.port))?;
            let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
                .await
                .map_err(|e| format!("could not listen on 127.0.0.1:{port}: {e}"))?;
            host.warn(format!("port {} unavailable ({first}); using {port}", cfg.port));
            cfg = config::update(|c| c.port = port);
            listener
        }
    };
    let shared = Arc::new(Shared::new(host, cfg.token.clone(), cfg.upstream.clone())?);
    *running() = Some(Running { shared: shared.clone(), config: cfg.clone() });
    let server = async move {
        proxy::serve(listener, shared).await;
        *running() = None;
    };
    Ok((cfg, server))
}

pub fn status() -> GatewayStatus {
    if let Some(r) = running().as_ref() {
        return GatewayStatus {
            running: true,
            port: r.config.port,
            url: Some(r.config.base_url()),
            upstream: r.shared.upstream(),
        };
    }
    let cfg = config::load_or_create();
    GatewayStatus { running: false, port: cfg.port, url: Some(cfg.base_url()), upstream: cfg.upstream }
}

/// The base URL Claude Code should use (`http://127.0.0.1:<port>/t/<token>`),
/// creating the config on first call. Used by the installer.
pub fn base_url() -> String {
    if let Some(r) = running().as_ref() {
        return r.config.base_url();
    }
    config::load_or_create().base_url()
}

/// Records the user's previous ANTHROPIC_BASE_URL as the upstream (installer).
/// Ignored if it is not an http(s) URL or points back at this gateway (a
/// reinstall would otherwise make the proxy forward to itself).
pub fn set_upstream(url: &str) {
    let url = url.trim().trim_end_matches('/').to_string();
    if !config::valid_upstream(&url) {
        return;
    }
    let cfg = config::load_or_create();
    let parsed = reqwest::Url::parse(&url).ok();
    let host = parsed.as_ref().and_then(|u| u.host_str()).unwrap_or("").to_ascii_lowercase();
    let loopback = matches!(host.as_str(), "127.0.0.1" | "localhost" | "[::1]" | "::1");
    let ours = loopback && parsed.as_ref().and_then(|u| u.port()) == Some(cfg.port);
    if ours || url.contains(&format!("/t/{}", cfg.token)) {
        return;
    }
    config::update(|c| c.upstream = url.clone());
    if let Some(r) = running().as_ref() {
        r.shared.set_upstream(url);
    }
}

/// Runs the gateway without the UI, with an in-memory engine (default policy, empty
/// vault), on the configured port; prints the base URL and logs each request
/// (method, path, masked count and keys, never values) to stderr. With
/// `ZUKO_GATEWAY_DUMP=<file>`, every masked body sent upstream is appended to that
/// file. Blocks until killed. For development and E2E tests.
pub fn dev_main() {
    use zuko_core::policy::Policy;
    use zuko_core::vault::Vault;

    let runtime = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("zuko-gateway: could not start the runtime: {e}");
            std::process::exit(1);
        }
    };
    let base = crate::engine::CtxBase {
        home: crate::platform::home_dir().to_string_lossy().to_string(),
        windows: cfg!(windows),
        ..Default::default()
    };
    let headless = Headless {
        engine: crate::engine::Engine::with_parts(Policy::default(), Vault::new(), base),
        verbose: true,
        dump: std::env::var_os("ZUKO_GATEWAY_DUMP").filter(|p| !p.is_empty()).map(Into::into),
    };
    runtime.block_on(async move {
        match launch(Host::Headless(headless)).await {
            Ok((cfg, server)) => {
                println!("{}", cfg.base_url());
                eprintln!("zuko-gateway: config {}", config::path().display());
                eprintln!("zuko-gateway: listening on 127.0.0.1:{} -> {}", cfg.port, cfg.upstream);
                server.await;
            }
            Err(e) => {
                eprintln!("zuko-gateway: {e}");
                std::process::exit(1);
            }
        }
    });
}
