// Zuko for Windows — app wiring and the commands the island calls.
//
// Plugins: single-instance, autostart, clipboard-manager and global-shortcut. The last
// two are used from Rust only (the clipboard hotkeys in commands.rs), so no capability
// grants the webview direct access to the clipboard or to shortcut registration.

mod auditlog;
mod chat;
mod clipcopy;
mod commands;
mod engine;
mod events;
mod files;
mod firewall;
mod gateway;
mod hooks;
mod island;
mod localai;
mod log;
mod nativehost;
mod pipe;
mod platform;
mod policystore;
mod sanitize;
mod secrets;
mod settings;
mod terminal;
mod tray;
mod vaultstore;
mod vscode;

use std::process::Command;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State, WebviewUrl, WebviewWindowBuilder};
use tauri_plugin_autostart::{ManagerExt, MacosLauncher};

use chat::{Chat, ChatContext, ChatModels, ChatReply, ChatStatus, Provider, Target};
use engine::Engine;
use events::PrivacyNote;
use files::DroppedFile;
use hooks::{HookPreview, HookStatus};
use island::{PollGate, ScreenInfo};
use pipe::Pending;
use settings::Settings;
use terminal::Terminals;

pub struct Shared {
    pub settings: Mutex<Settings>,
    pub gate: Arc<PollGate>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BootInfo {
    settings: Settings,
    screen: ScreenInfo,
    version: String,
    hook_path: String,
    /// False where the OS has no global cursor (Wayland): the page then reports
    /// the cursor from its own mouse events.
    cursor_poll: bool,
}

#[tauri::command]
fn boot(app: AppHandle, shared: State<Shared>) -> BootInfo {
    let mut settings = shared.settings.lock().unwrap().clone();
    // The real state of ~/.claude/settings.json wins over whatever we stored.
    settings.hooks_installed = hooks::status().installed;
    let screen = island::screen_info(&app, &settings.screen);
    BootInfo {
        settings,
        screen,
        version: env!("CARGO_PKG_VERSION").to_string(),
        hook_path: settings::hook_exe_path().to_string_lossy().to_string(),
        cursor_poll: platform::CURSOR_POLL,
    }
}

#[tauri::command]
fn save_settings(app: AppHandle, shared: State<Shared>, mut settings: Settings) {
    let (screen_changed, autostart_changed) = {
        let mut current = shared.settings.lock().unwrap();
        // The island's place only changes through island_drag and reset_island_position:
        // a window that has not heard of the last drag yet must not move it back.
        settings.island_offset = current.island_offset;
        let screen_changed = current.screen != settings.screen;
        let autostart_changed = current.autostart != settings.autostart;
        *current = settings.clone();
        (screen_changed, autostart_changed)
    };
    if let Err(err) = settings::save(&settings) {
        eprintln!("[zuko] could not save settings: {err}");
    }
    if autostart_changed {
        let manager = app.autolaunch();
        let result = if settings.autostart { manager.enable() } else { manager.disable() };
        if let Err(err) = result {
            eprintln!("[zuko] autostart: {err}");
        }
    }
    if screen_changed {
        let collapsed = shared.gate.collapsed.load(Ordering::Relaxed);
        island::apply_geometry(&app, &settings.screen, collapsed, settings.island_offset);
    }
    // Keep the other window in step (island ⇄ settings window).
    let _ = app.emit("settings-changed", settings);
}

/// The display preference and the island's place, as last saved.
fn placement(shared: &Shared) -> (String, f64) {
    let s = shared.settings.lock().unwrap();
    (s.screen.clone(), s.island_offset)
}

/// Dragging the island along the top edge (island.ts). `dx` is how far the pointer has
/// moved since the press, in logical pixels; the island follows it, clamped to the
/// display. "start" marks where the drag began, "move" follows, "end" keeps the new
/// place in settings and tells both windows.
#[tauri::command]
fn island_drag(app: AppHandle, shared: State<Shared>, phase: String, dx: f64) {
    let (pref, current) = placement(&shared);
    if phase == "start" {
        *shared.gate.drag_from.lock().unwrap() = Some(current);
        return;
    }
    // A move without its start (should not happen) starts the drag where the island is.
    let from = *shared.gate.drag_from.lock().unwrap().get_or_insert(current);
    let width = island::drag_monitor_width(&app, &pref);
    let offset = island::drag_offset(from, dx, width);
    let collapsed = shared.gate.collapsed.load(Ordering::Relaxed);
    island::move_to(&app, &pref, collapsed, offset);
    let updated = {
        let mut s = shared.settings.lock().unwrap();
        s.island_offset = offset;
        s.clone()
    };
    if phase == "end" {
        *shared.gate.drag_from.lock().unwrap() = None;
        store_placement(&app, updated);
    }
}

/// Settings → General → "Reset island position", and a double-click on the island's
/// header: back to the top centre.
#[tauri::command]
fn reset_island_position(app: AppHandle, shared: State<Shared>) {
    let (pref, _) = placement(&shared);
    let collapsed = shared.gate.collapsed.load(Ordering::Relaxed);
    island::apply_geometry(&app, &pref, collapsed, 0.0);
    let updated = {
        let mut s = shared.settings.lock().unwrap();
        s.island_offset = 0.0;
        s.clone()
    };
    store_placement(&app, updated);
}

fn store_placement(app: &AppHandle, settings: Settings) {
    if let Err(err) = settings::save(&settings) {
        log::line(format!("could not save the island's place: {err}"));
    }
    let _ = app.emit("settings-changed", settings);
}

/// Hidden island → shrink the window to the invisible wake strip and park the
/// cursor poll; anything else → full panel and 60 Hz polling.
#[tauri::command]
fn set_collapsed(app: AppHandle, shared: State<Shared>, collapsed: bool) {
    let (pref, offset) = placement(&shared);
    // Before the window changes: the click-through decision apply_geometry makes for the
    // new shape (the wake strip always takes the mouse) reads it, and so does any cursor
    // tick still on its way to the main thread.
    shared.gate.collapsed.store(collapsed, Ordering::Relaxed);
    island::apply_geometry(&app, &pref, collapsed, offset);
    shared.gate.set_active(!collapsed);
    // The cursor poll re-claims file drops whenever a press might start a drag, and it is
    // parked while the island is hidden. Whatever the window is now (the wake strip or the
    // panel), a file dragged onto it must still find Zuko's drop target.
    allow_file_drops(&app);
}

/// The front end pushes the island shape; Rust decides click-through from it.
#[tauri::command]
fn set_island_rect(app: AppHandle, shared: State<Shared>, x: f64, y: f64, width: f64, height: f64) {
    shared.gate.set_rect(island::IslandRect { x, y, w: width, h: height });
    // Without the cursor poll the input region is the click-through: it follows the island.
    if !platform::CURSOR_POLL {
        island::refresh_click_through(&app, &shared.gate);
    }
}

#[tauri::command]
fn focus_window(app: AppHandle, focused: bool) {
    let Some(win) = island::window(&app) else { return };
    platform::set_activating(&win, focused);
    if focused {
        let _ = win.set_focus();
    }
}

/// After a display change: the same place on the (possibly new) display, clamped to it.
#[tauri::command]
fn reposition(app: AppHandle, shared: State<Shared>) {
    let (pref, offset) = placement(&shared);
    let collapsed = shared.gate.collapsed.load(Ordering::Relaxed);
    island::apply_geometry(&app, &pref, collapsed, offset);
}

#[tauri::command]
fn open_url(url: String) {
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return;
    }
    platform::open_url(&url);
}

