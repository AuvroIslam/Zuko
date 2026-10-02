// Zuko's protection state, shared by the hook path (firewall.rs), the gateway
// (gateway.rs) and the UI commands (commands.rs).
//
// One `Engine` lives in Tauri's managed state for the whole run. It owns the
// policy, the compiled detector, the vault and the per-session taint ledgers.
// Locks are held only for the duration of a call; nothing here blocks on I/O
// except persistence, which happens outside the vault lock.
//
// * Per-project policy: `policy_for(cwd)` / `detector_for(cwd)` layer
//   `<cwd>/.zuko/policy.json` over the global policy with `Policy::merged_with` (a
//   project can only add restrictions). The result is cached per file and re-read when
//   the file's mtime or size changes, or when the global policy changes.
// * Vault persistence is debounced and serialized: `persist_vault()` only marks the
//   vault dirty; one background writer saves the latest state shortly after, never
//   two writes at once, and a burst of changes costs one write ("last state wins").
//   `flush_vault()` writes synchronously (exit, tests).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, SystemTime};

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
    /// Bumped by every `set_policy`; cached project layers older than this are stale.
    policy_generation: AtomicU64,
    project_cache: Mutex<HashMap<PathBuf, ProjectEntry>>,
    vault: Arc<Mutex<Vault>>,
    persister: Arc<Persister>,
    ledgers: Mutex<HashMap<String, Ledger>>,
    base: CtxBase,
    pub stats: Stats,
}

// ── Per-project policy ────────────────────────────────────────────────────────

/// A project policy larger than this is ignored (it is a few hundred bytes in practice).
const MAX_PROJECT_POLICY: u64 = 256 * 1024;
/// Distinct project folders remembered; the cache is cleared when it grows past this.
const MAX_PROJECT_CACHE: usize = 64;

type Layer = (Arc<Policy>, Arc<Detector>);

struct ProjectEntry {
    /// The file's modification time and size when it was read.
    stamp: (SystemTime, u64),
    generation: u64,
    /// `None`: the file did not parse (the global policy applies).
    layer: Option<Layer>,
}

// ── Debounced vault persistence ───────────────────────────────────────────────

/// How long a change waits for company before it is written.
const PERSIST_DEBOUNCE: Duration = Duration::from_millis(250);

type Sink = Box<dyn Fn(&Vault) + Send + Sync>;

struct Persister {
    vault: Arc<Mutex<Vault>>,
    /// Where snapshots go; `None` for engines that keep the vault in memory only.
    sink: Option<Sink>,
    debounce: Duration,
    dirty: AtomicBool,
    /// A background writer thread is alive (or about to be).
    worker: AtomicBool,
    /// Held for the whole of a write: one write in flight, and `flush` waits for it.
    io: Mutex<()>,
}

impl Persister {
    fn new(vault: Arc<Mutex<Vault>>, sink: Option<Sink>, debounce: Duration) -> Arc<Persister> {
        Arc::new(Persister {
            vault,
            sink,
            debounce,
            dirty: AtomicBool::new(false),
            worker: AtomicBool::new(false),
            io: Mutex::new(()),
        })
    }

    /// Notes that the vault changed and makes sure a writer will get to it.
    fn mark(self: &Arc<Self>) {
        if self.sink.is_none() {
            return;
        }
        self.dirty.store(true, Ordering::SeqCst);
        if self.worker.swap(true, Ordering::SeqCst) {
            return;
        }
        let me = self.clone();
        let spawned = std::thread::Builder::new().name("zuko-vault-writer".into()).spawn(move || loop {
            std::thread::sleep(me.debounce);
            me.write_if_dirty();
            me.worker.store(false, Ordering::SeqCst);
            // A change that slipped in after the write: take the job back, unless a
            // newly started writer already has it.
            if !me.dirty.load(Ordering::SeqCst) || me.worker.swap(true, Ordering::SeqCst) {
                break;
            }
        });
        if spawned.is_err() {
            // Out of threads: write on the caller's thread rather than not at all.
            self.worker.store(false, Ordering::SeqCst);
            self.write_if_dirty();
        }
    }

