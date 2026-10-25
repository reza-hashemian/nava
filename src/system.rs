//! Talking to the desktop: media keys and the system "now playing" widget
//! (MPRIS on Linux, Now Playing on macOS), plus global shortcuts so you can
//! control music without leaving your editor.

use std::path::Path;


use crate::ipc::Cmd;

/// Global shortcuts. Ctrl+Alt+arrows are taken by GNOME, so we use , and .
/// (Linux binds the same keys in `hotkeys` below.)
#[cfg(not(target_os = "linux"))]
pub const SHORTCUTS: &[(&str, Cmd)] = &[
    ("Ctrl+Alt+Space", Cmd::Toggle),
    ("Ctrl+Alt+.", Cmd::Next),
    ("Ctrl+Alt+,", Cmd::Prev),
    ("Ctrl+Alt+M", Cmd::ToggleMini),
];

#[cfg(target_os = "linux")]
type Media = crate::mpris::Mpris;
#[cfg(not(target_os = "linux"))]
type Media = media::Media;

pub struct System {
    media: Option<Media>,
    _hotkeys: Option<hotkeys::Handle>,
    #[cfg(target_os = "windows")]
    run: fn(Cmd),
}

impl System {
    /// `run` is called on the UI thread for every key press or media-key event.
    pub fn new(run: fn(Cmd)) -> System {
        #[cfg(target_os = "linux")]
        watch_screen_lock(run);
        // Windows media controls hang off the window, which doesn't exist yet:
        // they start in `attach_window`.
        #[cfg(target_os = "windows")]
        return System { media: None, _hotkeys: hotkeys::start(run), run };
        #[cfg(target_os = "macos")]
        return System { media: Media::new(run, None), _hotkeys: hotkeys::start(run) };
        #[cfg(target_os = "linux")]
        return System { media: Media::new(run), _hotkeys: hotkeys::start(run) };
    }

    /// Windows: start the media controls once the window (its HWND) exists.
    #[cfg(target_os = "windows")]
    pub fn attach_window(&mut self, hwnd: *mut std::ffi::c_void) {
        if self.media.is_none() {
            self.media = Media::new(self.run, Some(hwnd));
        }
    }

    pub fn set_track(&mut self, title: &str, artist: &str, album: &str, duration: f64, cover: Option<&Path>) {
        let url = cover.map(|p| format!("file://{}", p.display()));
        if let Some(m) = self.media.as_mut() {
            m.set_track(title, artist, album, duration, url);
        }
    }

    pub fn set_state(&mut self, playing: bool, position: f64, has_track: bool) {
        if let Some(m) = self.media.as_mut() {
            m.set_state(playing, position, has_track);
        }
    }
}

/// GNOME and KDE announce screen locking with an `ActiveChanged(bool)` signal.
/// A thread waits for it; no polling.
#[cfg(target_os = "linux")]
fn watch_screen_lock(run: fn(Cmd)) {
    let _ = std::thread::Builder::new().name("screen-lock".into()).spawn(move || {
        let watch = || -> zbus::Result<()> {
            let conn = zbus::blocking::Connection::session()?;
            let rule = zbus::MatchRule::builder()
                .msg_type(zbus::message::Type::Signal)
                .member("ActiveChanged")?
                .build();
            for msg in zbus::blocking::MessageIterator::for_match_rule(rule, &conn, None)? {
                let Ok(msg) = msg else { continue };
                let iface = msg.header().interface().map(|i| i.to_string()).unwrap_or_default();
                if !iface.ends_with("ScreenSaver") {
                    continue;
                }
                if let Ok(active) = msg.body().deserialize::<bool>() {
                    to_ui(run, Cmd::ScreenLocked(active));
                }
            }
            Ok(())
        };
        if let Err(e) = watch() {
            eprintln!("nava: can't watch the screen lock: {e}");
        }
    });
}

/// Whether the desktop shows tray icons. On Linux that needs a
/// StatusNotifierWatcher (KDE, Ubuntu's GNOME, or GNOME with the AppIndicator extension).
pub fn tray_available() -> bool {
    #[cfg(target_os = "linux")]
    {
        let check = || -> zbus::Result<bool> {
            let conn = zbus::blocking::Connection::session()?;
            let dbus = zbus::blocking::fdo::DBusProxy::new(&conn)?;
            Ok(dbus.name_has_owner("org.kde.StatusNotifierWatcher".try_into()?)?)
        };
        check().unwrap_or(false)
    }
    #[cfg(not(target_os = "linux"))]
    {
        true
    }
}

