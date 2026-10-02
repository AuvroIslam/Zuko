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

use crate::shell::{analyze, Dialect, ShellAnalysis};
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

impl Action {
    /// True if this action sends anything off the machine.
    pub fn is_egress(&self) -> bool {
        matches!(self.kind, ActionKind::Fetch | ActionKind::WebSearch | ActionKind::Mcp)
            || self.shell.as_ref().map(|s| s.network).unwrap_or(false)
            || !self.hosts.is_empty()
    }
}

fn str_field<'a>(input: &'a Value, key: &str) -> Option<&'a str> {
    input.get(key).and_then(Value::as_str)
}

fn collect_strings(v: &Value, out: &mut Vec<String>) {
    match v {
        Value::String(s) => out.push(s.clone()),
        Value::Array(a) => a.iter().for_each(|x| collect_strings(x, out)),
        Value::Object(o) => o.values().for_each(|x| collect_strings(x, out)),
        _ => {}
    }
}

/// Builds an Action from a hook's `tool_name` and `tool_input`.
pub fn from_tool_call(tool_name: &str, input: &Value, ctx: &Ctx) -> Action {
    let mut a = Action {
        tool: tool_name.to_string(),
        ..Default::default()
    };

    if let Some(rest) = tool_name.strip_prefix("mcp__") {
        a.kind = ActionKind::Mcp;
        let server = rest.split("__").next().unwrap_or(rest);
        a.mcp_server = Some(server.to_string());
        collect_strings(input, &mut a.egress_text);
        a.summary = format!("Call MCP tool {tool_name}");
        return a;
    }

    match tool_name {
        "Read" => {
            if let Some(p) = str_field(input, "file_path") {
                let n = normalize_path(p, ctx);
                a.summary = format!("Read {}", display_path(&n, ctx));
                a.reads.push(n);
            }
            a.kind = ActionKind::Read;
        }
        "Write" => {
            a.kind = ActionKind::Write;
            if let Some(p) = str_field(input, "file_path") {
                let n = normalize_path(p, ctx);
                a.summary = format!("Write {}", display_path(&n, ctx));
                if !is_within(&n, &norm_dir(&ctx.project_dir, ctx)) {
                    if let Some(c) = str_field(input, "content") {
                        a.egress_text.push(c.to_string());
                    }
                }
                a.writes.push(n);
            }
        }
        "Edit" | "MultiEdit" => {
            a.kind = ActionKind::Write;
            if let Some(p) = str_field(input, "file_path") {
                let n = normalize_path(p, ctx);
                a.summary = format!("Edit {}", display_path(&n, ctx));
                a.writes.push(n);
            }
        }
        "NotebookEdit" => {
            a.kind = ActionKind::Write;
            if let Some(p) = str_field(input, "notebook_path").or_else(|| str_field(input, "file_path")) {
                let n = normalize_path(p, ctx);
                a.summary = format!("Edit {}", display_path(&n, ctx));
                a.writes.push(n);
            }
        }
        "Glob" | "Grep" | "LS" => {
            a.kind = ActionKind::Search;
            let dir = str_field(input, "path").unwrap_or(&ctx.cwd);
            let n = normalize_path(dir, ctx);
            a.summary = format!("Search {}", display_path(&n, ctx));
            a.reads.push(n);
        }
        "WebFetch" => {
            a.kind = ActionKind::Fetch;
            if let Some(url) = str_field(input, "url") {
                a.urls.push(url.to_string());
                if let Some(h) = host_of(url) {
                    a.hosts.push(h);
                }
                a.egress_text.push(url.to_string());
                a.summary = format!("Fetch {}", a.hosts.first().cloned().unwrap_or_else(|| url.to_string()));
            }
            if let Some(prompt) = str_field(input, "prompt") {
                a.egress_text.push(prompt.to_string());
            }
        }
        "WebSearch" => {
            a.kind = ActionKind::WebSearch;
            a.hosts.push("websearch".into());
            if let Some(q) = str_field(input, "query") {
                a.egress_text.push(q.to_string());
                a.summary = format!("Web search: {}", truncate(q, 48));
            } else {
                a.summary = "Web search".into();
            }
        }
        "Bash" | "PowerShell" => {
            a.kind = ActionKind::Shell;
            let dialect = if tool_name == "PowerShell" { Dialect::PowerShell } else { Dialect::Bash };
            if let Some(cmd) = str_field(input, "command") {
                a.command = Some(cmd.to_string());
                let sh = analyze(cmd, dialect);
                for p in &sh.reads {
                    a.reads.push(normalize_path(p, ctx));
                }
                for p in &sh.writes {
                    a.writes.push(normalize_path(p, ctx));
                }
                for p in &sh.deletes {
                    a.deletes.push(normalize_path(p, ctx));
                }
                a.hosts = sh.hosts.clone();
                a.urls = sh.urls.clone();
                if sh.network || sh.pipe_to_shell {
                    a.egress_text.push(cmd.to_string());
                }
                a.summary = format!("Run {}", truncate(cmd.trim(), 56));
                a.shell = Some(sh);
            } else {
                a.summary = "Run a command".into();
            }
        }
        "Task" | "Agent" | "TodoWrite" | "ExitPlanMode" => {
            a.kind = ActionKind::Other;
            a.summary = tool_name.to_string();
        }
        _ => {
            a.kind = ActionKind::Other;
            a.summary = tool_name.to_string();
        }
    }
    a
}

