// Island window: placement on the chosen display, the two window sizes
// (full panel / invisible wake strip), click-through and the cursor poll.
//
// There is no notch on a PC, so the island is a black shape drawn at the top
// of the main display inside a borderless, transparent, always-on-top window
// that never takes focus. It starts centred, and the user can drag it along the
// top edge (island.ts): the window moves, the island stays centred in it. Its
// place is kept as `Settings.islandOffset`, the island centre's distance from
// the display centre as a fraction of the display width, so it lands in the same
// spot after a restart or a resolution change, always clamped to the display.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, Monitor, PhysicalPosition, PhysicalSize, WebviewWindow};

use crate::platform::{self, cursor_physical, left_button_down};

/// Logical size of the full window — the largest island view, like the macOS panel.
pub const PANEL_W: f64 = 720.0;
pub const PANEL_H: f64 = 320.0;
/// Logical size of the invisible strip that wakes the island when it is hidden.
pub const STRIP_W: f64 = 240.0;
pub const STRIP_H: f64 = 6.0;

pub const WINDOW_LABEL: &str = "island";

/// Margin around the island that still counts as "on the island", in logical px.
/// Wider than the macOS 6 pt because a click must never be swallowed.
const HIT_MARGIN: f64 = 14.0;

#[derive(Serialize, Clone)]
pub struct CursorPayload {
    pub x: f64,
    pub y: f64,
}

#[derive(Serialize, Clone)]
pub struct ScreenInfo {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub scale: f64,
}

/// The island shape in window-logical coordinates, pushed by the front end.
/// The poll thread owns the click-through decision so it lands in the same 16 ms
/// tick as the cursor read — an IPC round trip here loses clicks.
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct IslandRect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl IslandRect {
    /// The cursor (window-logical) is on the island, or within the margin that turns
    /// click-through off just before it gets there.
    pub fn takes(&self, x: f64, y: f64) -> bool {
        self.w > 0.0
            && x >= self.x - HIT_MARGIN
            && x <= self.x + self.w + HIT_MARGIN
            && y >= self.y - HIT_MARGIN
            && y <= self.y + self.h + HIT_MARGIN
    }
}

/// Wakes / parks the cursor poll thread so a hidden island costs literally nothing.
pub struct PollGate {
    active: Mutex<bool>,
    cv: Condvar,
    pub collapsed: AtomicBool,
    pub rect: Mutex<IslandRect>,
    /// Mirrors the window flag so we only call into the OS when it changes.
    ignoring: AtomicBool,
    /// The island changed shape or the window was resized: the next tick decides
    /// click-through again even if the cursor has not moved. Without it, a cursor
    /// resting where the island just shrank away kept swallowing clicks meant for the
    /// window behind until it moved, and one resting where the island just grew let
    /// clicks fall through it.
    recheck: AtomicBool,
    /// `islandOffset` when the current drag began (see `lib.rs` `island_drag`).
    pub drag_from: Mutex<Option<f64>>,
}

impl PollGate {
    pub fn new() -> Self {
        Self {
            active: Mutex::new(false),
            cv: Condvar::new(),
            collapsed: AtomicBool::new(true),
            rect: Mutex::new(IslandRect::default()),
            ignoring: AtomicBool::new(false),
            recheck: AtomicBool::new(true),
            drag_from: Mutex::new(None),
        }
    }

    pub fn set_rect(&self, rect: IslandRect) {
        let mut current = self.rect.lock().unwrap();
        if *current != rect {
            *current = rect;
            self.recheck.store(true, Ordering::Relaxed);
        }
    }

    /// Forces the next poll tick to re-apply the flag (after a window resize).
    pub fn forget_ignore_state(&self) {
        self.ignoring.store(false, Ordering::Relaxed);
        self.recheck.store(true, Ordering::Relaxed);
    }

    pub fn set_active(&self, on: bool) {
        let mut guard = self.active.lock().unwrap();
        *guard = on;
        self.cv.notify_all();
    }

    fn wait_until_active(&self) {
        let mut guard = self.active.lock().unwrap();
        while !*guard {
            guard = self.cv.wait(guard).unwrap();
        }
    }

    fn is_active(&self) -> bool {
        *self.active.lock().unwrap()
    }
}

pub fn window(app: &AppHandle) -> Option<WebviewWindow> {
    app.get_webview_window(WINDOW_LABEL)
}

