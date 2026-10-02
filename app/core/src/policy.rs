//! The user's policy: what the agent may and may not do, and how Zuko masks.
//!
//! Stored as JSON (`%APPDATA%\Zuko\policy.json`, optional `<project>/.zuko/policy.json`
//! merged on top). Every field has a default, so a partial file is valid.
//!
//! Matching rules:
//! * **Paths** ([`glob_match`]): gitignore-style globs over normalized paths. `**` spans
//!   directories, `*` and `?` stay within one component, `~/` expands to `ctx.home`, a
//!   pattern without `/` matches a file name at any depth (`.env` ≡ `**/.env`), a pattern
//!   ending in `/` or `/**` matches everything inside. Case-insensitive on Windows.
//! * **Domains** ([`domain_match`]): `example.com` matches the host and every subdomain;
//!   `*.example.com` matches subdomains only; an IP literal matches exactly.
//! * **Commands** ([`command_match`]): `*` wildcard over the whitespace-normalized
//!   command; a pattern matches if it matches any simple command segment
//!   (`git push --force*` matches `cd x && git push --force origin`).
//! * **Tools** ([`tool_match`]): exact name or `*` wildcard (`mcp__*`, `mcp__github__*`).

use crate::action::Action;
use crate::detect::DetectorConfig;
use crate::risk::Tier;
use crate::Ctx;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// Decisions are enforced.
    #[default]
    Enforce,
    /// Decisions are computed and logged, nothing is blocked or auto-approved.
    Monitor,
}

