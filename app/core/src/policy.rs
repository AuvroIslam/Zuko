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

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct CommandPolicy {
    pub blocked: Vec<String>,
    pub ask: Vec<String>,
    /// Commands always considered safe (auto-allow candidates), e.g. `npm test`.
    pub allowed: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ToolPolicy {
    pub blocked: Vec<String>,
    pub ask: Vec<String>,
    /// MCP tools the user has approved; others trigger `UNKNOWN_TOOL`.
    pub allowed_mcp: Vec<String>,
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

impl Default for NetworkPolicy {
    fn default() -> Self {
        let _ = ();
        todo!("blocked: pastebin.com, webhook.site, requestbin.net, *.ngrok.io, *.ngrok-free.app, transfer.sh, 0x0.st, *.trycloudflare.com, ipinfo.io …; allowed: github.com, api.github.com, githubusercontent.com, registry.npmjs.org, npmjs.com, pypi.org, files.pythonhosted.org, crates.io, static.crates.io, docs.rs, rust-lang.org, nodejs.org, developer.mozilla.org, stackoverflow.com, python.org, microsoft.com, anthropic.com, claude.com …; unknown: Allow")
    }
}

impl Default for FsPolicy {
    fn default() -> Self {
        todo!("blocked_read: ~/.ssh/**, ~/.aws/credentials, ~/.gnupg/**, browser profile dirs; blocked_write: C:/Windows/**, /etc/**, /usr/**; sensitive: .env, .env.*, *.pem, *.key, id_rsa*, id_ed25519*, credentials*, *.pfx, *.p12, .npmrc, .pypirc, .netrc, secrets.*, *.kdbx")
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

impl Policy {
    pub fn from_json(s: &str) -> Result<Self, String> {
        let _ = s;
        todo!()
    }

    pub fn to_json_pretty(&self) -> String {
        todo!()
    }

    /// `self` (global) with `project` layered on top: lists are unioned (blocked/ask/
    /// sensitive grow; a project cannot remove global blocks), scalars take the stricter
    /// value (Enforce beats Monitor, lower tiers for hold/block win, auto-allow is ANDed).
    pub fn merged_with(&self, project: &Policy) -> Policy {
        let _ = project;
        todo!()
    }

    /// SHA-256 (hex) of the canonical JSON (keys sorted), recorded in audit receipts.
    pub fn digest(&self) -> String {
        todo!()
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
    let _ = (policy, action, ctx);
    todo!()
}

pub fn glob_match(pattern: &str, path: &str, ctx: &Ctx) -> bool {
    let _ = (pattern, path, ctx);
    todo!()
}

pub fn domain_match(pattern: &str, host: &str) -> bool {
    let _ = (pattern, host);
    todo!()
}

pub fn command_match(pattern: &str, command: &str) -> bool {
    let _ = (pattern, command);
    todo!()
}

pub fn tool_match(pattern: &str, tool: &str) -> bool {
    let _ = (pattern, tool);
    todo!()
}
