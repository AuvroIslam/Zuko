// "Open terminal" — the terminal a session is already running in, not a new one.
//
// Claude Code starts the relay (zuko-hook) from inside its own terminal, so the
// relay is a descendant of whatever hosts that terminal: WindowsTerminal.exe,
// conhost.exe, Code.exe… The app already learns the relay's pid from the pipe
// (`platform::pipe_client_pid`), so every hook event is an opportunity to note
// which window the session belongs to: walk up from the relay until a process
// owns a visible top-level window, and remember that pid for the session id.
//
// A pid, not a window handle: handles are reused once a window closes, and the
// terminal may well be minimised, moved to another virtual desktop or showing a
// different tab by the time someone clicks the button, so the window is looked up
// again at that moment.
//
// Nothing here is essential — when the walk finds nothing (the terminal has been
// closed, or the platform cannot say) the button falls back to opening the project
// folder, which is what it always did.

use std::collections::HashMap;
use std::sync::Mutex;

use tauri::{AppHandle, Manager};

use crate::platform;

/// How far above the relay the terminal may be. The real chain is short
/// (relay → claude → shell → console host → terminal); the cap is what keeps a
/// surprising tree from walking us all the way up to the desktop.
const MAX_DEPTH: usize = 12;

/// Processes that own the desktop or a service, never a terminal. Reaching one of
/// these means the chain is broken — the terminal has exited — and focusing
/// explorer.exe (every folder window, and the desktop itself) would be worse than
/// doing nothing.
const NOT_A_TERMINAL: &[&str] = &[
    "explorer.exe",
    "dwm.exe",
    "sihost.exe",
    "winlogon.exe",
    "wininit.exe",
    "services.exe",
    "svchost.exe",
    "lsass.exe",
    "csrss.exe",
    "smss.exe",
    "system",
];

/// One process, as the walk sees it.
pub struct Proc {
    /// 0 when the parent is unknown, gone, or too new to be a real parent.
    pub ppid: u32,
    /// Executable file name, lowercase.
    pub name: String,
    /// Owns a visible top-level window of its own.
    pub has_window: bool,
}

/// The nearest ancestor of `start` (or `start` itself) that owns a window.
///
/// `lookup` answers for one pid at a time: None ends the walk, so a process that
/// has already exited never sends us further up a chain that may since have been
/// rebuilt under reused pids.
pub fn host_pid(start: u32, lookup: impl Fn(u32) -> Option<Proc>) -> Option<u32> {
    let mut pid = start;
    for _ in 0..MAX_DEPTH {
        if pid == 0 {
            return None;
        }
        let proc = lookup(pid)?;
        if NOT_A_TERMINAL.contains(&proc.name.as_str()) {
            return None;
        }
        if proc.has_window {
            return Some(pid);
        }
        if proc.ppid == pid {
            return None;
        }
        pid = proc.ppid;
    }
    None
}

/// Which of a terminal's windows to bring forward.
///
/// One window is the normal case. When there are several — VS Code with two
/// projects open, two Windows Terminal windows — the session's folder name in the
/// title is the only thing that tells them apart (VS Code puts the folder there, a
/// shell usually its working directory). No match: the first window, which is at
/// least the right application.
pub fn pick_window(windows: &[(isize, String)], folder_leaf: Option<&str>) -> Option<isize> {
    let leaf = folder_leaf.map(str::to_lowercase).filter(|l| !l.is_empty());
    if let Some(leaf) = leaf {
        if let Some((hwnd, _)) = windows.iter().find(|(_, title)| title.to_lowercase().contains(&leaf)) {
            return Some(*hwnd);
        }
    }
    windows.first().map(|(hwnd, _)| *hwnd)
}

/// The terminal each session the relay has reported runs in, by session id.
#[derive(Default)]
pub struct Terminals(Mutex<HashMap<String, u32>>);

/// Notes the terminal the relay at `client_pid` was started from against
/// `session_id`.
///
/// Called for every hook event, so the common case costs one liveness check and
/// nothing else: the session is already on record and its terminal still runs.
pub fn remember(app: &AppHandle, session_id: &str, client_pid: u32) {
    if session_id.is_empty() {
        return;
    }
    let state = app.state::<Terminals>();
    let known = state.0.lock().unwrap().get(session_id).copied();
    if known.is_some_and(platform::process_alive) {
        return;
    }
    let Some(pid) = resolve(client_pid) else { return };
    let mut map = state.0.lock().unwrap();
    // Every `claude` run brings a new session id; dropping the sessions whose
    // terminal is gone keeps the map down to the terminals that are still open.
    map.retain(|_, pid| platform::process_alive(*pid));
    map.insert(session_id.to_string(), pid);
}

/// The terminal process the relay at `client_pid` runs under.
fn resolve(client_pid: u32) -> Option<u32> {
    let table = platform::process_table();
    let windowed = platform::pids_with_windows();
    let by_pid: HashMap<u32, &platform::ProcRow> = table.iter().map(|row| (row.pid, row)).collect();
    host_pid(client_pid, |pid| {
        let row = by_pid.get(&pid)?;
        // A parent that started *after* its child is not its parent: the real one
        // exited and Windows handed its pid to something else.
        let ppid = match (platform::process_created(pid), platform::process_created(row.ppid)) {
            (Some(child), Some(parent)) if parent > child => 0,
            _ => row.ppid,
        };
        Some(Proc { ppid, name: row.name.clone(), has_window: windowed.contains(&pid) })
    })
}