    /// Writes the latest state if anything changed since the last write.
    fn write_if_dirty(&self) {
        let Some(sink) = &self.sink else { return };
        let _io = self.io.lock().unwrap_or_else(|e| e.into_inner());
        if self.dirty.swap(false, Ordering::SeqCst) {
            let snapshot = self.vault.lock().unwrap_or_else(|e| e.into_inner()).clone();
            sink(&snapshot);
        }
    }
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
        crate::settings::config_dir().to_string_lossy().to_string(),
        crate::settings::local_dir().to_string_lossy().to_string(),
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
    fn build(policy: Policy, vault: Vault, base: CtxBase, sink: Option<Sink>, debounce: Duration) -> Engine {
        let detector = Detector::new(&policy.privacy.detector);
        let vault = Arc::new(Mutex::new(vault));
        Engine {
            policy: RwLock::new(Arc::new(policy)),
            detector: RwLock::new(Arc::new(detector)),
            policy_generation: AtomicU64::new(0),
            project_cache: Mutex::new(HashMap::new()),
            persister: Persister::new(vault.clone(), sink, debounce),
            vault,
            ledgers: Mutex::new(HashMap::new()),
            base,
            stats: Stats::default(),
        }
    }

    /// An engine from explicit parts (tests, the headless gateway). The vault lives
    /// in memory only: `persist_vault` does nothing.
    pub fn with_parts(policy: Policy, vault: Vault, base: CtxBase) -> Engine {
        Engine::build(policy, vault, base, None, PERSIST_DEBOUNCE)
    }

    /// Like [`Engine::with_parts`], but vault snapshots go to `sink`, debounced by
    /// `debounce`. For tests of persistence.
    pub fn with_persistence(
        policy: Policy,
        vault: Vault,
        base: CtxBase,
        debounce: Duration,
        sink: impl Fn(&Vault) + Send + Sync + 'static,
    ) -> Engine {
        Engine::build(policy, vault, base, Some(Box::new(sink)), debounce)
    }

    pub fn base(&self) -> &CtxBase {
        &self.base
    }

