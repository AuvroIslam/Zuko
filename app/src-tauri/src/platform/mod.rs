// Everything that differs between operating systems, behind one set of names.
//
// The rest of the app calls `platform::…` and never touches Win32 or a Linux
// API directly. Each OS file exposes the same functions; the compiler picks one.

use std::path::PathBuf;

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use self::windows::*;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub use self::linux::*;

/// Wall-clock time in the user's time zone, for log lines and backup names.
pub struct LocalTime {
    pub year: u32,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
}

/// The directory named by an override variable — `ZUKO_CONFIG_DIR` (preferences
/// and policy) or `ZUKO_DATA_DIR` (relay, vault, audit log, inbox, zuko.log) —
/// when it is set to an absolute path. Development builds, end-to-end runs and
/// the relay's fallback use these to stay away from the real profile; the relay
/// honours the same variables, so both sides always agree.
///
/// Unit tests never get the real directories, whatever the environment says:
/// everything lands in a per-process folder under the system temp dir.
pub fn dir_override(var: &str) -> Option<PathBuf> {
    #[cfg(test)]
    {
        let leaf = if var == "ZUKO_CONFIG_DIR" { "config" } else { "data" };
        Some(std::env::temp_dir().join(format!("zuko-test-{}", std::process::id())).join(leaf))
    }
    #[cfg(not(test))]
    {
        std::env::var_os(var).map(PathBuf::from).filter(|p| p.is_absolute())
    }
}

/// The user's home directory, where `.claude/settings.json` lives.
pub fn home_dir() -> PathBuf {
    std::env::var_os(HOME_VAR)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}
