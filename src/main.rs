// Hide the console window on Windows release builds.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod browse;
mod catalog;
mod eq;
mod ipc;
mod library;
mod lyrics;
mod meta;
mod mpv;
#[cfg(target_os = "linux")]
mod mpris;
#[cfg(target_os = "linux")]
mod shape;
mod store;
mod system;
mod tools;
mod video;

use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use slint::winit_030::WinitWindowAccessor;
use slint::{ComponentHandle, Image, Model, ModelRc, SharedString, Timer, TimerMode, VecModel};

use ipc::Cmd;

use library::Pick;
use meta::{Art, Tags};
use mpv::{Event, Mpv};
use store::{Session, Store};

slint::include_modules!();

const CODE_SESSION: &str = "Code";
const HELP: &str = "\
Nava — a calm, glassy music player

USAGE
  nava                      Open the last session where you left off
  nava --code               Code mode: mini player, Code session, starts playing
  nava [OPTIONS] PATH...    Play files, folders or links (YouTube and more, via yt-dlp)

CONTROL A RUNNING NAVA
  nava toggle | play | pause | next | prev
  nava mini | full | show
  nava sleep 30 | end | off     sleep timer: minutes, end of this track, or off
  nava speed 1.5               playback speed, remembered for the folder
  nava mark [NOTE]             bookmark this moment (with an optional note)
  nava status [--format FMT]   e.g. --format \"{state} {title} · {pos}/{dur}\"
                               fields: {state} {title} {artist} {album} {pos} {dur} {session}

OPTIONS
  --session NAME     Use a session: Main, Code, Relax or one you made
  --limit N          Only queue N files
  --pick MODE        first | random | newest   (default: first)
  --no-subdirs       Don't look inside subfolders
  --audio-only       For links: play the sound only (no picture)
  -h, --help         Show this help

SHORTCUTS (work everywhere, X11/Windows/macOS)
  Ctrl+Alt+Space  play/pause    Ctrl+Alt+.  next    Ctrl+Alt+,  previous
  Ctrl+Alt+M      mini player / full window
";

#[derive(Default)]
struct Args {
    /// A command for an already running Nava, like `nava next`.
    remote: Option<Cmd>,
    code: bool,
    audio_only: bool,
    session: Option<String>,
    limit: Option<usize>,
    pick: Option<Pick>,
    recursive: bool,
    paths: Vec<PathBuf>,
}

fn parse_args() -> Result<Args, String> {
    let mut a = Args { recursive: true, ..Default::default() };
    let mut it = std::env::args().skip(1).peekable();
    // `nava next`, `nava status --format ...` and friends.
    if let Some(first) = it.peek().cloned() {
        let with_arg = |name: &str, it: &mut std::iter::Peekable<std::iter::Skip<std::env::Args>>| -> Result<String, String> {
            it.next();
            let rest: Vec<String> = it.by_ref().collect();
            if rest.is_empty() { Err(format!("{name} needs a value")) } else { Ok(rest.join(" ")) }
        };
        let cmd = match first.as_str() {
            "sleep" => Some(Cmd::Sleep(with_arg("sleep", &mut it)?)),
            "speed" => {
                let v = with_arg("speed", &mut it)?;
                Some(Cmd::Speed(v.trim_end_matches(['x', '×']).parse().map_err(|_| format!("'{v}' is not a speed"))?))
            }
            "mark" => {
                it.next();
                Some(Cmd::Mark(it.by_ref().collect::<Vec<_>>().join(" ")))
            }
            _ => None,
        };
        if let Some(cmd) = cmd {
            a.remote = Some(cmd);
            return Ok(a);
        }
        let simple = match first.as_str() {
            "toggle" => Some(Cmd::Toggle),
            "pause" => Some(Cmd::Pause),
            "next" => Some(Cmd::Next),
            "prev" | "previous" => Some(Cmd::Prev),
            "mini" => Some(Cmd::Mini),
            "full" => Some(Cmd::Full),
            "show" => Some(Cmd::Show),
            "status" => Some(Cmd::Status(String::new())),
            // `nava play` alone resumes; `nava play PATH...` plays those files.
            "play" => {
                it.next();
                if it.peek().is_none() { Some(Cmd::Play) } else { None }
            }
            _ => None,
        };
        if let Some(cmd) = simple {
            if !matches!(cmd, Cmd::Play) {
                it.next();
            }
            let cmd = match cmd {
                Cmd::Status(_) => {
                    let mut fmt = String::new();
                    while let Some(a) = it.next() {
                        match a.as_str() {
                            "--format" | "-f" => fmt = it.next().ok_or("--format needs a value")?,
                            other => return Err(format!("unknown status option '{other}'")),
                        }
                    }
                    Cmd::Status(fmt)
                }
                other => other,
            };
            a.remote = Some(cmd);
            return Ok(a);
        }
    }
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                print!("{HELP}");
                std::process::exit(0);
            }
            "--code" => a.code = true,
            "--no-subdirs" => a.recursive = false,
            "--audio-only" => a.audio_only = true,
            "--session" => {
                let v = it.next().ok_or("--session needs a name")?;
                // Sessions you made are checked once the store is open.
                let name = store::BUILT_IN_SESSIONS.iter().find(|s| s.eq_ignore_ascii_case(&v)).map(|s| s.to_string());
                a.session = Some(name.unwrap_or(v));
            }
            "--limit" => {
                let v = it.next().ok_or("--limit needs a number")?;
                a.limit = Some(v.parse().map_err(|_| format!("'{v}' is not a number"))?);
            }
            "--pick" => {
                let v = it.next().ok_or("--pick needs first, random or newest")?;
                a.pick = Some(Pick::parse(&v).ok_or(format!("unknown pick mode '{v}'"))?);
            }
            s if s.starts_with("--") => return Err(format!("unknown option '{s}'")),
            p => a.paths.push(PathBuf::from(p)),
        }
    }
    Ok(a)
}

struct Item {
    path: PathBuf,
    tags: Option<Tags>,
}

impl Item {
    fn new(path: PathBuf) -> Item {
        Item { path, tags: None }
    }
    fn row(&self) -> TrackRow {
        match &self.tags {
            Some(t) => TrackRow {
                title: t.title.clone().into(),
                artist: t.artist.clone().into(),
                duration: if t.duration > 0.0 { fmt_time(t.duration).into() } else { SharedString::new() },
            },
            None => TrackRow {
                title: file_stem(&self.path).into(),
                artist: SharedString::new(),
                duration: SharedString::new(),
            },
        }
    }
}

fn file_stem(p: &Path) -> String {
    p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
}

fn fmt_time(t: f64) -> String {
    let t = t.max(0.0) as u64;
    if t >= 3600 {
        format!("{}:{:02}:{:02}", t / 3600, t / 60 % 60, t % 60)
    } else {
        format!("{}:{:02}", t / 60, t % 60)
    }
}

struct Player {
    ui: slint::Weak<AppWindow>,
    mpv: Arc<Mpv>,
    store: Store,
    session: String,
    items: Vec<Item>,
    /// The order before shuffling, so turning shuffle off restores it.
    unshuffled: Option<Vec<PathBuf>>,
    index: Option<usize>,
    model: Rc<VecModel<TrackRow>>,
    volume: f64,
    shuffle: bool,
    repeat: i32,
    playing: bool,
    loaded: bool,
    position: f64,
    duration: f64,
    errors_in_a_row: usize,
    art_gen: u64,
    tags_gen: u64,
    sheet: Option<(PathBuf, Vec<PathBuf>)>,
    fade: Option<Timer>,
    note_timer: Option<Timer>,
    /// Moves the scrubber. Runs only while music plays, so a paused Nava sleeps.
    ticker: Timer,
    ticks: u32,
    mini: bool,
    system: Option<system::System>,
    cover_file: Option<PathBuf>,
    /// True when Slint draws with OpenGL, which video needs.
    gl: bool,
    is_video: bool,
    current_path: Option<PathBuf>,
    /// Hides the video controls a moment after the mouse stops.
    hud_timer: Timer,
    /// A video waiting for the renderer: (start, autoplay).
    pending_video: Option<(f64, bool)>,

    // Library browsing (see browse.rs)
    catalog: catalog::Catalog,
    view: String,
    /// The songs on screen, in order, for playing from a list.
    list: Vec<PathBuf>,
    lib_model: browse::LibModel,
    album_grid: Option<Rc<browse::AlbumGrid>>,
    artists: Vec<String>,
    filter: Option<catalog::Filter>,
    search_timer: Timer,
    rescan_timer: Timer,
    scanning: bool,

    eq: eq::Eq,
    pause_on_lock: bool,
    paused_by_lock: bool,

    // Sleep timer, speed, A-B loop, bookmarks (see tools.rs)
    sleep: Option<tools::Sleep>,
    sleep_timer: Timer,
    sleep_label_timer: Timer,
    speed: f64,
    ab: (f64, f64),
    bookmark_ids: Vec<(i64, f64)>,

    lyrics: Option<lyrics::Lyrics>,
    lyrics_gen: u64,
    lyrics_index: i32,
    show_lyrics: bool,
    online_lyrics: bool,

    /// A file waiting for "Move to Trash" to be confirmed.
    pending_delete: Option<PathBuf>,

