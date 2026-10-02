// The proxy itself: an HTTP/1 server on 127.0.0.1 (hyper) relaying to the
// upstream (reqwest, rustls), masking Messages API requests and rehydrating their
// responses. See the module docs in gateway.rs for the contract.
//
// Data flow for a masked route:
//   body (collected, ≤ MAX_BODY) → JSON → mask_request under the vault lock, with
//   a vault snapshot taken in the same critical section → compact JSON → upstream
//   → SSE: each upstream chunk goes through SseRehydrator::push and is sent on at
//     once (the rehydrator only holds an incomplete line, a possible placeholder
//     prefix, or an allowed tool's input JSON until its block stops)
//   → JSON 2xx: rehydrate_response; anything else: verbatim.
// Everything else is streamed both ways untouched.

use std::convert::Infallible;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::{Arc, RwLock};
use std::task::{Context, Poll};
use std::time::Duration;

use bytes::Bytes;
use futures_util::{Stream, StreamExt, TryStreamExt};
use http_body_util::combinators::UnsyncBoxBody;
use http_body_util::{BodyDataStream, BodyExt, Full, Limited, StreamBody};
use hyper::body::{Frame, Incoming};
use hyper::header::{self, HeaderMap, HeaderName, HeaderValue};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use serde_json::{json, Value};
use tokio::net::TcpListener;
use zuko_core::anthropic::{self, RequestOutcome, SinkPolicy, SseRehydrator};
use zuko_core::mask::{find_placeholders, MaskCtx};
use zuko_core::vault::Vault;

use super::config::token_eq;
use super::host::{body_digest, Host, MaskedReport};
use crate::engine;

pub type BoxError = Box<dyn std::error::Error + Send + Sync>;
pub type Body = UnsyncBoxBody<Bytes, BoxError>;

/// Largest request body the gateway will mask (base64 images and PDFs included).
const MAX_BODY: usize = 256 * 1024 * 1024;

const MESSAGES: &str = "/v1/messages";
const COUNT_TOKENS: &str = "/v1/messages/count_tokens";

/// Connection-scoped headers (RFC 9110 §7.6.1) never forwarded in either direction.
const HOP_BY_HOP: [&str; 9] = [
    "connection",
    "keep-alive",
    "proxy-connection",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
];

/// State shared by every connection of one gateway instance.
pub struct Shared {
    pub host: Host,
    token: String,
    upstream: RwLock<String>,
    client: reqwest::Client,
}

impl Shared {
    pub fn new(host: Host, token: String, upstream: String) -> Result<Shared, String> {
        let client = reqwest::Client::builder()
            .use_rustls_tls()
            // A proxy relays redirects; it doesn't follow them.
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(20))
            .pool_idle_timeout(Duration::from_secs(60))
            .tcp_nodelay(true)
            .build()
            .map_err(|e| format!("could not create the HTTP client: {e}"))?;
        Ok(Shared { host, token, upstream: RwLock::new(upstream), client })
    }

    pub fn upstream(&self) -> String {
        self.upstream.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn set_upstream(&self, url: String) {
        *self.upstream.write().unwrap_or_else(|e| e.into_inner()) = url;
    }
}

/// Accepts connections until the listener fails for good. Never returns early on a
/// per-connection error.
pub async fn serve(listener: TcpListener, shared: Arc<Shared>) {
    loop {
        let (stream, peer) = match listener.accept().await {
            Ok(conn) => conn,
            Err(e) => {
                // Usually EMFILE or a reset during accept; back off briefly.
                shared.host.warn(format!("accept failed: {e}"));
                tokio::time::sleep(Duration::from_millis(100)).await;
                continue;
            }
        };
        // Bound to 127.0.0.1, so this holds; checked anyway in case that changes.
        if !peer.ip().is_loopback() {
            continue;
        }
        let _ = stream.set_nodelay(true);
        let shared = shared.clone();
        tokio::spawn(async move {
            let service = service_fn(move |req| {
                let shared = shared.clone();
                async move { Ok::<_, Infallible>(handle(shared, peer, req).await) }
            });
            // Errors here are client disconnects and malformed requests: nothing to do.
            let _ = http1::Builder::new().serve_connection(TokioIo::new(stream), service).await;
        });
    }
}

