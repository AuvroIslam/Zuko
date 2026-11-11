// The policy file: %APPDATA%\Zuko\policy.json (pretty JSON, every field optional).
//
// The relay reads the same file for its stateless fallback, so the format is the
// engine's own `Policy` serialization and nothing else.

use std::path::{Path, PathBuf};

use zuko_core::policy::Policy;

pub fn path() -> PathBuf {
    crate::settings::config_dir().join("policy.json")
}

/// The saved policy, or the defaults when the file is missing. A file that does not
/// parse is kept aside as `policy.json.invalid-<ts>` and the defaults are used, so a
/// bad hand edit never leaves the user unprotected.
pub fn load() -> Policy {
    load_from(&path())
}

pub fn save(policy: &Policy) -> std::io::Result<()> {
    save_to(&path(), policy)
}

fn load_from(p: &Path) -> Policy {
    let Ok(text) = std::fs::read_to_string(p) else {
        return Policy::default();
    };
    match Policy::from_json(&text) {
        Ok(policy) => policy,
        Err(err) => {
            crate::log::line(format!("policy.json is invalid ({err}); using defaults"));
            let mut name = p.as_os_str().to_owned();
            name.push(format!(".invalid-{}", crate::engine::now()));
            let _ = std::fs::rename(p, PathBuf::from(name));
            Policy::default()
        }
    }
}

/// Atomic: the relay (a separate process) must never read half a policy.
fn save_to(p: &Path, policy: &Policy) -> std::io::Result<()> {
    crate::files::write_atomic(p, policy.to_json_pretty().as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_missing_and_invalid() {
        let dir = std::env::temp_dir().join(format!("zuko-ps-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let file = dir.join("policy.json");

        // Missing: the defaults.
        assert_eq!(load_from(&file), Policy::default());

        let mut p = Policy::default();
        p.network.blocked.push("evil.example".into());
        p.privacy.detector.custom_terms.push("Project Falcon".into());
        save_to(&file, &p).unwrap();
        assert_eq!(load_from(&file), p);
        // Hand-edited partial files are fine: every field is optional.
        std::fs::write(&file, r#"{ "mode": "monitor" }"#).unwrap();
        assert_eq!(load_from(&file).mode, zuko_core::policy::Mode::Monitor);

        // Broken: defaults, and the file is kept aside rather than overwritten.
        std::fs::write(&file, "{ not json").unwrap();
        assert_eq!(load_from(&file), Policy::default());
        assert!(!file.exists());
        let aside: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().contains(".invalid-"))
            .collect();
        assert_eq!(aside.len(), 1);
        assert_eq!(std::fs::read_to_string(aside[0].path()).unwrap(), "{ not json");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
