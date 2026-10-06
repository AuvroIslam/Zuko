// Windows: Win32 for the island window and the cursor, %APPDATA% for files.

use std::collections::HashSet;
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::Command;

use tauri::{AppHandle, Manager, WebviewWindow};

use ::windows::core::{BOOL, PCWSTR, PWSTR};
use ::windows::Win32::Foundation::{
    CloseHandle, ERROR_FILE_NOT_FOUND, ERROR_SUCCESS, FILETIME, HANDLE, HLOCAL, HWND, LPARAM, LocalFree, POINT,
    WIN32_ERROR,
};
use ::windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use ::windows::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegDeleteTreeW, RegGetValueW, RegOpenKeyExW, RegSetValueExW, HKEY, HKEY_CURRENT_USER,
    KEY_READ, KEY_SET_VALUE, REG_OPTION_NON_VOLATILE, REG_SZ, RRF_RT_REG_SZ,
};
use ::windows::Win32::Security::Authorization::ConvertSidToStringSidW;
use ::windows::Win32::Security::{GetTokenInformation, TokenUser, TOKEN_QUERY, TOKEN_USER};
use ::windows::Win32::System::SystemInformation::GetLocalTime;
use ::windows::Win32::System::Pipes::GetNamedPipeClientProcessId;
use ::windows::Win32::System::Threading::{
    AttachThreadInput, GetCurrentProcess, GetExitCodeProcess, GetProcessTimes, OpenProcess, OpenProcessToken,
    QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};
use ::windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VIRTUAL_KEY, VK_LBUTTON, VK_RBUTTON};
use ::windows::Win32::UI::WindowsAndMessaging::{
    BringWindowToTop, EnumWindows, GetCursorPos, GetForegroundWindow, GetWindow,
    GetWindowLongPtrW, GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId, IsIconic, IsWindow,
    IsWindowVisible, SetForegroundWindow, SetWindowLongPtrW, ShowWindow, SwitchToThisWindow, GWL_EXSTYLE, GW_OWNER,
    SW_RESTORE, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
};

use super::{LocalTime, ProcRow};
use crate::island::WINDOW_LABEL;

/// File name of the Claude Code relay.
pub const HOOK_EXE: &str = "zuko-hook.exe";

/// File name of the browser extension's native messaging host.
pub const NATIVE_HOST_EXE: &str = "zuko-native-host.exe";

/// Environment variable holding the home directory.
pub const HOME_VAR: &str = "USERPROFILE";

/// Keeps spawned helpers from flashing a console window.
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

// ── Files ─────────────────────────────────────────────────────────────────────

/// %APPDATA%\Zuko — preferences (or `ZUKO_CONFIG_DIR`, see `dir_override`).
pub fn config_dir() -> PathBuf {
    if let Some(dir) = super::dir_override("ZUKO_CONFIG_DIR") {
        return dir;
    }
    let base = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("Zuko")
}

/// %LOCALAPPDATA%\Zuko — where zuko-hook.exe, the inbox and the log live (or
/// `ZUKO_DATA_DIR`, see `dir_override`).
pub fn local_dir() -> PathBuf {
    if let Some(dir) = super::dir_override("ZUKO_DATA_DIR") {
        return dir;
    }
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("Zuko")
}

/// %APPDATA% and %LOCALAPPDATA% are already private to the user.
pub fn ensure_private_dir(dir: &std::path::Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)
}

/// Nothing to set up before the webview starts.
pub fn prepare_environment() {}

pub fn local_time() -> LocalTime {
    let t = unsafe { GetLocalTime() };
    LocalTime {
        year: t.wYear.into(),
        month: t.wMonth.into(),
        day: t.wDay.into(),
        hour: t.wHour.into(),
        minute: t.wMinute.into(),
        second: t.wSecond.into(),
    }
}

// ── Processes ─────────────────────────────────────────────────────────────────

/// Spawned helpers must never flash a console window.
pub fn no_console(cmd: &mut Command) -> &mut Command {
    cmd.creation_flags(CREATE_NO_WINDOW)
}

pub fn open_url(url: &str) {
    let _ = no_console(Command::new("rundll32.exe").args(["url.dll,FileProtocolHandler", url]))
        .spawn();
}

pub fn reveal_folder(path: &str) {
    let _ = Command::new("explorer").arg(path).spawn();
}