async fn handle(shared: Arc<Shared>, peer: SocketAddr, req: Request<Incoming>) -> Response<Body> {
    if !peer.ip().is_loopback() {
        return error(StatusCode::FORBIDDEN, "permission_error", "Zuko gateway: loopback clients only");
    }
    // Browsers always send Origin on cross-site fetches and form posts; Claude Code
    // never does. Refusing it keeps web pages from using the proxy.
    if req.headers().contains_key(header::ORIGIN) {
        return error(StatusCode::FORBIDDEN, "permission_error", "Zuko gateway: browser requests are not allowed");
    }
    let Some(rest) = strip_token(req.uri().path(), &shared.token) else {
        return error(StatusCode::FORBIDDEN, "permission_error", "Zuko gateway: missing or invalid path token");
    };
    let rest = rest.to_string();
    let query = req.uri().query().map(|q| format!("?{q}")).unwrap_or_default();
    let upstream = shared.upstream();
    let url = format!("{}{rest}{query}", upstream.trim_end_matches('/'));
    let label = format!("{} {rest}{query}", req.method());

    let route = match rest.as_str() {
        MESSAGES if req.method() == Method::POST => Some(MESSAGES),
        COUNT_TOKENS if req.method() == Method::POST => Some(COUNT_TOKENS),
        _ => None,
    };
    match route {
        Some(route) => masked(shared, req, route, url, upstream, label).await,
        None => passthrough(shared, req, url, upstream, label).await,
    }
}

/// `/t/<token>/rest` → `/rest` (`""` for `/t/<token>`); `None` if the token is
/// missing or wrong.
fn strip_token<'a>(path: &'a str, token: &str) -> Option<&'a str> {
    let after = path.strip_prefix("/t/")?;
    let (given, rest) = match after.find('/') {
        Some(i) => after.split_at(i),
        None => (after, ""),
    };
    token_eq(given, token).then_some(rest)
}

/// Any request on a route Zuko doesn't inspect: method, path, query, headers and
/// body streamed through; the response streamed back.
async fn passthrough(shared: Arc<Shared>, req: Request<Incoming>, url: String, upstream: String, label: String) -> Response<Body> {
    let (parts, body) = req.into_parts();
    // The body is relayed as is, so the client's Content-Length stays correct.
    let mut headers = upstream_headers(&parts.headers);
    if let Some(len) = parts.headers.get(header::CONTENT_LENGTH) {
        headers.insert(header::CONTENT_LENGTH, len.clone());
    }
    let mut request = shared.client.request(parts.method, url).headers(headers);
    if !hyper::body::Body::is_end_stream(&body) {
        request = request.body(reqwest::Body::wrap_stream(BodyDataStream::new(body)));
    }
    match request.send().await {
        Ok(resp) => {
            shared.host.trace(format!("{label} -> {}", resp.status().as_u16()));
            verbatim(resp)
        }
        Err(e) => {
            shared.host.trace(format!("{label} -> 502 ({})", error_chain(&e)));
            unreachable_upstream(&upstream, &e)
        }
    }
}

/// What masking produced for one request.
struct Masked {
    /// The body to send upstream.
    body: Bytes,
    /// Vault as it was right after masking, for rehydrating the response. `None` if
    /// the body was not JSON and went out unchanged.
    vault: Option<Vault>,
    outcome: RequestOutcome,
    report: Option<Report>,
}

/// Masking in the newest message, which is what the user just typed or a tool just
/// returned. History is re-masked on every request (and Claude Code sends several
/// requests per turn), so counting all of it would notify again and again.
struct Report {
    count: usize,
    keys: Vec<String>,
    labels: Vec<String>,
}

