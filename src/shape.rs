//! Rounded window corners on X11.
//!
//! Slint's light software renderer can't make the window see-through, so the
//! corners outside the rounded frame would show as grey squares. Instead we
//! ask the X server to cut the window itself to a rounded rectangle (the SHAPE
//! extension). It costs nothing per frame, but the edge is stair-stepped, so
//! it's only used when the window can't be see-through (see `compositing`).

use std::ffi::{c_char, c_int, c_ulong, c_void};
use std::sync::OnceLock;

#[repr(C)]
struct XRectangle {
    x: i16,
    y: i16,
    width: u16,
    height: u16,
}

type CombineRectangles =
    unsafe extern "C" fn(*mut c_void, c_ulong, c_int, c_int, c_int, *const XRectangle, c_int, c_int, c_int);
type Flush = unsafe extern "C" fn(*mut c_void) -> c_int;
type InternAtom = unsafe extern "C" fn(*mut c_void, *const c_char, c_int) -> c_ulong;
type GetSelectionOwner = unsafe extern "C" fn(*mut c_void, c_ulong) -> c_ulong;

unsafe extern "C" {
    fn dlopen(filename: *const c_char, flag: c_int) -> *mut c_void;
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
}

const RTLD_NOW: c_int = 2;
const SHAPE_BOUNDING: c_int = 0;
const SHAPE_SET: c_int = 0;
const UNSORTED: c_int = 0;

struct Lib {
    combine: CombineRectangles,
    flush: Flush,
    intern_atom: InternAtom,
    selection_owner: GetSelectionOwner,
}

/// libXext is loaded on first use, so Nava still starts where it's missing.
fn lib() -> Option<&'static Lib> {
    static LIB: OnceLock<Option<Lib>> = OnceLock::new();
    LIB.get_or_init(|| unsafe {
        let ext = dlopen(c"libXext.so.6".as_ptr(), RTLD_NOW);
        let x11 = dlopen(c"libX11.so.6".as_ptr(), RTLD_NOW);
        if ext.is_null() || x11.is_null() {
            return None;
        }
        let combine = dlsym(ext, c"XShapeCombineRectangles".as_ptr());
        let flush = dlsym(x11, c"XFlush".as_ptr());
        let intern_atom = dlsym(x11, c"XInternAtom".as_ptr());
        let selection_owner = dlsym(x11, c"XGetSelectionOwner".as_ptr());
        if combine.is_null() || flush.is_null() || intern_atom.is_null() || selection_owner.is_null() {
            return None;
        }
        Some(Lib {
            combine: std::mem::transmute::<*mut c_void, CombineRectangles>(combine),
            flush: std::mem::transmute::<*mut c_void, Flush>(flush),
            intern_atom: std::mem::transmute::<*mut c_void, InternAtom>(intern_atom),
            selection_owner: std::mem::transmute::<*mut c_void, GetSelectionOwner>(selection_owner),
        })
    })
    .as_ref()
}

/// One rectangle per row through the curved parts, and one for the middle.
fn rows(w: u32, h: u32, r: u32) -> Vec<XRectangle> {
    let r = r.min(w / 2).min(h / 2);
    let mut out = Vec::with_capacity(2 * r as usize + 1);
    for y in 0..r {
        let dy = r as f32 - y as f32 - 0.5;
        let inset = (r as f32 - (r as f32 * r as f32 - dy * dy).max(0.0).sqrt()).round() as u32;
        let width = w.saturating_sub(2 * inset) as u16;
        out.push(XRectangle { x: inset as i16, y: y as i16, width, height: 1 });
        out.push(XRectangle { x: inset as i16, y: (h - 1 - y) as i16, width, height: 1 });
    }
    out.push(XRectangle { x: 0, y: r as i16, width: w as u16, height: h.saturating_sub(2 * r) as u16 });
    out
}

/// Whether a compositor is running, so see-through windows really show through.
pub fn compositing(display: *mut c_void, screen: c_int) -> bool {
    let Some(lib) = lib() else { return false };
    if display.is_null() {
        return false;
    }
    let Ok(name) = std::ffi::CString::new(format!("_NET_WM_CM_S{screen}")) else { return false };
    unsafe {
        let atom = (lib.intern_atom)(display, name.as_ptr(), 0);
        atom != 0 && (lib.selection_owner)(display, atom) != 0
    }
}

/// Cut the window to a rounded rectangle of `radius` physical pixels (0 = square).
pub fn round(display: *mut c_void, window: c_ulong, width: u32, height: u32, radius: u32) {
    let Some(lib) = lib() else { return };
    if display.is_null() || window == 0 || width == 0 || height == 0 {
        return;
    }
    let rects = rows(width, height, radius);
    unsafe {
        (lib.combine)(display, window, SHAPE_BOUNDING, 0, 0, rects.as_ptr(), rects.len() as c_int, SHAPE_SET, UNSORTED);
        (lib.flush)(display);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_cover_the_window() {
        let r = rows(100, 50, 10);
        assert_eq!(r.len(), 21);
        // The very top row is inset; the middle block spans the full width.
        assert!(r[0].x > 3 && r[0].width < 100);
        assert_eq!(r.last().map(|m| (m.width, m.height)), Some((100, 30)));
        // Rows near the middle of the curve are barely inset.
        assert!(r[18].x <= 1);
    }
}
