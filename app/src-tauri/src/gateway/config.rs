// gateway.json: where the gateway listens, the path token Claude Code must
// present, and where masked traffic goes.
//
//   { "port": 47821, "token": "<64 hex chars>", "upstream": "https://api.anthropic.com" }
//
// The file lives in %LOCALAPPDATA%\Zuko (or $ZUKO_DATA_DIR, so tests and the dev
// gateway never touch the real one). It is created on first use; fields that are
// missing or invalid are regenerated and the rest kept, so a hand edit of one field
// survives. The token is a capability: anyone who knows it can use the proxy, so it
// is 32 bytes from the OS RNG and compared in constant time.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

pub const DEFAULT_PORT: u16 = 47821;
pub const DEFAULT_UPSTREAM: &str = "https://api.anthropic.com";
/// How many ports after the preferred one are tried before giving up.
const PORT_SCAN: u16 = 200;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Config {
    pub port: u16,
    pub token: String,
    pub upstream: String,
}

impl Config {
    /// `http://127.0.0.1:<port>/t/<token>`
    pub fn base_url(&self) -> String {
        format!("http://127.0.0.1:{}/t/{}", self.port, self.token)
    }
}

/// The file as found on disk: every field optional so one bad field doesn't
/// discard the others.
#[derive(Default, Deserialize)]
#[serde(default)]
struct Raw {
    port: Option<u16>,
    token: Option<String>,
    upstream: Option<String>,
}

/// Serializes read-modify-write cycles of the file within this process.
static LOCK: Mutex<()> = Mutex::new(());

/// %LOCALAPPDATA%\Zuko, or `$ZUKO_DATA_DIR` when set (tests, the dev gateway).
pub fn data_dir() -> PathBuf {
    match std::env::var_os("ZUKO_DATA_DIR") {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir),
        _ => crate::platform::local_dir(),
    }
}

pub fn path() -> PathBuf {
    data_dir().join("gateway.json")
}

/// Loads the config, creating or repairing the file as needed.
pub fn load_or_create() -> Config {
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    load_or_create_at(&path())
}

/// Applies `f` to the config and saves it if it changed. Returns the new config.
pub fn update(f: impl FnOnce(&mut Config)) -> Config {
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let path = path();
    let mut cfg = load_or_create_at(&path);
    let before = cfg.clone();
    f(&mut cfg);
    if cfg != before {
        save_at(&path, &cfg);
    }
    cfg
}

fn load_or_create_at(path: &Path) -> Config {
    let found = std::fs::read_to_string(path).ok();
    let raw: Raw = found.as_deref().and_then(|s| serde_json::from_str(s).ok()).unwrap_or_default();
    let mut changed = found.is_none();

    let port = match raw.port {
        Some(p) if p != 0 => p,
        _ => {
            changed = true;
            first_free_port(DEFAULT_PORT).unwrap_or(DEFAULT_PORT)
        }
    };
    let token = match raw.token {
        Some(t) if valid_token(&t) => t,
        _ => {
            changed = true;
            new_token()
        }
    };
    let upstream = match raw.upstream {
        Some(u) if valid_upstream(&u) => u,
        _ => {
            changed = true;
            DEFAULT_UPSTREAM.to_string()
        }
    };
    let cfg = Config { port, token, upstream };
    if changed {
        save_at(path, &cfg);
    }
    cfg
}

/// Writes the file atomically (temp file + rename). Failures are logged, not
/// fatal: the gateway still runs with the in-memory config.
fn save_at(path: &Path, cfg: &Config) {
    let result = (|| -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            crate::platform::ensure_private_dir(dir)?;
        }
        let text = serde_json::to_string_pretty(cfg).map_err(std::io::Error::other)?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, text)?;
        std::fs::rename(&tmp, path)
    })();
    if let Err(e) = result {
        eprintln!("zuko-gateway: could not save {}: {e}", path.display());
    }
}

fn valid_token(t: &str) -> bool {
    t.len() == 64 && t.bytes().all(|b| b.is_ascii_hexdigit())
}

/// An absolute http(s) URL with a host and no query or fragment (paths are
/// appended to it verbatim).
pub fn valid_upstream(u: &str) -> bool {
    match reqwest::Url::parse(u) {
        Ok(url) => {
            matches!(url.scheme(), "http" | "https")
                && url.host_str().is_some()
                && url.query().is_none()
                && url.fragment().is_none()
        }
        Err(_) => false,
    }
}

/// 32 random bytes, hex.
fn new_token() -> String {
    let mut buf = [0u8; 32];
    if getrandom::getrandom(&mut buf).is_err() {
        // The OS RNG does not fail on supported systems; if it ever does, derive the
        // token from process-unique, hard-to-guess inputs rather than refusing to run.
        let seed = format!(
            "{:?}|{}|{:p}|{}",
            std::time::SystemTime::now(),
            std::process::id(),
            &buf,
            std::hash::BuildHasher::hash_one(&std::collections::hash_map::RandomState::new(), 0u8),
        );
        let hex = zuko_core::audit::sha256_hex(seed.as_bytes());
        for (i, b) in buf.iter_mut().enumerate() {
            *b = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).unwrap_or(0);
        }
    }
    buf.iter().map(|b| format!("{b:02x}")).collect()
}

/// `preferred` if it can be bound on 127.0.0.1, else the next one that can.
pub fn first_free_port(preferred: u16) -> Option<u16> {
    (0..PORT_SCAN)
        .filter_map(|i| preferred.checked_add(i))
        .find(|&p| std::net::TcpListener::bind(("127.0.0.1", p)).is_ok())
}

/// Constant-time string equality (the path token).
pub fn token_eq(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_then_keeps_and_repairs() {
        let dir = std::env::temp_dir().join(format!("zuko-gw-cfg-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("gateway.json");

        let a = load_or_create_at(&path);
        assert!(path.exists());
        assert!(valid_token(&a.token));
        assert_eq!(a.upstream, DEFAULT_UPSTREAM);
        assert!(a.port >= DEFAULT_PORT);
        assert_eq!(load_or_create_at(&path), a, "stable across loads");

        // A broken token is regenerated; the other fields survive.
        std::fs::write(&path, r#"{"port":5,"token":"nope","upstream":"http://10.0.0.1:8080/x"}"#).unwrap();
        let b = load_or_create_at(&path);
        assert_eq!((b.port, b.upstream.as_str()), (5, "http://10.0.0.1:8080/x"));
        assert!(valid_token(&b.token) && b.token != a.token);
        assert!(b.base_url().starts_with("http://127.0.0.1:5/t/"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn validation() {
        assert!(valid_upstream("https://api.anthropic.com"));
        assert!(valid_upstream("http://127.0.0.1:9000/prefix"));
        assert!(!valid_upstream("ftp://x"));
        assert!(!valid_upstream("api.anthropic.com"));
        assert!(!valid_upstream("https://x/?a=1"));
        assert!(token_eq("abc", "abc"));
        assert!(!token_eq("abc", "abd"));
        assert!(!token_eq("abc", "abcd"));
    }
}