/// "Open terminal" opens the working folder in VS Code when `code` is on PATH,
/// and falls back to the file manager otherwise.
#[tauri::command]
fn open_in_vscode(path: Option<String>) -> bool {
    // No shell anywhere near this. The path is a project folder chosen by
    // whoever is using Claude Code, and a shell would happily read `&`, `^`, `%`
    // or `$` in a folder name as syntax. Finding the launcher ourselves and
    // handing the path over as a separate argument keeps it a path.
    let path = path.filter(|p| !p.is_empty());
    // It arrives in a hook payload: only an existing folder, given by its full
    // path, goes any further. `code` would read `--something` as an option, and
    // xdg-open would launch a file with whatever handles its type.
    if let Some(p) = path.as_deref() {
        let p = std::path::Path::new(p);
        if !(p.is_absolute() && p.is_dir()) {
            return false;
        }
    }
    if let Some(code) = platform::find_on_path("code") {
        let mut cmd = Command::new(code);
        if let Some(p) = path.as_deref() {
            cmd.arg(p);
        }
        if platform::no_console(&mut cmd).spawn().is_ok() {
            return true;
        }
    }
    if let Some(p) = path.as_deref() {
        platform::reveal_folder(p);
    }
    false
}

/// "Open file" on an activity row: opens the file Claude just wrote or edited in VS Code
/// when `code` is on PATH, otherwise shows its folder in the file manager. `cwd` is the
/// session's working folder: handed to VS Code first (`code <cwd> --goto <path>`), it
/// brings the file up in the window already running the task instead of a new one (see
/// vscode.rs). Same rules as `open_in_vscode`: no shell, every path is its own argument,
/// and only an existing file given by its full path gets this far (the path comes from a
/// hook payload; xdg-open or a shell would run a file with whatever handles its type).
#[tauri::command]
fn open_file(path: String, cwd: Option<String>) -> bool {
    let p = std::path::Path::new(&path);
    if !(p.is_absolute() && p.is_file()) {
        return false;
    }
    if let Some(code) = platform::find_on_path("code") {
        let folder = vscode::project_folder(p, cwd.as_deref());
        let mut cmd = Command::new(code);
        cmd.args(vscode::open_file_args(p, folder.as_deref()));
        if platform::no_console(&mut cmd).spawn().is_ok() {
            return true;
        }
    }
    if let Some(dir) = p.parent() {
        platform::reveal_folder(&dir.to_string_lossy());
    }
    false
}

