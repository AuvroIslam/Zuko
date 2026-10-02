// The vault on disk: %LOCALAPPDATA%\Zuko\vault.bin.
//
// Format: 24-byte XChaCha20-Poly1305 nonce || ciphertext of the vault JSON. The
// 32-byte key is generated on first save and kept in the OS keyring under
// `vault-key` (secrets.rs), base64-encoded, never on disk. Every save draws a fresh
// random nonce and replaces the file atomically (temp file + rename), so a crash
// leaves either the old vault or the new one, never half of one.
//
// A file that cannot be read back is never overwritten and never silently lost:
// it is moved to `vault.bin.unreadable-<ts>` and an empty vault is used. That
// covers a failed authentication tag (tampering, truncation, wrong key), a vault
// whose JSON no longer parses, and a vault file whose key is gone from the keyring.
// When the keyring is merely unreachable the file stays where it is; the next save
// retries, and merges what the old file held into the current vault.
//
// The key source is a trait so tests (and `cfg(test)` builds of the app) use an
// in-memory key and never touch the real keyring.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine as _;
use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{Key, XChaCha20Poly1305, XNonce};
use zuko_core::vault::Vault;

const NONCE_LEN: usize = 24;
const KEY_LEN: usize = 32;
/// Nonce plus the 16-byte Poly1305 tag: anything shorter cannot be a vault.
const MIN_FILE_LEN: usize = NONCE_LEN + 16;

pub fn path() -> PathBuf {
    crate::settings::local_dir().join("vault.bin")
}

// ── Key sources ───────────────────────────────────────────────────────────────

/// Where the vault key lives. Values are the base64 text stored in the keyring.
pub trait KeySource: Send + Sync {
    /// `Ok(None)` when nothing is stored, `Err` when the store cannot be reached.
    fn get(&self) -> Result<Option<String>, String>;
    fn set(&self, value: &str) -> Result<(), String>;
}

/// The OS keyring (Credential Manager / Secret Service) via secrets.rs.
pub struct KeyringKey;

impl KeySource for KeyringKey {
    fn get(&self) -> Result<Option<String>, String> {
        crate::secrets::get_checked(crate::secrets::VAULT_KEY)
    }

    fn set(&self, value: &str) -> Result<(), String> {
        crate::secrets::set(crate::secrets::VAULT_KEY, value)
    }
}

/// A key held in memory only (tests, and `cfg(test)` builds of the app).
#[cfg(test)]
#[derive(Default)]
pub struct MemoryKey {
    value: Mutex<Option<String>>,
    /// Makes `get` fail, to exercise an unreachable keyring.
    pub unreachable: std::sync::atomic::AtomicBool,
}

#[cfg(test)]
impl MemoryKey {
    pub fn with(value: &str) -> MemoryKey {
        MemoryKey { value: Mutex::new(Some(value.to_string())), ..Default::default() }
    }

    pub fn current(&self) -> Option<String> {
        self.value.lock().unwrap().clone()
    }
}

#[cfg(test)]
impl KeySource for MemoryKey {
    fn get(&self) -> Result<Option<String>, String> {
        if self.unreachable.load(std::sync::atomic::Ordering::Relaxed) {
            return Err("keyring unreachable".into());
        }
        Ok(self.value.lock().unwrap().clone())
    }

    fn set(&self, value: &str) -> Result<(), String> {
        *self.value.lock().unwrap() = Some(value.to_string());
        Ok(())
    }
}

impl<T: KeySource + ?Sized> KeySource for std::sync::Arc<T> {
    fn get(&self) -> Result<Option<String>, String> {
        (**self).get()
    }

    fn set(&self, value: &str) -> Result<(), String> {
        (**self).set(value)
    }
}

// ── The store ─────────────────────────────────────────────────────────────────

/// Why the key could not be used.
enum KeyState {
    Ready([u8; KEY_LEN]),
    /// Nothing usable stored: absent, or not 32 bytes of base64.
    Missing,
    /// The keyring could not be reached.
    Unreachable(String),
}

pub struct VaultStore {
    file: PathBuf,
    keys: Box<dyn KeySource>,
    /// Set when `load` met an existing file it could not judge (keyring down), so
    /// the next `save` must fold that file in instead of overwriting it.
    unverified: Mutex<bool>,
    /// Serializes load/save so the aside-and-rewrite steps never interleave.
    io: Mutex<()>,
}

impl VaultStore {
    pub fn new(file: PathBuf, keys: Box<dyn KeySource>) -> VaultStore {
        VaultStore { file, keys, unverified: Mutex::new(false), io: Mutex::new(()) }
    }