    /// Loads the policy and vault from disk (defaults when missing or unreadable).
    pub fn load() -> Engine {
        let policy = policystore::load();
        let vault = vaultstore::load();
        let home = crate::platform::home_dir().to_string_lossy().to_string();
        let base = CtxBase {
            protected_paths: protected_paths(&home),
            protected_processes: vec!["zuko.exe".into(), "zuko".into(), "zuko-hook.exe".into()],
            windows: cfg!(windows),
            home,
        };
        Engine::build(policy, vault, base, Some(Box::new(|v: &Vault| vaultstore::save(v))), PERSIST_DEBOUNCE)
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
        // After the swap: a project layer built from the old policy is now stale.
        self.policy_generation.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    pub fn detector(&self) -> Arc<Detector> {
        self.detector.read().unwrap().clone()
    }

    /// The policy in force for a session working in `cwd`: the global policy with the
    /// project's `.zuko/policy.json` layered on top when that file exists and parses.
    /// A project can only tighten the global policy (see `Policy::merged_with`).
    pub fn policy_for(&self, cwd: &str) -> Arc<Policy> {
        match self.project_layer(cwd) {
            Some((policy, _)) => policy,
            None => self.policy(),
        }
    }

    /// The detector matching [`Engine::policy_for`] (a project may add custom terms).
    pub fn detector_for(&self, cwd: &str) -> Arc<Detector> {
        match self.project_layer(cwd) {
            Some((_, detector)) => detector,
            None => self.detector(),
        }
    }

    /// `<cwd>/.zuko/policy.json`, when `cwd` is set.
    pub fn project_policy_path(cwd: &str) -> Option<PathBuf> {
        (!cwd.trim().is_empty()).then(|| std::path::Path::new(cwd).join(".zuko").join("policy.json"))
    }

    fn project_layer(&self, cwd: &str) -> Option<Layer> {
        let path = Engine::project_policy_path(cwd)?;
        let meta = match std::fs::metadata(&path) {
            Ok(m) if m.is_file() && m.len() <= MAX_PROJECT_POLICY => m,
            _ => {
                self.project_cache.lock().unwrap().remove(&path);
                return None;
            }
        };
        let stamp = (meta.modified().unwrap_or(SystemTime::UNIX_EPOCH), meta.len());
        // Read the generation before the global policy, so a policy swapped in
        // between leaves an entry that the next call refreshes.
        let generation = self.policy_generation.load(Ordering::SeqCst);
        if let Some(entry) = self.project_cache.lock().unwrap().get(&path) {
            if entry.stamp == stamp && entry.generation == generation {
                return entry.layer.clone();
            }
        }
        let parsed = std::fs::read_to_string(&path).map_err(|e| e.to_string()).and_then(|t| Policy::from_json(&t));
        let layer = match parsed {
            Ok(project) => {
                let global = self.policy();
                let merged = global.merged_with(&project);
                // Same detector settings as the global policy: share its compiled rules.
                let detector = if merged.privacy.detector == global.privacy.detector {
                    self.detector()
                } else {
                    Arc::new(Detector::new(&merged.privacy.detector))
                };
                Some((Arc::new(merged), detector))
            }
            Err(why) => {
                // Logged once per version of the file: the entry below caches the failure.
                crate::log::line(format!("ignoring {}: {why}", path.display()));
                None
            }
        };
        let mut cache = self.project_cache.lock().unwrap();
        if cache.len() >= MAX_PROJECT_CACHE {
            cache.clear();
        }
        cache.insert(path, ProjectEntry { stamp, generation, layer: layer.clone() });
        layer
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

    /// Schedules a write of the vault to disk (encrypted). Cheap to call after every
    /// change: calls within a moment of each other share one write of the latest state.
    pub fn persist_vault(&self) {
        self.persister.mark();
    }

    /// Writes any pending vault change now and waits for a write in flight. Call
    /// before exit; the tests use it to see the result of `persist_vault`.
    pub fn flush_vault(&self) {
        self.persister.write_if_dirty();
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    fn tmp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("zuko-eng-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn engine() -> Engine {
        Engine::with_parts(Policy::default(), Vault::new(), CtxBase::default())
    }

    fn write_project_policy(cwd: &std::path::Path, json: &str) {
        let dir = cwd.join(".zuko");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("policy.json"), json).unwrap();
    }

    #[test]
    fn project_policy_layers_over_the_global_one_and_is_cached() {
        let cwd = tmp("project");
        let e = engine();
        let cwd_s = cwd.to_string_lossy().to_string();

        // No project file: the global policy itself.
        assert!(Arc::ptr_eq(&e.policy_for(&cwd_s), &e.policy()));
        assert!(Arc::ptr_eq(&e.policy_for(""), &e.policy()));

        write_project_policy(
            &cwd,
            r#"{ "network": { "blocked": ["evil.example"] },
                 "privacy": { "detector": { "customTerms": ["Project Falcon"] } } }"#,
        );
        let merged = e.policy_for(&cwd_s);
        assert!(merged.network.blocked.contains(&"evil.example".to_string()));
        // Nothing the global policy blocks is lost.
        for b in &e.policy().network.blocked {
            assert!(merged.network.blocked.contains(b), "{b} must stay blocked");
        }
        assert!(!e.policy().network.blocked.contains(&"evil.example".to_string()));
        // The project's custom term reaches the detector for this folder only.
        let hits = |d: &Detector| d.scan("the Project Falcon launch").len();
        assert_eq!(hits(&e.detector_for(&cwd_s)), 1);
        assert_eq!(hits(&e.detector()), 0);

        // Unchanged file: the very same objects come back.
        assert!(Arc::ptr_eq(&merged, &e.policy_for(&cwd_s)));
        assert!(Arc::ptr_eq(&e.detector_for(&cwd_s), &e.detector_for(&cwd_s)));

        // An edit (different size) is picked up.
        write_project_policy(&cwd, r#"{ "network": { "blocked": ["evil.example", "worse.example"] } }"#);
        let edited = e.policy_for(&cwd_s);
        assert!(!Arc::ptr_eq(&merged, &edited));
        assert!(edited.network.blocked.contains(&"worse.example".to_string()));

        // A new global policy invalidates the cached layer.
        let mut g = Policy::default();
        g.network.blocked.push("global-new.example".into());
        e.set_policy(g).unwrap();
        assert!(e.policy_for(&cwd_s).network.blocked.contains(&"global-new.example".to_string()));

        // A broken file is ignored (global policy), a removed one too.
        write_project_policy(&cwd, "{ not json");
        assert!(Arc::ptr_eq(&e.policy_for(&cwd_s), &e.policy()));
        std::fs::remove_file(cwd.join(".zuko").join("policy.json")).unwrap();
        assert!(Arc::ptr_eq(&e.policy_for(&cwd_s), &e.policy()));
        let _ = std::fs::remove_dir_all(&cwd);
    }

    #[test]
    fn a_project_cannot_relax_the_global_policy() {
        let cwd = tmp("relax");
        let e = engine();
        write_project_policy(
            &cwd,
            r#"{ "mode": "monitor", "network": { "blocked": [] },
                 "approvals": { "autoAllowLowRisk": true, "holdToApproveFrom": "critical", "blockFrom": "critical" } }"#,
        );
        let merged = e.policy_for(&cwd.to_string_lossy());
        assert_eq!(merged.mode, zuko_core::policy::Mode::Enforce);
        assert!(!merged.network.blocked.is_empty());
        let _ = std::fs::remove_dir_all(&cwd);
    }

    /// A sink that records how often it ran, the last vault size, and whether two
    /// calls ever overlapped.
    struct Probe {
        calls: Arc<AtomicUsize>,
        last_len: Arc<AtomicUsize>,
        in_flight: Arc<AtomicUsize>,
        overlapped: Arc<AtomicBool>,
    }

    fn probe_engine(debounce_ms: u64, write_ms: u64) -> (Engine, Probe) {
        let probe = Probe {
            calls: Arc::new(AtomicUsize::new(0)),
            last_len: Arc::new(AtomicUsize::new(0)),
            in_flight: Arc::new(AtomicUsize::new(0)),
            overlapped: Arc::new(AtomicBool::new(false)),
        };
        let (calls, last_len, in_flight, overlapped) =
            (probe.calls.clone(), probe.last_len.clone(), probe.in_flight.clone(), probe.overlapped.clone());
        let e = Engine::with_persistence(
            Policy::default(),
            Vault::new(),
            CtxBase::default(),
            Duration::from_millis(debounce_ms),
            move |v| {
                if in_flight.fetch_add(1, Ordering::SeqCst) > 0 {
                    overlapped.store(true, Ordering::SeqCst);
                }
                std::thread::sleep(Duration::from_millis(write_ms));
                last_len.store(v.len(), Ordering::SeqCst);
                calls.fetch_add(1, Ordering::SeqCst);
                in_flight.fetch_sub(1, Ordering::SeqCst);
            },
        );
        (e, probe)
    }

    fn add(e: &Engine, n: usize) {
        e.with_vault(|v| v.add_manual(&format!("secret-value-{n:04}"), "SECRET", "Secret", 1));
        e.persist_vault();
    }

    #[test]
    fn a_burst_of_changes_is_one_write_of_the_last_state() {
        let (e, p) = probe_engine(60, 0);
        for n in 0..40 {
            add(&e, n);
        }
        assert_eq!(p.calls.load(Ordering::SeqCst), 0, "nothing is written inside the debounce window");
        e.flush_vault();
        assert_eq!(p.calls.load(Ordering::SeqCst), 1);
        assert_eq!(p.last_len.load(Ordering::SeqCst), 40);
        // The background writer finds nothing left to do.
        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(p.calls.load(Ordering::SeqCst), 1);
        // And it starts again for the next change.
        add(&e, 99);
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(p.calls.load(Ordering::SeqCst), 2);
        assert_eq!(p.last_len.load(Ordering::SeqCst), 41);
    }

    #[test]
    fn writes_never_overlap_and_the_last_state_wins() {
        let (e, p) = probe_engine(5, 30);
        let e = Arc::new(e);
        let threads: Vec<_> = (0..4)
            .map(|t| {
                let e = e.clone();
                std::thread::spawn(move || {
                    for n in 0..25 {
                        add(&e, t * 100 + n);
                        std::thread::sleep(Duration::from_millis(2));
                    }
                })
            })
            .collect();
        for t in threads {
            t.join().unwrap();
            // A flush racing the background writer must wait for it, not overlap it.
            e.flush_vault();
        }
        e.flush_vault();
        std::thread::sleep(Duration::from_millis(150));
        assert!(!p.overlapped.load(Ordering::SeqCst), "two writes were in flight at once");
        assert_eq!(p.last_len.load(Ordering::SeqCst), 100, "the final state must be what was written last");
        assert!(p.calls.load(Ordering::SeqCst) < 100, "writes should be coalesced");
    }

    #[test]
    fn engines_without_a_sink_never_write() {
        let e = engine();
        e.with_vault(|v| v.add_manual("secret-value-0001", "SECRET", "Secret", 1));
        e.persist_vault();
        e.flush_vault();
        assert_eq!(e.vault_snapshot().len(), 1);
    }
}
