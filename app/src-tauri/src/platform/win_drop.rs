// Zuko's own OLE drop target for its windows (Windows only).
//
// Files dragged onto the island (or the settings window's Documents zone) used to
// depend on which of WebView2's child windows OLE found under the cursor. OLE asks the
// window under the cursor itself for its drop target, and that window is one of
// WebView2's (Chrome_RenderWidgetHostHWND, owned by msedgewebview2.exe): it does not
// fall through to a target registered further up in Zuko's own windows. wry registers
// its target on the children that exist when the webview is created; WebView2 adds
// more later and can register its own target on them, which refuses every external
// file; and clearing those targets leaves nothing at all under the cursor. Either way
// the island showed the "no drop" cursor and the file was lost.
//
// So Zuko's target goes on the window itself and on every window inside it, put back
// whenever a drag may be starting. The target also drives the shell's drag-image
// helper, so the dragged file stays visible over the island instead of vanishing the
// moment it gets there.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;

use ::windows::core::{implement, Ref, BOOL, PCWSTR};
use ::windows::Win32::Foundation::{HWND, LPARAM, POINT, POINTL};
use ::windows::Win32::System::Com::{
    CoCreateInstance, IDataObject, CLSCTX_INPROC_SERVER, DVASPECT_CONTENT, FORMATETC, TYMED_HGLOBAL,
};
use ::windows::Win32::System::Ole::{
    IDropTarget, IDropTarget_Impl, RegisterDragDrop, ReleaseStgMedium, RevokeDragDrop, CF_HDROP, DROPEFFECT,
    DROPEFFECT_COPY, DROPEFFECT_NONE,
};
use ::windows::Win32::System::SystemServices::MODIFIERKEYS_FLAGS;
use ::windows::Win32::UI::Shell::{CLSID_DragDropHelper, DragQueryFileW, IDropTargetHelper, HDROP};
use ::windows::Win32::UI::WindowsAndMessaging::{EnumChildWindows, GetClassNameW, GetPropW};
use serde::Serialize;
use tauri::{AppHandle, Emitter};

/// The event the page listens to (`onDragDrop` in bridge.ts).
pub const FILE_DRAG_EVENT: &str = "file-drag";

#[derive(Clone, Serialize)]
struct FileDrag<'a> {
    /// The window the drag is over: every page hears the event and keeps its own.
    label: &'a str,
    #[serde(rename = "type")]
    kind: &'static str,
    paths: Vec<String>,
}

thread_local! {
    /// One target per top-level window. COM objects belong to the main thread (OLE
    /// calls them there), so this lives on it too.
    static TARGETS: RefCell<HashMap<isize, IDropTarget>> = RefCell::new(HashMap::new());
    /// The windows our target is registered on.
    static MINE: RefCell<HashSet<isize>> = RefCell::new(HashSet::new());
}

/// Makes Zuko's target the one OLE finds anywhere over `top`. Main thread only.
///
/// Cheap and idempotent: a window already carrying our target is left alone (a drag
/// may be over it); any other window, including ones WebView2 created or registered
/// since the last call, gets ours in place of whatever it had.
pub fn claim(app: &AppHandle, label: &str, top: HWND) {
    let target = TARGETS.with(|targets| {
        targets
            .borrow_mut()
            .entry(top.0 as isize)
            .or_insert_with(|| FileDropTarget::new(app.clone(), label.to_string(), top).into())
            .clone()
    });
    let mut windows = vec![top];
    unsafe {
        let _ = EnumChildWindows(Some(top), Some(collect_child), LPARAM(&mut windows as *mut Vec<HWND> as isize));
    }
    let mut taken = Vec::new();
    for hwnd in windows {
        let key = hwnd.0 as isize;
        if MINE.with(|mine| mine.borrow().contains(&key)) && is_registered(hwnd) {
            continue;
        }
        unsafe {
            // Whatever held the window before (WebView2's own target, wry's, tao's) gives way.
            let _ = RevokeDragDrop(hwnd);
            match RegisterDragDrop(hwnd, &target) {
                Ok(()) => {
                    MINE.with(|mine| mine.borrow_mut().insert(key));
                    taken.push(class_name(hwnd));
                }
                Err(e) => crate::log::line(format!("drag-diag: could not take file drops on {label} {}: {e}", class_name(hwnd))),
            }
        }
    }
    if !taken.is_empty() {
        crate::log::line(format!("drag-diag: file drops on {label} go to Zuko ({})", taken.join(", ")));
    }
}

fn is_registered(hwnd: HWND) -> bool {
    let name: Vec<u16> = "OleDropTargetInterface\0".encode_utf16().collect();
    unsafe { !GetPropW(hwnd, PCWSTR(name.as_ptr())).0.is_null() }
}

fn class_name(hwnd: HWND) -> String {
    let mut buf = [0u16; 64];
    let len = unsafe { GetClassNameW(hwnd, &mut buf) };
    String::from_utf16_lossy(&buf[..len.max(0) as usize])
}

unsafe extern "system" fn collect_child(hwnd: HWND, list: LPARAM) -> BOOL {
    let list = unsafe { &mut *(list.0 as *mut Vec<HWND>) };
    list.push(hwnd);
    true.into()
}