    tray: Option<NavaTray>,
    /// Closing the window keeps Nava running in the tray (only when a tray exists).
    close_to_tray: bool,
}


/// Set when the app should restart itself with OpenGL to show a video.
static RELAUNCH_FOR_VIDEO: AtomicBool = AtomicBool::new(false);
/// Set when closing: the video renderer must be freed on the render thread first.
static QUITTING: AtomicBool = AtomicBool::new(false);
/// Set once mpv's video renderer exists. Videos loaded before that get no picture.
static VIDEO_READY: AtomicBool = AtomicBool::new(false);
/// Whether Slint draws with OpenGL. A static so `quit()` can run while the player is borrowed.
static GL_MODE: AtomicBool = AtomicBool::new(false);
/// Whether the renderer can draw a see-through window (Skia and OpenGL can,
/// Slint's own software renderer can't).
static SEE_THROUGH: AtomicBool = AtomicBool::new(false);

/// Ask Slint for a new frame (called when mpv has a video frame ready).
pub fn request_redraw() {
    WINDOW.with(|w| {
        if let Some(ui) = w.borrow().as_ref().and_then(|w| w.upgrade()) {
            ui.window().request_redraw();
        }
    });
}

thread_local! {
    /// The window on its own, so redraws can be asked for while the player is borrowed.
    static WINDOW: RefCell<Option<slint::Weak<AppWindow>>> = const { RefCell::new(None) };
}

/// Quit, letting the video renderer shut down cleanly first.
fn quit() {
    if GL_MODE.load(Ordering::Acquire) {
        QUITTING.store(true, Ordering::Release);
        request_redraw();
        // If no frame comes (minimised window), quit anyway.
        Timer::single_shot(Duration::from_millis(400), || {
            let _ = slint::quit_event_loop();
        });
    } else {
        let _ = slint::quit_event_loop();
    }
}

thread_local! {
    static PLAYER: RefCell<Option<Player>> = const { RefCell::new(None) };
}

/// Run `f` against the player. Callbacks and worker results all come through here.
fn with<R>(f: impl FnOnce(&mut Player) -> R) -> Option<R> {
    PLAYER.with(|p| p.borrow_mut().as_mut().map(f))
}

impl Player {
    fn ui(&self) -> AppWindow {
        self.ui.upgrade().expect("window alive")
    }

    // ---------- sessions ----------

    fn snapshot(&self) -> Session {
        Session {
            queue: self.items.iter().map(|i| i.path.clone()).collect(),
            index: self.index.unwrap_or(0),
            position: self.position,
            volume: self.volume,
            shuffle: self.shuffle,
            repeat: self.repeat,
        }
    }

    fn save(&self) {
        self.remember_video_position();
        self.store.save_session(&self.session, &self.snapshot());
        self.store.set_setting("last_session", &self.session);
    }

    fn refresh_sessions(&self) {
        let rows: Vec<SessionRow> = self
            .store
            .session_names()
            .iter()
            .map(|name| {
                let n = if *name == self.session {
                    self.items.len()
                } else {
                    self.store.load_session(name).map(|s| s.queue.len()).unwrap_or(0)
                };
                let icon = match name.as_str() {
                    "Code" => "M8.5 7l-5 5 5 5 M15.5 7l5 5-5 5",
                    "Relax" => "M20 14.6A8 8 0 1 1 9.4 4a6.4 6.4 0 0 0 10.6 10.6z",
                    "Main" => "M12 3a9 9 0 1 0 0 18a9 9 0 1 0 0-18z M12 10a2 2 0 1 0 0 4a2 2 0 1 0 0-4z",
                    _ => "M9 6h11 M9 12h11 M9 18h11 M4.5 6h.01 M4.5 12h.01 M4.5 18h.01",
                };
                SessionRow {
                    name: name.as_str().into(),
                    icon: icon.into(),
                    custom: !store::BUILT_IN_SESSIONS.contains(&name.as_str()),
                    subtitle: match n {
                        0 => "Empty".into(),
                        1 => "1 track".into(),
                        n => format!("{n} tracks").into(),
                    },
                }
            })
            .collect();
        let ui = self.ui();
        ui.set_sessions(ModelRc::new(VecModel::from(rows)));
        ui.set_session(self.session.clone().into());
    }

    /// Load a saved session. `autoplay` starts it with a gentle fade-in.
    fn open_session(&mut self, name: &str, autoplay: bool) {
        self.session = name.to_string();
        let s = self.store.load_session(name).unwrap_or_default();
        let current = s.queue.get(s.index).cloned();
        let queue: Vec<PathBuf> = s.queue.into_iter().filter(|p| library::is_link(p) || library::is_media_file(p)).collect();
        // Keep pointing at the same file even if earlier ones went missing.
        let index = current.as_ref().and_then(|c| queue.iter().position(|p| p == c));
        let position = if index.is_some() { s.position } else { 0.0 };
        self.volume = s.volume;
        self.shuffle = s.shuffle;
        self.repeat = s.repeat;
        self.unshuffled = None;
        self.mpv.set_double("volume", self.volume);
        self.set_queue(queue, index.unwrap_or(0));
        let ui = self.ui();
        ui.set_volume(self.volume as f32);
        ui.set_shuffle(self.shuffle);
        ui.set_repeat_mode(self.repeat);
        if self.items.is_empty() {
            self.mpv.stop();
            self.clear_now_playing();
        } else {
            self.load_current(position, autoplay);
            if autoplay {
                self.fade_in(self.volume);
            }
            if position > 5.0 {
                self.show_note(format!("Resumed where you left off · {}", fmt_time(position)));
            }
        }
        self.refresh_sessions();
        self.store.set_setting("last_session", name);
    }

    fn switch_session(&mut self, name: &str) {
        if name == self.session {
            return;
        }
        self.save();
        self.open_session(name, name == CODE_SESSION);
    }

    /// A new, empty session from the sidebar's "+".
    fn create_session(&mut self, name: &str) {
        let Some(name) = self.store.add_session(name) else { return };
        if self.store.load_session(&name).is_none() {
            self.store.save_session(&name, &Session { volume: self.volume, ..Session::default() });
        }
        self.switch_session(&name);
        self.refresh_sessions();
    }

    /// From a session's right-click menu: "clear" empties its list (the files
    /// stay where they are), "delete" forgets a session you made.
    fn session_action(&mut self, name: &str, action: &str) {
        match action {
            "clear" if name == self.session => self.clear_queue(),
            "clear" => {
                if let Some(mut s) = self.store.load_session(name) {
                    s.queue.clear();
                    s.index = 0;
                    s.position = 0.0;
                    self.store.save_session(name, &s);
                }
            }
            "delete" => {
                if store::BUILT_IN_SESSIONS.contains(&name) {
                    return;
                }
                if name == self.session {
                    self.switch_session("Main");
                }
                self.store.delete_session(name);
            }
            _ => return,
        }
        self.refresh_sessions();
    }

    /// Empty the queue. Only the list goes; no file is touched.
    fn clear_queue(&mut self) {
        self.remember_video_position();
        self.mpv.stop();
        self.set_video_mode(false);
        self.current_path = None;
        self.items.clear();
        self.unshuffled = None;
        self.position = 0.0;
        self.duration = 0.0;
        self.clear_now_playing();
        self.rebuild_model();
        self.refresh_sessions();
        self.save();
        if let Some(tray) = &self.tray {
            tray.set_now_playing("Nothing playing".into());
        }
    }

    // ---------- library folders ----------

    fn refresh_library_folders(&self) {
        let rows: Vec<SharedString> = self.catalog.dirs().iter().map(|d| d.display().to_string().into()).collect();
        self.ui().set_library_folders(ModelRc::new(VecModel::from(rows)));
    }

