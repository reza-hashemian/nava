//! Thin, safe wrapper over the libmpv client API.
//!
//! We only bind the handful of functions we need, so there is no build-time
//! dependency beyond linking `libmpv` itself.

use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::ptr;

#[repr(C)]
pub struct MpvHandle {
    _private: [u8; 0],
}

#[repr(C)]
struct RawEvent {
    event_id: c_int,
    error: c_int,
    reply_userdata: u64,
    data: *mut c_void,
}

#[repr(C)]
struct RawEventProperty {
    name: *const c_char,
    format: c_int,
    data: *mut c_void,
}

#[repr(C)]
struct RawEventEndFile {
    reason: c_int,
    error: c_int,
}

const FORMAT_STRING: c_int = 1;
const FORMAT_FLAG: c_int = 3;
const FORMAT_DOUBLE: c_int = 5;

const EVENT_NONE: c_int = 0;
const EVENT_SHUTDOWN: c_int = 1;
const EVENT_END_FILE: c_int = 7;
const EVENT_FILE_LOADED: c_int = 8;
const EVENT_PROPERTY_CHANGE: c_int = 22;

const END_FILE_EOF: c_int = 0;
const END_FILE_ERROR: c_int = 4;

#[link(name = "mpv")]
unsafe extern "C" {
    fn mpv_create() -> *mut MpvHandle;
    fn mpv_initialize(ctx: *mut MpvHandle) -> c_int;
    fn mpv_terminate_destroy(ctx: *mut MpvHandle);
    fn mpv_set_option_string(ctx: *mut MpvHandle, name: *const c_char, data: *const c_char) -> c_int;
    fn mpv_command(ctx: *mut MpvHandle, args: *mut *const c_char) -> c_int;
    fn mpv_set_property(ctx: *mut MpvHandle, name: *const c_char, format: c_int, data: *mut c_void) -> c_int;
    fn mpv_set_property_string(ctx: *mut MpvHandle, name: *const c_char, data: *const c_char) -> c_int;
    fn mpv_free(data: *mut c_void);
    fn mpv_get_property(ctx: *mut MpvHandle, name: *const c_char, format: c_int, data: *mut c_void) -> c_int;
    fn mpv_observe_property(ctx: *mut MpvHandle, userdata: u64, name: *const c_char, format: c_int) -> c_int;
    fn mpv_wait_event(ctx: *mut MpvHandle, timeout: f64) -> *mut RawEvent;
    fn mpv_error_string(error: c_int) -> *const c_char;
}

unsafe extern "C" {
    fn setlocale(category: c_int, locale: *const c_char) -> *mut c_char;
}

#[cfg(target_os = "linux")]
const LC_NUMERIC: c_int = 1;
#[cfg(target_os = "macos")]
const LC_NUMERIC: c_int = 4;
#[cfg(target_os = "windows")]
const LC_NUMERIC: c_int = 4;

/// What the event thread reports back to the app.
#[derive(Debug, Clone)]
pub enum Event {
    FileLoaded,
    /// The current file finished on its own (not because we replaced it).
    EndOfFile,
    /// The current file could not be played.
    FileError,
    Duration(f64),
    Paused(bool),
    Shutdown,
}

pub struct Mpv {
    ctx: *mut MpvHandle,
}

// The libmpv client API is thread-safe; the handle may be used from any thread.
unsafe impl Send for Mpv {}
unsafe impl Sync for Mpv {}

fn cstr(s: &str) -> CString {
    CString::new(s.replace('\0', "")).expect("no interior nul")
}

fn check(code: c_int, what: &str) -> anyhow::Result<()> {
    if code >= 0 {
        return Ok(());
    }
    let msg = unsafe { CStr::from_ptr(mpv_error_string(code)) }.to_string_lossy();
    anyhow::bail!("mpv: {what}: {msg}")
}

impl Mpv {
    pub fn new() -> anyhow::Result<Self> {
        // mpv refuses to start unless numbers are formatted the C way.
        unsafe { setlocale(LC_NUMERIC, c"C".as_ptr()) };
        let ctx = unsafe { mpv_create() };
        if ctx.is_null() {
            anyhow::bail!("mpv_create failed");
        }
        let mpv = Mpv { ctx };
        for (k, v) in [
            ("config", "no"),
            ("terminal", "no"),
            ("idle", "yes"),
            // Video is drawn by Nava through the render API (see video.rs).
            ("vo", "libmpv"),
            ("hwdec", "auto-safe"),
            // Simpler scaling shaders: much lighter on modest GPUs, hard to tell apart.
            ("profile", "fast"),
            ("audio-display", "no"),
            ("keep-open", "no"),
            // Subtitles: find matching files next to the video, detect their
            // encoding (old Persian .srt files are often Windows-1256), and use
            // a font that renders Persian well.
            ("sub-auto", "fuzzy"),
            ("sub-codepage", "auto"),
            ("sub-font", "Vazirmatn"),
            ("sub-font-size", "44"),
            ("sub-border-size", "2.4"),
            ("sub-shadow-offset", "1.2"),
            ("sub-shadow-color", "#80000000"),
            ("sub-margin-y", "34"),
            ("osd-level", "0"),
            ("input-default-bindings", "no"),
            ("input-vo-keyboard", "no"),
            ("osc", "no"),
            // Links (YouTube and the like) go through yt-dlp. Prefer H.264 up to
            // 1080p: it's what most GPUs, including older Intel ones, decode in hardware.
            ("ytdl", "yes"),
            ("ytdl-format", "bv*[height<=?1080][vcodec^=avc1]+ba/b[height<=?1080]/bv*+ba/b"),
            ("load-scripts", "no"),
            ("audio-client-name", "Nava"),
            ("volume-max", "100"),
            ("gapless-audio", "weak"),
        ] {
            mpv.set_option(k, v)?;
        }
        // Debugging: NAVA_MPV_LOG=1 prints mpv's own log to the terminal.
        if std::env::var_os("NAVA_MPV_LOG").is_some() {
            mpv.set_option("terminal", "yes")?;
            mpv.set_option("msg-level", &std::env::var("NAVA_MPV_LOG").unwrap_or_default().replace("1", "all=info,vo=v,vd=v"))?;
        }
        // Lets tests and screenshots run silently: NAVA_AO=null
        if let Ok(ao) = std::env::var("NAVA_AO") {
            mpv.set_option("ao", &ao)?;
        }
        check(unsafe { mpv_initialize(ctx) }, "initialize")?;
        for (id, name, fmt) in [(1, "duration", FORMAT_DOUBLE), (2, "pause", FORMAT_FLAG)] {
            let n = cstr(name);
            check(unsafe { mpv_observe_property(ctx, id, n.as_ptr(), fmt) }, name)?;
        }
        Ok(mpv)
    }