/// "Open terminal" on the island: brings the terminal window this session is
/// already running in to the front — the one Claude Code is asking its question in.
/// The app noted it from the relay's own process when the session's first hook
/// event arrived (terminal.rs).
///
/// False when there is no such window to raise (the terminal has been closed, the
/// session predates this Zuko, or the platform cannot raise other windows at all);
/// the island then falls back to `open_in_vscode`, which is what the button did
/// before. `cwd` only helps tell two windows of the same terminal apart.
#[tauri::command]
fn focus_terminal(app: AppHandle, session_id: Option<String>, cwd: Option<String>) -> bool {
    let focused = terminal::focus(&app, session_id.as_deref(), cwd.as_deref());
    if !focused {
        log::line("open terminal: no window for this session — opening the folder instead");
    }
    focused
}

#[tauri::command]
fn quit_app(app: AppHandle) {
    app.exit(0);
}

/// Tray → Pause. The island declines approvals while paused; protection itself
/// (policy, masking) keeps running.
#[tauri::command]
fn set_paused(paused: bool) {
    log::line(format!("paused: {paused}"));
}

// ── Claude Code hooks ─────────────────────────────────────────────────────────

#[tauri::command]
fn hooks_status() -> HookStatus {
    hooks::status()
}

/// Returns the diff the user has to look at before anything is written.
#[tauri::command]
fn hooks_preview(install: bool) -> Result<HookPreview, String> {
    hooks::preview(install)
}

/// Only ever called from an explicit click in the settings window.
#[tauri::command]
fn hooks_apply(
    app: AppHandle,
    shared: State<Shared>,
    install: bool,
    fingerprint: String,
) -> Result<String, String> {
    // The fingerprint comes from the preview the user actually looked at, so a
    // settings.json that changed in between is refused rather than overwritten.
    let backup = hooks::write(install, &fingerprint)?;
    let updated = {
        let mut current = shared.settings.lock().unwrap();
        current.hooks_installed = install;
        let _ = settings::save(&current);
        current.clone()
    };
    let _ = app.emit("settings-changed", updated);
    events::protection_changed(&app);
    Ok(backup)
}

#[tauri::command]
fn approval_decision(app: AppHandle, request_id: String, decision: String, elapsed_ms: Option<u64>) {
    pipe::answer(&app, &request_id, &decision, elapsed_ms);
}

/// The island has the card on screen, so the long wait for a human may begin.
/// Until this arrives the relay only waits a few hundred milliseconds, which is
/// what stops a paused or unresponsive island from freezing Claude Code.
#[tauri::command]
fn approval_ack(app: AppHandle, request_id: String) {
    pipe::acknowledge(&app, &request_id);
}

/// Nobody can act on this request — the island is paused, or another card is
/// already up. Claude Code falls back to asking in the terminal immediately.
#[tauri::command]
fn approval_decline(app: AppHandle, request_id: String) {
    pipe::decline(&app, &request_id);
}