fn monitor_contains(m: &Monitor, x: f64, y: f64) -> bool {
    let p = m.position();
    let s = m.size();
    x >= p.x as f64
        && x < (p.x + s.width as i32) as f64
        && y >= p.y as f64
        && y < (p.y + s.height as i32) as f64
}

/// The display the island lives on: the primary one, or the one under the cursor.
fn target_monitor(app: &AppHandle, pref: &str) -> Option<Monitor> {
    let monitors = app.available_monitors().ok()?;
    if pref == "cursor" {
        if let Some((cx, cy)) = cursor_physical() {
            if let Some(m) = monitors.iter().find(|m| monitor_contains(m, cx, cy)) {
                return Some(m.clone());
            }
        }
    }
    app.primary_monitor()
        .ok()
        .flatten()
        .or_else(|| monitors.into_iter().next())
}

pub fn screen_info(app: &AppHandle, pref: &str) -> ScreenInfo {
    match target_monitor(app, pref) {
        Some(m) => {
            let scale = m.scale_factor();
            let p = m.position();
            let s = m.size();
            ScreenInfo {
                x: p.x as f64 / scale,
                y: p.y as f64 / scale,
                width: s.width as f64 / scale,
                height: s.height as f64 / scale,
                scale,
            }
        }
        None => ScreenInfo { x: 0.0, y: 0.0, width: 1920.0, height: 1080.0, scale: 1.0 },
    }
}

/// `offset` (fraction of the display width, island centre right of the display centre)
/// limited so the panel window stays on a display `monitor_w` wide. Any unit, as long
/// as both widths share it. Garbage (NaN, ±inf) is centred.
pub fn clamp_offset(offset: f64, monitor_w: f64, panel_w: f64) -> f64 {
    if !offset.is_finite() || monitor_w <= 0.0 {
        return 0.0;
    }
    let max = ((monitor_w - panel_w) / 2.0 / monitor_w).max(0.0);
    offset.clamp(-max, max)
}

/// The left edge (physical) of a window `window_w` wide centred on the island, on a
/// display at `monitor_x` that is `monitor_w` wide. The island centre is clamped as if
/// the window were the full panel (`panel_w`), so the panel and the wake strip always
/// share it; the window itself never leaves the display.
pub fn window_x(monitor_x: i32, monitor_w: u32, panel_w: u32, window_w: u32, offset: f64) -> i32 {
    let (mw, pw, ww) = (monitor_w as f64, panel_w as f64, window_w as f64);
    let centre = mw / 2.0 + clamp_offset(offset, mw, pw) * mw;
    let left = (centre - ww / 2.0).round().clamp(0.0, (mw - ww).max(0.0));
    monitor_x + left as i32
}

/// Places and sizes the window. `collapsed` picks the wake strip instead of the panel;
/// `offset` is `Settings.islandOffset`.
pub fn apply_geometry(app: &AppHandle, pref: &str, collapsed: bool, offset: f64) {
    let Some(win) = window(app) else { return };
    let Some(m) = target_monitor(app, pref) else { return };

    let scale = m.scale_factor();
    let mp = *m.position();
    let ms = *m.size();

    let (lw, lh) = if collapsed { (STRIP_W, STRIP_H) } else { (PANEL_W, PANEL_H) };
    let pw = (lw * scale).round().max(1.0) as u32;
    let ph = (lh * scale).round().max(1.0) as u32;
    let panel_w = (PANEL_W * scale).round().max(1.0) as u32;
    let x = window_x(mp.x, ms.width, panel_w, pw, offset);
    let y = mp.y;

    let _ = win.set_size(PhysicalSize::new(pw, ph));
    let _ = win.set_position(PhysicalPosition::new(x, y));
    // Moving across displays can rescale the window: re-assert the physical size.
    let _ = win.set_size(PhysicalSize::new(pw, ph));
    let _ = win.set_always_on_top(true);
}

/// Moves the window to `offset` without touching its size: the drag path, which runs
/// on every frame of a drag and must not resize anything under the cursor.
pub fn move_to(app: &AppHandle, pref: &str, collapsed: bool, offset: f64) {
    let Some(win) = window(app) else { return };
    let Some(m) = target_monitor(app, pref) else { return };
    let scale = m.scale_factor();
    let lw = if collapsed { STRIP_W } else { PANEL_W };
    let pw = (lw * scale).round().max(1.0) as u32;
    let panel_w = (PANEL_W * scale).round().max(1.0) as u32;
    let x = window_x(m.position().x, m.size().width, panel_w, pw, offset);
    let _ = win.set_position(PhysicalPosition::new(x, m.position().y));
}

