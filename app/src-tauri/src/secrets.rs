// API keys and the vault key live in the Windows Credential Manager or, on Linux,
// the Secret Service (GNOME Keyring, KWallet) — never on disk and never in the
// front end. The island may only ask whether a user key is present, and may never
// see, set or clear the vault key (see `ui_may_touch`).

use keyring::Entry;

const SERVICE: &str = "app.zuko.desktop";

/// The vault's encryption key (base64 of 32 bytes, vaultstore.rs). Internal: never
/// reachable from the UI commands.
pub const VAULT_KEY: &str = "vault-key";

/// Every key Zuko may store. Anything outside this list is refused.
pub const KNOWN_KEYS: &[&str] = &[
    // The user's own key for the island chat.
    "anthropic-api-key",
    VAULT_KEY,
];

/// True for keys the UI commands (`secret_present` / `secret_set` / `secret_clear`)
/// may act on. Losing the vault key would make every stored value unrecoverable, so
/// it stays out of reach of the webview.
pub fn ui_may_touch(key: &str) -> bool {
    KNOWN_KEYS.contains(&key) && key != VAULT_KEY
}

fn entry(key: &str) -> Option<Entry> {
    if !KNOWN_KEYS.contains(&key) {
        return None;
    }
    Entry::new(SERVICE, key).ok()
}

pub fn get(key: &str) -> Option<String> {
    entry(key)?.get_password().ok().filter(|v| !v.is_empty())
}

/// Like [`get`], but tells "nothing stored" (`Ok(None)`) apart from "the keyring
/// could not be reached" (`Err`). The vault store needs the difference: a missing
/// key means the old vault is gone for good, an unreachable keyring does not.
pub fn get_checked(key: &str) -> Result<Option<String>, String> {
    let entry = entry(key).ok_or_else(|| format!("unknown key {key}"))?;
    match entry.get_password() {
        Ok(v) if v.is_empty() => Ok(None),
        Ok(v) => Ok(Some(v)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

pub fn set(key: &str, value: &str) -> Result<(), String> {
    let entry = entry(key).ok_or_else(|| format!("unknown key {key}"))?;
    if value.is_empty() {
        let _ = entry.delete_credential();
        return Ok(());
    }
    entry.set_password(value).map_err(|e| e.to_string())
}

pub fn clear(key: &str) -> Result<(), String> {
    let entry = entry(key).ok_or_else(|| format!("unknown key {key}"))?;
    match entry.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(e.to_string()),
    }
}

pub fn present(key: &str) -> bool {
    get(key).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vault_key_is_known_but_out_of_ui_reach() {
        assert!(KNOWN_KEYS.contains(&VAULT_KEY));
        assert!(ui_may_touch("anthropic-api-key"));
        assert!(!ui_may_touch(VAULT_KEY));
        assert!(!ui_may_touch("github-token"));
    }
}
