//! Policy model: path globs on Windows paths, domains, command matching, merge, digest.

use zuko_core::action::{from_tool_call, Action, ActionKind};
use zuko_core::policy::{command_match, domain_match, evaluate, glob_match, tool_match, Mode, Policy, RuleVerdict};
use zuko_core::Ctx;

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
fn windows_path_globs() {
    let c = ctx();
    assert!(glob_match("**/.env", "c:/users/me/proj/.env", &c));
    assert!(glob_match(".env", "c:/users/me/proj/sub/dir/.env", &c));
    assert!(glob_match("~/.ssh/**", "c:/users/me/.ssh/known_hosts", &c));
    assert!(glob_match("~/.ssh/**", "c:/users/me/.ssh", &c), "dir itself matches /**");
    assert!(glob_match("C:/Windows/**", "c:/windows/system32/cmd.exe", &c));
    assert!(!glob_match("src/*.rs", "c:/users/me/proj/src/sub/x.rs", &c), "* stays in one component");
    assert!(glob_match("src/**/*.rs", "c:/users/me/proj/src/sub/x.rs", &c));
}

#[test]
fn domain_rules() {
    assert!(domain_match("example.com", "example.com"));
    assert!(domain_match("example.com", "api.v2.example.com"));
    assert!(!domain_match("example.com", "evilexample.com"));
    assert!(domain_match("*.ngrok.io", "abc.ngrok.io"));
    assert!(!domain_match("*.ngrok.io", "ngrok.io"));
}

#[test]
fn command_segment_matching() {
    assert!(command_match("git push --force*", "cd /tmp && git push --force origin main"));
    assert!(command_match("rm -rf /*", "rm -rf /"));
    assert!(!command_match("git push --force*", "git push origin main"));
    assert!(tool_match("mcp__*", "mcp__x__y"));
    assert!(!tool_match("mcp__github__*", "mcp__other__y"));
}

#[test]
fn evaluate_reports_rule_ids() {
    let c = ctx();
    let p = Policy::default();
    let mut a = Action::default();
    a.tool = "WebFetch".into();
    a.kind = ActionKind::Fetch;
    a.hosts = vec!["pastebin.com".into()];
    let hits = evaluate(&p, &a, &c);
    assert!(hits.iter().any(|h| h.rule.starts_with("network.blocked:") && h.verdict == RuleVerdict::Deny));

    let read = from_tool_call("Read", &serde_json::json!({"file_path": "~/.ssh/id_rsa"}), &c);
    let hits = evaluate(&p, &read, &c);
    assert!(hits.iter().any(|h| h.rule.starts_with("filesystem.blockedRead:")));
}

#[test]
fn json_round_trip_partial_and_digest() {
    let p = Policy::default();
    let s = p.to_json_pretty();
    assert_eq!(Policy::from_json(&s).unwrap(), p);

    // A partial policy still has defaults filled in.
    let partial = Policy::from_json(r#"{"version":1,"commands":{"blocked":["danger *"]}}"#).unwrap();
    assert!(partial.commands.blocked.contains(&"danger *".to_string()));
    assert!(!partial.network.blocked.is_empty());

    // Digest is stable and 64 hex chars.
    assert_eq!(p.digest(), Policy::from_json(&s).unwrap().digest());
    assert_eq!(p.digest().len(), 64);
}

#[test]
fn merge_prefers_stricter() {
    let global = Policy::default();
    let mut project = Policy::default();
    project.mode = Mode::Monitor; // weaker; global enforce wins
    project.network.blocked = vec!["internal.corp".into()];
    project.approvals.auto_allow_low_risk = false; // stricter wins (AND)
    let m = global.merged_with(&project);
    assert_eq!(m.mode, Mode::Enforce);
    assert!(m.network.blocked.contains(&"internal.corp".to_string()));
    assert!(m.network.blocked.contains(&"pastebin.com".to_string()));
    assert!(!m.approvals.auto_allow_low_risk);
}

#[test]
fn invalid_json_errors() {
    assert!(Policy::from_json("{ not json").is_err());
}