/// Width of the island's display in logical pixels (drag deltas arrive in those).
pub fn monitor_logical_width(app: &AppHandle, pref: &str) -> f64 {
    screen_info(app, pref).width
}

/// The island as drawn now, in physical screen pixels `[left, top, right, bottom]`, with
/// the display it is on. Collapsed, it is the wake strip.
pub fn visible_bounds(app: &AppHandle, gate: &PollGate) -> Option<(Monitor, [i32; 4])> {
    let win = window(app)?;
    let pos = win.outer_position().ok()?;
    let size = win.outer_size().ok()?;
    let scale = win.scale_factor().unwrap_or(1.0);
    let monitor = win.current_monitor().ok().flatten()?;
    let r = *gate.rect.lock().unwrap();
    if gate.collapsed.load(Ordering::Relaxed) || r.w <= 0.0 {
        return Some((monitor, [pos.x, pos.y, pos.x + size.width as i32, pos.y + size.height as i32]));
    }
    let px = |v: f64| (v * scale).round() as i32;
    Some((monitor, [pos.x + px(r.x), pos.y + px(r.y), pos.x + px(r.x + r.w), pos.y + px(r.y + r.h)]))
}

/// A rectangle in physical pixels: position and size.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
}

impl Rect {
    fn right(&self) -> i32 {
        self.x + self.w as i32
    }
    fn bottom(&self) -> i32 {
        self.y + self.h as i32
    }
}

/// Where a window (`win`, e.g. Settings) must go so the island (`island`, as
/// `[left, top, right, bottom]`) does not cover its title bar, within the display's work
/// area `work`. A window that does not overlap the island's columns, or already starts
/// below it, stays put (only pulled into the work area). Otherwise it moves down to
/// `gap` below the island and, if it would then run off the bottom, gets shorter, down to
/// `min_h`: the title bar (minimize, close) stays reachable whatever happens to the rest.
pub fn place_below(win: Rect, island: [i32; 4], work: Rect, gap: i32, min_h: u32) -> Rect {
    let mut out = win;
    out.x = out.x.clamp(work.x, (work.right() - win.w as i32).max(work.x));
    out.y = out.y.max(work.y);
    let overlaps_columns = out.x < island[2] && out.right() > island[0];
    let floor = island[3] + gap;
    if overlaps_columns && out.y < floor {
        out.y = floor;
        let room = (work.bottom() - out.y).max(0) as u32;
        if out.h > room {
            out.h = room.max(min_h);
        }
    }
    out
}

/// Position, size and scale of the monitor the island lives on. Any change here
/// means the island has to be placed again.
fn current_screen_key(app: &AppHandle) -> Option<(i32, i32, u32, u32, u64)> {
    let pref = app
        .try_state::<crate::Shared>()
        .map(|s| s.settings.lock().unwrap().screen.clone())
        .unwrap_or_else(|| "primary".into());
    let m = target_monitor(app, &pref)?;
    let p = m.position();
    let size = m.size();
    Some((p.x, p.y, size.width, size.height, m.scale_factor().to_bits()))
}