async fn masked(shared: Arc<Shared>, req: Request<Incoming>, route: &'static str, url: String, upstream: String, label: String) -> Response<Body> {
    let (parts, body) = req.into_parts();
    let raw = match Limited::new(body, MAX_BODY).collect().await {
        Ok(collected) => collected.to_bytes(),
        Err(e) => {
            let too_large = e.downcast_ref::<http_body_util::LengthLimitError>().is_some();
            return if too_large {
                error(StatusCode::PAYLOAD_TOO_LARGE, "request_too_large", "Zuko gateway: request body too large to inspect")
            } else {
                error(StatusCode::BAD_REQUEST, "invalid_request_error", "Zuko gateway: could not read the request body")
            };
        }
    };
    let session_id = parts
        .headers
        .get("x-claude-code-session-id")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);

    let work = {
        let shared = shared.clone();
        let raw = raw.clone();
        tokio::task::spawn_blocking(move || mask_body(&shared.host, &raw))
    };
    let m = match work.await {
        Ok(Some(m)) => m,
        // Fail closed: never forward a body we could not mask.
        Ok(None) => return error(StatusCode::SERVICE_UNAVAILABLE, "api_error", "Zuko gateway: privacy engine not available"),
        Err(_) => return error(StatusCode::INTERNAL_SERVER_ERROR, "api_error", "Zuko gateway: masking failed"),
    };

    let o = &m.outcome;
    let mut line = format!("{label} masked {} (+{} known)", o.masked.count, o.known_replaced);
    if !o.masked.keys.is_empty() {
        line.push_str(&format!(" [{}]", o.masked.keys.join(", ")));
    }
    if o.opaque_blocks > 0 {
        line.push_str(&format!(", {} opaque block(s)", o.opaque_blocks));
    }
    if !o.masked.new_keys.is_empty() {
        shared.host.persist_vault();
    }
    // count_tokens mirrors the next messages request; only the latter notifies.
    if let (Some(r), MESSAGES) = (&m.report, route) {
        shared.host.masked(&MaskedReport {
            session_id: session_id.clone(),
            route,
            count: r.count,
            keys: r.keys.clone(),
            labels: r.labels.clone(),
            new_keys: o.masked.new_keys.clone(),
            input_sha256: body_digest(&raw),
        });
    }
    shared.host.dump(&m.body);

    let mut headers = upstream_headers(&parts.headers);
    headers.remove(header::CONTENT_LENGTH); // reqwest sets it for the new body
    let sent = shared.client.request(parts.method, url).headers(headers).body(m.body).send().await;
    let resp = match sent {
        Ok(resp) => resp,
        Err(e) => {
            shared.host.trace(format!("{line} -> 502 ({})", error_chain(&e)));
            return unreachable_upstream(&upstream, &e);
        }
    };
    shared.host.trace(format!("{line} -> {}", resp.status().as_u16()));

    let rehydrate = match (route, m.vault) {
        (MESSAGES, Some(vault)) if resp.status().is_success() && !encoded(resp.headers()) => vault,
        _ => return verbatim(resp),
    };
    let ctype = resp
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();
    if ctype.starts_with("text/event-stream") {
        sse(shared, resp, rehydrate, session_id)
    } else if ctype.starts_with("application/json") {
        json_message(shared, resp, rehydrate, session_id).await
    } else {
        verbatim(resp)
    }
}

/// Masks `raw` with the shared engine. `None` if there is no engine.
fn mask_body(host: &Host, raw: &Bytes) -> Option<Masked> {
    let engine = host.engine()?;
    let mut body: Value = match serde_json::from_slice(raw) {
        Ok(v) => v,
        Err(e) => {
            host.warn(format!("request body is not JSON ({e}); forwarded unchanged"));
            return Some(Masked { body: raw.clone(), vault: None, outcome: RequestOutcome::default(), report: None });
        }
    };
    let detector = engine.detector();
    let ctx = MaskCtx { source: "gateway".into(), now: engine::now() };
    let newest_before = newest_message(&body).map(placeholder_counts);
    let (outcome, vault) = engine.with_vault(|v| {
        let outcome = anthropic::mask_request(&detector, v, &mut body, &ctx);
        (outcome, v.clone())
    });
    let sent = match serde_json::to_vec(&body) {
        Ok(b) => Bytes::from(b),
        // Serializing a Value cannot fail; if it ever did, refuse rather than leak.
        Err(_) => return None,
    };

    // Placeholders that masking added to the newest message, plus any new vault
    // entry (a value seen for the first time anywhere in the request).
    let mut keys: Vec<String> = Vec::new();
    let mut count = 0;
    if let Some(after) = newest_message(&body).map(placeholder_counts) {
        let before = newest_before.unwrap_or_default();
        for (key, n) in after {
            let was = before.iter().find(|(k, _)| *k == key).map_or(0, |(_, n)| *n);
            if n > was && vault.get(&key).is_some() {
                count += n - was;
                keys.push(key);
            }
        }
    }
    for k in &outcome.masked.new_keys {
        if !keys.contains(k) {
            keys.push(k.clone());
            count += 1;
        }
    }
    let report = (count > 0).then(|| Report {
        count,
        labels: keys.iter().map(|k| vault.get(k).map(|e| e.label.clone()).unwrap_or_default()).collect(),
        keys,
    });
    Some(Masked { body: sent, vault: Some(vault), outcome, report })
}