/// Brings the session's terminal window to the front. False when there is no such
/// window, and the caller falls back to opening the project folder.
pub fn focus(app: &AppHandle, session_id: Option<&str>, cwd: Option<&str>) -> bool {
    let Some(pid) = terminal_pid(app, session_id) else { return false };
    let windows = platform::windows_of_pid(pid);
    let leaf = cwd.map(folder_leaf);
    let Some(hwnd) = pick_window(&windows, leaf.as_deref()) else { return false };
    platform::focus_window_handle(hwnd)
}

/// The terminal recorded for this session — or, when the session is unknown (a card
/// from before the app restarted, an event that came without an id), the only
/// terminal on record, if there is exactly one.
fn terminal_pid(app: &AppHandle, session_id: Option<&str>) -> Option<u32> {
    let state = app.state::<Terminals>();
    let map = state.0.lock().unwrap();
    if let Some(pid) = session_id.filter(|s| !s.is_empty()).and_then(|s| map.get(s)).copied() {
        return platform::process_alive(pid).then_some(pid);
    }
    let mut live = map.values().copied().filter(|pid| platform::process_alive(*pid));
    let only = live.next()?;
    live.next().is_none().then_some(only)
}

/// Last path component of a folder: the part a window title carries.
fn folder_leaf(path: &str) -> String {
    let cleaned = path.trim().trim_end_matches(|c| c == '\\' || c == '/');
    match cleaned.rfind(|c| c == '\\' || c == '/') {
        Some(i) => cleaned[i + 1..].to_string(),
        None => cleaned.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `(pid, ppid, name, has_window)` rows as a lookup for [`host_pid`].
    fn table<'a>(rows: &'a [(u32, u32, &'a str, bool)]) -> impl Fn(u32) -> Option<Proc> + 'a {
        move |pid| {
            rows.iter()
                .find(|r| r.0 == pid)
                .map(|r| Proc { ppid: r.1, name: r.2.to_string(), has_window: r.3 })
        }
    }

    #[test]
    fn the_walk_stops_at_the_terminal_that_owns_a_window() {
        // Windows Terminal: the relay sits four processes below the window.
        let wt = [
            (100, 90, "zuko-hook.exe", false),
            (90, 80, "node.exe", false),
            (80, 70, "pwsh.exe", false),
            (70, 60, "openconsole.exe", false),
            (60, 10, "windowsterminal.exe", true),
            (10, 1, "explorer.exe", true),
        ];
        assert_eq!(host_pid(100, table(&wt)), Some(60));

        // VS Code's integrated terminal: the pty host has no window, the editor has.
        let code = [
            (200, 190, "zuko-hook.exe", false),
            (190, 180, "node.exe", false),
            (180, 170, "cmd.exe", false),
            (170, 160, "code.exe", false),
            (160, 10, "code.exe", true),
        ];
        assert_eq!(host_pid(200, table(&code)), Some(160));

        // A console window right above the relay wins at once.
        let conhost = [(100, 60, "zuko-hook.exe", false), (60, 10, "conhost.exe", true)];
        assert_eq!(host_pid(100, table(&conhost)), Some(60));
    }

    #[test]
    fn nothing_is_focused_when_the_terminal_is_gone() {
        // The chain ends: the parent has exited and nobody answers for it.
        assert_eq!(host_pid(100, table(&[(100, 90, "zuko-hook.exe", false)])), None);
        // It reaches the desktop instead: explorer.exe is not a terminal.
        let orphan = [(100, 10, "zuko-hook.exe", false), (10, 1, "explorer.exe", true)];
        assert_eq!(host_pid(100, table(&orphan)), None);
        // A process that claims to be its own parent, and a parent pid of 0.
        assert_eq!(host_pid(100, table(&[(100, 100, "zuko-hook.exe", false)])), None);
        assert_eq!(host_pid(100, table(&[(100, 0, "zuko-hook.exe", false)])), None);
        // A chain longer than the cap, no windows anywhere on it.
        let deep: Vec<(u32, u32, &str, bool)> = (1..=40u32).map(|p| (p, p + 1, "node.exe", false)).collect();
        assert_eq!(host_pid(1, table(&deep)), None);
    }

    #[test]
    fn the_window_with_the_session_folder_in_its_title_wins() {
        let windows = vec![
            (1isize, "other-project - Visual Studio Code".to_string()),
            (2, "Zuko - Visual Studio Code".to_string()),
        ];
        assert_eq!(pick_window(&windows, Some("Zuko")), Some(2));
        // Case does not matter; anything else falls back to the first window.
        assert_eq!(pick_window(&windows, Some("zuko")), Some(2));
        assert_eq!(pick_window(&windows, Some("elsewhere")), Some(1));
        assert_eq!(pick_window(&windows, Some("")), Some(1));
        assert_eq!(pick_window(&windows, None), Some(1));
        assert_eq!(pick_window(&[], Some("Zuko")), None);
    }

    #[test]
    fn a_folder_is_known_by_its_last_component() {
        assert_eq!(folder_leaf(r"E:\Projects\ZukoAvr\Zuko"), "Zuko");
        assert_eq!(folder_leaf(r"E:\Projects\ZukoAvr\Zuko\"), "Zuko");
        assert_eq!(folder_leaf("/home/me/zuko/"), "zuko");
        assert_eq!(folder_leaf("Zuko"), "Zuko");
        assert_eq!(folder_leaf(""), "");
    }

    /// The real walk, from the test process itself: it must answer without
    /// panicking, whatever this machine's process tree looks like.
    #[test]
    fn resolving_from_a_live_process_is_safe() {
        let _ = resolve(std::process::id());
        assert!(platform::process_alive(std::process::id()));
    }
}