/// Emits `cursor` (window-logical coordinates) at ~60 Hz while the island is
/// visible. Parked on a condvar the rest of the time.
pub fn spawn_cursor_poll(app: AppHandle, gate: Arc<PollGate>) {
    std::thread::spawn(move || {
        let mut was_down = false;
        // Remembered across wakes so a display change while hidden is noticed the
        // moment the island comes back.
        let mut last_screen: Option<(i32, i32, u32, u32, u64)> = None;
        loop {
            gate.wait_until_active();
            let mut last = (f64::MIN, f64::MIN);
            let mut ticks: u32 = 0;
            while gate.is_active() {
                std::thread::sleep(Duration::from_millis(16));

                // Monitors get plugged in, unplugged, rearranged and rescaled, and
                // an island pinned to coordinates that no longer exist is an island
                // nobody can reach. Checked about twice a second — the cursor poll
                // is already running, so this costs one monitor query.
                ticks = ticks.wrapping_add(1);
                if ticks % 30 == 0 {
                    let now = current_screen_key(&app);
                    if now.is_some() && now != last_screen {
                        let first = last_screen.is_none();
                        last_screen = now;
                        if !first {
                            crate::log::line("display layout changed — repositioning".to_string());
                            let _ = app.emit_to(WINDOW_LABEL, "screen-changed", ());
                        }
                    }
                }

                let Some(win) = window(&app) else { continue };
                let Ok(origin) = win.outer_position() else { continue };
                let scale = win.scale_factor().unwrap_or(1.0);
                let Some((cx, cy)) = cursor_physical() else { continue };
                let x = (cx - origin.x as f64) / scale;
                let y = (cy - origin.y as f64) / scale;
                let size = match win.inner_size() {
                    Ok(s) => (s.width as f64 / scale, s.height as f64 / scale),
                    Err(_) => (PANEL_W, PANEL_H),
                };
                // A still cursor needs no new decision, unless the island changed under it.
                let recheck = gate.recheck.swap(false, Ordering::Relaxed);
                if !recheck && (x - last.0).abs() < 1.0 && (y - last.1).abs() < 1.0 {
                    continue;
                }
                last = (x, y);

                // Click-through: the window only takes the mouse over the island
                // shape. A small entry margin means the flag is already off by the
                // time a moving cursor reaches a button.
                let on_island = gate.rect.lock().unwrap().takes(x, y);

                // A file being dragged has to be able to find us. WS_EX_TRANSPARENT
                // — what click-through is on Windows — hides the window from
                // WindowFromPoint, so OLE finds no drop target and shows the "no
                // drop" cursor. macOS has no such problem: AppKit delivers drags to
                // registered destinations whatever ignoresMouseEvents says. So while
                // a button is held anywhere over the panel, the whole panel takes
                // the mouse, which also makes the drop zone as forgiving as the Mac's.
                // A press may be the start of a drag: make sure the drop target is
                // ours before the file arrives.
                let down = left_button_down();
                if down && !was_down {
                    let handle = app.clone();
                    let _ = app.run_on_main_thread(move || platform::unblock_webview_drops(&handle));
                }
                was_down = down;

                let dragging = down
                    && x >= 0.0
                    && x <= size.0
                    && y >= 0.0
                    && y <= size.1;

                let accept = on_island || dragging;
                if gate.ignoring.load(Ordering::Relaxed) == accept {
                    gate.ignoring.store(!accept, Ordering::Relaxed);
                    let _ = win.set_ignore_cursor_events(!accept);
                }

                let _ = win.emit("cursor", CursorPayload { x, y });
            }
        }
    });
}

/// Re-applies click-through after the window or the island changed shape.
///
/// With the cursor poll (Windows) the window takes the mouse again and the next
/// tick decides from the cursor. Without it (Linux) the input region is set to
/// the island itself, or to the whole wake strip while collapsed.
pub fn refresh_click_through(app: &AppHandle, gate: &PollGate) {
    if platform::CURSOR_POLL {
        set_ignore_cursor(app, false);
        gate.forget_ignore_state();
        return;
    }
    let Some(win) = window(app) else { return };
    let region = if gate.collapsed.load(Ordering::Relaxed) {
        None
    } else {
        let r = *gate.rect.lock().unwrap();
        if r.w <= 0.0 {
            // Nothing drawn yet: nothing takes the mouse.
            Some((0.0, 0.0, 0.0, 0.0))
        } else {
            let x0 = (r.x - HIT_MARGIN).max(0.0);
            let y0 = (r.y - HIT_MARGIN).max(0.0);
            let x1 = r.x + r.w + HIT_MARGIN;
            let y1 = r.y + r.h + HIT_MARGIN;
            Some((x0, y0, x1 - x0, y1 - y0))
        }
    };
    platform::set_input_region(&win, region);
}