fn newest_message(body: &Value) -> Option<&Value> {
    body.get("messages")?.as_array()?.last()
}

/// Placeholder occurrences per key in a JSON value, in first-seen order.
fn placeholder_counts(v: &Value) -> Vec<(String, usize)> {
    let text = v.to_string();
    let mut counts: Vec<(String, usize)> = Vec::new();
    for (_, _, key) in find_placeholders(&text) {
        match counts.iter_mut().find(|(k, _)| *k == key) {
            Some((_, n)) => *n += 1,
            None => counts.push((key, 1)),
        }
    }
    counts
}

/// A streamed Messages response, rehydrated chunk by chunk.
fn sse(shared: Arc<Shared>, resp: reqwest::Response, vault: Vault, session_id: Option<String>) -> Response<Body> {
    let (status, headers) = (resp.status(), downstream_headers(resp.headers()));
    let relay = SseRelay {
        upstream: Box::pin(resp.bytes_stream()),
        rehydrator: SseRehydrator::new(SinkPolicy::default()),
        vault,
        done: false,
        shared,
        session_id,
    };
    respond(status, headers, StreamBody::new(relay).boxed_unsync())
}

struct SseRelay {
    upstream: Pin<Box<dyn Stream<Item = reqwest::Result<Bytes>> + Send>>,
    rehydrator: SseRehydrator,
    vault: Vault,
    done: bool,
    shared: Arc<Shared>,
    session_id: Option<String>,
}

impl Stream for SseRelay {
    type Item = Result<Frame<Bytes>, BoxError>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        if this.done {
            return Poll::Ready(None);
        }
        loop {
            match this.upstream.poll_next_unpin(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(Some(Ok(chunk))) => {
                    let out = this.rehydrator.push(&this.vault, &chunk);
                    if !out.is_empty() {
                        return Poll::Ready(Some(Ok(Frame::data(Bytes::from(out)))));
                    }
                    // Everything was held back (partial line, placeholder prefix,
                    // buffered tool input): wait for the next chunk.
                }
                Poll::Ready(Some(Err(e))) => {
                    // Upstream broke off: end our body with an error too, so the
                    // client sees a dropped stream (and retries) rather than a
                    // clean but truncated one.
                    this.done = true;
                    this.shared.host.trace(format!("  upstream stream failed: {}", error_chain(&e)));
                    return Poll::Ready(Some(Err(Box::new(e))));
                }
                Poll::Ready(None) => {
                    this.done = true;
                    let out = this.rehydrator.finish(&this.vault);
                    this.shared.host.rehydrated(this.session_id.take(), &this.rehydrator.keys());
                    return Poll::Ready((!out.is_empty()).then(|| Ok(Frame::data(Bytes::from(out)))));
                }
            }
        }
    }
}

/// A non-streamed Messages response: text and allowed tool inputs rehydrated.
async fn json_message(shared: Arc<Shared>, resp: reqwest::Response, vault: Vault, session_id: Option<String>) -> Response<Body> {
    let (status, headers) = (resp.status(), downstream_headers(resp.headers()));
    let bytes = match resp.bytes().await {
        Ok(b) => b,
        Err(e) => {
            return error(StatusCode::BAD_GATEWAY, "api_error", &format!("Zuko gateway: upstream response failed: {}", error_chain(&e)))
        }
    };
    let body = match serde_json::from_slice::<Value>(&bytes) {
        Ok(mut v) => {
            let keys = anthropic::rehydrate_response(&vault, &mut v, &SinkPolicy::default());
            if keys.is_empty() {
                bytes
            } else {
                shared.host.rehydrated(session_id, &keys);
                serde_json::to_vec(&v).map(Bytes::from).unwrap_or(bytes)
            }
        }
        Err(_) => bytes,
    };
    respond(status, headers, full(body))
}

