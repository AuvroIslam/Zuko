// Preferences, stored as plain JSON in settings.json under config_dir() below.
// No secret ever lands here — API keys live in the OS keychain (see secrets.rs).

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
    pub sound_enabled: bool,
    pub sound_volume: f64,
    pub auto_close_interval: f64,
    pub absence_interval: f64,
    /// "primary" = the main display, "cursor" = whichever display the mouse is on.
    pub screen: String,
    pub autostart: bool,
    pub hooks_installed: bool,
    /// Claude model used by the chat. Changeable in the settings window.
    /// Defaulted explicitly so a settings.json written by an older build still loads.
    #[serde(default = "default_model")]
    pub model: String,
}

fn default_model() -> String {
    crate::claude::DEFAULT_MODEL.to_string()
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            sound_enabled: true,
            sound_volume: 0.12,
            auto_close_interval: 15.0,
            absence_interval: 180.0,
            screen: "primary".into(),
            autostart: false,
            hooks_installed: false,
            model: default_model(),
        }
    }
}

/// Where settings.json and policy.json live: %APPDATA%\Zuko (platform default), or
/// `ZUKO_CONFIG_DIR` when set. The override exists for development and tests, which
/// must never touch the user's real Zuko data. The relay always reads the platform
/// default, so its stateless fallback ignores an overridden policy.
pub fn config_dir() -> PathBuf {
    #[cfg(test)]
    return test_root().join("config");
    #[cfg(not(test))]
    dir_override("ZUKO_CONFIG_DIR").unwrap_or_else(crate::platform::config_dir)
}

/// Where the vault, the audit log, the inbox and the relay live: %LOCALAPPDATA%\Zuko
/// (platform default), or `ZUKO_DATA_DIR` when set (development and tests).
pub fn local_dir() -> PathBuf {
    #[cfg(test)]
    return test_root().join("local");
    #[cfg(not(test))]
    dir_override("ZUKO_DATA_DIR").unwrap_or_else(crate::platform::local_dir)
}

/// An absolute directory from `var`; relative or empty values are ignored so a
/// stray variable can never scatter Zuko's data into the current directory.
#[cfg(not(test))]
fn dir_override(var: &str) -> Option<PathBuf> {
    let value = std::env::var_os(var)?;
    let path = PathBuf::from(value);
    path.is_absolute().then_some(path)
}

/// Unit tests never see the real data directories, whatever the environment says:
/// every path above resolves into a per-process temp folder instead.
#[cfg(test)]
pub fn test_root() -> PathBuf {
    std::env::temp_dir().join(format!("zuko-test-data-{}", std::process::id()))
}

pub fn hook_exe_path() -> PathBuf {
    local_dir().join("bin").join(crate::platform::HOOK_EXE)
}

fn settings_path() -> PathBuf {
    config_dir().join("settings.json")
}

pub fn load() -> Settings {
    match std::fs::read(settings_path()) {
        Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_default(),
        Err(_) => Settings::default(),
    }
}

pub fn save(settings: &Settings) -> std::io::Result<()> {
    let dir = config_dir();
    crate::platform::ensure_private_dir(&dir)?;
    let json = serde_json::to_vec_pretty(settings)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    std::fs::write(settings_path(), json)
}