pub fn to_ui(run: fn(Cmd), cmd: Cmd) {
    let _ = slint::invoke_from_event_loop(move || run(cmd));
}

#[cfg(not(target_os = "linux"))]
mod media {
    use std::time::Duration;

    use souvlaki::{MediaControlEvent, MediaControls, MediaMetadata, MediaPlayback, MediaPosition, PlatformConfig, SeekDirection};

    use super::{Cmd, to_ui};

    pub struct Media(MediaControls);

    impl Media {
        pub fn new(run: fn(Cmd), hwnd: Option<*mut std::ffi::c_void>) -> Option<Media> {
            let config = PlatformConfig { display_name: "Nava", dbus_name: "nava", hwnd };
            let mut controls = MediaControls::new(config).map_err(|e| eprintln!("nava: media keys unavailable: {e:?}")).ok()?;
            controls
                .attach(move |event| {
                    let cmd = match event {
                        MediaControlEvent::Play => Cmd::Play,
                        MediaControlEvent::Pause | MediaControlEvent::Stop => Cmd::Pause,
                        MediaControlEvent::Toggle => Cmd::Toggle,
                        MediaControlEvent::Next => Cmd::Next,
                        MediaControlEvent::Previous => Cmd::Prev,
                        MediaControlEvent::Seek(SeekDirection::Forward) => Cmd::SeekBy(10.0),
                        MediaControlEvent::Seek(SeekDirection::Backward) => Cmd::SeekBy(-10.0),
                        MediaControlEvent::SeekBy(SeekDirection::Forward, d) => Cmd::SeekBy(d.as_secs_f64()),
                        MediaControlEvent::SeekBy(SeekDirection::Backward, d) => Cmd::SeekBy(-d.as_secs_f64()),
                        MediaControlEvent::Raise => Cmd::Show,
                        _ => return,
                    };
                    to_ui(run, cmd);
                })
                .map_err(|e| eprintln!("nava: media keys unavailable: {e:?}"))
                .ok()?;
            Some(Media(controls))
        }

        pub fn set_track(&mut self, title: &str, artist: &str, album: &str, duration: f64, art: Option<String>) {
            let _ = self.0.set_metadata(MediaMetadata {
                title: Some(title),
                artist: (!artist.is_empty()).then_some(artist),
                album: (!album.is_empty()).then_some(album),
                cover_url: art.as_deref(),
                duration: (duration > 0.0).then(|| Duration::from_secs_f64(duration)),
            });
        }

        pub fn set_state(&mut self, playing: bool, position: f64, has_track: bool) {
            let progress = Some(MediaPosition(Duration::from_secs_f64(position.max(0.0))));
            let _ = self.0.set_playback(match (has_track, playing) {
                (false, _) => MediaPlayback::Stopped,
                (true, true) => MediaPlayback::Playing { progress },
                (true, false) => MediaPlayback::Paused { progress },
            });
        }
    }
}

/// On Linux we grab the keys on X11 ourselves: the thread blocks until a key
/// arrives, so idle cost is zero (the global-hotkey crate polls 20x a second).
#[cfg(target_os = "linux")]
mod hotkeys {
    use std::os::raw::c_int;
    use std::sync::atomic::{AtomicPtr, Ordering};

    use x11_dl::{keysym, xlib};

    use super::{Cmd, to_ui};

    pub struct Handle;

    static OUR_DISPLAY: AtomicPtr<xlib::Display> = AtomicPtr::new(std::ptr::null_mut());
    static mut PREVIOUS: Option<unsafe extern "C" fn(*mut xlib::Display, *mut xlib::XErrorEvent) -> c_int> = None;

    /// A key already taken by another app makes X report an error. Ignore errors
    /// on our own connection and pass everything else to the previous handler.
    unsafe extern "C" fn on_error(d: *mut xlib::Display, e: *mut xlib::XErrorEvent) -> c_int {
        if d == OUR_DISPLAY.load(Ordering::Relaxed) {
            return 0;
        }
        match unsafe { PREVIOUS } {
            Some(prev) => unsafe { prev(d, e) },
            None => 0,
        }
    }