    /// Take a folder out of the library. Nothing is deleted from disk.
    fn remove_library_folder(&mut self, i: usize) {
        let Some(dir) = self.catalog.dirs().get(i).cloned() else { return };
        self.catalog.remove_dir(&dir);
        self.refresh_library_folders();
        self.ui().set_library_empty(self.catalog.is_empty());
        self.album_grid = None;
        if self.view != "now" {
            let v = if matches!(self.view.as_str(), "album" | "artist" | "search") { "songs".to_string() } else { self.view.clone() };
            self.show_view(&v);
        }
        let name = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| dir.display().to_string());
        self.show_note(format!("Removed “{name}” from the library"));
    }

    // ---------- queue ----------

    fn set_queue(&mut self, paths: Vec<PathBuf>, index: usize) {
        self.items = paths.into_iter().map(Item::new).collect();
        self.index = if self.items.is_empty() { None } else { Some(index.min(self.items.len() - 1)) };
        self.rebuild_model();
        self.read_queue_tags();
    }

    fn rebuild_model(&mut self) {
        self.model = Rc::new(VecModel::from(self.items.iter().map(Item::row).collect::<Vec<_>>()));
        let ui = self.ui();
        ui.set_queue(ModelRc::from(self.model.clone()));
        ui.set_current_index(self.index.map(|i| i as i32).unwrap_or(-1));
    }

    /// Read tags for the whole queue on a worker thread, in small batches.
    fn read_queue_tags(&mut self) {
        self.tags_gen += 1;
        let generation = self.tags_gen;
        let todo: Vec<(usize, PathBuf)> = self
            .items
            .iter()
            .enumerate()
            .filter(|(_, it)| it.tags.is_none())
            .map(|(i, it)| (i, it.path.clone()))
            .collect();
        if todo.is_empty() {
            return;
        }
        std::thread::spawn(move || {
            for chunk in todo.chunks(24) {
                let batch: Vec<(usize, PathBuf, Tags)> =
                    chunk.iter().map(|(i, p)| (*i, p.clone(), meta::read_tags(p))).collect();
                let sent = slint::invoke_from_event_loop(move || {
                    with(|pl| pl.apply_tags(generation, batch));
                });
                if sent.is_err() {
                    return;
                }
            }
        });
    }

    fn apply_tags(&mut self, generation: u64, batch: Vec<(usize, PathBuf, Tags)>) {
        if generation != self.tags_gen {
            return;
        }
        for (i, path, tags) in batch {
            let Some(item) = self.items.get_mut(i) else { continue };
            if item.path != path {
                continue;
            }
            item.tags = Some(tags);
            if i < self.model.row_count() {
                self.model.set_row_data(i, item.row());
            }
            if Some(i) == self.index {
                self.show_titles();
            }
        }
    }

    // ---------- playback ----------

    fn remember_video_position(&self) {
        if let (true, Some(path)) = (self.is_video, &self.current_path) {
            self.store.set_video_position(path, self.position, self.duration);
        }
    }

    fn load_current(&mut self, start: f64, autoplay: bool) {
        let Some(i) = self.index else { return };
        let path = self.items[i].path.clone();
        let video = library::is_video(&path);
        if video && !self.gl {
            // The light software renderer can't show video: restart with OpenGL.
            self.position = start;
            self.save();
            self.save_geometry();
            RELAUNCH_FOR_VIDEO.store(true, Ordering::Release);
            let _ = slint::quit_event_loop();
            return;
        }
        if self.current_path.as_ref() != Some(&path) {
            self.remember_video_position();
        }
        let mut start = start;
        if video && start < 1.0 {
            if let Some(saved) = self.store.video_position(&path) {
                start = saved;
                self.show_note(format!("Resumed at {}", fmt_time(saved)));
            }
        }
        self.set_video_mode(video);
        let changed = self.current_path.as_ref() != Some(&path);
        self.current_path = Some(path.clone());
        if changed {
            self.on_track_changed();
            self.track_tools_changed(&path);
            self.request_lyrics();
            self.eq_track_changed();
        }
        if video && !VIDEO_READY.load(Ordering::Acquire) {
            // The window hasn't drawn yet; play once the video renderer exists.
            self.pending_video = Some((start, autoplay));
            self.position = start;
            self.show_titles();
            return;
        }
        if self.items[i].tags.is_none() {
            self.items[i].tags = Some(meta::read_tags(&path));
            self.model.set_row_data(i, self.items[i].row());
        }
        self.mpv.set_flag("pause", !autoplay);
        if let Err(e) = self.mpv.load(&path.to_string_lossy(), start) {
            eprintln!("nava: {e}");
        }
        self.position = start;
        self.duration = self.items[i].tags.as_ref().map(|t| t.duration).unwrap_or(0.0);
        self.set_playing(autoplay);
        let ui = self.ui();
        ui.set_current_index(i as i32);
        ui.set_position(start as f32);
        ui.set_duration(self.duration as f32);
        self.show_titles();
        self.request_art(path);
    }

    fn show_titles(&mut self) {
        let Some(item) = self.index.and_then(|i| self.items.get(i)) else { return };
        let ui = self.ui();
        let t = item.tags.clone().unwrap_or_default();
        let title = if t.title.is_empty() { file_stem(&item.path) } else { t.title.clone() };
        ui.set_track_title(title.clone().into());
        ui.set_track_artist(t.artist.clone().into());
        ui.set_track_album(t.album.clone().into());
        ui.set_window_title(format!("{title} — Nava").into());
        let cover = self.cover_file.clone();
        if let Some(sys) = self.system.as_mut() {
            sys.set_track(&title, &t.artist, &t.album, t.duration, cover.as_deref());
        }
        if let Some(tray) = &self.tray {
            let line = if t.artist.is_empty() { title.clone() } else { format!("{title} — {}", t.artist) };
            tray.set_now_playing(line.into());
            tray.set_has_track(true);
        }
    }

    fn clear_now_playing(&mut self) {
        self.index = None;
        self.loaded = false;
        self.set_playing(false);
        let ui = self.ui();
        ui.set_current_index(-1);
        ui.set_window_title("Nava".into());
        // No artwork yet: paint the glass with a soft generated gradient instead.
        self.request_art(PathBuf::from("/nava/welcome"));
    }

    fn request_art(&mut self, path: PathBuf) {
        self.art_gen += 1;
        let generation = self.art_gen;
        std::thread::spawn(move || {
            let art = meta::load_art(&path);
            let _ = slint::invoke_from_event_loop(move || {
                with(|p| p.apply_art(generation, art));
            });
        });
    }

    fn apply_art(&mut self, generation: u64, art: Art) {
        if generation != self.art_gen {
            return;
        }
        if self.cover_file != art.file {
            self.cover_file = art.file.clone();
            self.show_titles();
        }
        let ui = self.ui();
        ui.set_artwork(Image::from_rgba8(art.cover));
        ui.set_artwork_small(Image::from_rgba8(art.small));
        ui.set_artwork_generated(art.generated);
        let color = |c: u32| slint::Color::from_rgb_u8((c >> 16) as u8, (c >> 8) as u8, c as u8);
        let [a, b, c, d] = art.palette.glow;
        ui.set_glow_1(color(a));
        ui.set_glow_2(color(b));
        ui.set_glow_3(color(c));
        ui.set_glow_4(color(d));
        let c = art.palette.accent;
        ui.set_accent(slint::Color::from_rgb_u8((c >> 16) as u8, (c >> 8) as u8, c as u8));
        let m = art.palette.mood;
        let lum = (0.2126 * ((m >> 16) & 255) as f32 + 0.7152 * ((m >> 8) & 255) as f32 + 0.0722 * (m & 255) as f32) / 255.0;
        ui.set_mood_lum(lum);
    }

    fn play_pause(&mut self) {
        if self.index.is_none() {
            return;
        }
        if !self.loaded {
            self.load_current(self.position, true);
            return;
        }
        self.mpv.set_flag("pause", self.playing);
    }

    fn play_index(&mut self, i: usize) {
        if i < self.items.len() {
            self.index = Some(i);
            self.errors_in_a_row = 0;
            self.load_current(0.0, true);
        }
    }

    fn next(&mut self) {
        let Some(i) = self.index else { return };
        self.play_index((i + 1) % self.items.len());
    }

    fn prev(&mut self) {
        let Some(i) = self.index else { return };
        if self.position > 3.0 {
            self.seek(0.0);
        } else {
            self.play_index(if i == 0 { self.items.len() - 1 } else { i - 1 });
        }
    }

    fn seek(&mut self, secs: f64) {
        if !self.loaded {
            self.load_current(secs, false);
            return;
        }
        self.mpv.seek_absolute(secs);
        self.position = secs;
        self.ui().set_position(secs as f32);
        self.sync_system();
    }

    fn set_volume(&mut self, v: f64) {
        self.cancel_fade();
        self.volume = v.clamp(0.0, 100.0);
        self.mpv.set_double("volume", self.volume);
        self.ui().set_volume(self.volume as f32);
    }

    fn toggle_shuffle(&mut self) {
        let Some(i) = self.index else { return };
        let current = self.items[i].path.clone();
        let mut items = std::mem::take(&mut self.items);
        if !self.shuffle {
            self.unshuffled = Some(items.iter().map(|it| it.path.clone()).collect());
            let cur = items.remove(i);
            fastrand::shuffle(&mut items);
            items.insert(0, cur);
        } else if let Some(order) = self.unshuffled.take() {
            let mut by_path: std::collections::HashMap<PathBuf, Item> =
                items.drain(..).map(|it| (it.path.clone(), it)).collect();
            items = order.into_iter().filter_map(|p| by_path.remove(&p)).collect();
            items.extend(by_path.into_values());
        }
        self.items = items;
        self.index = self.items.iter().position(|it| it.path == current);
        self.shuffle = !self.shuffle;
        self.ui().set_shuffle(self.shuffle);
        self.rebuild_model();
    }

    fn cycle_repeat(&mut self) {
        self.repeat = (self.repeat + 1) % 3;
        self.ui().set_repeat_mode(self.repeat);
    }

    fn on_mpv(&mut self, ev: Event) {
        match ev {
            Event::FileLoaded => {
                self.loaded = true;
                self.errors_in_a_row = 0;
                self.take_link_title();
                if self.is_video {
                    self.refresh_tracks();
                    self.video_activity();
                }
            }
            Event::Duration(d) => {
                self.duration = d;
                self.ui().set_duration(d as f32);
            }
            Event::Paused(p) => {
                let playing = !p && self.index.is_some();
                self.set_playing(playing);
                if !playing {
                    self.save();
                }
            }
            Event::EndOfFile => {
                self.loaded = false;
                self.advance_after_end();
            }
            Event::FileError => {
                self.loaded = false;
                self.errors_in_a_row += 1;
                let link = self.index.and_then(|i| self.items.get(i)).is_some_and(|it| library::is_link(&it.path));
                if self.errors_in_a_row < self.items.len() {
                    if let Some(i) = self.index {
                        self.index = Some((i + 1) % self.items.len());
                        self.load_current(0.0, true);
                    }
                } else {
                    // Nothing in the queue would play: stop and say why.
                    self.set_playing(false);
                    self.show_note(if link {
                        "Couldn't open this link. yt-dlp may need an update.".into()
                    } else {
                        "Couldn't play this file.".into()
                    });
                }
            }
            Event::Shutdown => {}
        }
    }

    fn advance_after_end(&mut self) {
        let Some(i) = self.index else { return };
        if matches!(self.sleep, Some(tools::Sleep::EndOfTrack)) {
            // Sleep timer: stop here, ready on the next track.
            self.set_sleep("off");
            self.index = Some((i + 1) % self.items.len());
            self.load_current(0.0, false);
            self.save();
            return;
        }
        let last = i + 1 >= self.items.len();
        match self.repeat {
            2 => self.load_current(0.0, true),
            1 => self.play_index((i + 1) % self.items.len()),
            _ if !last => self.play_index(i + 1),
            _ => {
                // End of the queue: rewind to the start and wait.
                self.index = Some(0);
                self.load_current(0.0, false);
            }
        }
        self.save();
    }

    fn set_playing(&mut self, playing: bool) {
        self.playing = playing;
        self.ui().set_playing(playing);
        if playing {
            self.restart_ticker();
        } else {
            self.ticker.stop();
        }
        self.sync_system();
    }

    fn sync_system(&mut self) {
        let (playing, position, has) = (self.playing, self.position, self.index.is_some());
        if let Some(tray) = &self.tray {
            tray.set_playing(playing);
            tray.set_has_track(has);
        }
        if let Some(sys) = self.system.as_mut() {
            sys.set_state(playing, position, has);
        }
    }

    /// Once a second, or four times a second while timed lyrics are on screen.
    fn restart_ticker(&mut self) {
        let fast = self.show_lyrics && self.lyrics.as_ref().is_some_and(|l| l.synced);
        let every = Duration::from_millis(if fast { 250 } else { 1000 });
        if self.ticker.running() && self.ticker.interval() == every {
            return;
        }
        self.ticker.start(TimerMode::Repeated, every, || {
            with(|p| p.tick());
        });
    }

    /// Moves the scrubber, and saves the session every five seconds.
    fn tick(&mut self) {
        if self.loaded {
            if let Some(t) = self.mpv.get_double("time-pos") {
                self.position = t;
                self.ui().set_position(t as f32);
            }
        }
        self.follow_lyrics();
        self.ticks = self.ticks.wrapping_add(1);
        let per_5s = if self.ticker.interval() < Duration::from_millis(500) { 20 } else { 5 };
        if self.ticks % per_5s == 0 {
            self.save();
        }
    }

    fn fade_in(&mut self, target: f64) {
        self.mpv.set_double("volume", 0.0);
        let timer = Timer::default();
        let start = std::time::Instant::now();
        timer.start(TimerMode::Repeated, Duration::from_millis(40), move || {
            let k = (start.elapsed().as_secs_f64() / 3.0).min(1.0);
            // Ease out, so the music arrives softly.
            let v = target * (1.0 - (1.0 - k).powi(3));
            with(|p| {
                p.mpv.set_double("volume", v);
                if k >= 1.0 {
                    p.fade = None;
                }
            });
        });
        self.fade = Some(timer);
    }

    fn cancel_fade(&mut self) {
        if self.fade.take().is_some() {
            self.mpv.set_double("volume", self.volume);
        }
    }

    fn show_note(&mut self, text: String) {
        self.ui().set_resume_note(text.into());
        let t = Timer::default();
        t.start(TimerMode::SingleShot, Duration::from_secs(6), || {
            with(|p| {
                p.ui().set_resume_note(SharedString::new());
                p.note_timer = None;
            });
        });
        self.note_timer = Some(t);
    }

    // ---------- video ----------

    fn video_renderer_ready(&mut self) {
        if let Some((start, autoplay)) = self.pending_video.take() {
            self.load_current(start, autoplay);
            if autoplay {
                self.fade_in(self.volume);
            }
        }
    }

    fn set_video_mode(&mut self, video: bool) {
        if video == self.is_video {
            return;
        }
        // The mini player and picture-in-picture remember separate spots.
        if self.mini {
            self.save_geometry();
        }
        self.is_video = video;
        let ui = self.ui();
        ui.set_video_mode(video);
        // A new video opens full size; music brings the normal view back.
        ui.set_video_browse(false);
        self.set_controls(true);
        let ui = self.ui();
        if !video {
            ui.set_sub_tracks(ModelRc::default());
            ui.set_audio_tracks(ModelRc::default());
            let w = ui.window();
            if w.is_fullscreen() {
                w.set_fullscreen(false);
            }
        }
        if self.mini {
            Timer::single_shot(Duration::from_millis(30), || {
                with(|p| p.restore_geometry());
            });
        }
    }

    /// Show the controls, and hide them again 2.5 s after the mouse stops.
    fn video_activity(&mut self) {
        if !self.ui().get_controls_visible() {
            self.set_controls(true);
        }
        self.hud_timer.start(TimerMode::SingleShot, Duration::from_millis(2500), || {
            with(|p| {
                if p.playing {
                    p.set_controls(false);
                }
            });
        });
    }

    /// Show or hide the video controls. Subtitles move up out of their way.
    fn set_controls(&mut self, visible: bool) {
        self.ui().set_controls_visible(visible);
        let lift = !self.mini && (visible || !self.playing);
        self.mpv.set_string("sub-pos", if lift { "83" } else { "100" });
    }

    fn toggle_fullscreen(&mut self) {
        let ui = self.ui();
        let w = ui.window();
        w.set_fullscreen(!w.is_fullscreen());
    }

    /// Read subtitle and audio tracks from mpv for the menus.
    fn refresh_tracks(&mut self) {
        let count: usize = self.mpv.get_string("track-list/count").and_then(|c| c.parse().ok()).unwrap_or(0);
        let mut subs = vec![TrackOption { id: "no".into(), label: "Off".into(), selected: true }];
        let mut audio = Vec::new();
        for i in 0..count {
            let get = |f: &str| self.mpv.get_string(&format!("track-list/{i}/{f}")).unwrap_or_default();
            let kind = get("type");
            if kind != "sub" && kind != "audio" {
                continue;
            }
            let id = get("id");
            let selected = get("selected") == "yes";
            let mut label = language_name(&get("lang"));
            let title = get("title");
            if !title.is_empty() && !label.contains(&title) {
                label = if label.is_empty() { title } else { format!("{label} · {title}") };
            }
            if label.is_empty() {
                label = format!("Track {id}");
            }
            if get("external") == "yes" {
                label.push_str("  (file)");
            }
            let opt = TrackOption { id: id.into(), label: label.into(), selected };
            if kind == "sub" {
                if selected {
                    subs[0].selected = false;
                }
                subs.push(opt);
            } else {
                audio.push(opt);
            }
        }
        let ui = self.ui();
        ui.set_sub_tracks(ModelRc::new(VecModel::from(subs)));
        ui.set_audio_tracks(ModelRc::new(VecModel::from(audio)));
    }

    fn select_track(&mut self, property: &str, id: &str) {
        self.mpv.set_string(property, id);
        // mpv applies it asynchronously; read the lists back a moment later.
        Timer::single_shot(Duration::from_millis(120), || {
            with(|p| p.refresh_tracks());
        });
    }

    // ---------- lyrics ----------

    /// Lyrics, and the other small switches from Appearance.
    fn load_lyrics_settings(&mut self) {
        self.show_lyrics = self.store.setting("show_lyrics").as_deref() == Some("1");
        self.online_lyrics = self.store.setting("online_lyrics").as_deref() != Some("0");
        self.pause_on_lock = self.store.setting("pause_on_lock").as_deref() != Some("0");
        let ui = self.ui();
        ui.set_show_lyrics(self.show_lyrics);
        ui.set_online_lyrics(self.online_lyrics);
        ui.set_pause_on_lock(self.pause_on_lock);
    }

    fn request_lyrics(&mut self) {
        self.lyrics_gen += 1;
        self.lyrics = None;
        self.lyrics_index = -1;
        let ui = self.ui();
        ui.set_lyrics(ModelRc::default());
        ui.set_lyrics_index(-1);
        ui.set_lyrics_source(SharedString::new());
        ui.set_lyrics_status("Looking for lyrics…".into());
        let Some(item) = self.index.and_then(|i| self.items.get(i)) else { return };
        if library::is_video(&item.path) && !library::is_link(&item.path) {
            ui.set_lyrics_status("No lyrics for videos.".into());
            return;
        }
        let t = item.tags.clone().unwrap_or_default();
        let (path, generation, online) = (item.path.clone(), self.lyrics_gen, self.online_lyrics);
        std::thread::spawn(move || {
            let found = lyrics::find(&path, &t.title, &t.artist, &t.album, t.duration, online);
            let _ = slint::invoke_from_event_loop(move || {
                with(|p| p.apply_lyrics(generation, found));
            });
        });
    }

    fn apply_lyrics(&mut self, generation: u64, found: Option<lyrics::Lyrics>) {
        if generation != self.lyrics_gen {
            return;
        }
        let ui = self.ui();
        match &found {
            Some(l) => {
                let lines: Vec<LyricLine> = l.lines.iter().map(|(_, t)| LyricLine { text: t.clone().into() }).collect();
                ui.set_lyrics(ModelRc::new(VecModel::from(lines)));
                ui.set_lyrics_synced(l.synced);
                ui.set_lyrics_source(if l.source == "LRCLIB" { "Lyrics from LRCLIB".into() } else { SharedString::new() });
            }
            None => {
                ui.set_lyrics_status(if self.online_lyrics {
                    "No lyrics found for this song.".into()
                } else {
                    "No lyrics in this file. Add a .lrc file next to it, or turn on \"Find lyrics online\".".into()
                });
            }
        }
        self.lyrics = found;
        self.follow_lyrics();
        if self.playing {
            self.restart_ticker();
        }
    }

    /// Light up the line being sung.
    fn follow_lyrics(&mut self) {
        let Some(l) = &self.lyrics else { return };
        let i = l.line_at(self.position).map(|i| i as i32).unwrap_or(-1);
        if i != self.lyrics_index {
            self.lyrics_index = i;
            self.ui().set_lyrics_index(i);
        }
    }

    fn seek_lyric(&mut self, i: usize) {
        if let Some(&(t, _)) = self.lyrics.as_ref().and_then(|l| l.lines.get(i)) {
            if t >= 0.0 {
                self.seek(t);
                self.follow_lyrics();
            }
        }
    }

    // ---------- links ----------

    /// Links get their real title from yt-dlp once loaded.
    fn take_link_title(&mut self) {
        let Some(i) = self.index else { return };
        if !library::is_link(&self.items[i].path) {
            return;
        }
        let title = self.mpv.get_string("media-title").unwrap_or_default();
        if title.is_empty() || title.starts_with("http") {
            return;
        }
        let uploader = self.mpv.get_string("metadata/by-key/uploader").or_else(|| self.mpv.get_string("metadata/by-key/artist"));
        let tags = self.items[i].tags.get_or_insert_with(Tags::default);
        tags.title = title;
        tags.artist = uploader.unwrap_or_default();
        if let Some(d) = self.mpv.get_double("duration") {
            tags.duration = d;
        }
        let row = self.items[i].row();
        self.model.set_row_data(i, row);
        self.show_titles();
        self.request_lyrics();
        self.eq_track_changed();
    }

    fn open_link(&mut self, url: &str, audio_only: bool) {
        let url = url.trim();
        if !(url.starts_with("http://") || url.starts_with("https://")) {
            self.show_note("That doesn't look like a link".into());
            return;
        }
        let path = if audio_only { format!("{}{url}", library::AUDIO_LINK) } else { url.to_string() };
        self.ui().set_show_link_sheet(false);
        self.start_queue(vec![PathBuf::from(path)]);
    }

    // ---------- row menus: play next, show in folder, move to trash ----------

    fn row_action(&mut self, from: &str, i: usize, action: &str) {
        let path = match from {
            "queue" => self.items.get(i).map(|it| it.path.clone()),
            _ => self.list.get(i).cloned(),
        };
        let Some(path) = path else { return };
        match action {
            "next" => self.play_next(path),
            "folder" => show_in_folder(&path),
            "trash" => self.ask_delete(path),
            _ => {}
        }
    }

    /// Put a song right after the current one.
    fn play_next(&mut self, path: PathBuf) {
        let at = self.index.map(|i| i + 1).unwrap_or(0).min(self.items.len());
        self.items.insert(at, Item::new(path));
        if self.index.is_none() {
            self.index = Some(0);
        }
        self.rebuild_model();
        self.read_queue_tags();
        self.refresh_sessions();
        self.show_note("Playing next".into());
    }

    fn ask_delete(&mut self, path: PathBuf) {
        if library::is_link(&path) {
            // Links aren't files: just take them off the queue.
            self.remove_everywhere(&path);
            return;
        }
        let title = self
            .items
            .iter()
            .find(|it| it.path == path)
            .and_then(|it| it.tags.as_ref().map(|t| t.title.clone()))
            .unwrap_or_else(|| file_stem(&path));
        let ui = self.ui();
        ui.set_delete_title(title.into());
        ui.set_delete_path(path.display().to_string().into());
        ui.set_show_delete_sheet(true);
        self.pending_delete = Some(path);
    }

    fn confirm_delete(&mut self) {
        self.ui().set_show_delete_sheet(false);
        let Some(path) = self.pending_delete.take() else { return };
        // If it's playing, move on (or stop) before the file goes away.
        if self.current_path.as_ref() == Some(&path) {
            if self.items.len() > 1 {
                let was_playing = self.playing;
                self.next();
                if !was_playing {
                    self.mpv.set_flag("pause", true);
                }
            } else {
                self.mpv.stop();
                self.loaded = false;
            }
        }
        if let Err(e) = trash::delete(&path) {
            self.show_note(format!("Couldn't move it to the Trash: {e}"));
            return;
        }
        self.catalog.remove(&path);
        self.remove_everywhere(&path);
        self.show_note("Moved to the Trash".into());
    }

    /// Take a path off the queue and the list on screen.
    fn remove_everywhere(&mut self, path: &PathBuf) {
        let current = self.index.and_then(|i| self.items.get(i)).map(|it| it.path.clone());
        self.items.retain(|it| &it.path != path);
        self.index = if self.items.is_empty() {
            None
        } else {
            current.and_then(|c| self.items.iter().position(|it| it.path == c)).or(Some(0))
        };
        if self.items.is_empty() {
            self.clear_now_playing();
        }
        self.rebuild_model();
        if let Some(i) = self.list.iter().position(|p| p == path) {
            self.list.remove(i);
            if i < self.lib_model.row_count() {
                self.lib_model.remove(i);
            }
        }
        self.refresh_sessions();
        self.save();
    }

    // ---------- window shape ----------

    /// Rounded corners for the window itself (X11). Square when maximised.
    fn apply_shape(&self) {
        #[cfg(target_os = "linux")]
        {
            use slint::winit_030::winit::raw_window_handle::{HasDisplayHandle, HasWindowHandle, RawDisplayHandle, RawWindowHandle};
            let ui = self.ui();
            let w = ui.window();
            let radius = if w.is_maximized() || w.is_fullscreen() { 0.0 } else if self.mini { 16.0 } else { 12.0 };
            let px = (radius * w.scale_factor()).round() as u32;
            let size = w.size();
            w.with_winit_window(|ww| {
                let RawWindowHandle::Xlib(h) = ww.window_handle().ok()?.as_raw() else { return None };
                let RawDisplayHandle::Xlib(d) = ww.display_handle().ok()?.as_raw() else { return None };
                let display = d.display?.as_ptr();
                // A see-through window on a compositing desktop already has smooth
                // rounded corners; the cut-out is only for when it can't be see-through.
                if SEE_THROUGH.load(Ordering::Acquire) && shape::compositing(display, d.screen) {
                    return None;
                }
                shape::round(display, h.window, size.width, size.height, px);
                Some(())
            });
        }
    }

    // ---------- tray ----------

    /// Close the window but keep playing, if there's a tray to come back from.
    fn close_window(&mut self) {
        self.save_geometry();
        if !self.close_to_tray {
            quit();
            return;
        }
        if self.is_video && self.playing {
            self.play_pause();
        }
        self.save();
        let _ = self.ui().hide();
    }

    /// The window's OpenGL went away (hidden): reload the video when it's back.
    fn video_output_lost(&mut self) {
        if self.is_video && self.current_path.is_some() {
            self.pending_video = Some((self.position, false));
        }
    }

    // ---------- mini player and code mode ----------

    fn set_mini(&mut self, mini: bool) {
        if mini == self.mini {
            return;
        }
        if let Some(tray) = &self.tray {
            tray.set_mini(mini);
        }
        self.save_geometry();
        self.mini = mini;
        let ui = self.ui();
        if mini {
            ui.set_show_settings(false);
            ui.set_show_open_sheet(false);
        }
        ui.set_mini(mini);
        // Let the new size limits apply before resizing.
        Timer::single_shot(Duration::from_millis(30), || {
            with(|p| p.restore_geometry());
        });
    }

    fn geometry_key(&self) -> &'static str {
        match (self.mini, self.is_video) {
            (true, true) => "geometry_pip",
            (true, false) => "geometry_mini",
            _ => "geometry_full",
        }
    }

    fn save_geometry(&self) {
        let w = self.ui();
        let w = w.window();
        if w.is_minimized() || w.is_maximized() || w.is_fullscreen() {
            return;
        }
        let (pos, size) = (w.position(), w.size());
        self.store.set_setting(self.geometry_key(), &format!("{},{},{},{}", pos.x, pos.y, size.width, size.height));
    }

    /// Put the window back where it was last time in this mode. The mini player
    /// starts in the bottom-right corner the first time.
    fn restore_geometry(&self) {
        let ui = self.ui();
        let w = ui.window();
        let saved: Option<Vec<i32>> = self
            .store
            .setting(self.geometry_key())
            .map(|s| s.split(',').filter_map(|n| n.parse().ok()).collect())
            .filter(|v: &Vec<i32>| v.len() == 4);
        match saved {
            Some(v) => {
                w.set_size(slint::PhysicalSize::new(v[2].max(1) as u32, v[3].max(1) as u32));
                w.set_position(slint::PhysicalPosition::new(v[0], v[1]));
            }
            None if self.mini => {
                let scale = w.scale_factor();
                let (lw, lh) = if self.is_video { (480.0, 270.0) } else { (440.0, 84.0) };
                let (mw, mh) = ((lw * scale) as i32, (lh * scale) as i32);
                w.set_size(slint::PhysicalSize::new(mw as u32, mh as u32));
                let corner = w.with_winit_window(|ww| {
                    ww.current_monitor().map(|m| {
                        let (p, s) = (m.position(), m.size());
                        let margin = (28.0 * scale) as i32;
                        (p.x + s.width as i32 - mw - margin, p.y + s.height as i32 - mh - margin - (40.0 * scale) as i32)
                    })
                });
                if let Some(Some((x, y))) = corner {
                    w.set_position(slint::PhysicalPosition::new(x, y));
                }
            }
            None => {
                w.set_size(slint::LogicalSize::new(1140.0, 730.0));
            }
        }
    }

    fn show_window(&self) {
        let ui = self.ui();
        let w = ui.window();
        w.set_minimized(false);
        let _ = w.show();
        w.with_winit_window(|ww| ww.focus_window());
    }

    /// Code mode: the Code session, in the mini player, playing.
    fn enter_code_mode(&mut self) {
        if self.session != CODE_SESSION {
            self.switch_session(CODE_SESSION);
        } else if !self.playing && self.index.is_some() {
            self.play_pause();
            self.fade_in(self.volume);
        }
        self.set_mini(true);
        self.show_window();
    }

    fn command(&mut self, cmd: Cmd) -> String {
        match cmd {
            Cmd::Toggle => self.play_pause(),
            Cmd::Play => {
                if !self.playing {
                    self.play_pause();
                }
            }
            Cmd::Pause => {
                if self.playing {
                    self.play_pause();
                }
            }
            Cmd::Next => self.next(),
            Cmd::Prev => self.prev(),
            Cmd::Show => self.show_window(),
            Cmd::Code => self.enter_code_mode(),
            Cmd::Mini => self.set_mini(true),
            Cmd::Full => {
                self.set_mini(false);
                self.show_window();
            }
            Cmd::ToggleMini => {
                self.set_mini(!self.mini);
                self.show_window();
            }
            Cmd::SeekBy(d) => {
                let target = (self.position + d).clamp(0.0, self.duration.max(0.0));
                self.seek(target);
            }
            Cmd::Status(fmt) => return self.status(&fmt),
            Cmd::Sleep(arg) => {
                self.set_sleep(&arg);
                return if self.sleep.is_some() { format!("sleep: {}", self.sleep_label()) } else { "sleep timer off".into() };
            }
            Cmd::Speed(v) => self.set_speed(v),
            Cmd::Mark(note) => {
                if self.current_path.is_none() {
                    return "error: nothing is playing".into();
                }
                self.add_bookmark(&note);
                return format!("marked {}", fmt_time(self.position));
            }
            Cmd::ScreenLocked(true) => {
                if self.pause_on_lock && self.playing {
                    self.play_pause();
                    self.paused_by_lock = true;
                }
            }
            Cmd::ScreenLocked(false) => {
                if std::mem::take(&mut self.paused_by_lock) && !self.playing {
                    self.play_pause();
                    self.fade_in(self.volume);
                }
            }
            Cmd::Load { paths, limit, pick, recursive } => {
                let files = collect(&paths, recursive);
                let chosen = library::choose(files, Pick::parse(&pick).unwrap_or(Pick::First), limit);
                if chosen.is_empty() {
                    return "error: no playable files found".into();
                }
                self.start_queue(chosen);
                self.show_window();
            }
        }
        "ok".into()
    }

    /// One line describing what's playing, for prompts and status bars.
    fn status(&mut self, fmt: &str) -> String {
        let Some(item) = self.index.and_then(|i| self.items.get(i)) else { return String::new() };
        if self.playing && self.loaded {
            if let Some(t) = self.mpv.get_double("time-pos") {
                self.position = t;
            }
        }
        let t = item.tags.clone().unwrap_or_default();
        let title = if t.title.is_empty() { file_stem(&item.path) } else { t.title };
        let fmt = if !fmt.is_empty() {
            fmt.to_string()
        } else if t.artist.is_empty() {
            "{state} {title}  {pos} / {dur}".to_string()
        } else {
            "{state} {title} — {artist}  {pos} / {dur}".to_string()
        };
        fmt.replace("{state}", if self.playing { "▶" } else { "⏸" })
            .replace("{title}", &title)
            .replace("{artist}", &t.artist)
            .replace("{album}", &t.album)
            .replace("{pos}", &fmt_time(self.position))
            .replace("{dur}", &fmt_time(self.duration))
            .replace("{session}", &self.session)
    }

    // ---------- open folder ----------

    fn scan_for_sheet(&mut self, dir: PathBuf) {
        let recursive = self.ui().get_sheet_recursive();
        std::thread::spawn(move || {
            let files = library::scan(&dir, recursive);
            let _ = slint::invoke_from_event_loop(move || {
                with(|p| p.show_sheet(dir, files));
            });
        });
    }

    fn show_sheet(&mut self, dir: PathBuf, files: Vec<PathBuf>) {
        let ui = self.ui();
        let name = dir.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| dir.display().to_string());
        ui.set_sheet_folder(name.into());
        ui.set_sheet_path(dir.display().to_string().into());
        ui.set_sheet_found(files.len() as i32);
        if ui.get_sheet_limit() > files.len() as i32 {
            ui.set_sheet_limit((files.len() as i32).max(1));
        }
        ui.set_show_open_sheet(true);
        self.sheet = Some((dir, files));
    }

    fn rescan_sheet(&mut self) {
        if let Some((dir, _)) = self.sheet.clone() {
            self.scan_for_sheet(dir);
        }
    }

    fn confirm_sheet(&mut self) {
        let ui = self.ui();
        let Some((_, files)) = self.sheet.take() else { return };
        ui.set_show_open_sheet(false);
        if files.is_empty() {
            return;
        }
        let limit = (ui.get_sheet_mode() == 1).then(|| ui.get_sheet_limit().max(1) as usize);
        let pick = if limit.is_some() { Pick::from_index(ui.get_sheet_pick()) } else { Pick::First };
        let chosen = library::choose(files, pick, limit);
        self.start_queue(chosen);
    }

    fn start_queue(&mut self, paths: Vec<PathBuf>) {
        self.start_queue_at(paths, 0);
    }

    fn start_queue_at(&mut self, paths: Vec<PathBuf>, index: usize) {
        self.note_timer = None;
        self.ui().set_resume_note(SharedString::new());
        self.shuffle = false;
        self.unshuffled = None;
        self.ui().set_shuffle(false);
        self.set_queue(paths, index);
        self.errors_in_a_row = 0;
        self.load_current(0.0, true);
        self.refresh_sessions();
        self.save();
    }
}

