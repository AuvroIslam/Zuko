//! Normalizes a Claude Code tool call (`tool_name` + `tool_input` from a hook payload)
//! into an [`Action`]: what it reads, writes, deletes, which hosts it contacts, what
//! text it sends out. Everything downstream (policy, risk, taint) works on Actions.
//!
//! Tool coverage (input field names from the Claude Code tools reference):
//! * `Read {file_path, offset, limit}` → reads
//! * `Write {file_path, content}` → writes (+ content as outbound text only if the path is
//!   outside the project, see `egress_text` docs)
//! * `Edit {file_path, old_string, new_string, replace_all}`, `MultiEdit {file_path, edits}`,
//!   `NotebookEdit {notebook_path, new_source, …}` → writes
//! * `Glob {pattern, path}`, `Grep {pattern, path, glob, …}`, `LS {path}` → reads (the
//!   searched directory)
//! * `WebFetch {url, prompt}` → hosts/urls, egress_text = url
//! * `WebSearch {query, allowed_domains, blocked_domains}` → host `websearch`, egress_text = query
//! * `Bash {command, …}` → [`crate::shell::analyze`] with `Dialect::Bash`
//! * `PowerShell {command, …}` → `Dialect::PowerShell`
//! * `mcp__<server>__<tool>` → kind `Mcp`, mcp_server, egress_text = every string leaf
//! * `Task`/`Agent`, `TodoWrite`, `ExitPlanMode`, others → kind `Other` (low impact)
//!
//! Paths: [`normalize_path`] resolves relative paths against `ctx.cwd`, expands `~`,
//! collapses `.`/`..`, converts `\` to `/`, lowercases the drive letter, and on Windows
//! lowercases the whole path for comparisons (`ctx.windows`).

use crate::shell::ShellAnalysis;
use crate::Ctx;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionKind {
    Read,
    Write,
    Search,
    Fetch,
    WebSearch,
    Shell,
    Mcp,
    #[default]
    Other,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Action {
    pub tool: String,
    pub kind: ActionKind,
    /// Normalized absolute paths.
    pub reads: Vec<String>,
    pub writes: Vec<String>,
    pub deletes: Vec<String>,
    /// Lowercased hosts.
    pub hosts: Vec<String>,
    pub urls: Vec<String>,
    /// The shell command, for Bash/PowerShell.
    pub command: Option<String>,
    pub shell: Option<ShellAnalysis>,
    pub mcp_server: Option<String>,
    /// Text that leaves the machine if this action runs: shell command lines of
    /// networked commands, WebFetch URLs, WebSearch queries, MCP arguments. Used by the
    /// `SECRET_EGRESS` invariant to look for secret values (and their encodings).
    pub egress_text: Vec<String>,
    /// Short human summary for feeds and cards: "Read .env", "Run npm test",
    /// "Fetch docs.python.org", "Edit src/main.rs". Paths shown relative to the project
    /// when inside it.
    pub summary: String,
}

/// Builds an Action from a hook's `tool_name` and `tool_input`.
pub fn from_tool_call(tool_name: &str, input: &Value, ctx: &Ctx) -> Action {
    let _ = (tool_name, input, ctx);
    todo!()
}

/// Resolves `p` to a normalized absolute path (see module docs). Returns `p` cleaned up
/// as best it can if it cannot be resolved.
pub fn normalize_path(p: &str, ctx: &Ctx) -> String {
    let _ = (p, ctx);
    todo!()
}

/// True if normalized `path` is `dir` or inside it (component-wise, not string prefix).
pub fn is_within(path: &str, dir: &str) -> bool {
    let _ = (path, dir);
    todo!()
}

/// Shows `path` relative to `ctx.project_dir` when inside it, else as given.
pub fn display_path(path: &str, ctx: &Ctx) -> String {
    let _ = (path, ctx);
    todo!()
}
