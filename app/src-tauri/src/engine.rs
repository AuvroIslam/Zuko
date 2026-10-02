// Zuko's protection state, shared by the hook path (firewall.rs), the gateway
// (gateway.rs) and the UI commands (commands.rs).
//
// One `Engine` lives in Tauri's managed state for the whole run. It owns the
// policy, the compiled detector, the vault and the per-session taint ledgers.
// Locks are held only for the duration of a call; nothing here blocks on I/O
// except persistence, which happens outside the vault lock.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use zuko_core::detect::Detector;
use zuko_core::policy::Policy;
use zuko_core::taint::Ledger;
use zuko_core::vault::Vault;
use zuko_core::Ctx;

use crate::{policystore, vaultstore};

/// Counters shown in the UI (since launch).
#[derive(Default)]
pub struct Stats {
    pub masked: AtomicU64,
    pub blocked: AtomicU64,
    pub asked: AtomicU64,
    pub auto_allowed: AtomicU64,
    /// Unix seconds of the last message from the browser extension.
    pub extension_seen: AtomicU64,
}

impl Stats {
    pub fn add(counter: &AtomicU64, n: u64) {
        counter.fetch_add(n, Ordering::Relaxed);
    }
}

/// Machine facts that go into every `Ctx`.
#[derive(Clone, Debug, Default)]
pub struct CtxBase {
    pub home: String,
    pub protected_paths: Vec<String>,
    pub protected_processes: Vec<String>,
    pub windows: bool,
}

pub struct Engine {
    policy: RwLock<Arc<Policy>>,
    detector: RwLock<Arc<Detector>>,
    vault: Mutex<Vault>,
    ledgers: Mutex<HashMap<String, Ledger>>,
    base: CtxBase,
    pub stats: Stats,
}

/// Unix seconds.
pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Unix milliseconds.
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Paths the agent must never modify: Zuko's own data and binaries, and Claude
/// Code's settings files (where Zuko's hooks and gateway live).
fn protected_paths(home: &str) -> Vec<String> {
    let mut v = vec![
        crate::platform::config_dir().to_string_lossy().to_string(),
        crate::platform::local_dir().to_string_lossy().to_string(),
    ];
    let claude = std::path::Path::new(home).join(".claude");
    for f in ["settings.json", "settings.local.json"] {
        v.push(claude.join(f).to_string_lossy().to_string());
    }
    if let Ok(exe) = std::env::current_exe() {
        v.push(exe.to_string_lossy().to_string());
    }
    v
}

impl Engine {
    /// Loads the policy and vault from disk (defaults when missing or unreadable).
    pub fn load() -> Engine {
        let policy = policystore::load();
        let detector = Detector::new(&policy.privacy.detector);
        let vault = vaultstore::load();
        let home = crate::platform::home_dir().to_string_lossy().to_string();
        let base = CtxBase {
            protected_paths: protected_paths(&home),
            protected_processes: vec!["zuko.exe".into(), "zuko".into(), "zuko-hook.exe".into()],
            windows: cfg!(windows),
            home,
        };
        Engine {
            policy: RwLock::new(Arc::new(policy)),
            detector: RwLock::new(Arc::new(detector)),
            vault: Mutex::new(vault),
            ledgers: Mutex::new(HashMap::new()),
            base,
            stats: Stats::default(),
        }
    }

    pub fn policy(&self) -> Arc<Policy> {
        self.policy.read().unwrap().clone()
    }

    /// Validates, saves and activates a new policy; rebuilds the detector.
    pub fn set_policy(&self, policy: Policy) -> Result<(), String> {
        policystore::save(&policy).map_err(|e| format!("could not save the policy: {e}"))?;
        let detector = Detector::new(&policy.privacy.detector);
        *self.detector.write().unwrap() = Arc::new(detector);
        *self.policy.write().unwrap() = Arc::new(policy);
        Ok(())
    }

    pub fn detector(&self) -> Arc<Detector> {
        self.detector.read().unwrap().clone()
    }

    /// Runs `f` with the vault locked. Call [`Engine::persist_vault`] afterwards
    /// if `f` may have added entries.
    pub fn with_vault<R>(&self, f: impl FnOnce(&mut Vault) -> R) -> R {
        let mut v = self.vault.lock().unwrap();
        f(&mut v)
    }

    /// A copy of the vault for long read-only work (e.g. one streamed response).
    pub fn vault_snapshot(&self) -> Vault {
        self.vault.lock().unwrap().clone()
    }

    /// Writes the vault to disk (encrypted). Cheap to call after every change.
    pub fn persist_vault(&self) {
        let snapshot = self.vault_snapshot();
        vaultstore::save(&snapshot);
    }

    /// Builds the engine context for a session.
    pub fn ctx(&self, cwd: &str, gateway_active: bool) -> Ctx {
        Ctx {
            cwd: cwd.to_string(),
            home: self.base.home.clone(),
            project_dir: cwd.to_string(),
            protected_paths: self.base.protected_paths.clone(),
            protected_processes: self.base.protected_processes.clone(),
            windows: self.base.windows,
            gateway_active,
            now: now(),
        }
    }

    /// Runs `f` with the session's taint ledger (created on first use).
    pub fn with_ledger<R>(&self, session_id: &str, f: impl FnOnce(&mut Ledger) -> R) -> R {
        let mut ledgers = self.ledgers.lock().unwrap();
        let ledger = ledgers
            .entry(session_id.to_string())
            .or_insert_with(|| Ledger::new(session_id));
        f(ledger)
    }

    /// Drops a finished session's ledger.
    pub fn end_session(&self, session_id: &str) {
        self.ledgers.lock().unwrap().remove(session_id);
    }
}