    pub fn start(run: fn(Cmd)) -> Option<Handle> {
        let xl = xlib::Xlib::open().ok()?;
        let display = unsafe { (xl.XOpenDisplay)(std::ptr::null()) };
        if display.is_null() {
            return None; // Wayland without XWayland, or no display at all
        }
        OUR_DISPLAY.store(display, Ordering::Relaxed);
        let keys: Vec<(u32, Cmd)> = unsafe {
            PREVIOUS = (xl.XSetErrorHandler)(Some(on_error));
            let root = (xl.XDefaultRootWindow)(display);
            let base = xlib::ControlMask | xlib::Mod1Mask;
            // Also grab with Caps Lock / Num Lock on, or the shortcut silently fails.
            let locks = [0, xlib::LockMask, xlib::Mod2Mask, xlib::LockMask | xlib::Mod2Mask];
            let wanted = [
                (keysym::XK_space, Cmd::Toggle),
                (keysym::XK_period, Cmd::Next),
                (keysym::XK_comma, Cmd::Prev),
                (keysym::XK_m, Cmd::ToggleMini),
            ];
            let mut keys = Vec::new();
            for (sym, cmd) in wanted {
                let code = (xl.XKeysymToKeycode)(display, sym as u64);
                if code == 0 {
                    continue;
                }
                for lock in locks {
                    (xl.XGrabKey)(display, code as c_int, base | lock, root, 1, xlib::GrabModeAsync, xlib::GrabModeAsync);
                }
                keys.push((code as u32, cmd));
            }
            (xl.XSync)(display, 0);
            keys
        };
        let display = display as usize; // raw pointers aren't Send
        std::thread::Builder::new()
            .name("hotkeys".into())
            .spawn(move || {
                let display = display as *mut xlib::Display;
                let mut ev: xlib::XEvent = unsafe { std::mem::zeroed() };
                loop {
                    unsafe { (xl.XNextEvent)(display, &mut ev) };
                    if ev.get_type() != xlib::KeyPress {
                        continue;
                    }
                    let k = unsafe { ev.key };
                    let mods = k.state & (xlib::ControlMask | xlib::Mod1Mask | xlib::ShiftMask | xlib::Mod4Mask);
                    if mods != xlib::ControlMask | xlib::Mod1Mask {
                        continue;
                    }
                    if let Some((_, cmd)) = keys.iter().find(|(code, _)| *code == k.keycode) {
                        to_ui(run, cmd.clone());
                    }
                }
            })
            .ok()?;
        Some(Handle)
    }
}

#[cfg(not(target_os = "linux"))]
mod hotkeys {
    use global_hotkey::hotkey::{Code, HotKey, Modifiers};
    use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};

    use super::{Cmd, SHORTCUTS, to_ui};

    pub type Handle = GlobalHotKeyManager;

    pub fn start(run: fn(Cmd)) -> Option<Handle> {
        let manager = GlobalHotKeyManager::new().map_err(|e| eprintln!("nava: global shortcuts unavailable: {e}")).ok()?;
        let mut bound: Vec<(u32, Cmd)> = Vec::new();
        for (name, cmd) in SHORTCUTS {
            let key = match *name {
                "Ctrl+Alt+Space" => Code::Space,
                "Ctrl+Alt+." => Code::Period,
                "Ctrl+Alt+," => Code::Comma,
                _ => Code::KeyM,
            };
            let hk = HotKey::new(Some(Modifiers::CONTROL | Modifiers::ALT), key);
            match manager.register(hk) {
                Ok(()) => bound.push((hk.id(), cmd.clone())),
                Err(e) => eprintln!("nava: couldn't bind {name}: {e}"),
            }
        }
        GlobalHotKeyEvent::set_event_handler(Some(move |e: GlobalHotKeyEvent| {
            if e.state == HotKeyState::Pressed {
                if let Some((_, cmd)) = bound.iter().find(|(id, _)| *id == e.id) {
                    to_ui(run, cmd.clone());
                }
            }
        }));
        Some(manager)
    }
}