fn collect(paths: &[PathBuf], recursive: bool) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for p in paths {
        if library::is_link(p) {
            files.push(p.clone());
        } else if p.is_dir() {
            files.extend(library::scan(p, recursive));
        } else if p.is_file() {
            files.push(p.clone());
        } else {
            eprintln!("nava: can't find '{}'", p.display());
        }
    }
    files
}

/// The Xlib display behind a window, if it is an X11 window.
fn x11_display(w: &slint::winit_030::winit::window::Window) -> Option<*mut std::ffi::c_void> {
    use slint::winit_030::winit::raw_window_handle::{HasDisplayHandle, RawDisplayHandle};
    match w.display_handle().ok()?.as_raw() {
        RawDisplayHandle::Xlib(h) => h.display.map(|d| d.as_ptr()),
        _ => None,
    }
}


/// Open the file manager at a file's folder.
fn show_in_folder(path: &Path) {
    let dir = path.parent().unwrap_or(path);
    #[cfg(target_os = "macos")]
    let r = std::process::Command::new("open").arg("-R").arg(path).spawn();
    #[cfg(target_os = "windows")]
    let r = std::process::Command::new("explorer").arg(format!("/select,{}", path.display())).spawn();
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let r = std::process::Command::new("xdg-open").arg(dir).spawn();
    let _ = (r, dir);
}