// ── Chat, files and secrets ───────────────────────────────────────────────────

/// One chat turn with the provider chosen in Settings → Chat. API keys and any file
/// bytes stay on the Rust side.
#[tauri::command]
async fn chat_send(
    app: AppHandle,
    shared: State<'_, Shared>,
    chat: State<'_, Chat>,
    engine: State<'_, Engine>,
    query: String,
    context: Option<ChatContext>,
) -> Result<ChatReply, String> {
    let target = {
        let s = shared.settings.lock().unwrap();
        Target { provider: s.chat_provider, model: s.chat_model(s.chat_provider) }
    };
    let reply = chat::send(&engine, &chat, &target, query, context).await?;
    // Tell the user what was masked before the message left (keys only).
    if reply.report.count > 0 {
        let to = match target.provider {
            Provider::Ollama => "Ollama on this PC".to_string(),
            cloud => cloud.label().to_string(),
        };
        events::announce_privacy(
            &app,
            &engine,
            PrivacyNote {
                source: "chat",
                event: "Chat",
                tool: "message",
                summary: format!(
                    "Masked {} before sending to {to}",
                    if reply.report.count == 1 { "1 value".to_string() } else { format!("{} values", reply.report.count) }
                ),
                direction: "masked",
                count: reply.report.count,
                keys: reply.report.keys.clone(),
                new_keys: reply.report.new_keys.clone(),
                session_id: None,
            },
        );
    }
    Ok(reply)
}

#[tauri::command]
fn chat_reset(chat: State<Chat>) {
    chat.reset();
}

fn chat_provider(name: &str) -> Result<Provider, String> {
    Provider::parse(name).ok_or_else(|| format!("unknown chat provider {name}"))
}

/// The model dropdown for one provider in Settings → Chat. OpenAI's list is fetched
/// here with the stored key (which never reaches the webview); Ollama's comes from
/// /api/tags on the loopback endpoint.
#[tauri::command]
async fn chat_models(shared: State<'_, Shared>, engine: State<'_, Engine>, provider: String) -> Result<ChatModels, String> {
    let provider = chat_provider(&provider)?;
    let saved = shared.settings.lock().unwrap().chat_model(provider);
    Ok(chat::models(&engine, provider, &saved).await)
}

/// Can the chat work with `provider` (default: the chosen one)? A stored key for the
/// cloud providers (presence only, nothing is sent), Ollama running with the model.
#[tauri::command]
async fn chat_status(shared: State<'_, Shared>, engine: State<'_, Engine>, provider: Option<String>) -> Result<ChatStatus, String> {
    let (provider, model) = {
        let s = shared.settings.lock().unwrap();
        let provider = match provider.as_deref() {
            Some(name) => chat_provider(name)?,
            None => s.chat_provider,
        };
        (provider, s.chat_model(provider))
    };
    Ok(chat::status(&engine, provider, &model).await)
}

/// Copies a dropped file into the inbox and reports its name back.
#[tauri::command]
fn ingest_file(path: String) -> Result<DroppedFile, String> {
    files::ingest(&path)
}

/// The island may only ask whether a key exists — never read it. The vault key is
/// not reachable from here at all (see `secrets::ui_may_touch`).
#[tauri::command]
fn secret_present(key: String) -> bool {
    secrets::ui_may_touch(&key) && secrets::present(&key)
}

#[tauri::command]
fn secret_set(key: String, value: String) -> Result<(), String> {
    if !secrets::ui_may_touch(&key) {
        return Err(format!("unknown key {key}"));
    }
    secrets::set(&key, &value)
}

#[tauri::command]
fn secret_clear(key: String) -> Result<(), String> {
    if !secrets::ui_may_touch(&key) {
        return Err(format!("unknown key {key}"));
    }
    secrets::clear(&key)
}

/// Lets the island write to the same log as the Rust side.
#[tauri::command]
fn log_line(message: String) {
    log::line(format!("ui  {message}"));
}

// ── Settings window ───────────────────────────────────────────────────────────

/// WebView2 allows exactly one browser environment per app, and its options are
/// fixed by whichever webview is created first. Every window must therefore ask
/// for the *same* arguments as the island (see `additionalBrowserArgs` in
/// tauri.conf.json) — a mismatch makes the second window come up blank, with no
/// error anywhere.
const BROWSER_ARGS: &str = "--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection --autoplay-policy=no-user-gesture-required";