    fn key(&self) -> KeyState {
        match self.keys.get() {
            Err(e) => KeyState::Unreachable(e),
            Ok(None) => KeyState::Missing,
            Ok(Some(text)) => match B64.decode(text.trim()) {
                Ok(bytes) => match <[u8; KEY_LEN]>::try_from(bytes.as_slice()) {
                    Ok(key) => KeyState::Ready(key),
                    Err(_) => KeyState::Missing,
                },
                Err(_) => KeyState::Missing,
            },
        }
    }

    /// Generates a fresh key and stores it. Only called once the old file (if any)
    /// is out of the way, so the new key can never orphan a readable vault.
    fn create_key(&self) -> Result<[u8; KEY_LEN], String> {
        let mut key = [0u8; KEY_LEN];
        getrandom::getrandom(&mut key).map_err(|e| format!("no random source: {e}"))?;
        self.keys.set(&B64.encode(key))?;
        Ok(key)
    }

    /// The saved vault; an empty one when there is none or it cannot be read (see the
    /// module doc for what happens to a file that fails).
    pub fn load(&self) -> Vault {
        let _io = self.io.lock().unwrap();
        let bytes = match std::fs::read(&self.file) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Vault::new(),
            Err(e) => {
                crate::log::line(format!("vault: cannot read {}: {e}", self.file.display()));
                *self.unverified.lock().unwrap() = true;
                return Vault::new();
            }
        };
        match self.key() {
            KeyState::Ready(key) => match decrypt(&key, &bytes) {
                Ok(vault) => vault,
                Err(why) => {
                    self.set_aside(&format!("it could not be decrypted ({why})"));
                    Vault::new()
                }
            },
            KeyState::Missing => {
                self.set_aside("its key is gone from the keyring");
                Vault::new()
            }
            KeyState::Unreachable(e) => {
                crate::log::line(format!("vault: keyring unreachable ({e}); vault.bin left untouched"));
                *self.unverified.lock().unwrap() = true;
                Vault::new()
            }
        }
    }

    /// Encrypts and writes `vault`. Failures are logged, never raised: the in-memory
    /// vault keeps working and the next save tries again.
    pub fn save(&self, vault: &Vault) {
        if let Err(e) = self.try_save(vault) {
            crate::log::line(format!("vault: not saved: {e}"));
        }
    }

    pub fn try_save(&self, vault: &Vault) -> Result<(), String> {
        let _io = self.io.lock().unwrap();
        let mut unverified = self.unverified.lock().unwrap();
        let mut merged;
        let mut vault = vault;

        // A file `load` could not judge is judged now: readable → fold it in, so
        // nothing it held is overwritten; unreadable → set aside; keyring still
        // down → write nothing (the file is the only copy).
        if *unverified && self.file.exists() {
            match self.key() {
                KeyState::Unreachable(e) => return Err(format!("keyring unreachable: {e}")),
                KeyState::Ready(key) => match std::fs::read(&self.file).map_err(|e| e.to_string()).and_then(|b| decrypt(&key, &b)) {
                    Ok(old) => {
                        merged = old;
                        merged.merge(vault, crate::engine::now());
                        vault = &merged;
                    }
                    Err(why) => self.set_aside(&format!("it could not be decrypted ({why})")),
                },
                KeyState::Missing => self.set_aside("its key is gone from the keyring"),
            }
        }

        let key = match self.key() {
            KeyState::Ready(k) => k,
            KeyState::Missing => self.create_key()?,
            KeyState::Unreachable(e) => return Err(format!("keyring unreachable: {e}")),
        };
        let blob = encrypt(&key, vault.to_json().as_bytes())?;
        write_atomic(&self.file, &blob).map_err(|e| format!("cannot write {}: {e}", self.file.display()))?;
        *unverified = false;
        Ok(())
    }

    /// Moves the current file to `vault.bin.unreadable-<ts>` (never deleting it).
    fn set_aside(&self, why: &str) {
        let ts = crate::engine::now();
        let mut name = self.file.as_os_str().to_owned();
        name.push(format!(".unreadable-{ts}"));
        let mut aside = PathBuf::from(&name);
        let mut n = 1;
        while aside.exists() {
            let mut next = name.clone();
            next.push(format!("-{n}"));
            aside = PathBuf::from(next);
            n += 1;
        }
        match std::fs::rename(&self.file, &aside) {
            Ok(()) => crate::log::line(format!("vault: {why}; kept as {}", aside.display())),
            Err(e) => crate::log::line(format!("vault: {why}, and it could not be moved aside: {e}")),
        }
    }
}

