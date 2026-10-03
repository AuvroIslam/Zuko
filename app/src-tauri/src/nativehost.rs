// The browser bridge: registers the extension's native messaging host (`app.zuko.host`)
// for the current user, so the extension reaches the app without a setup step.
//
// A registration is two things (the same layout extension/scripts/register-host.mjs
// writes, which stays for development):
// * the host manifest, `<local data>\native-host\app.zuko.host.json` (%LOCALAPPDATA%\Zuko,
//   or ZUKO_DATA_DIR): the host's name, the path of the INSTALLED native host (the copy
//   hooks::ensure_hook_exe keeps in bin\, the only place the pipe accepts extension
//   messages from, see pipe.rs `classify`), `stdio`, and exactly one allowed origin, the
//   Zuko extension;
// * per browser, what tells it about that file: on Windows the default value of
//   HKCU\<browser>\NativeMessagingHosts\app.zuko.host (Chrome and Edge always, Chromium
//   and Brave when installed); on Linux a copy of the manifest in each installed
//   browser's NativeMessagingHosts folder. Never machine-wide.
//
// Registering is idempotent: only what differs is written, so the launch-time check costs
// a file read and a few registry reads. Turning the bridge off in Settings → Browser
// removes the registration and keeps it off across launches (`Settings.browserBridge`).
//
// The browsers are reached through `HostStore`, so the tests drive a fake one and never
// touch the real registry or browser folders.

use std::io;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::json;

use crate::platform;

/// The name the extension connects to (`chrome.runtime.connectNative`).
pub const HOST_NAME: &str = "app.zuko.host";

/// The extension's ID, pinned by the public `key` in extension/manifest.json (derived the
/// way Chrome does it by extension/scripts/ext-id.mjs; the extension's tests check it).
pub const EXTENSION_ID: &str = "cbdnjagdcfchakoejiahgeclappplcba";

const DESCRIPTION: &str = "Zuko: browser extension to desktop app bridge";

/// A browser that can start the host.
#[derive(Clone, Copy, Debug)]
pub struct Browser {
    pub name: &'static str,
    /// Windows: its key under HKCU; Linux: its folder under ~/.config.
    pub home: &'static str,
    /// Registered even when it does not look installed (the extension's targets).
    pub always: bool,
}

pub fn browsers() -> Vec<Browser> {
    platform::NATIVE_MESSAGING_BROWSERS
        .iter()
        .map(|&(name, home, always)| Browser { name, home, always })
        .collect()
}

/// Where the browsers learn about the host.
pub trait HostStore {
    /// The browser has settings for this user.
    fn installed(&self, b: &Browser) -> bool;
    /// What the browser holds for the host now (see [`wanted`]).
    fn get(&self, b: &Browser) -> Option<String>;
    fn set(&self, b: &Browser, value: &str) -> io::Result<()>;
    /// Removes the browser's registration of the host; already gone is fine.
    fn remove(&self, b: &Browser) -> io::Result<()>;
}

/// What a registered browser holds: the manifest's path on Windows, its text on Linux.
pub fn wanted(manifest_path: &Path, manifest_json: &str) -> String {
    if platform::NATIVE_MESSAGING_BY_PATH {
        manifest_path.to_string_lossy().into_owned()
    } else {
        manifest_json.to_string()
    }
}

/// The host manifest, byte for byte what extension/scripts/register-host.mjs writes.
pub fn manifest_json(host_exe: &Path) -> String {
    let manifest = json!({
        "name": HOST_NAME,
        "description": DESCRIPTION,
        "path": host_exe.to_string_lossy(),
        "type": "stdio",
        "allowed_origins": [format!("chrome-extension://{EXTENSION_ID}/")],
    });
    let mut text = serde_json::to_string_pretty(&manifest).unwrap_or_default();
    text.push('\n');
    text
}