/// In a dev build the pages are served by Vite, so the second window needs the
/// absolute dev URL; a bundled build resolves it inside the app bundle.
fn settings_page_url(app: &AppHandle) -> WebviewUrl {
    #[cfg(dev)]
    if let Some(mut base) = app.config().build.dev_url.clone() {
        base.set_path("/settings.html");
        return WebviewUrl::External(base);
    }
    let _ = app;
    WebviewUrl::App("settings.html".into())
}

/// The settings window is created hidden at launch and only ever shown and
/// hidden afterwards. A WebView2 window created later — on the main thread or
/// not — silently comes up blank in this app, so the window that works is the
/// one that exists before the island's webview does.
fn create_settings_window(app: &AppHandle) {
    let url = settings_page_url(app);
    match WebviewWindowBuilder::new(app, "settings", url)
        .additional_browser_args(BROWSER_ARGS)
        .title("Settings — Zuko")
        .inner_size(560.0, 680.0)
        .min_inner_size(460.0, 480.0)
        .resizable(true)
        .visible(false)
        .center()
        .build()
    {
        Ok(win) => {
            let hidden = win.clone();
            let handle = app.clone();
            win.on_window_event(move |event| match event {
                // Closing it must only hide it, or it could never be reopened.
                tauri::WindowEvent::CloseRequested { api, .. } => {
                    api.prevent_close();
                    let _ = hidden.hide();
                }
                // The island makes room: it folds back to compact unless an approval
                // card is waiting (island.ts decides, it knows what is on screen).
                tauri::WindowEvent::Focused(true) => {
                    let _ = handle.emit_to(island::WINDOW_LABEL, "settings-focused", ());
                }
                _ => {}
            });
        }
        Err(err) => log::line(format!("settings window failed: {err}")),
    }
}

pub fn show_settings_window(app: &AppHandle) {
    let Some(win) = app.get_webview_window("settings") else {
        log::line("settings window missing");
        return;
    };
    let _ = win.unminimize();
    keep_below_island(app, &win);
    let _ = win.show();
    let _ = win.set_focus();
    allow_file_drops(app);
}

/// Gap between the island's bottom edge and a window moved out from under it, and the
/// shortest the settings window gets (its `min_inner_size`), in logical pixels.
const BELOW_ISLAND_GAP: f64 = 12.0;
const SETTINGS_MIN_H: f64 = 480.0;

/// The island is always on top, so a settings window under it has its title bar
/// (minimize, close) out of reach. Before it shows, a window that would sit under the
/// island moves down below it, within the work area of the island's display, getting
/// shorter if it must; one the user put elsewhere stays where it is.
fn keep_below_island(app: &AppHandle, win: &tauri::WebviewWindow) {
    let Some(shared) = app.try_state::<Shared>() else { return };
    let Some((monitor, island)) = island::visible_bounds(app, &shared.gate) else { return };
    let (Ok(pos), Ok(outer), Ok(inner)) = (win.outer_position(), win.outer_size(), win.inner_size()) else { return };
    let scale = monitor.scale_factor();
    let work = monitor.work_area();
    let work = island::Rect { x: work.position.x, y: work.position.y, w: work.size.width, h: work.size.height };
    // Only on the island's display.
    let (cx, cy) = (pos.x + outer.width as i32 / 2, pos.y + outer.height as i32 / 2);
    let (mp, ms) = (monitor.position(), monitor.size());
    if !(cx >= mp.x && cx < mp.x + ms.width as i32 && cy >= mp.y && cy < mp.y + ms.height as i32) {
        return;
    }
    let current = island::Rect { x: pos.x, y: pos.y, w: outer.width, h: outer.height };
    let gap = (BELOW_ISLAND_GAP * scale).round() as i32;
    let min_h = (SETTINGS_MIN_H * scale).round() as u32 + (outer.height.saturating_sub(inner.height));
    let placed = island::place_below(current, island, work, gap, min_h);
    if placed == current {
        return;
    }
    if placed.h < current.h {
        let shorter = inner.height.saturating_sub(current.h - placed.h);
        let _ = win.set_size(tauri::PhysicalSize::new(inner.width, shorter));
    }
    let _ = win.set_position(tauri::PhysicalPosition::new(placed.x, placed.y));
}

