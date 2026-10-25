//! Browsing the library: songs, albums, artists, favourites, search.

use std::cell::Cell;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use slint::{Image, Model, ModelNotify, ModelRc, ModelTracker, SharedString, TimerMode, VecModel};

use crate::catalog::{self, Album, Filter, Song};
use crate::{AlbumCell, AlbumRow, ArtistRow, LibRow, Player, fmt_time, with};

/// A soft colour picked from a name, for albums and artists without artwork.
pub fn tint_for(name: &str) -> slint::Color {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    name.to_lowercase().hash(&mut h);
    let hue = (h.finish() % 360) as f32;
    let (s, v) = (0.55f32, 0.78f32);
    let c = v * s;
    let x = c * (1.0 - ((hue / 60.0).rem_euclid(2.0) - 1.0).abs());
    let m = v - c;
    let (r, g, b) = match (hue / 60.0) as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    slint::Color::from_rgb_f32(r + m, g + m, b + m)
}

/// The album grid as rows of `columns` tiles. Covers are loaded only for the
/// rows the list actually shows, so a big library doesn't fill memory.
pub struct AlbumGrid {
    albums: Rc<Vec<Album>>,
    columns: Cell<usize>,
    notify: ModelNotify,
}

impl AlbumGrid {
    pub fn new(albums: Rc<Vec<Album>>, columns: usize) -> AlbumGrid {
        AlbumGrid { albums, columns: Cell::new(columns.max(1)), notify: ModelNotify::default() }
    }

    pub fn set_columns(&self, columns: usize) {
        self.columns.set(columns.max(1));
        self.notify.reset();
    }

    pub fn len(&self) -> usize {
        self.albums.len()
    }

    pub fn album(&self, row: usize, col: usize) -> Option<&Album> {
        self.albums.get(row * self.columns.get() + col)
    }
}

pub fn album_cell(a: &Album) -> AlbumCell {
    let thumb = catalog::thumbnail_path(&a.name);
    let cover = thumb.exists().then(|| Image::load_from_path(&thumb).ok()).flatten();
    AlbumCell {
        name: a.name.clone().into(),
        artist: a.artist.clone().into(),
        has_cover: cover.is_some(),
        cover: cover.unwrap_or_default(),
        tint: tint_for(&a.name),
    }
}

impl Model for AlbumGrid {
    type Data = AlbumRow;

    fn row_count(&self) -> usize {
        self.albums.len().div_ceil(self.columns.get())
    }

    fn row_data(&self, row: usize) -> Option<AlbumRow> {
        let cols = self.columns.get();
        let start = row * cols;
        if start >= self.albums.len() {
            return None;
        }
        let cells: Vec<AlbumCell> = self.albums[start..(start + cols).min(self.albums.len())].iter().map(album_cell).collect();
        Some(AlbumRow { cells: ModelRc::new(VecModel::from(cells)) })
    }

    fn model_tracker(&self) -> &dyn ModelTracker {
        &self.notify
    }
}

fn total_time(songs: &[Song]) -> String {
    let secs: f64 = songs.iter().map(|s| s.duration).sum();
    let mins = (secs / 60.0).round() as u64;
    if mins >= 60 { format!("{} hr {} min", mins / 60, mins % 60) } else { format!("{mins} min") }
}

impl Player {
    fn lib_row(&self, s: &Song) -> LibRow {
        LibRow {
            title: s.title.clone().into(),
            artist: s.artist.clone().into(),
            album: s.album.clone().into(),
            duration: if s.duration > 0.0 { fmt_time(s.duration).into() } else { SharedString::new() },
            favorite: s.favorite,
            current: self.current_path.as_ref() == Some(&s.path),
        }
    }

    pub fn show_view(&mut self, view: &str) {
        let ui = self.ui();
        ui.set_view(view.into());
        self.view = view.to_string();
        match view {
            "songs" => self.show_songs(Filter::Songs, "Songs", None),
            "videos" => self.show_songs(Filter::Videos, "Videos", None),
            "favorites" => self.show_songs(Filter::Favorites, "Favorites", None),
            "recent" => self.show_songs(Filter::Recent, "Recently Added", None),
            "albums" => self.show_albums(),
            "artists" => self.show_artists(),
            _ => {}
        }
    }