/// Whether a program is on the PATH.
fn which(program: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|paths| std::env::split_paths(&paths).any(|d| d.join(program).is_file()))
}

/// "fa" / "per" → "Persian", for the subtitle and audio menus.
fn language_name(code: &str) -> String {
    match code.to_ascii_lowercase().as_str() {
        "" | "und" => String::new(),
        "fa" | "per" | "fas" => "Persian".into(),
        "en" | "eng" => "English".into(),
        "ar" | "ara" => "Arabic".into(),
        "tr" | "tur" => "Turkish".into(),
        "fr" | "fre" | "fra" => "French".into(),
        "de" | "ger" | "deu" => "German".into(),
        "es" | "spa" => "Spanish".into(),
        "ru" | "rus" => "Russian".into(),
        "ja" | "jpn" => "Japanese".into(),
        "ko" | "kor" => "Korean".into(),
        "zh" | "chi" | "zho" => "Chinese".into(),
        "hi" | "hin" => "Hindi".into(),
        other => other.to_string(),
    }
}

/// Commands from the terminal, media keys and global shortcuts land here.
fn handle_cmd(cmd: Cmd) -> String {
    with(|p| p.command(cmd)).unwrap_or_default()
}

fn run_cmd(cmd: Cmd) {
    handle_cmd(cmd);
}