/// The island and the Documents drop zone take dropped files through Zuko's own drop
/// target (`platform::claim_file_drops`), never WebView2's. WebView2 creates more child
/// windows a moment after the window first shows, so the claim is made now and again
/// shortly after; it is idempotent.
fn allow_file_drops(app: &AppHandle) {
    let now = app.clone();
    let _ = app.run_on_main_thread(move || platform::claim_file_drops(&now));
    let later = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(600));
        let handle = later.clone();
        let _ = later.run_on_main_thread(move || platform::claim_file_drops(&handle));
    });
}

#[tauri::command]
fn open_settings_window(app: AppHandle) {
    show_settings_window(&app);
}

/// Headless gateway for development and end-to-end tests (`cargo run --bin
/// zuko-gateway`): the proxy with an in-memory engine, no UI.
pub fn gateway_dev_main() {
    gateway::dev_main();
}

pub fn run() {
    platform::prepare_environment();
    let loaded = settings::load();
    let gate = Arc::new(PollGate::new());

    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            let _ = app.emit_to(island::WINDOW_LABEL, "tray", "open".to_string());
        }))
        .plugin(tauri_plugin_autostart::init(MacosLauncher::LaunchAgent, None))
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .manage(Shared {
            settings: Mutex::new(loaded.clone()),
            gate: gate.clone(),
        })
        .manage(Pending::default())
        .manage(Terminals::default())
        .manage(Chat::default())
        .manage(Engine::load())
        .invoke_handler(tauri::generate_handler![
            boot,
            save_settings,
            set_collapsed,
            set_island_rect,
            island_drag,
            reset_island_position,
            focus_window,
            reposition,
            open_url,
            open_in_vscode,
            open_file,
            focus_terminal,
            quit_app,
            hooks_status,
            hooks_preview,
            hooks_apply,
            approval_decision,
            approval_ack,
            approval_decline,
            log_line,
            chat_send,
            chat_reset,
            chat_models,
            chat_status,
            ingest_file,
            secret_present,
            secret_set,
            secret_clear,
            open_settings_window,
            set_paused,
            commands::protection_status,
            commands::protection_preview,
            commands::protection_apply,
            commands::policy_get,
            commands::policy_set,
            commands::policy_reset,
            commands::localai_status,
            commands::localai_test,
            commands::vault_list,
            commands::vault_add,
            commands::vault_forget,
            commands::vault_clear,
            commands::vault_reveal,
            commands::vault_insights,
            commands::vault_copy,
            commands::native_host_status,
            commands::native_host_set,
            commands::activity_recent,
            commands::audit_verify,
            commands::audit_open_folder,
            commands::mask_text,
            commands::unmask_text,
            commands::sanitize_file,
            commands::reveal_path,
            commands::clipboard_mask,
            commands::clipboard_unmask,
        ])
        .setup(move |app| {
            let handle = app.handle().clone();
            tray::build(&handle)?;
            // Before the island: see create_settings_window.
            create_settings_window(&handle);

            // The full panel, so the launch greeting has room. Placing it also decides
            // click-through: nothing is drawn yet, so nothing takes the mouse until the
            // page reports the island's shape.
            gate.collapsed.store(false, Ordering::Relaxed);
            if let Some(win) = island::window(&handle) {
                platform::make_non_activating(&win);
                island::apply_geometry(&handle, &loaded.screen, false, loaded.island_offset);
                let _ = win.show();
            }
            gate.set_active(true);
            island::spawn_cursor_poll(handle.clone(), gate.clone());

            log::line(format!("--- Zuko {} started ---", env!("CARGO_PKG_VERSION")));
            // Resume the audit chain, seed the activity feed and count today's receipts
            // before the first event.
            auditlog::init();
            events::spawn_status_watch(&handle);
            commands::register_hotkeys(&handle);
            hooks::ensure_hook_exe(&handle);
            // After the native host is in place in bin\: the browsers are pointed at it.
            nativehost::ensure_at_startup(loaded.browser_bridge);
            pipe::start(handle.clone());
            gateway::start(handle.clone());
            localai::warm_up(&handle);
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building Zuko")
        .run(|app, event| {
            // A vault change still waiting out its debounce must not be lost on exit.
            if let tauri::RunEvent::Exit = event {
                app.state::<Engine>().flush_vault();
            }
        });
}