/// Policy-level verdict for a rule hit.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RuleVerdict {
    Allow,
    Ask,
    Deny,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct NetworkPolicy {
    pub blocked: Vec<String>,
    /// Hosts considered known-good (never "unknown host" risk).
    pub allowed: Vec<String>,
    /// What to do with hosts in neither list.
    pub unknown: RuleVerdict,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct FsPolicy {
    pub blocked_read: Vec<String>,
    pub blocked_write: Vec<String>,
    /// Files whose contents are secret (reading them taints the session).
    pub sensitive: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct CommandPolicy {
    pub blocked: Vec<String>,
    pub ask: Vec<String>,
    /// Commands always considered safe (auto-allow candidates), e.g. `npm test`.
    pub allowed: Vec<String>,
}

impl Default for CommandPolicy {
    fn default() -> Self {
        Self {
            blocked: strs(&[
                "rm -rf /",
                "rm -rf /*",
                "rm -fr /",
                "rm -rf ~",
                "rm -rf ~/*",
                "rm -rf --no-preserve-root*",
                "format *",
                "mkfs*",
                "diskpart*",
                "dd if=* of=/dev/sd*",
                ":(){ :|:& };:",
            ]),
            ask: strs(&[
                "npm publish*",
                "git push*",
                "pip install*",
                "npx *",
                "sudo *",
            ]),
            allowed: strs(&[
                "git status*",
                "git diff*",
                "git log*",
                "git branch",
                "git fetch*",
                "npm test*",
                "npm run *",
                "npm ci",
                "cargo test*",
                "cargo build*",
                "cargo check*",
                "cargo fmt*",
                "cargo clippy*",
                "ls*",
                "pwd",
                "echo *",
            ]),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ToolPolicy {
    pub blocked: Vec<String>,
    pub ask: Vec<String>,
    /// MCP tools the user has approved; others trigger `UNKNOWN_TOOL`.
    pub allowed_mcp: Vec<String>,
}

impl Default for ToolPolicy {
    fn default() -> Self {
        Self {
            blocked: Vec::new(),
            ask: strs(&["mcp__*"]),
            allowed_mcp: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Approvals {
    /// PreToolUse answers `allow` for low-risk actions so no prompt appears.
    pub auto_allow_low_risk: bool,
    /// From this tier, PreToolUse answers `ask` even if Claude Code would allow, and
    /// the island requires hold-to-approve.
    pub hold_to_approve_from: Tier,
    /// From this tier, the action is denied.
    pub block_from: Tier,
    /// How long Allow must be held, in milliseconds, for `High`; `Critical` doubles it.
    pub hold_ms: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct PrivacyPolicy {
    pub detector: DetectorConfig,
    /// Mask prompts in hooks-only mode by blocking and offering a masked copy.
    pub block_secret_prompts_without_gateway: bool,
    /// Mask Bash/PowerShell output in hooks-only mode (PostToolUse updatedToolOutput).
    pub mask_tool_output: bool,
    /// Per-secret egress allowlist: vault key → hosts it may be sent to
    /// (e.g. `API_KEY_1` → `api.openai.com`). Used by `SECRET_EGRESS`.
    pub secret_hosts: std::collections::BTreeMap<String, Vec<String>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Policy {
    pub version: u32,
    pub mode: Mode,
    pub network: NetworkPolicy,
    pub filesystem: FsPolicy,
    pub commands: CommandPolicy,
    pub tools: ToolPolicy,
    pub approvals: Approvals,
    pub privacy: PrivacyPolicy,
}

fn strs(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

impl Default for NetworkPolicy {
    fn default() -> Self {
        Self {
            blocked: strs(&[
                "pastebin.com",
                "webhook.site",
                "requestbin.net",
                "*.ngrok.io",
                "*.ngrok-free.app",
                "transfer.sh",
                "0x0.st",
                "*.trycloudflare.com",
                "ngrok.com",
                "termbin.com",
                "file.io",
                "gofile.io",
                "dpaste.com",
                "ix.io",
                "paste.ee",
                "glot.io",
            ]),
            allowed: strs(&[
                "github.com",
                "api.github.com",
                "githubusercontent.com",
                "raw.githubusercontent.com",
                "objects.githubusercontent.com",
                "registry.npmjs.org",
                "npmjs.com",
                "pypi.org",
                "files.pythonhosted.org",
                "crates.io",
                "static.crates.io",
                "docs.rs",
                "rust-lang.org",
                "nodejs.org",
                "developer.mozilla.org",
                "stackoverflow.com",
                "python.org",
                "docs.python.org",
                "microsoft.com",
                "go.dev",
                "golang.org",
                "anthropic.com",
                "claude.com",
                "api.anthropic.com",
            ]),
            unknown: RuleVerdict::Allow,
        }
    }
}

impl Default for FsPolicy {
    fn default() -> Self {
        Self {
            blocked_read: strs(&[
                "~/.ssh/**",
                "~/.aws/credentials",
                "~/.aws/**",
                "~/.gnupg/**",
                "~/.config/gcloud/**",
                "~/.docker/config.json",
                "~/.kube/config",
                "~/AppData/**/Google/Chrome/User Data/**",
                "~/AppData/**/Mozilla/Firefox/Profiles/**",
                "~/Library/Keychains/**",
            ]),
            blocked_write: strs(&[
                "C:/Windows/**",
                "C:/Program Files/**",
                "C:/Program Files (x86)/**",
                "/etc/**",
                "/usr/**",
                "/bin/**",
                "/sbin/**",
                "/boot/**",
            ]),
            sensitive: strs(&[
                "**/.env",
                "**/.env.*",
                "*.pem",
                "*.key",
                "id_rsa*",
                "id_ed25519*",
                "id_ecdsa*",
                "credentials*",
                "*.pfx",
                "*.p12",
                ".npmrc",
                ".pypirc",
                ".netrc",
                "secrets.*",
                "*.kdbx",
                "*.ppk",
                "service-account*.json",
            ]),
        }
    }
}

impl Default for Approvals {
    fn default() -> Self {
        Self {
            auto_allow_low_risk: true,
            hold_to_approve_from: Tier::High,
            block_from: Tier::Critical,
            hold_ms: 1500,
        }
    }
}

impl Default for PrivacyPolicy {
    fn default() -> Self {
        Self {
            detector: DetectorConfig::default(),
            block_secret_prompts_without_gateway: true,
            mask_tool_output: true,
            secret_hosts: Default::default(),
        }
    }
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            version: 1,
            mode: Mode::Enforce,
            network: NetworkPolicy::default(),
            filesystem: FsPolicy::default(),
            commands: CommandPolicy::default(),
            tools: ToolPolicy::default(),
            approvals: Approvals::default(),
            privacy: PrivacyPolicy::default(),
        }
    }
}

fn union(a: &[String], b: &[String]) -> Vec<String> {
    let mut out = a.to_vec();
    for x in b {
        if !out.contains(x) {
            out.push(x.clone());
        }
    }
    out
}

fn intersect(a: &[String], b: &[String]) -> Vec<String> {
    a.iter().filter(|x| b.contains(x)).cloned().collect()
}

impl Policy {
    pub fn from_json(s: &str) -> Result<Self, String> {
        serde_json::from_str(s).map_err(|e| e.to_string())
    }

    pub fn to_json_pretty(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_else(|_| "{}".into())
    }

    /// `self` (global) with `project` layered on top: lists are unioned (blocked/ask/
    /// sensitive grow; a project cannot remove global blocks), scalars take the stricter
    /// value (Enforce beats Monitor, lower tiers for hold/block win, auto-allow is ANDed).
    pub fn merged_with(&self, project: &Policy) -> Policy {
        let det_a = &self.privacy.detector;
        let det_b = &project.privacy.detector;
        let detector = DetectorConfig {
            secrets: det_a.secrets || det_b.secrets,
            pii: det_a.pii || det_b.pii,
            emails: det_a.emails || det_b.emails,
            phones: det_a.phones || det_b.phones,
            cards: det_a.cards || det_b.cards,
            ibans: det_a.ibans || det_b.ibans,
            ips: det_a.ips || det_b.ips,
            national_ids: det_a.national_ids || det_b.national_ids,
            generic_entropy: det_a.generic_entropy || det_b.generic_entropy,
            min_entropy: det_a.min_entropy.min(det_b.min_entropy),
            custom_terms: union(&det_a.custom_terms, &det_b.custom_terms),
            // A stricter allowlist is a smaller one (fewer values exempt from masking).
            allowlist: intersect(&det_a.allowlist, &det_b.allowlist),
        };
        let mut secret_hosts = self.privacy.secret_hosts.clone();
        for (k, hosts) in &project.privacy.secret_hosts {
            secret_hosts
                .entry(k.clone())
                .and_modify(|cur| *cur = intersect(cur, hosts))
                .or_insert_with(|| hosts.clone());
        }
        Policy {
            version: self.version.max(project.version),
            mode: if self.mode == Mode::Enforce || project.mode == Mode::Enforce {
                Mode::Enforce
            } else {
                Mode::Monitor
            },
            network: NetworkPolicy {
                blocked: union(&self.network.blocked, &project.network.blocked),
                allowed: intersect(&self.network.allowed, &project.network.allowed),
                unknown: self.network.unknown.max(project.network.unknown),
            },
            filesystem: FsPolicy {
                blocked_read: union(&self.filesystem.blocked_read, &project.filesystem.blocked_read),
                blocked_write: union(&self.filesystem.blocked_write, &project.filesystem.blocked_write),
                sensitive: union(&self.filesystem.sensitive, &project.filesystem.sensitive),
            },
            commands: CommandPolicy {
                blocked: union(&self.commands.blocked, &project.commands.blocked),
                ask: union(&self.commands.ask, &project.commands.ask),
                allowed: intersect(&self.commands.allowed, &project.commands.allowed),
            },
            tools: ToolPolicy {
                blocked: union(&self.tools.blocked, &project.tools.blocked),
                ask: union(&self.tools.ask, &project.tools.ask),
                allowed_mcp: intersect(&self.tools.allowed_mcp, &project.tools.allowed_mcp),
            },
            approvals: Approvals {
                auto_allow_low_risk: self.approvals.auto_allow_low_risk && project.approvals.auto_allow_low_risk,
                hold_to_approve_from: self.approvals.hold_to_approve_from.min(project.approvals.hold_to_approve_from),
                block_from: self.approvals.block_from.min(project.approvals.block_from),
                hold_ms: self.approvals.hold_ms.max(project.approvals.hold_ms),
            },
            privacy: PrivacyPolicy {
                detector,
                block_secret_prompts_without_gateway: self.privacy.block_secret_prompts_without_gateway
                    || project.privacy.block_secret_prompts_without_gateway,
                mask_tool_output: self.privacy.mask_tool_output || project.privacy.mask_tool_output,
                secret_hosts,
            },
        }
    }

    /// SHA-256 (hex) of the canonical JSON (keys sorted), recorded in audit receipts.
    pub fn digest(&self) -> String {
        let v = serde_json::to_value(self).unwrap_or(serde_json::Value::Null);
        crate::audit::sha256_hex(crate::audit::canonical_json(&v).as_bytes())
    }
}

/// One rule that matched an action.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PolicyHit {
    /// e.g. `network.blocked:pastebin.com`, `filesystem.blockedRead:~/.ssh/**`,
    /// `commands.blocked:git push --force*`, `tools.ask:mcp__*`.
    pub rule: String,
    pub verdict: RuleVerdict,
    /// Plain English: "pastebin.com is on your blocked list".
    pub reason: String,
}

/// Every rule in `policy` that matches `action`. `Allow` hits come from
/// `commands.allowed` and `network.allowed` and only lower risk; they never override
/// a Deny or Ask hit.
pub fn evaluate(policy: &Policy, action: &Action, ctx: &Ctx) -> Vec<PolicyHit> {
    use crate::action::ActionKind;
    let mut hits = Vec::new();

    // Filesystem reads.
    for p in &action.reads {
        for pat in &policy.filesystem.blocked_read {
            if glob_match(pat, p, ctx) {
                hits.push(PolicyHit {
                    rule: format!("filesystem.blockedRead:{pat}"),
                    verdict: RuleVerdict::Deny,
                    reason: format!("reading {} is blocked by your policy ({pat})", crate::action::display_path(p, ctx)),
                });
            }
        }
    }
    // Filesystem writes and deletes.
    for p in action.writes.iter().chain(action.deletes.iter()) {
        for pat in &policy.filesystem.blocked_write {
            if glob_match(pat, p, ctx) {
                hits.push(PolicyHit {
                    rule: format!("filesystem.blockedWrite:{pat}"),
                    verdict: RuleVerdict::Deny,
                    reason: format!("writing to {} is blocked by your policy ({pat})", crate::action::display_path(p, ctx)),
                });
            }
        }
    }

    // Network hosts.
    for host in &action.hosts {
        if host == "websearch" {
            continue;
        }
        let mut blocked = false;
        for pat in &policy.network.blocked {
            if domain_match(pat, host) {
                blocked = true;
                hits.push(PolicyHit {
                    rule: format!("network.blocked:{pat}"),
                    verdict: RuleVerdict::Deny,
                    reason: format!("{host} is on your blocked list"),
                });
            }
        }
        if blocked {
            continue;
        }
        let allowed = policy.network.allowed.iter().any(|pat| domain_match(pat, host));
        if allowed {
            hits.push(PolicyHit {
                rule: format!("network.allowed:{host}"),
                verdict: RuleVerdict::Allow,
                reason: format!("{host} is on your allowed list"),
            });
        } else if policy.network.unknown != RuleVerdict::Allow {
            hits.push(PolicyHit {
                rule: format!("network.unknown:{host}"),
                verdict: policy.network.unknown,
                reason: format!("{host} is not on your allowed list"),
            });
        }
    }

    // Commands.
    if let Some(cmd) = &action.command {
        for pat in &policy.commands.blocked {
            if command_match(pat, cmd) {
                hits.push(PolicyHit {
                    rule: format!("commands.blocked:{pat}"),
                    verdict: RuleVerdict::Deny,
                    reason: format!("this command matches a blocked pattern ({pat})"),
                });
            }
        }
        for pat in &policy.commands.ask {
            if command_match(pat, cmd) {
                hits.push(PolicyHit {
                    rule: format!("commands.ask:{pat}"),
                    verdict: RuleVerdict::Ask,
                    reason: format!("this command needs your approval ({pat})"),
                });
            }
        }
        for pat in &policy.commands.allowed {
            if command_match(pat, cmd) {
                hits.push(PolicyHit {
                    rule: format!("commands.allowed:{pat}"),
                    verdict: RuleVerdict::Allow,
                    reason: format!("this command is on your allowed list ({pat})"),
                });
            }
        }
    }

    // Tools (by name). MCP and any tool.
    for pat in &policy.tools.blocked {
        if tool_match(pat, &action.tool) {
            hits.push(PolicyHit {
                rule: format!("tools.blocked:{pat}"),
                verdict: RuleVerdict::Deny,
                reason: format!("the tool {} is blocked ({pat})", action.tool),
            });
        }
    }
    if action.kind == ActionKind::Mcp {
        for pat in &policy.tools.ask {
            if tool_match(pat, &action.tool) {
                hits.push(PolicyHit {
                    rule: format!("tools.ask:{pat}"),
                    verdict: RuleVerdict::Ask,
                    reason: format!("the MCP tool {} needs your approval ({pat})", action.tool),
                });
            }
        }
    } else {
        for pat in &policy.tools.ask {
            if tool_match(pat, &action.tool) {
                hits.push(PolicyHit {
                    rule: format!("tools.ask:{pat}"),
                    verdict: RuleVerdict::Ask,
                    reason: format!("the tool {} needs your approval ({pat})", action.tool),
                });
            }
        }
    }

    hits
}

/// Expands `~`, normalizes slashes, lowercases on Windows.
fn norm_pattern(pattern: &str, ctx: &Ctx) -> String {
    let mut p = pattern.trim().to_string();
    if p == "~" {
        p = ctx.home.clone();
    } else if let Some(rest) = p.strip_prefix("~/").or_else(|| p.strip_prefix("~\\")) {
        let home = ctx.home.replace('\\', "/");
        p = format!("{}/{}", home.trim_end_matches('/'), rest);
    }
    p = p.replace('\\', "/");
    if ctx.windows {
        p = p.to_lowercase();
    }
    p
}

pub fn glob_match(pattern: &str, path: &str, ctx: &Ctx) -> bool {
    let mut pat = norm_pattern(pattern, ctx);
    let path = if ctx.windows { path.to_lowercase() } else { path.to_string() };

    // A trailing "/" means "everything inside"; normalize it to "/**".
    if pat.ends_with('/') {
        pat.push_str("**");
    }
    // A relative pattern (no drive, not rooted) matches at any depth, gitignore-style:
    // `.env`, `src/**/*.rs` and `*.pem` all match anywhere under the tree.
    let absolute = pat.starts_with('/')
        || (pat.len() >= 2 && pat.as_bytes()[1] == b':' && pat.as_bytes()[0].is_ascii_alphabetic());
    if !absolute && !pat.starts_with("**/") {
        pat = format!("**/{pat}");
    }
    let pat_segs: Vec<&str> = pat.split('/').filter(|s| !s.is_empty()).collect();
    let path_segs: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    match_path(&pat_segs, &path_segs)
}

/// Matches path segments; `**` spans zero or more segments.
fn match_path(pat: &[&str], s: &[&str]) -> bool {
    if pat.is_empty() {
        return s.is_empty();
    }
    if pat[0] == "**" {
        // Collapse consecutive "**".
        let mut rest = &pat[1..];
        while rest.first() == Some(&"**") {
            rest = &rest[1..];
        }
        if rest.is_empty() {
            return true;
        }
        for i in 0..=s.len() {
            if match_path(rest, &s[i..]) {
                return true;
            }
        }
        return false;
    }
    if s.is_empty() {
        return false;
    }
    if !simple_wild(pat[0].as_bytes(), s[0].as_bytes()) {
        return false;
    }
    match_path(&pat[1..], &s[1..])
}

/// Classic `*` (any run) / `?` (one char) matcher within a single string, no regex.
fn simple_wild(pat: &[u8], s: &[u8]) -> bool {
    let (mut p, mut i) = (0usize, 0usize);
    let (mut star_p, mut star_i): (Option<usize>, usize) = (None, 0);
    while i < s.len() {
        if p < pat.len() && (pat[p] == b'?' || pat[p] == s[i]) {
            p += 1;
            i += 1;
        } else if p < pat.len() && pat[p] == b'*' {
            star_p = Some(p);
            star_i = i;
            p += 1;
        } else if let Some(sp) = star_p {
            p = sp + 1;
            star_i += 1;
            i = star_i;
        } else {
            return false;
        }
    }
    while p < pat.len() && pat[p] == b'*' {
        p += 1;
    }
    p == pat.len()
}

pub fn domain_match(pattern: &str, host: &str) -> bool {
    let pat = pattern.trim().to_lowercase();
    let host = host.trim().trim_end_matches('.').to_lowercase();
    if pat.is_empty() || host.is_empty() {
        return false;
    }
    if let Some(base) = pat.strip_prefix("*.") {
        // Subdomains only.
        return host != base && host.ends_with(&format!(".{base}"));
    }
    host == pat || host.ends_with(&format!(".{pat}"))
}

/// Simple `*`/`?` glob match anchored over the whole string (`*` spans any character).
fn wildcard_match(pattern: &str, s: &str) -> bool {
    simple_wild(pattern.as_bytes(), s.as_bytes())
}

fn ws_normalize(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn command_segments(command: &str) -> Vec<String> {
    let replaced = command
        .replace("&&", "\n")
        .replace("||", "\n")
        .replace('|', "\n")
        .replace(';', "\n")
        .replace('&', "\n");
    replaced
        .split('\n')
        .map(|s| ws_normalize(s))
        .filter(|s| !s.is_empty())
        .collect()
}

pub fn command_match(pattern: &str, command: &str) -> bool {
    let pat = ws_normalize(pattern).to_lowercase();
    if pat.is_empty() {
        return false;
    }
    let whole = ws_normalize(command).to_lowercase();
    if wildcard_match(&pat, &whole) {
        return true;
    }
    command_segments(&whole).iter().any(|seg| wildcard_match(&pat, seg))
}

pub fn tool_match(pattern: &str, tool: &str) -> bool {
    let pat = pattern.trim();
    if pat == tool {
        return true;
    }
    if pat.contains('*') {
        return wildcard_match(pat, tool);
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::action::{Action, ActionKind};

    fn ctx() -> Ctx {
        Ctx {
            cwd: "c:/users/me/proj".into(),
            home: "c:/users/me".into(),
            project_dir: "c:/users/me/proj".into(),
            windows: true,
            ..Default::default()
        }
    }

    #[test]
    fn globs() {
        let c = ctx();
        assert!(glob_match(".env", "c:/users/me/proj/.env", &c));
        assert!(glob_match("**/.env", "c:/users/me/proj/sub/.env", &c));
        assert!(glob_match("~/.ssh/**", "c:/users/me/.ssh/id_rsa", &c));
        assert!(glob_match("*.pem", "c:/x/key.pem", &c));
        assert!(!glob_match("*.pem", "c:/x/key.pub", &c));
        assert!(glob_match("D:/Personal/**", "d:/personal/tax.pdf", &c));
    }

    #[test]
    fn domains() {
        assert!(domain_match("example.com", "example.com"));
        assert!(domain_match("example.com", "api.example.com"));
        assert!(!domain_match("*.example.com", "example.com"));
        assert!(domain_match("*.example.com", "api.example.com"));
        assert!(domain_match("pastebin.com", "pastebin.com"));
        assert!(!domain_match("pastebin.com", "notpastebin.com"));
    }

    #[test]
    fn commands_and_tools() {
        assert!(command_match("git push --force*", "cd x && git push --force origin"));
        assert!(command_match("npm publish*", "npm publish --tag beta"));
        assert!(!command_match("git push --force*", "git push origin main"));
        assert!(tool_match("mcp__*", "mcp__github__create_issue"));
        assert!(tool_match("Bash", "Bash"));
        assert!(!tool_match("mcp__github__*", "mcp__gitlab__x"));
    }

    #[test]
    fn json_round_trip_and_partial() {
        let p = Policy::default();
        let s = p.to_json_pretty();
        let back = Policy::from_json(&s).unwrap();
        assert_eq!(p, back);
        // Partial JSON accepted.
        let partial = Policy::from_json(r#"{"mode":"monitor"}"#).unwrap();
        assert_eq!(partial.mode, Mode::Monitor);
        assert!(!partial.network.blocked.is_empty());
    }

    #[test]
    fn digest_is_stable_and_order_independent() {
        let p = Policy::default();
        assert_eq!(p.digest(), p.digest());
        assert_eq!(p.digest().len(), 64);
    }

    #[test]
    fn merge_is_stricter() {
        let g = Policy::default();
        let mut proj = Policy::default();
        proj.network.blocked = vec!["corp-internal.example".into()];
        proj.mode = Mode::Monitor;
        let m = g.merged_with(&proj);
        assert!(m.network.blocked.contains(&"pastebin.com".to_string()));
        assert!(m.network.blocked.contains(&"corp-internal.example".to_string()));
        assert_eq!(m.mode, Mode::Enforce);
    }

    #[test]
    fn evaluate_blocks_and_allows() {
        let c = ctx();
        let p = Policy::default();
        let mut a = Action::default();
        a.kind = ActionKind::Fetch;
        a.hosts = vec!["pastebin.com".into()];
        let hits = evaluate(&p, &a, &c);
        assert!(hits.iter().any(|h| h.verdict == RuleVerdict::Deny));

        let mut a2 = Action::default();
        a2.kind = ActionKind::Read;
        a2.reads = vec!["c:/users/me/.ssh/id_rsa".into()];
        let hits = evaluate(&p, &a2, &c);
        assert!(hits.iter().any(|h| h.rule.starts_with("filesystem.blockedRead")));
    }
}
