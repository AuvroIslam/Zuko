// Preferences, stored as plain JSON in settings.json under config_dir() below.
// No secret ever lands here — API keys live in the OS keychain (see secrets.rs).

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::chat::Provider;

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
    /// Who answers the island chat: "anthropic" (the default), "openai" or "ollama".
    /// See chat.rs for how an unknown value is read.
    pub chat_provider: Provider,
    /// The Claude model for the chat (the Anthropic provider). It kept its original
    /// name, so a choice made with an older build survives.
    /// Defaulted explicitly so a settings.json written by an older build still loads.
    #[serde(default = "default_model")]
    pub model: String,
    /// The OpenAI model for the chat.
    #[serde(default = "default_openai_model")]
    pub openai_model: String,
    /// The Ollama model for the chat (independent of the local AI's scan model).
    #[serde(default = "default_ollama_model")]
    pub ollama_model: String,
    /// The browser bridge: Zuko registers the extension's native messaging host for this
    /// user at every launch (nativehost.rs). Off once the user unregisters it in Settings →
    /// Browser, so it is not quietly put back.
    #[serde(default = "default_true")]
    pub browser_bridge: bool,
}

fn default_true() -> bool {
    true
}

fn default_model() -> String {
    Provider::Anthropic.default_model().to_string()
}

fn default_openai_model() -> String {
    Provider::OpenAi.default_model().to_string()
}

fn default_ollama_model() -> String {
    Provider::Ollama.default_model().to_string()
}

impl Settings {
    /// The model the chat uses with `provider` (its default when the field is blank).
    pub fn chat_model(&self, provider: Provider) -> String {
        let model = match provider {
            Provider::Anthropic => &self.model,
            Provider::OpenAi => &self.openai_model,
            Provider::Ollama => &self.ollama_model,
        };
        match model.trim() {
            "" => provider.default_model().to_string(),
            m => m.to_string(),
        }
    }
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
            chat_provider: Provider::Anthropic,
            model: default_model(),
            openai_model: default_openai_model(),
            ollama_model: default_ollama_model(),
            browser_bridge: true,
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

/// True when either directory is redirected (development and tests). Secrets then live
/// under a separate keyring service (secrets.rs), so a development run can never read or
/// replace the vault key of the real installation.
pub fn dirs_overridden() -> bool {
    #[cfg(test)]
    return true;
    #[cfg(not(test))]
    return dir_override("ZUKO_CONFIG_DIR").is_some() || dir_override("ZUKO_DATA_DIR").is_some();
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

/// Where the browser extension's native host is installed (next to the relay).
pub fn native_host_exe_path() -> PathBuf {
    local_dir().join("bin").join(crate::platform::NATIVE_HOST_EXE)
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
    let json = serde_json::to_vec_pretty(settings)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    crate::files::write_atomic(&settings_path(), &json)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_from_older_builds_keep_their_choices() {
        // Written before the chat had providers: it stays with Claude and that model.
        let old: Settings = serde_json::from_str(r#"{"soundEnabled":false,"screen":"cursor","model":"claude-sonnet-5"}"#).unwrap();
        assert!(!old.sound_enabled);
        assert_eq!(old.screen, "cursor");
        assert_eq!(old.chat_provider, Provider::Anthropic);
        assert_eq!(old.chat_model(Provider::Anthropic), "claude-sonnet-5");
        assert_eq!(old.chat_model(Provider::OpenAi), Provider::OpenAi.default_model());
        assert_eq!(old.chat_model(Provider::Ollama), "gemma3:4b");
        assert!(old.browser_bridge, "the browser bridge is on unless turned off");

        // The new fields round-trip.
        let mut s = Settings { chat_provider: Provider::OpenAi, openai_model: "gpt-4.1".into(), ..Settings::default() };
        let json = serde_json::to_value(&s).unwrap();
        assert_eq!(json["chatProvider"], "openai");
        assert_eq!(json["openaiModel"], "gpt-4.1");
        assert_eq!(json["ollamaModel"], "gemma3:4b");
        assert_eq!(serde_json::from_value::<Settings>(json).unwrap().chat_model(Provider::OpenAi), "gpt-4.1");
        s.ollama_model = "  ".into();
        assert_eq!(s.chat_model(Provider::Ollama), "gemma3:4b");

        // A provider this build does not know keeps every other preference and stays local.
        let newer: Settings = serde_json::from_str(r#"{"soundEnabled":false,"chatProvider":"gemini"}"#).unwrap();
        assert!(!newer.sound_enabled);
        assert_eq!(newer.chat_provider, Provider::Ollama);
    }
}
