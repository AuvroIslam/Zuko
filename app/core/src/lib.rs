//! zuko-core — the engine shared by the Zuko app, the `zuko-hook` relay, the native
//! messaging host and (compiled to WASM) the browser extension.
//!
//! Two halves:
//! * **Privacy shield** — [`detect`] finds secrets and PII, [`vault`] maps each value to a
//!   stable placeholder like `{{API_KEY_1}}`, [`mask`] and [`stream`] swap values out and
//!   back in (whole texts, JSON trees, or chunked SSE streams), [`anthropic`] applies
//!   that to Messages API requests and responses.
//! * **Agent firewall** — [`action`] turns a Claude Code tool call into a normalized
//!   [`action::Action`], [`shell`] analyses Bash/PowerShell commands, [`policy`] holds
//!   the user's rules, [`risk`] scores the action, [`taint`] tracks what a session has
//!   seen and enforces chain invariants, [`guard`] combines all of it into one
//!   [`guard::Decision`], [`hookio`] renders Claude Code hook output, [`audit`] seals
//!   tamper-evident receipts.
//! * **Local AI (optional)** — [`localai`] holds the config, prompts, strict answer
//!   validation and stricter-only merges for an on-device LLM; the app does the HTTP.
//!
//! Rules for this crate: no filesystem, network, clock or environment access (callers
//! pass time and paths in through [`Ctx`] and parameters); no C dependencies; nothing
//! here may depend on JSON key order.

pub mod action;
pub mod anthropic;
pub mod audit;
pub mod detect;
pub mod guard;
pub mod hookio;
pub mod insights;
pub mod localai;
pub mod mask;
pub mod placeholder;
pub mod policy;
pub mod risk;
pub mod shell;
pub mod stream;
pub mod taint;
pub mod vault;

#[cfg(target_arch = "wasm32")]
pub mod wasm;

use serde::{Deserialize, Serialize};

/// Everything the engine needs to know about the machine, passed in by the caller.
///
/// Paths are given in the caller's native form; the engine normalizes them with
/// [`action::normalize_path`] (forward slashes, lowercase drive letter on Windows).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Ctx {
    /// The session's working directory (`cwd` from the hook payload).
    pub cwd: String,
    /// The user's home directory (`%USERPROFILE%` / `$HOME`).
    pub home: String,
    /// The project root, usually equal to `cwd`. Writes outside it raise the risk.
    pub project_dir: String,
    /// Paths the agent must never touch (Zuko's own data and binaries, Claude Code's
    /// settings files). Matched as path prefixes or globs by the `SELF_PROTECT` invariant.
    pub protected_paths: Vec<String>,
    /// Process names that identify Zuko (e.g. `zuko.exe`), so killing it is blocked.
    pub protected_processes: Vec<String>,
    /// True on Windows: paths compare case-insensitively and PowerShell is a shell.
    pub windows: bool,
    /// True when the Zuko gateway is the session's `ANTHROPIC_BASE_URL`.
    pub gateway_active: bool,
    /// Unix seconds, supplied by the caller (the core has no clock).
    pub now: u64,
}

/// Version of the engine, reported in audit receipts and the UI.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