fn encrypt(key: &[u8; KEY_LEN], plain: &[u8]) -> Result<Vec<u8>, String> {
    let mut nonce = [0u8; NONCE_LEN];
    getrandom::getrandom(&mut nonce).map_err(|e| format!("no random source: {e}"))?;
    let cipher = XChaCha20Poly1305::new(Key::from_slice(key));
    let sealed = cipher
        .encrypt(XNonce::from_slice(&nonce), plain)
        .map_err(|_| "encryption failed".to_string())?;
    let mut out = Vec::with_capacity(NONCE_LEN + sealed.len());
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&sealed);
    Ok(out)
}

fn decrypt(key: &[u8; KEY_LEN], blob: &[u8]) -> Result<Vault, String> {
    if blob.len() < MIN_FILE_LEN {
        return Err("file too short".into());
    }
    let (nonce, sealed) = blob.split_at(NONCE_LEN);
    let cipher = XChaCha20Poly1305::new(Key::from_slice(key));
    let plain = cipher
        .decrypt(XNonce::from_slice(nonce), sealed)
        .map_err(|_| "authentication failed".to_string())?;
    let text = String::from_utf8(plain).map_err(|_| "not UTF-8".to_string())?;
    Vault::from_json(&text).map_err(|e| format!("bad vault JSON: {e}"))
}