fn truncate(s: &str, max: usize) -> String {
    let s = s.replace('\n', " ");
    if s.chars().count() <= max {
        s
    } else {
        let t: String = s.chars().take(max.saturating_sub(1)).collect();
        format!("{t}…")
    }
}

fn host_of(url: &str) -> Option<String> {
    let pos = url.find("://")?;
    let after = &url[pos + 3..];
    let hostport = after.split(['/', '?', '#']).next().unwrap_or(after);
    let host = hostport.rsplit('@').next().unwrap_or(hostport);
    let host = host.split(':').next().unwrap_or(host);
    if host.is_empty() {
        None
    } else {
        Some(host.to_lowercase())
    }
}

fn norm_dir(dir: &str, ctx: &Ctx) -> String {
    if dir.is_empty() {
        normalize_path(&ctx.cwd, ctx)
    } else {
        normalize_path(dir, ctx)
    }
}

fn is_absolute(s: &str) -> bool {
    s.starts_with('/')
        || (s.len() >= 2 && s.as_bytes()[1] == b':' && s.as_bytes()[0].is_ascii_alphabetic())
}

/// Resolves `p` to a normalized absolute path (see module docs). Returns `p` cleaned up
/// as best it can if it cannot be resolved.
pub fn normalize_path(p: &str, ctx: &Ctx) -> String {
    let mut s = p.trim().trim_matches('"').trim_matches('\'').to_string();
    if s.is_empty() {
        return s;
    }
    // ~ expansion.
    if s == "~" {
        s = ctx.home.clone();
    } else if let Some(rest) = s.strip_prefix("~/").or_else(|| s.strip_prefix("~\\")) {
        let home = ctx.home.replace('\\', "/");
        s = format!("{}/{}", home.trim_end_matches('/'), rest);
    }
    s = s.replace('\\', "/");
    if !is_absolute(&s) {
        let cwd = ctx.cwd.replace('\\', "/");
        if !cwd.is_empty() {
            s = format!("{}/{}", cwd.trim_end_matches('/'), s);
        }
    }
    s = collapse(&s);
    // Lowercase the drive letter.
    if s.len() >= 2 && s.as_bytes()[1] == b':' && s.as_bytes()[0].is_ascii_alphabetic() {
        let mut chars: Vec<char> = s.chars().collect();
        chars[0] = chars[0].to_ascii_lowercase();
        s = chars.into_iter().collect();
    }
    if ctx.windows {
        s = s.to_lowercase();
    }
    s
}

