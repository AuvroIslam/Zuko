//! Risk scoring: an impact vector with plain-English factors, a 0–100 score and a tier.
//!
//! Factors (each adds weight and one sentence; the score is the clamped sum, with the
//! highest single factor setting a floor so one severe trait is never diluted):
//! * irreversible / destructive operations (shell `destructive`, deletes, `Write` over an
//!   existing file outside the project) — up to 60
//! * blast radius: writes/deletes outside `ctx.project_dir`, in system dirs, in home
//!   dotfiles, recursive deletes of many files — up to 40
//! * network egress (shell `network`, WebFetch, MCP) — 20; unknown host (not in
//!   `network.allowed`) — +15; IP-literal host — +10
//! * data sensitivity: reads of `filesystem.sensitive` paths — 25; secret findings in
//!   the action's text — 30
//! * privilege escalation — 45
//! * supply chain: package installs — 15; pipe-to-shell — 60
//! * obfuscation — 50; unparseable command — 30 (UNKNOWN is not SAFE)
//! * killing processes — 20
//! * read-only actions inside the project (Read, Glob, Grep, LS, `git status`, `ls`,
//!   `cat` of non-sensitive files) score 0–10; `commands.allowed` matches cap the score
//!   at 10 unless a destructive/privilege/obfuscation factor is present.
//!
//! Tiers: Low < 25 ≤ Medium < 50 ≤ High < 80 ≤ Critical.
//!
//! The headline leads with the consequence, in capitals for the verb:
//! "DELETES build/ and everything inside it", "SENDS data to webhook.site (unknown host)",
//! "READS .env (secret file)", "RUNS a script downloaded from the internet",
//! "INSTALLS 3 npm packages", "EDITS src/main.rs". Read-only low-risk actions get a calm
//! headline ("Reads src/lib.rs").

use crate::action::Action;
use crate::policy::Policy;
use crate::Ctx;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Tier {
    #[default]
    Low,
    Medium,
    High,
    Critical,
}

impl Tier {
    pub fn from_score(score: u8) -> Tier {
        match score {
            0..=24 => Tier::Low,
            25..=49 => Tier::Medium,
            50..=79 => Tier::High,
            _ => Tier::Critical,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Tier::Low => "low",
            Tier::Medium => "medium",
            Tier::High => "high",
            Tier::Critical => "critical",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Factor {
    /// `destructive`, `outside_project`, `system_path`, `egress`, `unknown_host`,
    /// `ip_host`, `sensitive_read`, `secret_in_args`, `privilege`, `install`,
    /// `pipe_to_shell`, `obfuscated`, `unparseable`, `kill`, `allowed_command`.
    pub id: String,
    pub weight: u8,
    /// One plain-English sentence.
    pub text: String,
}

/// HAIEC-style impact vector, shown as chips in the UI.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImpactVector {
    /// `reversible`, `partial`, `irreversible`
    pub reversibility: String,
    /// `none`, `project`, `user`, `system`
    pub blast_radius: String,
    /// `none`, `known_host`, `unknown_host`
    pub egress: String,
    /// `public`, `internal`, `secret`
    pub sensitivity: String,
    pub privilege: bool,
    pub obfuscated: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RiskReport {
    pub score: u8,
    pub tier: Tier,
    pub headline: String,
    pub factors: Vec<Factor>,
    pub vector: ImpactVector,
}

/// Scores `action`. `secret_hits` is the number of detector findings in the action's
/// outbound text (computed by the caller, which owns the detector).
pub fn assess(action: &Action, policy: &Policy, ctx: &Ctx, secret_hits: usize) -> RiskReport {
    let _ = (action, policy, ctx, secret_hits);
    todo!()
}