fn main() -> anyhow::Result<()> {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("nava: {e}\n\n{HELP}");
            std::process::exit(2);
        }
    };

    // `nava next` and friends talk to the running instance and exit.
    if let Some(cmd) = args.remote.clone() {
        match ipc::send(&cmd) {
            Ok(reply) => {
                if !reply.is_empty() && reply != "ok" {
                    println!("{reply}");
                }
                std::process::exit(if reply.starts_with("error") { 1 } else { 0 });
            }
            // For status bars: print nothing when Nava isn't running.
            Err(_) if matches!(cmd, Cmd::Status(_)) => std::process::exit(1),
            Err(_) => {
                eprintln!("nava: Nava isn't running");
                std::process::exit(1);
            }
        }
    }

    // If Nava is already open, hand this launch over to it.
    let paths: Vec<PathBuf> = args
        .paths
        .iter()
        .map(|p| {
            if library::is_link(p) {
                if args.audio_only { PathBuf::from(format!("{}{}", library::AUDIO_LINK, p.display())) } else { p.clone() }
            } else {
                p.canonicalize().unwrap_or_else(|_| p.clone())
            }
        })
        .collect();
    let handoff = if args.code {
        Cmd::Code
    } else if !paths.is_empty() {
        let pick = match args.pick.unwrap_or(Pick::First) {
            Pick::First => "first",
            Pick::Random => "random",
            Pick::Newest => "newest",
        };
        Cmd::Load { paths: paths.clone(), limit: args.limit, pick: pick.into(), recursive: args.recursive }
    } else {
        Cmd::Show
    };
    if ipc::send(&handoff).is_ok() {
        return Ok(());
    }

    // Clear last run's cover copies (link thumbnails in links/ are kept).
    if let Ok(entries) = std::fs::read_dir(meta::cover_cache_dir()) {
        for e in entries.flatten().filter(|e| e.path().is_file()) {
            let _ = std::fs::remove_file(e.path());
        }
    }
    let store = Store::open()?;
    let session = if args.code {
        CODE_SESSION.to_string()
    } else {
        args.session.clone().or_else(|| store.setting("last_session")).unwrap_or_else(|| "Main".into())
    };
    let session = match store.session_names().into_iter().find(|n| n.eq_ignore_ascii_case(&session)) {
        Some(name) => name,
        None if args.session.is_some() => {
            eprintln!("nava: unknown session '{session}'");
            std::process::exit(2);
        }
        None => "Main".into(),
    };
    let fresh = library::choose(collect(&paths, args.recursive), args.pick.unwrap_or(Pick::First), args.limit);

    // Pick the renderer. The software one is lighter, but video needs OpenGL,
    // so use OpenGL when the first thing to play is a video. (If a video comes
    // up later, Nava restarts itself with OpenGL, see RELAUNCH_FOR_VIDEO.)
    let starts_with_video = match fresh.first() {
        Some(first) => library::is_video(first),
        None => store.load_session(&session).and_then(|s| s.queue.get(s.index).cloned()).is_some_and(|p| library::is_video(&p)),
    };
    let renderer = std::env::var("NAVA_RENDERER").ok();
    let want_gl = match renderer.as_deref() {
        Some("gl") => true,
        Some("software" | "skia") => false,
        _ => starts_with_video,
    };
    // A see-through window: the compositor then blends the rounded corners
    // smoothly, instead of the stair-steps a cut-out (SHAPE) window gets.
    let select = |name: &str| {
        slint::BackendSelector::new()
            .renderer_name(name.into())
            .with_winit_window_attributes_hook(|a| a.with_transparent(true))
            .select()
            .is_ok()
    };
    let gl = want_gl && select("femtovg");
    if !gl {
        // Skia draws on the CPU here, with finer edges and hinted text than
        // Slint's own software renderer (still available as NAVA_RENDERER=software).
        let sharp = renderer.as_deref() != Some("software");
        if sharp && select("skia-software") {
            SEE_THROUGH.store(true, Ordering::Release);
        } else {
            select("software");
        }
    } else {
        SEE_THROUGH.store(true, Ordering::Release);
    }

    GL_MODE.store(gl, Ordering::Release);
    let ui = AppWindow::new()?;
    ui.set_has_ytdlp(which("yt-dlp"));
    WINDOW.with(|w| *w.borrow_mut() = Some(ui.as_weak()));
    let mpv = Arc::new(Mpv::new()?);
    let catalog = catalog::Catalog::open()?;

    // Appearance settings
    let glass = store.setting("glass").and_then(|v| v.parse::<f32>().ok()).unwrap_or(0.8);
    let theme = store.setting("theme").and_then(|v| v.parse::<i32>().ok()).unwrap_or(0);
    ui.set_glass(glass.clamp(0.0, 1.0));
    ui.set_theme_mode(theme);

    // mpv events arrive on their own thread and are handed to the UI thread.
    let events = {
        let mpv = mpv.clone();
        std::thread::spawn(move || {
            loop {
                match mpv.wait_event(-1.0) {
                    Some(Event::Shutdown) => break,
                    Some(ev) => {
                        let _ = slint::invoke_from_event_loop(move || {
                            with(|p| p.on_mpv(ev));
                        });
                    }
                    None => {}
                }
            }
        })
    };

    PLAYER.with(|p| {
        *p.borrow_mut() = Some(Player {
            ui: ui.as_weak(),
            mpv: mpv.clone(),
            store,
            session: session.clone(),
            items: Vec::new(),
            unshuffled: None,
            index: None,
            model: Rc::new(VecModel::default()),
            volume: 80.0,
            shuffle: false,
            repeat: 1,
            playing: false,
            loaded: false,
            position: 0.0,
            duration: 0.0,
            errors_in_a_row: 0,
            art_gen: 0,
            tags_gen: 0,
            sheet: None,
            fade: None,
            note_timer: None,
            ticker: Timer::default(),
            ticks: 0,
            mini: false,
            system: None,
            cover_file: None,
            gl,
            is_video: false,
            current_path: None,
            hud_timer: Timer::default(),
            pending_video: None,
            catalog,
            view: "now".into(),
            list: Vec::new(),
            lib_model: Rc::new(VecModel::default()),
            album_grid: None,
            artists: Vec::new(),
            filter: None,
            search_timer: Timer::default(),
            rescan_timer: Timer::default(),
            scanning: false,
            eq: eq::Eq::default(),
            pause_on_lock: true,
            paused_by_lock: false,
            sleep: None,
            sleep_timer: Timer::default(),
            sleep_label_timer: Timer::default(),
            speed: 1.0,
            ab: (-1.0, -1.0),
            bookmark_ids: Vec::new(),
            lyrics: None,
            lyrics_gen: 0,
            lyrics_index: -1,
            show_lyrics: false,
            online_lyrics: true,
            pending_delete: None,
            tray: None,
            close_to_tray: false,
        })
    });

    // ---- wire up the UI ----
    ui.on_play_pause(|| {
        with(|p| p.play_pause());
    });
    ui.on_next(|| {
        with(|p| p.next());
    });
    ui.on_prev(|| {
        with(|p| p.prev());
    });
    ui.on_seek(|s| {
        with(|p| p.seek(s as f64));
    });
    ui.on_set_volume(|v| {
        with(|p| p.set_volume(v as f64));
    });
    ui.on_toggle_shuffle(|| {
        with(|p| p.toggle_shuffle());
    });
    ui.on_cycle_repeat(|| {
        with(|p| p.cycle_repeat());
    });
    ui.on_play_index(|i| {
        with(|p| p.play_index(i.max(0) as usize));
    });
    ui.on_switch_session(|name| {
        with(|p| p.switch_session(&name));
    });
    ui.on_create_session(|name| {
        with(|p| p.create_session(&name));
    });
    ui.on_session_action(|name, action| {
        with(|p| p.session_action(&name, &action));
    });
    ui.on_clear_queue(|| {
        with(|p| p.clear_queue());
    });
    ui.on_remove_library_folder(|i| {
        with(|p| p.remove_library_folder(i.max(0) as usize));
    });
    ui.on_sheet_rescan(|| {
        with(|p| p.rescan_sheet());
    });
    ui.on_sheet_confirm(|| {
        with(|p| p.confirm_sheet());
    });
    ui.on_glass_changed(|g| {
        with(|p| p.store.set_setting("glass", &format!("{g:.3}")));
    });
    ui.on_theme_changed(|t| {
        with(|p| p.store.set_setting("theme", &t.to_string()));
    });
    ui.on_toggle_mini(|| {
        with(|p| p.set_mini(!p.mini));
    });
    ui.on_open_folder(|| {
        with(|p| p.set_mini(false));
        let _ = slint::spawn_local(async {
            let picked = rfd::AsyncFileDialog::new().set_title("Choose a folder to play").pick_folder().await;
            if let Some(folder) = picked {
                with(|p| p.scan_for_sheet(folder.path().to_path_buf()));
            }
        });
    });
    ui.on_close_window(|| {
        with(|p| p.close_window());
    });
    ui.window().on_close_requested(|| {
        with(|p| p.close_window());
        slint::CloseRequestResponse::KeepWindowShown
    });
    ui.on_show_view(|v| {
        with(|p| p.show_view(&v));
    });
    ui.on_search_changed(|t| {
        with(|p| p.search_changed(t.to_string()));
    });
    ui.on_play_from_list(|i| {
        with(|p| p.play_from_list(i.max(0) as usize));
    });
    ui.on_play_list(|shuffled| {
        with(|p| p.play_list(shuffled));
    });
    ui.on_open_album(|r, c| {
        with(|p| p.open_album(r.max(0) as usize, c.max(0) as usize));
    });
    ui.on_open_artist(|i| {
        with(|p| p.open_artist(i.max(0) as usize));
    });
    ui.on_heart_row(|i| {
        with(|p| p.heart_row(i.max(0) as usize));
    });
    ui.on_heart_current(|| {
        with(|p| p.heart_current());
    });
    ui.on_add_library_folder(|| {
        with(|p| p.add_library_folder());
    });
    ui.on_open_link(|url, audio| {
        with(|p| p.open_link(&url, audio));
    });
    ui.on_window_resized(|| {
        with(|p| p.apply_shape());
    });
    ui.on_row_action(|from, i, action| {
        with(|p| p.row_action(&from, i.max(0) as usize, &action));
    });
    ui.on_confirm_delete(|| {
        with(|p| p.confirm_delete());
    });
    ui.on_go_back(|| {
        with(|p| p.go_back());
    });
    ui.on_album_columns_changed(|n| {
        with(|p| p.album_columns_changed(n.max(1) as usize));
    });
    ui.on_eq_band(|i, g| {
        with(|p| p.eq_band(i.max(0) as usize, g));
    });
    ui.on_eq_toggle(|on| {
        with(|p| p.eq_set_on(on));
    });
    ui.on_eq_auto_changed(|on| {
        with(|p| p.eq_set_auto(on));
    });
    ui.on_eq_preamp_changed(|db| {
        with(|p| p.eq_set_preamp(db));
    });
    ui.on_eq_save(|name| {
        with(|p| p.eq_save_custom(&name));
    });
    ui.on_eq_delete(|i| {
        with(|p| p.eq_delete_custom(i.max(0) as usize));
    });
    ui.on_eq_preset_chosen(|name| {
        with(|p| p.eq_choose(&name));
    });
    ui.on_pause_on_lock_changed(|on| {
        with(|p| {
            p.pause_on_lock = on;
            p.store.set_setting("pause_on_lock", if on { "1" } else { "0" });
        });
    });
    ui.on_set_sleep(|arg| {
        with(|p| p.set_sleep(&arg));
    });
    ui.on_set_speed(|v| {
        with(|p| p.set_speed(v.parse().unwrap_or(1.0)));
    });
    ui.on_ab_loop(|| {
        with(|p| p.ab_loop());
    });
    ui.on_add_bookmark(|note| {
        with(|p| p.add_bookmark(&note));
    });
    ui.on_seek_bookmark(|i| {
        with(|p| p.seek_bookmark(i.max(0) as usize));
    });
    ui.on_delete_bookmark(|i| {
        with(|p| p.delete_bookmark(i.max(0) as usize));
    });
    ui.on_seek_lyric(|i| {
        with(|p| p.seek_lyric(i.max(0) as usize));
    });
    ui.on_lyrics_toggled(|on| {
        with(|p| {
            p.show_lyrics = on;
            p.store.set_setting("show_lyrics", if on { "1" } else { "0" });
            if p.playing {
                p.restart_ticker();
            }
        });
    });
    ui.on_online_lyrics_changed(|on| {
        with(|p| {
            p.online_lyrics = on;
            p.store.set_setting("online_lyrics", if on { "1" } else { "0" });
            if on && p.lyrics.is_none() {
                p.request_lyrics();
            }
        });
    });
    ui.on_export_notes(|| {
        with(|p| p.export_notes());
    });
    ui.on_video_activity(|| {
        with(|p| p.video_activity());
    });
    ui.on_toggle_fullscreen(|| {
        with(|p| p.toggle_fullscreen());
    });
    ui.on_select_sub(|id| {
        with(|p| p.select_track("sid", &id));
    });
    ui.on_select_audio(|id| {
        with(|p| p.select_track("aid", &id));
    });

    // Video frames: mpv draws into a texture just before Slint draws the window.
    if gl {
        let weak = ui.as_weak();
        let raw = mpv.raw() as usize;
        let mut out: Option<video::VideoOut> = None;
        let notifier = ui.window().set_rendering_notifier(move |state, api| match state {
            slint::RenderingState::RenderingSetup => {
                if let slint::GraphicsAPI::NativeOpenGL { get_proc_address } = api {
                    let display = weak
                        .upgrade()
                        .and_then(|ui| ui.window().with_winit_window(x11_display))
                        .flatten()
                        .unwrap_or(std::ptr::null_mut());
                    out = video::VideoOut::new(raw as *mut mpv::MpvHandle, get_proc_address, display);
                    if out.is_some() {
                        VIDEO_READY.store(true, Ordering::Release);
                        let _ = slint::invoke_from_event_loop(|| {
                            with(|p| p.video_renderer_ready());
                        });
                    }
                }
            }
            slint::RenderingState::BeforeRendering => {
                if QUITTING.load(Ordering::Acquire) {
                    out = None; // frees mpv's renderer while OpenGL is current
                    let _ = slint::quit_event_loop();
                    return;
                }
                let (Some(v), Some(ui)) = (out.as_mut(), weak.upgrade()) else { return };
                if ui.get_video_mode() {
                    // Full window, or a 16:9 picture while browsing the library.
                    let size = ui.window().size();
                    let (w, h) = if ui.get_video_browse() && !ui.get_mini() { (960, 540) } else { (size.width, size.height) };
                    if let Some(image) = v.render(w, h) {
                        ui.set_video_frame(image);
                    }
                }
            }
            slint::RenderingState::AfterRendering => {
                if let Some(v) = &out {
                    v.report_swap();
                }
            }
            slint::RenderingState::RenderingTeardown => {
                if out.take().is_some() {
                    VIDEO_READY.store(false, Ordering::Release);
                    let _ = slint::invoke_from_event_loop(|| {
                        with(|p| p.video_output_lost());
                    });
                }
            }
            _ => {}
        });
        if notifier.is_err() {
            eprintln!("nava: video output unavailable");
        }
    }

    // Terminal commands, media keys and global shortcuts.
    if let Err(e) = ipc::serve(handle_cmd) {
        eprintln!("nava: can't listen for commands: {e}");
    }
    with(|p| p.system = Some(system::System::new(run_cmd)));

    // Tray icon. Closing the window hides it only if the desktop shows tray icons.
    let tray = NavaTray::new().ok();
    if let Some(t) = &tray {
        t.on_play_pause(|| run_cmd(Cmd::Toggle));
        t.on_next(|| run_cmd(Cmd::Next));
        t.on_prev(|| run_cmd(Cmd::Prev));
        t.on_toggle_mini(|| run_cmd(Cmd::ToggleMini));
        t.on_show_window(|| run_cmd(Cmd::Show));
        t.on_quit(quit);
    }
    with(|p| {
        p.close_to_tray = tray.is_some() && system::tray_available();
        p.tray = tray;
    });

    // ---- start: a fresh queue from the command line, or the saved session ----
    let relaunched = std::env::var_os("NAVA_RELAUNCHED").is_some();
    with(|p| {
        if args.code || std::env::var_os("NAVA_MINI").is_some() {
            p.mini = true;
            p.ui().set_mini(true);
        }
        if !fresh.is_empty() {
            p.open_session(&session, false);
            p.start_queue(fresh);
        } else {
            p.open_session(&session, args.code || relaunched);
        }
    });
    with(|p| {
        p.init_library();
        p.load_eq();
        p.load_lyrics_settings();
        p.refresh_tool_menus();
    });

    // Once the window is on screen, put it where it was last time. (Placing it
    // earlier gets overridden by the window manager.)
    Timer::single_shot(Duration::from_millis(150), || {
        with(|p| {
            p.restore_geometry();
            p.apply_shape();
        });
    });

    ui.show()?;
    // Keep running with the window hidden (tray); `quit()` ends the loop.
    slint::run_event_loop_until_quit()?;
    let relaunch = RELAUNCH_FOR_VIDEO.load(Ordering::Acquire);
    let was_mini = with(|p| p.mini).unwrap_or(false);

    with(|p| {
        p.save();
        p.system = None;
        let _ = p.mpv.command(&["quit"]);
    });
    ipc::cleanup();
    let _ = events.join();

    if relaunch {
        // Same session, same spot, now with OpenGL.
        let mut cmd = std::process::Command::new(std::env::current_exe()?);
        cmd.env("NAVA_RENDERER", "gl").env("NAVA_RELAUNCHED", "1");
        if was_mini {
            cmd.env("NAVA_MINI", "1");
        }
        cmd.spawn()?;
    }
    if gl {
        // The video renderer lives inside Slint's window and may still point at
        // mpv; leaving now is safer than tearing mpv down underneath it.
        std::process::exit(0);
    }
    PLAYER.with(|p| p.borrow_mut().take());
    Ok(())
}