/// Writes `bytes` next to `target` and renames it into place.
pub(crate) fn write_atomic(target: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = target.parent() {
        crate::platform::ensure_private_dir(dir)?;
    }
    let mut tmp_name = target.as_os_str().to_owned();
    tmp_name.push(format!(".tmp-{}", std::process::id()));
    let tmp = PathBuf::from(tmp_name);
    let result = (|| {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let mut file = options.open(&tmp)?;
        file.write_all(bytes)?;
        // The rename must not publish a file whose bytes are still in a cache.
        file.sync_all()?;
        drop(file);
        std::fs::rename(&tmp, target)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

// ── The app's store ───────────────────────────────────────────────────────────

fn default_store() -> &'static VaultStore {
    static STORE: OnceLock<VaultStore> = OnceLock::new();
    STORE.get_or_init(|| {
        #[cfg(not(test))]
        let keys: Box<dyn KeySource> = Box::new(KeyringKey);
        // Unit tests of the app never reach the real keyring.
        #[cfg(test)]
        let keys: Box<dyn KeySource> = Box::new(MemoryKey::default());
        VaultStore::new(path(), keys)
    })
}

pub fn load() -> Vault {
    default_store().load()
}

pub fn save(vault: &Vault) {
    default_store().save(vault);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use zuko_core::detect::Category;

    fn tmp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("zuko-vs-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn sample() -> Vault {
        let mut v = Vault::new();
        v.add_manual("sk-proj-abcdefghijklmnopqrstuvwx1234", "API_KEY", "OpenAI API key", 5).unwrap();
        v.add_manual("alice@example.org", "EMAIL", "Email address", 6).unwrap();
        v
    }

    fn store(dir: &Path, keys: Arc<MemoryKey>) -> VaultStore {
        VaultStore::new(dir.join("vault.bin"), Box::new(keys))
    }

    fn asides(dir: &Path) -> Vec<PathBuf> {
        std::fs::read_dir(dir)
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.file_name().unwrap().to_string_lossy().contains(".unreadable-"))
            .collect()
    }

    #[test]
    fn round_trip_and_key_creation() {
        let dir = tmp("rt");
        let keys = Arc::new(MemoryKey::default());
        let s = store(&dir, keys.clone());
        // Nothing yet: empty vault, and no key is created just by loading.
        assert!(s.load().is_empty());
        assert!(keys.current().is_none());

        s.try_save(&sample()).unwrap();
        let key_text = keys.current().expect("key stored on first save");
        assert_eq!(B64.decode(&key_text).unwrap().len(), 32);

        let back = store(&dir, keys.clone()).load();
        assert_eq!(back.len(), 2);
        assert_eq!(back.get("API_KEY_1").unwrap().value, "sk-proj-abcdefghijklmnopqrstuvwx1234");
        assert_eq!(back.get("EMAIL_1").unwrap().category, Category::Custom);
        assert!(asides(&dir).is_empty());

        // The file holds no plaintext, and each save uses a fresh nonce.
        let a = std::fs::read(dir.join("vault.bin")).unwrap();
        assert!(!String::from_utf8_lossy(&a).contains("alice@example.org"));
        s.try_save(&sample()).unwrap();
        let b = std::fs::read(dir.join("vault.bin")).unwrap();
        assert_ne!(a[..NONCE_LEN], b[..NONCE_LEN]);
        // The key is stable across saves.
        assert_eq!(keys.current().unwrap(), key_text);
        // No temp file is left behind.
        let leftovers = std::fs::read_dir(&dir).unwrap().flatten().filter(|e| e.file_name().to_string_lossy().contains(".tmp-")).count();
        assert_eq!(leftovers, 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn tampered_file_is_set_aside() {
        let dir = tmp("tamper");
        let keys = Arc::new(MemoryKey::default());
        let s = store(&dir, keys.clone());
        s.try_save(&sample()).unwrap();
        for position in [0usize, NONCE_LEN + 3, std::fs::metadata(dir.join("vault.bin")).unwrap().len() as usize - 1] {
            let mut bytes = std::fs::read(dir.join("vault.bin")).unwrap();
            bytes[position] ^= 0x01;
            std::fs::write(dir.join("vault.bin"), &bytes).unwrap();
            assert!(store(&dir, keys.clone()).load().is_empty(), "flip at {position} must be detected");
            assert!(!dir.join("vault.bin").exists(), "the damaged file must be moved away");
            // Put a good vault back for the next flip (the aside copies pile up, uniquely named).
            s.try_save(&sample()).unwrap();
        }
        assert_eq!(asides(&dir).len(), 3);
        // Truncation is detected too.
        let bytes = std::fs::read(dir.join("vault.bin")).unwrap();
        std::fs::write(dir.join("vault.bin"), &bytes[..10]).unwrap();
        assert!(store(&dir, keys).load().is_empty());
        assert_eq!(asides(&dir).len(), 4);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn wrong_key_is_set_aside_and_not_overwritten() {
        let dir = tmp("wrong");
        let s = store(&dir, Arc::new(MemoryKey::default()));
        s.try_save(&sample()).unwrap();
        let original = std::fs::read(dir.join("vault.bin")).unwrap();

        let other = Arc::new(MemoryKey::with(&B64.encode([7u8; 32])));
        let loaded = store(&dir, other.clone()).load();
        assert!(loaded.is_empty());
        let asides = asides(&dir);
        assert_eq!(asides.len(), 1);
        assert_eq!(std::fs::read(&asides[0]).unwrap(), original, "the old vault is kept byte for byte");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_or_garbled_key_with_existing_file() {
        let dir = tmp("nokey");
        let s = store(&dir, Arc::new(MemoryKey::default()));
        s.try_save(&sample()).unwrap();

        // The keyring entry vanished.
        let gone = Arc::new(MemoryKey::default());
        assert!(store(&dir, gone.clone()).load().is_empty());
        assert_eq!(asides(&dir).len(), 1);
        assert!(!dir.join("vault.bin").exists());

        // A fresh save works and creates a new key.
        let fresh = store(&dir, gone.clone());
        fresh.try_save(&sample()).unwrap();
        assert!(gone.current().is_some());
        assert_eq!(store(&dir, gone).load().len(), 2);

        // A key that is not 32 bytes of base64 counts as missing.
        let bad = Arc::new(MemoryKey::with("not-a-key"));
        assert!(store(&dir, bad).load().is_empty());
        assert_eq!(asides(&dir).len(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unreachable_keyring_keeps_the_file_and_merges_on_recovery() {
        let dir = tmp("down");
        let keys = Arc::new(MemoryKey::default());
        store(&dir, keys.clone()).try_save(&sample()).unwrap();
        let original = std::fs::read(dir.join("vault.bin")).unwrap();

        keys.unreachable.store(true, std::sync::atomic::Ordering::Relaxed);
        let s = store(&dir, keys.clone());
        let mut working = s.load();
        assert!(working.is_empty());
        assert!(asides(&dir).is_empty(), "an unreachable keyring must not cost the user the vault");
        working.add_manual("another-secret-value-123", "SECRET", "Secret", 9).unwrap();
        // Still down: nothing is written, the old file is untouched.
        assert!(s.try_save(&working).is_err());
        assert_eq!(std::fs::read(dir.join("vault.bin")).unwrap(), original);

        // Back up: the old entries and the new one end up together.
        keys.unreachable.store(false, std::sync::atomic::Ordering::Relaxed);
        s.try_save(&working).unwrap();
        let back = store(&dir, keys).load();
        assert_eq!(back.len(), 3);
        assert!(back.key_for_value("alice@example.org").is_some());
        assert!(back.key_for_value("another-secret-value-123").is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