/// Our own `where`: walks %PATH% against %PATHEXT%, no shell involved.
/// Rust quotes arguments correctly for `.cmd`/`.bat` targets since 1.77, so
/// spawning `code.cmd` directly is safe.
pub fn find_on_path(stem: &str) -> Option<PathBuf> {
    let exts = std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into());
    let dirs = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&dirs) {
        for ext in exts.split(';').filter(|e| !e.is_empty()) {
            let candidate = dir.join(format!("{stem}{}", ext.to_lowercase()));
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

// ── Who we are ────────────────────────────────────────────────────────────────
//
// Named pipes share one machine-wide namespace, so the SID in the name is what
// keeps two accounts on the same machine from ever meeting on `zuko-*`.
// zuko-hook computes the same string (hook/src/win.rs) and additionally checks
// that the process serving the pipe really is us.

/// The SID of the account this process runs as, as `S-1-5-21-…`.
pub fn current_user_sid() -> Option<String> {
    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).ok()?;

        // First call sizes the buffer, second fills it.
        let mut needed = 0u32;
        let _ = GetTokenInformation(token, TokenUser, None, 0, &mut needed);
        if needed == 0 {
            let _ = CloseHandle(token);
            return None;
        }
        let mut buf = vec![0u8; needed as usize];
        let ok = GetTokenInformation(
            token,
            TokenUser,
            Some(buf.as_mut_ptr().cast()),
            needed,
            &mut needed,
        )
        .is_ok();
        let _ = CloseHandle(token);
        if !ok {
            return None;
        }

        let user = &*(buf.as_ptr() as *const TOKEN_USER);
        let mut text = PWSTR::null();
        ConvertSidToStringSidW(user.User.Sid, &mut text).ok()?;
        let sid = text.to_string().ok();
        let _ = LocalFree(Some(HLOCAL(text.0 as *mut _)));
        sid
    }
}

// ── Cursor ────────────────────────────────────────────────────────────────────

/// The 60 Hz poll reads the cursor and flips click-through from it.
pub const CURSOR_POLL: bool = true;

/// Cursor position in physical screen pixels.
pub fn cursor_physical() -> Option<(f64, f64)> {
    let mut p = POINT::default();
    unsafe { GetCursorPos(&mut p).ok()? };
    Some((p.x as f64, p.y as f64))
}

/// True while a mouse button that can drag is held — the only signal we get that a
/// drag might be in flight before it reaches the window. Both buttons: Explorer drags
/// with either, and GetAsyncKeyState reads the physical buttons, so with the buttons
/// swapped a plain drag holds the physical right one.
pub fn drag_button_down() -> bool {
    let down = |vk: VIRTUAL_KEY| unsafe { (GetAsyncKeyState(vk.0 as i32) as u16 & 0x8000) != 0 };
    down(VK_LBUTTON) || down(VK_RBUTTON)
}

// ── Island window ─────────────────────────────────────────────────────────────

fn hwnd_of(win: &WebviewWindow) -> Option<HWND> {
    let raw = win.hwnd().ok()?.0 as isize;
    if raw == 0 {
        return None;
    }
    Some(HWND(raw as *mut _))
}

/// Makes dropped files reach the app: Zuko's own drop target on each window and on
/// every WebView2 window inside it (see `win_drop`).
///
/// Cheap and idempotent, so it is simply re-run whenever a drag might be starting.
pub fn claim_file_drops(app: &AppHandle) {
    for label in [WINDOW_LABEL, "settings"] {
        let Some(win) = app.get_webview_window(label) else { continue };
        let Some(hwnd) = hwnd_of(&win) else { continue };
        super::win_drop::claim(app, label, hwnd);
    }
}

/// WS_EX_NOACTIVATE keeps clicks from stealing focus; WS_EX_TOOLWINDOW keeps the
/// island out of Alt-Tab.
pub fn make_non_activating(win: &WebviewWindow) {
    let Some(hwnd) = hwnd_of(win) else { return };
    unsafe {
        let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        let want = ex | WS_EX_NOACTIVATE.0 as isize | WS_EX_TOOLWINDOW.0 as isize;
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, want);
    }
}