pub fn set_ignore_cursor(app: &AppHandle, ignore: bool) {
    if let Some(win) = window(app) {
        let _ = win.set_ignore_cursor_events(ignore);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_island_and_its_margin_take_the_mouse() {
        // The expanded island, centred in the 720-wide window.
        let r = IslandRect { x: 40.0, y: 0.0, w: 640.0, h: 160.0 };
        assert!(r.takes(360.0, 80.0), "on the island");
        assert!(r.takes(40.0 - HIT_MARGIN, 0.0) && r.takes(680.0 + HIT_MARGIN, 160.0 + HIT_MARGIN), "the entry margin");
        // The transparent rest of the window passes clicks to the apps behind.
        assert!(!r.takes(10.0, 20.0), "left of the island");
        assert!(!r.takes(700.0, 20.0), "right of the island");
        assert!(!r.takes(360.0, 200.0), "under the island");
        // Compact: only the small bar.
        let compact = IslandRect { x: 216.0, y: 0.0, w: 288.0, h: 32.0 };
        assert!(compact.takes(360.0, 16.0));
        assert!(!compact.takes(100.0, 16.0) && !compact.takes(360.0, 100.0));
        // Nothing drawn yet: nothing takes the mouse.
        assert!(!IslandRect::default().takes(0.0, 0.0));
    }

    #[test]
    fn a_new_shape_is_rechecked_even_under_a_still_cursor() {
        let gate = PollGate::new();
        assert!(gate.recheck.swap(false, Ordering::Relaxed), "the first tick always decides");
        let r = IslandRect { x: 40.0, y: 0.0, w: 640.0, h: 160.0 };
        gate.set_rect(r);
        assert!(gate.recheck.swap(false, Ordering::Relaxed));
        gate.set_rect(r);
        assert!(!gate.recheck.load(Ordering::Relaxed), "the same shape again changes nothing");
        gate.set_rect(IslandRect { h: 32.0, w: 288.0, x: 216.0, ..r });
        assert!(gate.recheck.load(Ordering::Relaxed));
        gate.recheck.store(false, Ordering::Relaxed);
        gate.forget_ignore_state();
        assert!(gate.recheck.load(Ordering::Relaxed), "a resized window is rechecked too");
    }

    #[test]
    fn the_island_keeps_its_place_and_never_leaves_the_display() {
        // A 1920 px display at x = 0, the panel 720 px wide, scale 1.
        assert_eq!(window_x(0, 1920, 720, 720, 0.0), 600, "centred by default");
        // A quarter of the display to the right: the centre at 1440.
        assert_eq!(window_x(0, 1920, 720, 720, 0.25), 1440 - 360);
        // Too far: clamped so the panel ends at the display's edge, on both sides.
        assert_eq!(window_x(0, 1920, 720, 720, 0.9), 1200);
        assert_eq!(window_x(0, 1920, 720, 720, -0.9), 0);
        // The wake strip shares the island's centre, wherever it was dragged.
        let centre = window_x(0, 1920, 720, 720, 0.25) + 360;
        assert_eq!(window_x(0, 1920, 720, 240, 0.25) + 120, centre);
        assert_eq!(window_x(0, 1920, 720, 240, 0.9) + 120, 1200 + 360, "clamped like the panel");
        // A second display to the left of the first, 2560 px wide at 150 %: same fraction.
        let x = window_x(-2560, 2560, 1080, 1080, 0.25);
        assert_eq!(x + 540, -2560 + 1280 + 640);
        // A display narrower than the panel: pinned to its left edge, not off it.
        assert_eq!(window_x(100, 640, 720, 720, 0.3), 100);
        // Garbage in a hand-edited settings.json is centred.
        assert_eq!(clamp_offset(f64::NAN, 1920.0, 720.0), 0.0);
        assert_eq!(clamp_offset(0.1, 1920.0, 720.0), 0.1);
        assert!((clamp_offset(1.0, 1920.0, 720.0) - 0.3125).abs() < 1e-9);
    }

    #[test]
    fn the_settings_window_goes_below_the_island() {
        let work = Rect { x: 0, y: 0, w: 1920, h: 1040 };
        // Centred 560×680 settings window, under an expanded island 300 px tall.
        let island = [640, 0, 1280, 300];
        let win = Rect { x: 680, y: 200, w: 560, h: 680 };
        let placed = place_below(win, island, work, 12, 400);
        assert_eq!(placed.y, 312, "its title bar is below the island");
        assert_eq!((placed.x, placed.w), (680, 560));
        assert!(placed.y + placed.h as i32 <= 1040, "and it still fits in the work area: {placed:?}");
        // Already below: left alone.
        let low = Rect { y: 330, ..win };
        assert_eq!(place_below(low, island, work, 12, 400), low);
        // Beside the island (dragged to a corner): not its business.
        let beside = Rect { x: 1300, y: 40, ..win };
        assert_eq!(place_below(beside, island, work, 12, 400), beside);
        // A small display: never shorter than min_h, the title bar still below the island.
        let small = Rect { x: 0, y: 0, w: 1280, h: 680 };
        let p = place_below(Rect { x: 360, y: 0, w: 560, h: 680 }, [320, 0, 960, 300], small, 12, 400);
        assert_eq!((p.y, p.h), (312, 400));
        // Off the work area's side: pulled back in.
        let off = place_below(Rect { x: 1700, y: 600, w: 560, h: 300 }, island, work, 12, 400);
        assert_eq!(off.x, 1920 - 560);
    }
}