    fn set_option(&self, name: &str, value: &str) -> anyhow::Result<()> {
        let (n, v) = (cstr(name), cstr(value));
        check(unsafe { mpv_set_option_string(self.ctx, n.as_ptr(), v.as_ptr()) }, name)
    }

    pub fn command(&self, args: &[&str]) -> anyhow::Result<()> {
        let owned: Vec<CString> = args.iter().map(|a| cstr(a)).collect();
        let mut ptrs: Vec<*const c_char> = owned.iter().map(|c| c.as_ptr()).collect();
        ptrs.push(ptr::null());
        check(unsafe { mpv_command(self.ctx, ptrs.as_mut_ptr()) }, args.first().copied().unwrap_or(""))
    }

    /// Replace whatever is playing with `path`, starting at `start` seconds.
    pub fn load(&self, path: &str, start: f64) -> anyhow::Result<()> {
        let (target, audio_only) = crate::library::link_target(std::path::Path::new(path));
        let mut opts = format!("start={start:.3}");
        if audio_only {
            opts.push_str(",vid=no,ytdl-format=ba/b");
        }
        self.command(&["loadfile", &target, "replace", "-1", &opts])
    }

    pub fn stop(&self) {
        let _ = self.command(&["stop"]);
    }

    pub fn seek_absolute(&self, secs: f64) {
        let s = format!("{secs:.3}");
        let _ = self.command(&["seek", &s, "absolute+exact"]);
    }

    pub fn set_double(&self, name: &str, mut v: f64) {
        let n = cstr(name);
        unsafe { mpv_set_property(self.ctx, n.as_ptr(), FORMAT_DOUBLE, &mut v as *mut f64 as *mut c_void) };
    }

    pub fn set_flag(&self, name: &str, v: bool) {
        let n = cstr(name);
        let mut f: c_int = v as c_int;
        unsafe { mpv_set_property(self.ctx, n.as_ptr(), FORMAT_FLAG, &mut f as *mut c_int as *mut c_void) };
    }

    pub fn raw(&self) -> *mut MpvHandle {
        self.ctx
    }

    pub fn set_string(&self, name: &str, value: &str) {
        let (n, v) = (cstr(name), cstr(value));
        unsafe { mpv_set_property_string(self.ctx, n.as_ptr(), v.as_ptr()) };
    }

    pub fn get_string(&self, name: &str) -> Option<String> {
        let n = cstr(name);
        let mut p: *mut c_char = ptr::null_mut();
        let r = unsafe { mpv_get_property(self.ctx, n.as_ptr(), FORMAT_STRING, &mut p as *mut *mut c_char as *mut c_void) };
        if r < 0 || p.is_null() {
            return None;
        }
        let s = unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned();
        unsafe { mpv_free(p as *mut c_void) };
        Some(s)
    }

    pub fn get_double(&self, name: &str) -> Option<f64> {
        let n = cstr(name);
        let mut v: f64 = 0.0;
        let r = unsafe { mpv_get_property(self.ctx, n.as_ptr(), FORMAT_DOUBLE, &mut v as *mut f64 as *mut c_void) };
        (r >= 0).then_some(v)
    }

    /// Block until the next event. Only one thread may call this.
    pub fn wait_event(&self, timeout: f64) -> Option<Event> {
        let ev = unsafe { &*mpv_wait_event(self.ctx, timeout) };
        match ev.event_id {
            EVENT_NONE => None,
            EVENT_SHUTDOWN => Some(Event::Shutdown),
            EVENT_FILE_LOADED => Some(Event::FileLoaded),
            EVENT_END_FILE => {
                let end = unsafe { &*(ev.data as *const RawEventEndFile) };
                match end.reason {
                    END_FILE_EOF => Some(Event::EndOfFile),
                    END_FILE_ERROR => Some(Event::FileError),
                    _ => None,
                }
            }
            EVENT_PROPERTY_CHANGE => {
                let p = unsafe { &*(ev.data as *const RawEventProperty) };
                if p.data.is_null() {
                    return None;
                }
                let name = unsafe { CStr::from_ptr(p.name) }.to_str().unwrap_or("");
                match (name, p.format) {
                    ("duration", FORMAT_DOUBLE) => Some(Event::Duration(unsafe { *(p.data as *const f64) })),
                    ("pause", FORMAT_FLAG) => Some(Event::Paused(unsafe { *(p.data as *const c_int) } != 0)),
                    _ => None,
                }
            }
            _ => None,
        }
    }
}

impl Drop for Mpv {
    fn drop(&mut self) {
        unsafe { mpv_terminate_destroy(self.ctx) };
    }
}