    fn show_songs(&mut self, filter: Filter, title: &str, cover: Option<Image>) {
        let songs = self.catalog.songs(&filter);
        let rows: Vec<LibRow> = songs.iter().map(|s| self.lib_row(s)).collect();
        let subtitle = match songs.len() {
            0 => String::new(),
            1 => format!("1 item · {}", total_time(&songs)),
            n => format!("{n} items · {}", total_time(&songs)),
        };
        self.list = songs.into_iter().map(|s| s.path).collect();
        self.lib_model = Rc::new(VecModel::from(rows));
        let ui = self.ui();
        ui.set_lib_songs(ModelRc::from(self.lib_model.clone()));
        ui.set_view_title(title.into());
        ui.set_view_subtitle(subtitle.into());
        ui.set_view_has_cover(cover.is_some());
        ui.set_view_cover(cover.unwrap_or_default());
        self.filter = Some(filter);
    }

    fn show_albums(&mut self) {
        let albums = Rc::new(self.catalog.albums());
        let ui = self.ui();
        ui.set_view_title("Albums".into());
        ui.set_view_subtitle(format!("{} albums", albums.len()).into());
        ui.set_view_has_cover(false);
        let grid = Rc::new(AlbumGrid::new(albums, ui.get_album_columns().max(1) as usize));
        ui.set_lib_album_rows(ModelRc::from(grid.clone()));
        self.album_grid = Some(grid);
        self.filter = None;
    }

    fn show_artists(&mut self) {
        let artists = self.catalog.artists();
        let rows: Vec<ArtistRow> = artists
            .iter()
            .map(|(name, n)| ArtistRow {
                name: name.clone().into(),
                count: if *n == 1 { "1 song".into() } else { format!("{n} songs").into() },
                tint: tint_for(name),
            })
            .collect();
        let ui = self.ui();
        ui.set_view_title("Artists".into());
        ui.set_view_subtitle(format!("{} artists", artists.len()).into());
        ui.set_view_has_cover(false);
        ui.set_lib_artists(ModelRc::new(VecModel::from(rows)));
        self.artists = artists.into_iter().map(|(n, _)| n).collect();
        self.filter = None;
    }

    pub fn open_album(&mut self, row: usize, col: usize) {
        let Some(album) = self.album_grid.as_ref().and_then(|g| g.album(row, col)).cloned() else { return };
        let thumb = catalog::thumbnail_path(&album.name);
        let cover = thumb.exists().then(|| Image::load_from_path(&thumb).ok()).flatten();
        self.ui().set_view("album".into());
        self.view = "album".into();
        self.show_songs(Filter::Album(album.name.clone()), &album.name, cover);
        if !album.artist.is_empty() {
            let sub = self.ui().get_view_subtitle();
            self.ui().set_view_subtitle(format!("{} · {sub}", album.artist).into());
        }
    }

    pub fn open_artist(&mut self, i: usize) {
        let Some(name) = self.artists.get(i).cloned() else { return };
        self.ui().set_view("artist".into());
        self.view = "artist".into();
        self.show_songs(Filter::Artist(name.clone()), &name, None);
    }

    /// Back from an album or artist to the grid or list, as it was.
    pub fn go_back(&mut self) {
        match self.view.as_str() {
            "album" => {
                let ui = self.ui();
                ui.set_view("albums".into());
                self.view = "albums".into();
                match self.album_grid.clone() {
                    // Keep the same grid so covers don't reload and the scroll spot holds.
                    Some(grid) => {
                        ui.set_view_title("Albums".into());
                        ui.set_view_subtitle(format!("{} albums", grid.len()).into());
                        ui.set_view_has_cover(false);
                        ui.set_lib_album_rows(ModelRc::from(grid));
                        self.filter = None;
                    }
                    None => self.show_view("albums"),
                }
            }
            "artist" => self.show_view("artists"),
            _ => {}
        }
    }

    pub fn album_columns_changed(&mut self, n: usize) {
        if let Some(g) = &self.album_grid {
            g.set_columns(n);
        }
    }

    /// Search as you type, after a short pause.
    pub fn search_changed(&mut self, text: String) {
        self.search_timer.start(TimerMode::SingleShot, Duration::from_millis(160), move || {
            let text = text.clone();
            with(|p| {
                if text.trim().is_empty() {
                    if p.view == "search" {
                        p.show_view("songs");
                    }
                    return;
                }
                p.ui().set_view("search".into());
                p.view = "search".into();
                let title = format!("“{}”", text.trim());
                p.show_songs(Filter::Search(text.clone()), &title, None);
            });
        });
    }

    pub fn play_from_list(&mut self, i: usize) {
        if i < self.list.len() {
            let list = self.list.clone();
            self.start_queue_at(list, i);
        }
    }