/// The upstream response as received (status, headers, streamed body).
fn verbatim(resp: reqwest::Response) -> Response<Body> {
    let (status, headers) = (resp.status(), downstream_headers(resp.headers()));
    let body = resp.bytes_stream().map_ok(Frame::data).map_err(|e| -> BoxError { Box::new(e) });
    respond(status, headers, StreamBody::new(body).boxed_unsync())
}

fn respond(status: StatusCode, headers: HeaderMap, body: Body) -> Response<Body> {
    let mut resp = Response::new(body);
    *resp.status_mut() = status;
    *resp.headers_mut() = headers;
    resp
}

/// Request headers for the upstream: everything except hop-by-hop headers, Host and
/// Content-Length; Accept-Encoding forced to identity so responses can be read.
fn upstream_headers(src: &HeaderMap) -> HeaderMap {
    let mut out = filter_headers(src, &[header::HOST, header::CONTENT_LENGTH, header::ACCEPT_ENCODING]);
    out.insert(header::ACCEPT_ENCODING, HeaderValue::from_static("identity"));
    out
}

/// Response headers for the client: everything except hop-by-hop headers and
/// Content-Length (the body may change size; hyper frames it). Rate-limit, retry
/// and request-id headers pass unchanged.
fn downstream_headers(src: &HeaderMap) -> HeaderMap {
    filter_headers(src, &[header::CONTENT_LENGTH])
}

fn filter_headers(src: &HeaderMap, drop: &[HeaderName]) -> HeaderMap {
    // Headers named in Connection are hop-by-hop too.
    let listed: Vec<String> = src
        .get_all(header::CONNECTION)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .map(|t| t.trim().to_ascii_lowercase())
        .collect();
    let mut out = HeaderMap::with_capacity(src.len());
    for (name, value) in src {
        let n = name.as_str();
        if HOP_BY_HOP.contains(&n) || drop.contains(name) || listed.iter().any(|l| l == n) {
            continue;
        }
        out.append(name.clone(), value.clone());
    }
    out
}

/// True if the body is compressed despite `accept-encoding: identity` (then it is
/// relayed untouched rather than garbled).
fn encoded(headers: &HeaderMap) -> bool {
    headers
        .get(header::CONTENT_ENCODING)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| !v.trim().eq_ignore_ascii_case("identity"))
}

fn full(bytes: Bytes) -> Body {
    Full::new(bytes).map_err(|never| match never {}).boxed_unsync()
}

/// An Anthropic-style error response.
fn error(status: StatusCode, kind: &str, message: &str) -> Response<Body> {
    let body = json!({ "type": "error", "error": { "type": kind, "message": message } });
    let mut headers = HeaderMap::new();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/json"));
    respond(status, headers, full(Bytes::from(body.to_string())))
}

fn unreachable_upstream(upstream: &str, e: &reqwest::Error) -> Response<Body> {
    let message = format!("Zuko gateway could not reach {upstream}: {}", error_chain(e));
    error(StatusCode::BAD_GATEWAY, "api_error", &message)
}

/// `e` and its sources, joined (reqwest's top-level message alone is vague).
fn error_chain(e: &(dyn std::error::Error + 'static)) -> String {
    let mut text = e.to_string();
    let mut source = e.source();
    while let Some(s) = source {
        let s_text = s.to_string();
        if !text.contains(&s_text) {
            text.push_str(": ");
            text.push_str(&s_text);
        }
        source = s.source();
    }
    text
}

/// Starts serving `shared` on `listener` in the background (used by tests).
#[cfg(test)]
pub fn spawn(listener: TcpListener, shared: Arc<Shared>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(serve(listener, shared))
}
