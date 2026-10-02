// The policy file: %APPDATA%\Zuko\policy.json (pretty JSON, every field optional).
//
// The relay reads the same file for its stateless fallback, so the format is the
// engine's own `Policy` serialization and nothing else.

use std::path::PathBuf;

use zuko_core::policy::Policy;

pub fn path() -> PathBuf {
    crate::platform::config_dir().join("policy.json")
}

/// The saved policy, or the defaults when the file is missing. A file that does
/// not parse is kept aside as `policy.json.invalid-<ts>` and the defaults are used,
/// so a bad hand edit never leaves the user unprotected.
pub fn load() -> Policy {
    let p = path();
    let Ok(text) = std::fs::read_to_string(&p) else {
        return Policy::default();
    };
    match Policy::from_json(&text) {
        Ok(policy) => policy,
        Err(err) => {
            crate::log::line(format!("policy.json is invalid ({err}); using defaults"));
            let aside = p.with_extension(format!("json.invalid-{}", crate::engine::now()));
            let _ = std::fs::rename(&p, aside);
            Policy::default()
        }
    }
}

pub fn save(policy: &Policy) -> std::io::Result<()> {
    let dir = crate::platform::config_dir();
    crate::platform::ensure_private_dir(&dir)?;
    let tmp = path().with_extension(format!("json.tmp-{}", std::process::id()));
    std::fs::write(&tmp, policy.to_json_pretty())?;
    std::fs::rename(tmp, path())
}