/// Temporarily allow activation so a text field inside the island can be typed in.
pub fn set_activating(win: &WebviewWindow, activating: bool) {
    let Some(hwnd) = hwnd_of(win) else { return };
    unsafe {
        let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        let want = if activating {
            ex & !(WS_EX_NOACTIVATE.0 as isize)
        } else {
            ex | WS_EX_NOACTIVATE.0 as isize
        };
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, want);
    }
}

/// Click-through here is the poll's WS_EX_TRANSPARENT toggle, not a region.
pub fn set_input_region(_win: &WebviewWindow, _rect: Option<(f64, f64, f64, f64)>) {}

// ── Browser native messaging (HKCU only) ─────────────────────────────────────
//
// Chrome-family browsers find a native messaging host through the default value of
// HKCU\<browser>\NativeMessagingHosts\<host name>, which names the host's manifest file.
// Only the current user's hive is ever read or written here.

/// Browsers that can start the extension's native host: name, where the browser keeps
/// its settings under HKCU, and whether to register it even when that key is missing.
/// Chrome and Edge are the extension's targets and are always registered (a browser that
/// has not run yet has no key); Chromium and Brave only when they are there.
pub const NATIVE_MESSAGING_BROWSERS: &[(&str, &str, bool)] = &[
    ("Chrome", r"Software\Google\Chrome", true),
    ("Edge", r"Software\Microsoft\Edge", true),
    ("Chromium", r"Software\Chromium", false),
    ("Brave", r"Software\BraveSoftware\Brave-Browser", false),
];

/// Windows browsers are told where the manifest file is (Linux ones get a copy of it).
pub const NATIVE_MESSAGING_BY_PATH: bool = true;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn win32(r: WIN32_ERROR) -> std::io::Result<()> {
    if r == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(std::io::Error::from_raw_os_error(r.0 as i32))
    }
}

fn host_key(browser_home: &str, host: &str) -> Vec<u16> {
    wide(&format!(r"{browser_home}\NativeMessagingHosts\{host}"))
}

/// The browser has settings for this user (it is installed and has run).
pub fn native_messaging_browser_present(browser_home: &str) -> bool {
    let key = wide(browser_home);
    let mut hkey = HKEY::default();
    // SAFETY: a read-only open of a key in the user's own hive; closed right away.
    let r = unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, PCWSTR(key.as_ptr()), None, KEY_READ, &mut hkey) };
    if r != ERROR_SUCCESS {
        return false;
    }
    unsafe {
        let _ = RegCloseKey(hkey);
    }
    true
}

/// What the browser is told about `host` now: the manifest path in the key's default value.
pub fn native_messaging_read(browser_home: &str, host: &str) -> Option<String> {
    let key = host_key(browser_home, host);
    let mut bytes = 0u32;
    // SAFETY: first call sizes the buffer, second fills it; both read the user's own hive.
    unsafe { win32(RegGetValueW(HKEY_CURRENT_USER, PCWSTR(key.as_ptr()), PCWSTR::null(), RRF_RT_REG_SZ, None, None, Some(&mut bytes))) }.ok()?;
    let mut buf = vec![0u16; (bytes as usize).div_ceil(2).max(1)];
    let mut bytes = (buf.len() * 2) as u32;
    unsafe {
        win32(RegGetValueW(
            HKEY_CURRENT_USER,
            PCWSTR(key.as_ptr()),
            PCWSTR::null(),
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr().cast()),
            Some(&mut bytes),
        ))
    }
    .ok()?;
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    Some(String::from_utf16_lossy(&buf[..len]))
}

/// Points the browser at the manifest: creates the key and sets its default value.
pub fn native_messaging_write(browser_home: &str, host: &str, manifest_path: &str) -> std::io::Result<()> {
    let key = host_key(browser_home, host);
    let mut hkey = HKEY::default();
    // SAFETY: creates (or opens) a key under the user's own hive for writing; closed below.
    unsafe {
        win32(RegCreateKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(key.as_ptr()),
            None,
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE,
            None,
            &mut hkey,
            None,
        ))?;
    }
    let data = wide(manifest_path);
    // REG_SZ data is the UTF-16 string with its terminating NUL, as bytes.
    let bytes: Vec<u8> = data.iter().flat_map(|c| c.to_le_bytes()).collect();
    let set = unsafe { win32(RegSetValueExW(hkey, PCWSTR::null(), None, REG_SZ, Some(&bytes))) };
    unsafe {
        let _ = RegCloseKey(hkey);
    }
    set
}