#[implement(IDropTarget)]
struct FileDropTarget {
    app: AppHandle,
    label: String,
    hwnd: HWND,
    /// The shell's helper that draws the dragged file under the cursor. Without it a
    /// drag image disappears over any window that does not ask for it.
    helper: Option<IDropTargetHelper>,
    /// What this drag carries: the files, or nothing we take (then the cursor says so).
    accepts: RefCell<bool>,
}

impl FileDropTarget {
    fn new(app: AppHandle, label: String, hwnd: HWND) -> Self {
        let helper = unsafe { CoCreateInstance(&CLSID_DragDropHelper, None, CLSCTX_INPROC_SERVER).ok() };
        Self { app, label, hwnd, helper, accepts: RefCell::new(false) }
    }

    fn emit(&self, kind: &'static str, paths: Vec<String>) {
        if kind != "over" {
            crate::log::line(format!("drag-diag: {kind} {} file(s) on {}", paths.len(), self.label));
        }
        let _ = self.app.emit(FILE_DRAG_EVENT, FileDrag { label: &self.label, kind, paths });
    }

    /// The effect to show: a copy when the drag carries files and copying is allowed.
    /// Never a move: the source would delete the file once we said we took it.
    fn effect(&self, allowed: DROPEFFECT) -> DROPEFFECT {
        if *self.accepts.borrow() && (allowed & DROPEFFECT_COPY) == DROPEFFECT_COPY {
            DROPEFFECT_COPY
        } else {
            DROPEFFECT_NONE
        }
    }
}

/// The paths of the files a drag carries (CF_HDROP), or none.
fn dropped_paths(data: Option<&IDataObject>) -> Vec<String> {
    let Some(data) = data else { return Vec::new() };
    let format = FORMATETC {
        cfFormat: CF_HDROP.0,
        ptd: std::ptr::null_mut(),
        dwAspect: DVASPECT_CONTENT.0,
        lindex: -1,
        tymed: TYMED_HGLOBAL.0 as u32,
    };
    let Ok(mut medium) = (unsafe { data.GetData(&format) }) else { return Vec::new() };
    let mut paths = Vec::new();
    unsafe {
        let hdrop = HDROP(medium.u.hGlobal.0 as _);
        let count = DragQueryFileW(hdrop, u32::MAX, None);
        for i in 0..count {
            let len = DragQueryFileW(hdrop, i, None) as usize;
            let mut buf = vec![0u16; len + 1];
            DragQueryFileW(hdrop, i, Some(&mut buf));
            paths.push(OsString::from_wide(&buf[..len]).to_string_lossy().into_owned());
        }
        ReleaseStgMedium(&mut medium);
    }
    paths
}

fn point(pt: &POINTL) -> POINT {
    POINT { x: pt.x, y: pt.y }
}

#[allow(non_snake_case)]
impl IDropTarget_Impl for FileDropTarget_Impl {
    fn DragEnter(
        &self,
        data: Ref<'_, IDataObject>,
        _keys: MODIFIERKEYS_FLAGS,
        pt: &POINTL,
        effect: *mut DROPEFFECT,
    ) -> ::windows::core::Result<()> {
        let paths = dropped_paths(data.as_ref());
        *self.accepts.borrow_mut() = !paths.is_empty();
        let chosen = unsafe {
            let e = self.effect(*effect);
            *effect = e;
            e
        };
        if let Some(helper) = &self.helper {
            let _ = unsafe { helper.DragEnter(self.hwnd, data.as_ref(), &point(pt), chosen) };
        }
        if paths.is_empty() {
            crate::log::line(format!("drag-diag: a drag with no files entered {}", self.label));
        } else {
            self.emit("enter", paths);
        }
        Ok(())
    }

    fn DragOver(&self, _keys: MODIFIERKEYS_FLAGS, pt: &POINTL, effect: *mut DROPEFFECT) -> ::windows::core::Result<()> {
        let chosen = unsafe {
            let e = self.effect(*effect);
            *effect = e;
            e
        };
        if let Some(helper) = &self.helper {
            let _ = unsafe { helper.DragOver(&point(pt), chosen) };
        }
        if *self.accepts.borrow() {
            self.emit("over", Vec::new());
        }
        Ok(())
    }

    fn DragLeave(&self) -> ::windows::core::Result<()> {
        if let Some(helper) = &self.helper {
            let _ = unsafe { helper.DragLeave() };
        }
        if std::mem::take(&mut *self.accepts.borrow_mut()) {
            self.emit("leave", Vec::new());
        }
        Ok(())
    }

    fn Drop(
        &self,
        data: Ref<'_, IDataObject>,
        _keys: MODIFIERKEYS_FLAGS,
        pt: &POINTL,
        effect: *mut DROPEFFECT,
    ) -> ::windows::core::Result<()> {
        let paths = dropped_paths(data.as_ref());
        *self.accepts.borrow_mut() = !paths.is_empty();
        let chosen = unsafe {
            let e = self.effect(*effect);
            *effect = e;
            e
        };
        if let Some(helper) = &self.helper {
            let _ = unsafe { helper.Drop(data.as_ref(), &point(pt), chosen) };
        }
        *self.accepts.borrow_mut() = false;
        if !paths.is_empty() {
            self.emit("drop", paths);
        }
        Ok(())
    }
}