/// `<local data>/native-host/app.zuko.host.json`.
pub fn manifest_path() -> PathBuf {
    crate::settings::local_dir().join("native-host").join(format!("{HOST_NAME}.json"))
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserStatus {
    pub name: String,
    pub installed: bool,
    pub registered: bool,
}

/// Settings → Browser (CONTRACTS.md `NativeHostStatus`).
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeHostStatus {
    /// The manifest is right and every browser that should know the host does.
    pub registered: bool,
    /// The bridge is on: Zuko registers it at every launch (`Settings.browserBridge`).
    pub enabled: bool,
    pub browsers: Vec<BrowserStatus>,
    pub manifest_path: String,
    pub host_path: String,
    /// The native host is installed where the manifest points.
    pub host_present: bool,
    pub extension_id: String,
    /// The last register / unregister failure, if any.
    pub error: Option<String>,
}

/// Brings the manifest and every browser in line, writing only what differs. Returns
/// the browsers that had to be (re)registered.
pub fn register(store: &dyn HostStore, manifest: &Path, host_exe: &Path) -> io::Result<Vec<&'static str>> {
    let json = manifest_json(host_exe);
    if std::fs::read_to_string(manifest).ok().as_deref() != Some(json.as_str()) {
        crate::files::write_atomic(manifest, json.as_bytes())?;
    }
    let value = wanted(manifest, &json);
    let mut written = Vec::new();
    let mut failed: Option<io::Error> = None;
    for b in browsers().iter().filter(|b| b.always || store.installed(b)) {
        if store.get(b).as_deref() == Some(value.as_str()) {
            continue;
        }
        match store.set(b, &value) {
            Ok(()) => written.push(b.name),
            Err(e) => {
                failed.get_or_insert(io::Error::new(e.kind(), format!("{}: {e}", b.name)));
            }
        }
    }
    match failed {
        Some(e) => Err(e),
        None => Ok(written),
    }
}

/// Removes every browser's registration of the host and the manifest.
pub fn unregister(store: &dyn HostStore, manifest: &Path) -> io::Result<()> {
    let mut failed: Option<io::Error> = None;
    for b in browsers() {
        if store.get(&b).is_some() {
            if let Err(e) = store.remove(&b) {
                failed.get_or_insert(io::Error::new(e.kind(), format!("{}: {e}", b.name)));
            }
        }
    }
    match std::fs::remove_file(manifest) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => {
            failed.get_or_insert(e);
        }
        _ => {}
    }
    match failed {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

/// Where things stand. `enabled` and `error` are the caller's to fill in.
pub fn status(store: &dyn HostStore, manifest: &Path, host_exe: &Path) -> NativeHostStatus {
    let json = manifest_json(host_exe);
    let manifest_ok = std::fs::read_to_string(manifest).ok().as_deref() == Some(json.as_str());
    let value = wanted(manifest, &json);
    let mut expected_all = true;
    let mut any = false;
    let browsers: Vec<BrowserStatus> = browsers()
        .iter()
        .map(|b| {
            let installed = store.installed(b);
            let registered = store.get(b).as_deref() == Some(value.as_str());
            if b.always || installed {
                expected_all &= registered;
            }
            any |= registered;
            BrowserStatus { name: b.name.to_string(), installed, registered }
        })
        .collect();
    NativeHostStatus {
        registered: manifest_ok && any && expected_all,
        enabled: false,
        browsers,
        manifest_path: manifest.to_string_lossy().into_owned(),
        host_path: host_exe.to_string_lossy().into_owned(),
        host_present: host_exe.is_file(),
        extension_id: EXTENSION_ID.to_string(),
        error: None,
    }
}

// ── The real browsers ─────────────────────────────────────────────────────────

/// The current user's browsers, through `platform::native_messaging_*` (HKCU on Windows,
/// ~/.config on Linux).
struct SystemStore;

impl SystemStore {
    /// Unit tests drive a fake store; reaching the real one from a test is a bug.
    fn guard() -> io::Result<()> {
        if cfg!(test) {
            return Err(io::Error::other("unit tests must not change the real browser registration"));
        }
        Ok(())
    }
}

impl HostStore for SystemStore {
    fn installed(&self, b: &Browser) -> bool {
        platform::native_messaging_browser_present(b.home)
    }
    fn get(&self, b: &Browser) -> Option<String> {
        platform::native_messaging_read(b.home, HOST_NAME)
    }
    fn set(&self, b: &Browser, value: &str) -> io::Result<()> {
        SystemStore::guard()?;
        platform::native_messaging_write(b.home, HOST_NAME, value)
    }
    fn remove(&self, b: &Browser) -> io::Result<()> {
        SystemStore::guard()?;
        platform::native_messaging_remove(b.home, HOST_NAME)
    }
}

/// At launch, with the bridge on: register for this user. A development run with its data
/// folder redirected leaves the registration alone, since it is per user and pointing the
/// browsers at a temporary folder would cut off the installed app's bridge.
pub fn ensure_at_startup(enabled: bool) {
    if !enabled {
        return;
    }
    if crate::settings::dirs_overridden() {
        crate::log::line("browser bridge: not registered by a development run (data folder redirected)");
        return;
    }
    match register(&SystemStore, &manifest_path(), &crate::settings::native_host_exe_path()) {
        Ok(written) if !written.is_empty() => {
            crate::log::line(format!("browser bridge registered for {}", written.join(", ")));
        }
        Ok(_) => {}
        Err(e) => crate::log::line(format!("browser bridge not registered: {e}")),
    }
}

/// Settings → Browser: register now.
pub fn register_now() -> io::Result<Vec<&'static str>> {
    register(&SystemStore, &manifest_path(), &crate::settings::native_host_exe_path())
}