/// Removes the browser's registration of `host` (nothing else). Already gone is fine.
pub fn native_messaging_remove(browser_home: &str, host: &str) -> std::io::Result<()> {
    let key = host_key(browser_home, host);
    // SAFETY: deletes only HKCU\<browser>\NativeMessagingHosts\<host> and what is under it.
    let r = unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, PCWSTR(key.as_ptr())) };
    if r == ERROR_FILE_NOT_FOUND {
        return Ok(());
    }
    win32(r)
}

/// Full path of the executable on the other end of a connected pipe instance, or
/// None if Windows will not say. Used to accept hook events only from Zuko's own
/// relay and extension messages only from Zuko's own native host.
pub fn pipe_client_exe(pipe: std::os::windows::io::RawHandle) -> Option<PathBuf> {
    let pid = pipe_client_pid(pipe)?;
    // SAFETY: plain query on a process we only read the image name of.
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.ok()?;
    let mut buf = vec![0u16; 32_768];
    let mut len = buf.len() as u32;
    let ok = unsafe { QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len) };
    unsafe {
        let _ = CloseHandle(process);
    }
    ok.ok()?;
    Some(PathBuf::from(String::from_utf16_lossy(&buf[..len as usize])))
}

/// Process id on the other end of a connected pipe instance. The relay's pid: the
/// terminal it was started in is somewhere above it (see `terminal.rs`).
pub fn pipe_client_pid(pipe: std::os::windows::io::RawHandle) -> Option<u32> {
    let mut pid = 0u32;
    // SAFETY: `pipe` is a live, connected pipe instance owned by the caller.
    unsafe { GetNamedPipeClientProcessId(HANDLE(pipe as _), &mut pid) }.ok()?;
    (pid != 0).then_some(pid)
}

// ── Finding the terminal a session runs in ───────────────────────────────────
//
// Three cheap reads, all of them snapshots of a moment: the process tree, which
// processes own a window, and when a process started. terminal.rs puts them
// together; nothing here holds a handle or changes anything.

/// Every process with its parent, from one Toolhelp snapshot.
pub fn process_table() -> Vec<ProcRow> {
    let mut rows = Vec::new();
    // SAFETY: the snapshot handle is closed before returning, and every call only
    // reads from it. A failed snapshot (a transient ERROR_BAD_LENGTH) is no table.
    unsafe {
        let Ok(snapshot) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else { return rows };
        let mut entry = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        if Process32FirstW(snapshot, &mut entry).is_ok() {
            loop {
                let name = &entry.szExeFile;
                let len = name.iter().position(|c| *c == 0).unwrap_or(name.len());
                rows.push(ProcRow {
                    pid: entry.th32ProcessID,
                    ppid: entry.th32ParentProcessID,
                    name: String::from_utf16_lossy(&name[..len]).to_lowercase(),
                });
                if Process32NextW(snapshot, &mut entry).is_err() {
                    break;
                }
            }
        }
        let _ = CloseHandle(snapshot);
    }
    rows
}

/// Processes that own at least one window a person can click on.
pub fn pids_with_windows() -> HashSet<u32> {
    let mut pids: HashSet<u32> = HashSet::new();
    // SAFETY: EnumWindows calls `collect_pid` on this thread with our own pointer,
    // and does not keep it after the call returns.
    unsafe {
        let _ = EnumWindows(Some(collect_pid), LPARAM(&mut pids as *mut HashSet<u32> as isize));
    }
    pids
}

unsafe extern "system" fn collect_pid(hwnd: HWND, param: LPARAM) -> BOOL {
    if unsafe { is_user_window(hwnd) } {
        let mut pid = 0u32;
        unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
        if pid != 0 {
            unsafe { &mut *(param.0 as *mut HashSet<u32>) }.insert(pid);
        }
    }
    true.into()
}

/// Windows of one process, each with its title — what tells two VS Code windows
/// (or two terminal windows) apart.
pub fn windows_of_pid(pid: u32) -> Vec<(isize, String)> {
    let mut found = Titled { pid, windows: Vec::new() };
    // SAFETY: as above — the pointer is only used for the duration of the call.
    unsafe {
        let _ = EnumWindows(Some(collect_titled), LPARAM(&mut found as *mut Titled as isize));
    }
    found.windows
}

