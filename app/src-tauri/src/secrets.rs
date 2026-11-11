// API keys and the vault key live in the Windows Credential Manager or, on Linux,
// the Secret Service (GNOME Keyring, KWallet) — never on disk and never in the
// front end. The island may only ask whether a user key is present, and may never
// see, set or clear the vault key (see `ui_may_touch`).

#[cfg_attr(test, allow(dead_code))]
const SERVICE: &str = "app.zuko.desktop";
/// The service used while Zuko's directories are redirected (settings::dirs_overridden).
#[cfg_attr(test, allow(dead_code))]
const DEV_SERVICE: &str = "app.zuko.desktop.dev";

/// The vault's encryption key (base64 of 32 bytes, vaultstore.rs). Internal: never
/// reachable from the UI commands.
pub const VAULT_KEY: &str = "vault-key";

/// Every key Zuko may store. Anything outside this list is refused.
pub const KNOWN_KEYS: &[&str] = &[
    // The user's own keys for the island chat (Claude or OpenAI, Settings → Chat).
    "anthropic-api-key",
    "openai-api-key",
    VAULT_KEY,
];

/// True for keys the UI commands (`secret_present` / `secret_set` / `secret_clear`)
/// may act on. Losing the vault key would make every stored value unrecoverable, so
/// it stays out of reach of the webview.
pub fn ui_may_touch(key: &str) -> bool {
    KNOWN_KEYS.contains(&key) && key != VAULT_KEY
}

/// Where the secrets really live. Unit tests get an in-memory stand-in so that no
/// test, however it is wired, can read or overwrite the user's real keyring entries.
#[cfg(not(test))]
mod backend {
    use keyring::Entry;

    fn entry(key: &str) -> Result<Entry, String> {
        let service = if crate::settings::dirs_overridden() { super::DEV_SERVICE } else { super::SERVICE };
        Entry::new(service, key).map_err(|e| e.to_string())
    }

    pub fn get(key: &str) -> Result<Option<String>, String> {
        match entry(key)?.get_password() {
            Ok(v) => Ok(Some(v)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(e.to_string()),
        }
    }

    pub fn set(key: &str, value: &str) -> Result<(), String> {
        entry(key)?.set_password(value).map_err(|e| e.to_string())
    }

    pub fn delete(key: &str) -> Result<(), String> {
        match entry(key)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    }
}

#[cfg(test)]
mod backend {
    use std::collections::HashMap;
    use std::sync::Mutex;

    static STORE: Mutex<Option<HashMap<String, String>>> = Mutex::new(None);

    pub fn get(key: &str) -> Result<Option<String>, String> {
        Ok(STORE.lock().unwrap().as_ref().and_then(|m| m.get(key).cloned()))
    }

    pub fn set(key: &str, value: &str) -> Result<(), String> {
        STORE.lock().unwrap().get_or_insert_with(HashMap::new).insert(key.into(), value.into());
        Ok(())
    }

    pub fn delete(key: &str) -> Result<(), String> {
        if let Some(m) = STORE.lock().unwrap().as_mut() {
            m.remove(key);
        }
        Ok(())
    }
}

fn known(key: &str) -> Result<(), String> {
    if KNOWN_KEYS.contains(&key) {
        Ok(())
    } else {
        Err(format!("unknown key {key}"))
    }
}

pub fn get(key: &str) -> Option<String> {
    get_checked(key).ok().flatten()
}

/// Like [`get`], but tells "nothing stored" (`Ok(None)`) apart from "the keyring
/// could not be reached" (`Err`). The vault store needs the difference: a missing
/// key means the old vault is gone for good, an unreachable keyring does not.
pub fn get_checked(key: &str) -> Result<Option<String>, String> {
    known(key)?;
    Ok(backend::get(key)?.filter(|v| !v.is_empty()))
}

pub fn set(key: &str, value: &str) -> Result<(), String> {
    known(key)?;
    if value.is_empty() {
        return backend::delete(key);
    }
    backend::set(key, value)
}

pub fn clear(key: &str) -> Result<(), String> {
    known(key)?;
    backend::delete(key)
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
        assert!(ui_may_touch("openai-api-key"));
        assert!(!ui_may_touch(VAULT_KEY));
        assert!(!ui_may_touch("github-token"));
    }

    #[test]
    fn unknown_keys_are_refused_and_values_round_trip() {
        assert!(set("github-token", "x").is_err());
        assert!(get_checked("github-token").is_err());
        assert_eq!(get_checked("anthropic-api-key").unwrap(), None);
        set("anthropic-api-key", "k").unwrap();
        assert_eq!(get("anthropic-api-key").as_deref(), Some("k"));
        assert!(present("anthropic-api-key"));
        clear("anthropic-api-key").unwrap();
        assert!(!present("anthropic-api-key"));
    }
}