/// Settings → Browser: unregister now.
pub fn unregister_now() -> io::Result<()> {
    unregister(&SystemStore, &manifest_path())
}

/// Settings → Browser: where things stand.
pub fn status_now(enabled: bool) -> NativeHostStatus {
    NativeHostStatus { enabled, ..status(&SystemStore, &manifest_path(), &crate::settings::native_host_exe_path()) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::HashMap;

    /// Browsers in memory: which are "installed", what each holds, how often it was written.
    #[derive(Default)]
    struct Fake {
        installed: Vec<&'static str>,
        held: RefCell<HashMap<&'static str, String>>,
        writes: RefCell<u32>,
        broken: Option<&'static str>,
    }

    impl HostStore for Fake {
        fn installed(&self, b: &Browser) -> bool {
            self.installed.contains(&b.name)
        }
        fn get(&self, b: &Browser) -> Option<String> {
            self.held.borrow().get(b.name).cloned()
        }
        fn set(&self, b: &Browser, value: &str) -> io::Result<()> {
            if self.broken == Some(b.name) {
                return Err(io::Error::new(io::ErrorKind::PermissionDenied, "access denied"));
            }
            *self.writes.borrow_mut() += 1;
            self.held.borrow_mut().insert(b.name, value.to_string());
            Ok(())
        }
        fn remove(&self, b: &Browser) -> io::Result<()> {
            self.held.borrow_mut().remove(b.name);
            Ok(())
        }
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("zuko-nh-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// The browsers registered no matter what (Chrome and Edge on Windows, none on Linux).
    fn always() -> Vec<&'static str> {
        browsers().iter().filter(|b| b.always).map(|b| b.name).collect()
    }

    #[test]
    fn the_manifest_names_the_installed_host_and_only_this_extension() {
        let host = Path::new(r"C:\Users\me\AppData\Local\Zuko\bin\zuko-native-host.exe");
        let text = manifest_json(host);
        let m: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(m["name"], HOST_NAME);
        assert_eq!(m["type"], "stdio");
        assert_eq!(m["path"], host.to_string_lossy().as_ref());
        assert_eq!(m["allowed_origins"], json!(["chrome-extension://cbdnjagdcfchakoejiahgeclappplcba/"]));
        // Same order and layout as JSON.stringify(manifest, null, 2) in register-host.mjs.
        let keys: Vec<&str> = text.lines().filter_map(|l| l.trim().strip_prefix('"')?.split('"').next()).take(5).collect();
        assert_eq!(keys, ["name", "description", "path", "type", "allowed_origins"]);
        assert!(text.ends_with("]\n}\n") && text.starts_with("{\n  \"name\""));
        // The pinned ID is the one extension/manifest.json's key derives to.
        assert!(EXTENSION_ID.len() == 32 && EXTENSION_ID.bytes().all(|c| (b'a'..=b'p').contains(&c)));
        let ext_manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../extension/scripts/register-host.mjs");
        if let Ok(script) = std::fs::read_to_string(ext_manifest) {
            assert!(script.contains(&format!("\"{DESCRIPTION}\"")), "description in step with register-host.mjs");
        }
    }

    #[test]
    fn registering_writes_once_then_only_what_drifted() {
        let dir = scratch("register");
        let manifest = dir.join("native-host").join("app.zuko.host.json");
        let host = dir.join("bin").join("zuko-native-host.exe");
        // Brave installed, Chromium not.
        let store = Fake { installed: vec!["Brave"], ..Default::default() };

        let written = register(&store, &manifest, &host).unwrap();
        let mut expect = always();
        expect.push("Brave");
        assert_eq!(written, expect);
        assert_eq!(std::fs::read_to_string(&manifest).unwrap(), manifest_json(&host));
        let value = wanted(&manifest, &manifest_json(&host));
        assert_eq!(store.get(&browsers().into_iter().find(|b| b.name == "Brave").unwrap()), Some(value.clone()));
        assert!(store.held.borrow().get("Chromium").is_none(), "a browser that is not there is left alone");

        // Nothing differs: nothing is written, the manifest file is not even touched.
        let before = std::fs::metadata(&manifest).unwrap().modified().unwrap();
        let writes = *store.writes.borrow();
        assert!(register(&store, &manifest, &host).unwrap().is_empty());
        assert_eq!(*store.writes.borrow(), writes);
        assert_eq!(std::fs::metadata(&manifest).unwrap().modified().unwrap(), before);
        let s = status(&store, &manifest, &host);
        assert!(s.registered, "{s:?}");
        assert!(!s.host_present, "the host binary is not in this scratch folder");

        // One browser pointed elsewhere (an old dev registration): only it is fixed.
        store.held.borrow_mut().insert("Brave", "C:\\elsewhere\\app.zuko.host.json".into());
        assert!(!status(&store, &manifest, &host).registered);
        assert_eq!(register(&store, &manifest, &host).unwrap(), vec!["Brave"]);
        // An edited manifest is rewritten.
        std::fs::write(&manifest, "{}").unwrap();
        register(&store, &manifest, &host).unwrap();
        assert_eq!(std::fs::read_to_string(&manifest).unwrap(), manifest_json(&host));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unregistering_removes_every_trace_and_status_says_so() {
        let dir = scratch("unregister");
        let manifest = dir.join("app.zuko.host.json");
        let host = dir.join("zuko-native-host.exe");
        std::fs::write(&host, b"").unwrap();
        let store = Fake { installed: vec!["Chrome", "Edge", "Brave"], ..Default::default() };
        register(&store, &manifest, &host).unwrap();
        let s = status(&store, &manifest, &host);
        assert!(s.registered && s.host_present);
        assert_eq!(s.extension_id, EXTENSION_ID);
        assert!(s.browsers.iter().filter(|b| b.name != "Chromium").all(|b| b.registered), "{s:?}");

        unregister(&store, &manifest).unwrap();
        assert!(store.held.borrow().is_empty());
        assert!(!manifest.exists());
        let s = status(&store, &manifest, &host);
        assert!(!s.registered && s.browsers.iter().all(|b| !b.registered));
        // Twice is fine.
        unregister(&store, &manifest).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_browser_that_refuses_is_reported_and_the_others_still_registered() {
        let dir = scratch("refuse");
        let manifest = dir.join("app.zuko.host.json");
        let host = dir.join("zuko-native-host.exe");
        let store = Fake { installed: vec!["Chrome", "Edge", "Chromium"], broken: Some("Edge"), ..Default::default() };
        let err = register(&store, &manifest, &host).unwrap_err();
        assert!(err.to_string().contains("Edge"), "{err}");
        assert!(store.held.borrow().contains_key("Chrome") && store.held.borrow().contains_key("Chromium"));
        assert!(!status(&store, &manifest, &host).registered);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_real_browsers_are_out_of_reach_of_tests() {
        let b = browsers()[0];
        assert!(SystemStore.set(&b, "x").is_err());
        assert!(SystemStore.remove(&b).is_err());
    }
}
