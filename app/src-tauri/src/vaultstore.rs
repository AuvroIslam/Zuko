// The vault on disk: %LOCALAPPDATA%\Zuko\vault.bin.
//
// Format: 24-byte XChaCha20-Poly1305 nonce || ciphertext of the vault JSON. The
// 32-byte key is generated on first use and kept in the OS keyring under
// `vault-key` (secrets.rs), never on disk. A file that fails to decrypt is kept
// aside as `vault.bin.unreadable-<ts>` and an empty vault is used.
//
// OWNER: state & features (wave 2). Stub until then: in-memory only.

use std::path::PathBuf;

use zuko_core::vault::Vault;

pub fn path() -> PathBuf {
    crate::platform::local_dir().join("vault.bin")
}

pub fn load() -> Vault {
    Vault::new()
}

pub fn save(vault: &Vault) {
    let _ = vault;
}