    pub fn play_list(&mut self, shuffled: bool) {
        let mut list = self.list.clone();
        if list.is_empty() {
            return;
        }
        if shuffled {
            fastrand::shuffle(&mut list);
        }
        self.start_queue_at(list, 0);
    }

    // ---------- favourites ----------

    pub fn heart_row(&mut self, i: usize) {
        let Some(path) = self.list.get(i).cloned() else { return };
        let fav = !self.catalog.is_favorite(&path);
        self.catalog.set_favorite(&path, fav);
        self.after_heart(&path, fav);
    }

    pub fn heart_current(&mut self) {
        let Some(path) = self.current_path.clone() else { return };
        let fav = !self.catalog.is_favorite(&path);
        self.catalog.set_favorite(&path, fav);
        self.after_heart(&path, fav);
    }

    fn after_heart(&mut self, path: &PathBuf, fav: bool) {
        if self.current_path.as_ref() == Some(path) {
            self.ui().set_current_favorite(fav);
        }
        if self.view == "favorites" {
            self.show_view("favorites");
            return;
        }
        if let Some(i) = self.list.iter().position(|p| p == path) {
            if let Some(mut row) = self.lib_model.row_data(i) {
                row.favorite = fav;
                self.lib_model.set_row_data(i, row);
            }
        }
    }

    /// After the current track changes: heart state, play count, list markers.
    pub fn on_track_changed(&mut self) {
        let Some(path) = self.current_path.clone() else { return };
        self.ui().set_current_favorite(self.catalog.is_favorite(&path));
        self.catalog.record_play(&path);
        for i in 0..self.lib_model.row_count() {
            let Some(mut row) = self.lib_model.row_data(i) else { continue };
            let current = self.list.get(i) == Some(&path);
            if row.current != current {
                row.current = current;
                self.lib_model.set_row_data(i, row);
            }
        }
    }

    // ---------- library folders ----------

    pub fn add_library_folder(&mut self) {
        let _ = slint::spawn_local(async {
            let picked = rfd::AsyncFileDialog::new().set_title("Add a folder to your library").pick_folder().await;
            if let Some(folder) = picked {
                with(|p| {
                    p.catalog.add_dir(folder.path());
                    p.refresh_library_folders();
                    p.start_scan();
                });
            }
        });
    }

    /// Re-index the library folders in the background.
    pub fn start_scan(&mut self) {
        if self.scanning || self.catalog.all_dirs().is_empty() {
            return;
        }
        self.scanning = true;
        let first_time = self.catalog.is_empty();
        if first_time {
            self.ui().set_scan_status("Looking for music…".into());
        }
        std::thread::spawn(move || {
            let changed = catalog::scan(|n| {
                if first_time {
                    let _ = slint::invoke_from_event_loop(move || {
                        with(|p| p.ui().set_scan_status(format!("Indexed {n} files…").into()));
                    });
                }
            });
            let _ = slint::invoke_from_event_loop(move || {
                with(|p| p.scan_finished(changed));
            });
        });
    }

    fn scan_finished(&mut self, changed: bool) {
        self.scanning = false;
        let ui = self.ui();
        ui.set_scan_status(SharedString::new());
        ui.set_library_empty(self.catalog.is_empty());
        if changed && self.view != "now" {
            // Refresh what's on screen.
            match self.filter.clone() {
                Some(f) if !matches!(f, Filter::Album(_) | Filter::Artist(_) | Filter::Search(_)) => {
                    let v = self.view.clone();
                    self.show_view(&v);
                }
                None => {
                    let v = self.view.clone();
                    self.show_view(&v);
                }
                _ => {}
            }
        }
        // Check again every 10 minutes for new files.
        self.rescan_timer.start(TimerMode::SingleShot, Duration::from_secs(600), || {
            with(|p| p.start_scan());
        });
    }

    /// The library follows the session: show this session's folders.
    pub fn library_follow_session(&mut self) {
        self.catalog.set_session(&self.session);
        self.ui().set_library_empty(self.catalog.is_empty());
        self.refresh_library_folders();
        self.album_grid = None;
        if self.view != "now" {
            let v = if matches!(self.view.as_str(), "album" | "artist" | "search") { "songs".to_string() } else { self.view.clone() };
            self.show_view(&v);
        }
    }

    pub fn init_library(&mut self) {
        self.ui().set_library_empty(self.catalog.is_empty());
        self.refresh_library_folders();
        self.start_scan();
    }
}

pub type LibModel = Rc<VecModel<LibRow>>;