struct Titled {
    pid: u32,
    windows: Vec<(isize, String)>,
}

unsafe extern "system" fn collect_titled(hwnd: HWND, param: LPARAM) -> BOOL {
    let found = unsafe { &mut *(param.0 as *mut Titled) };
    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    if pid == found.pid && unsafe { is_user_window(hwnd) } {
        let mut text = [0u16; 512];
        let len = unsafe { GetWindowTextW(hwnd, &mut text) };
        let title = String::from_utf16_lossy(&text[..len.max(0) as usize]);
        found.windows.push((hwnd.0 as isize, title));
    }
    true.into()
}

/// A window somebody could switch to: visible, top-level (owned windows are
/// dialogs and tooltips) and carrying a title.
unsafe fn is_user_window(hwnd: HWND) -> bool {
    unsafe {
        IsWindowVisible(hwnd).as_bool()
            && GetWindow(hwnd, GW_OWNER).is_err()
            && GetWindowTextLengthW(hwnd) > 0
    }
}

/// When a process started, in 100 ns ticks. Comparable between processes, which is
/// how a parent pid that has since been reused is spotted.
pub fn process_created(pid: u32) -> Option<u64> {
    if pid == 0 {
        return None;
    }
    // SAFETY: a query-only handle, closed before returning; the times are plain out
    // parameters we own.
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut created = FILETIME::default();
        let (mut exit, mut kernel, mut user) = (FILETIME::default(), FILETIME::default(), FILETIME::default());
        let ok = GetProcessTimes(process, &mut created, &mut exit, &mut kernel, &mut user);
        let _ = CloseHandle(process);
        ok.ok()?;
        Some(((created.dwHighDateTime as u64) << 32) | created.dwLowDateTime as u64)
    }
}

/// True while `pid` is a process that is still running (not merely one whose pid
/// we can still open).
pub fn process_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    // SAFETY: a query-only handle, closed before returning.
    unsafe {
        let Ok(process) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else { return false };
        let mut code = 0u32;
        let ok = GetExitCodeProcess(process, &mut code).is_ok();
        let _ = CloseHandle(process);
        ok && code == STILL_RUNNING
    }
}

/// `GetExitCodeProcess` reports this while the process has not exited.
const STILL_RUNNING: u32 = 259;

/// Brings a window to the front, un-minimising it first.
///
/// Windows only lets the foreground process hand the foreground to someone else,
/// and the island is WS_EX_NOACTIVATE — Zuko is never it. So `SetForegroundWindow`
/// is likely to be refused (it flashes the taskbar button instead) and there are
/// two more tries behind it: `SwitchToThisWindow`, the switch Alt-Tab itself uses,
/// and then borrowing the foreground window's input queue, which makes the call
/// come from a thread that is allowed to make it. Each one is checked rather than
/// assumed, and `false` means the window stayed where it was.
pub fn focus_window_handle(handle: isize) -> bool {
    let hwnd = HWND(handle as *mut _);
    // SAFETY: every call takes a window handle and nothing else; a handle that has
    // gone stale makes them fail, which is the `false` below.
    unsafe {
        if !IsWindow(Some(hwnd)).as_bool() {
            return false;
        }
        if IsIconic(hwnd).as_bool() {
            let _ = ShowWindow(hwnd, SW_RESTORE);
        }
        if SetForegroundWindow(hwnd).as_bool() || GetForegroundWindow() == hwnd {
            return true;
        }
        let _ = BringWindowToTop(hwnd);
        SwitchToThisWindow(hwnd, true);
        if GetForegroundWindow() == hwnd {
            return true;
        }
        // Last resort: the thread that owns the foreground window may do this, so
        // attach to it for the one call and let go again straight away.
        let ours = GetWindowThreadProcessId(GetForegroundWindow(), None);
        let theirs = GetWindowThreadProcessId(hwnd, None);
        if ours != 0 && theirs != 0 && ours != theirs {
            let attached = AttachThreadInput(ours, theirs, true).as_bool();
            let _ = SetForegroundWindow(hwnd);
            if attached {
                let _ = AttachThreadInput(ours, theirs, false);
            }
        }
        GetForegroundWindow() == hwnd
    }
}