fn collapse(s: &str) -> String {
    let has_root = s.starts_with('/');
    let mut drive = String::new();
    let mut comps: Vec<&str> = Vec::new();
    for (i, part) in s.split('/').enumerate() {
        if i == 0 && part.len() == 2 && part.ends_with(':') && part.as_bytes()[0].is_ascii_alphabetic() {
            drive = part.to_string();
            continue;
        }
        match part {
            "" | "." => {}
            ".." => {
                if comps.last().map_or(false, |c| *c != "..") {
                    comps.pop();
                } else if !has_root && drive.is_empty() {
                    comps.push("..");
                }
            }
            p => comps.push(p),
        }
    }
    let body = comps.join("/");
    if !drive.is_empty() {
        format!("{drive}/{body}")
    } else if has_root {
        format!("/{body}")
    } else {
        body
    }
}

fn components(s: &str) -> Vec<&str> {
    s.split('/').filter(|c| !c.is_empty() && *c != ".").collect()
}

/// True if normalized `path` is `dir` or inside it (component-wise, not string prefix).
pub fn is_within(path: &str, dir: &str) -> bool {
    if dir.is_empty() {
        return false;
    }
    let pc = components(path);
    let dc = components(dir);
    if dc.is_empty() {
        // dir is root "/"
        return path.starts_with('/');
    }
    if pc.len() < dc.len() {
        return false;
    }
    pc.iter().zip(dc.iter()).all(|(a, b)| a == b)
}

/// Shows `path` relative to `ctx.project_dir` when inside it, else as given.
pub fn display_path(path: &str, ctx: &Ctx) -> String {
    let dir = norm_dir(&ctx.project_dir, ctx);
    if !dir.is_empty() && is_within(path, &dir) {
        let pc = components(path);
        let dc = components(&dir);
        let rel: Vec<&str> = pc.into_iter().skip(dc.len()).collect();
        if rel.is_empty() {
            ".".to_string()
        } else {
            rel.join("/")
        }
    } else {
        path.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ctx() -> Ctx {
        Ctx {
            cwd: "C:/Users/Me/proj".into(),
            home: "C:/Users/Me".into(),
            project_dir: "C:/Users/Me/proj".into(),
            windows: true,
            ..Default::default()
        }
    }

    #[test]
    fn windows_normalization() {
        let c = ctx();
        assert_eq!(normalize_path(r"C:\Users\Me\proj\.env", &c), "c:/users/me/proj/.env");
        assert_eq!(normalize_path(".env", &c), "c:/users/me/proj/.env");
        assert_eq!(normalize_path("~/.ssh/id_rsa", &c), "c:/users/me/.ssh/id_rsa");
        assert_eq!(normalize_path("src/../a/./b.rs", &c), "c:/users/me/proj/a/b.rs");
    }

    #[test]
    fn within_and_display() {
        let c = ctx();
        assert!(is_within("c:/users/me/proj/src/x.rs", "c:/users/me/proj"));
        assert!(!is_within("c:/users/me/other", "c:/users/me/proj"));
        assert_eq!(display_path("c:/users/me/proj/src/x.rs", &c), "src/x.rs");
    }

    #[test]
    fn builds_actions() {
        let c = ctx();
        let a = from_tool_call("Read", &json!({"file_path": ".env"}), &c);
        assert_eq!(a.kind, ActionKind::Read);
        assert_eq!(a.reads, vec!["c:/users/me/proj/.env"]);

        let a = from_tool_call("WebFetch", &json!({"url": "https://pastebin.com/raw/x", "prompt": "hi"}), &c);
        assert_eq!(a.hosts, vec!["pastebin.com"]);
        assert!(a.egress_text.iter().any(|t| t.contains("pastebin")));

        let a = from_tool_call("Bash", &json!({"command": "rm -rf build"}), &c);
        assert_eq!(a.kind, ActionKind::Shell);
        assert!(a.shell.as_ref().unwrap().destructive);

        let a = from_tool_call("mcp__github__create_issue", &json!({"title": "x"}), &c);
        assert_eq!(a.kind, ActionKind::Mcp);
        assert_eq!(a.mcp_server.as_deref(), Some("github"));
    }
}
