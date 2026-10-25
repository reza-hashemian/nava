//! The music library: folders you add, and every track in them, indexed in
//! SQLite so browsing is instant. Rescans only re-read files that changed.

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use rusqlite::{Connection, OptionalExtension, params};

use crate::library;

#[derive(Clone, Debug)]
pub struct Song {
    pub path: PathBuf,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration: f64,
    pub favorite: bool,
}

#[derive(Clone, Debug)]
pub struct Album {
    pub name: String,
    pub artist: String,
}

/// Which songs a list shows.
#[derive(Clone, Debug, PartialEq)]
pub enum Filter {
    Songs,
    Search(String),
    Album(String),
    Artist(String),
    Favorites,
    Recent,
    Videos,
}

/// The session whose library is everything (the others show only their own folders).
pub const MAIN_SESSION: &str = "Main";

pub struct Catalog {
    db: Connection,
    /// The session being browsed: its folders are what the library shows.
    session: String,
    /// SQL condition on `path` for that session's folders ("1" = everything).
    scope: String,
}

pub fn db_path() -> PathBuf {
    dirs::data_dir().unwrap_or_else(std::env::temp_dir).join("nava").join("nava.db")
}

/// Where album covers for the grid are cached.
pub fn thumbnail_path(album: &str) -> PathBuf {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    album.to_lowercase().hash(&mut h);
    dirs::cache_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("nava")
        .join("albums")
        .join(format!("{:016x}.png", h.finish()))
}

fn open(path: &Path) -> rusqlite::Result<Connection> {
    let db = Connection::open(path)?;
    db.busy_timeout(std::time::Duration::from_secs(5))?;
    db.execute_batch(
        "PRAGMA journal_mode=WAL;
         PRAGMA synchronous=NORMAL;
         CREATE TABLE IF NOT EXISTS library_dirs (path TEXT PRIMARY KEY);
         -- Each session has its own library folders. (library_dirs is the old,
         -- shared list: its folders move to Main.)
         CREATE TABLE IF NOT EXISTS session_dirs (
             session TEXT NOT NULL,
             path TEXT NOT NULL,
             PRIMARY KEY (session, path)
         );
         INSERT OR IGNORE INTO session_dirs (session, path) SELECT 'Main', path FROM library_dirs;
         DELETE FROM library_dirs;
         CREATE TABLE IF NOT EXISTS tracks (
             path TEXT PRIMARY KEY,
             title TEXT NOT NULL,
             artist TEXT NOT NULL,
             album TEXT NOT NULL,
             track_no INTEGER NOT NULL,
             duration REAL NOT NULL,
             mtime INTEGER NOT NULL,
             added_at INTEGER NOT NULL,
             play_count INTEGER NOT NULL DEFAULT 0,
             last_played INTEGER,
             favorite INTEGER NOT NULL DEFAULT 0,
             is_video INTEGER NOT NULL DEFAULT 0
         );
         CREATE INDEX IF NOT EXISTS tracks_album ON tracks(album);
         CREATE INDEX IF NOT EXISTS tracks_artist ON tracks(artist);
         CREATE INDEX IF NOT EXISTS tracks_added ON tracks(added_at);",
    )?;
    Ok(db)
}

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

fn mtime(p: &Path) -> i64 {
    std::fs::metadata(p)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn song(r: &rusqlite::Row) -> rusqlite::Result<Song> {
    Ok(Song {
        path: PathBuf::from(r.get::<_, String>(0)?),
        title: r.get(1)?,
        artist: r.get(2)?,
        album: r.get(3)?,
        duration: r.get(4)?,
        favorite: r.get::<_, i64>(5)? != 0,
    })
}

const SONG_COLS: &str = "path, title, artist, album, duration, favorite";

/// SQL condition: `path` is inside one of `dirs`. Paths are compared as plain
/// text prefixes (LIKE would treat `_` and `%` in folder names as wildcards).
fn scope_sql(dirs: &[PathBuf]) -> String {
    if dirs.is_empty() {
        return "0".into();
    }
    let sep = std::path::MAIN_SEPARATOR;
    let parts: Vec<String> = dirs
        .iter()
        .map(|d| {
            let d = d.to_string_lossy();
            let prefix = format!("{}{sep}", d.trim_end_matches(sep));
            format!("substr(path, 1, {}) = '{}'", prefix.chars().count(), prefix.replace('\'', "''"))
        })
        .collect();
    format!("({})", parts.join(" OR "))
}

fn query_dirs(db: &Connection, sql: &str, arg: Option<&str>) -> Vec<PathBuf> {
    let Ok(mut st) = db.prepare(sql) else { return Vec::new() };
    let get = |r: &rusqlite::Row| r.get::<_, String>(0);
    let rows = match arg {
        Some(a) => st.query_map([a], get).map(|r| r.flatten().collect::<Vec<_>>()),
        None => st.query_map([], get).map(|r| r.flatten().collect::<Vec<_>>()),
    };
    rows.unwrap_or_default().into_iter().map(PathBuf::from).collect()
}

impl Catalog {
    pub fn open() -> anyhow::Result<Catalog> {
        Self::with_db(open(&db_path())?)
    }

    #[cfg(test)]
    fn open_at(path: &Path) -> anyhow::Result<Catalog> {
        Self::with_db(open(path)?)
    }

    fn with_db(db: Connection) -> anyhow::Result<Catalog> {
        let mut c = Catalog { db, session: MAIN_SESSION.into(), scope: "1".into() };
        c.set_session(MAIN_SESSION);
        Ok(c)
    }

    /// Browse another session's library.
    pub fn set_session(&mut self, name: &str) {
        self.session = name.to_string();
        self.scope = if name == MAIN_SESSION { "1".into() } else { scope_sql(&self.dirs()) };
    }

    /// Every library folder, of every session (what the scanner indexes).
    pub fn all_dirs(&self) -> Vec<PathBuf> {
        query_dirs(&self.db, "SELECT DISTINCT path FROM session_dirs ORDER BY path", None)
    }

    /// The current session's folders. Main's library is all of them.
    pub fn dirs(&self) -> Vec<PathBuf> {
        if self.session == MAIN_SESSION {
            return self.all_dirs();
        }
        query_dirs(&self.db, "SELECT path FROM session_dirs WHERE session = ?1 ORDER BY path", Some(&self.session))
    }

    pub fn add_dir(&mut self, dir: &Path) {
        let _ = self.db.execute(
            "INSERT OR IGNORE INTO session_dirs (session, path) VALUES (?1, ?2)",
            params![self.session, dir.to_string_lossy()],
        );
        let name = self.session.clone();
        self.set_session(&name);
    }

    /// A deleted session's folders leave with it.
    pub fn forget_session(&mut self, name: &str) {
        let current = std::mem::replace(&mut self.session, name.to_string());
        for d in self.dirs() {
            self.remove_dir(&d);
        }
        self.set_session(&current);
    }

    /// Take a folder out of this session's library (out of every session's, in
    /// Main). Its files stay on disk; they just leave the index unless another
    /// library folder still holds them.
    pub fn remove_dir(&mut self, dir: &Path) {
        let p = dir.to_string_lossy();
        let _ = if self.session == MAIN_SESSION {
            self.db.execute("DELETE FROM session_dirs WHERE path = ?1", [&p])
        } else {
            self.db.execute("DELETE FROM session_dirs WHERE session = ?1 AND path = ?2", params![self.session, p])
        };
        let name = self.session.clone();
        self.set_session(&name);
        let others = self.all_dirs();
        let Ok(mut st) = self.db.prepare("SELECT path FROM tracks") else { return };
        let paths: Vec<String> =
            st.query_map([], |r| r.get::<_, String>(0)).map(|r| r.flatten().collect()).unwrap_or_default();
        drop(st);
        let Ok(tx) = self.db.unchecked_transaction() else { return };
        for p in paths {
            let path = Path::new(&p);
            if path.starts_with(dir) && !others.iter().any(|d| path.starts_with(d)) {
                let _ = tx.execute("DELETE FROM tracks WHERE path = ?1", [&p]);
            }
        }
        let _ = tx.commit();
    }

    /// Whether this session's library has nothing in it.
    pub fn is_empty(&self) -> bool {
        let sql = format!("SELECT COUNT(*) FROM tracks WHERE {}", self.scope);
        self.db.query_row(&sql, [], |r| r.get::<_, i64>(0)).unwrap_or(0) == 0
    }

    pub fn songs(&self, filter: &Filter) -> Vec<Song> {
        let sc = &self.scope;
        let (sql, arg): (String, Option<String>) = match filter {
            Filter::Songs => (format!("SELECT {SONG_COLS} FROM tracks WHERE is_video = 0 AND {sc} ORDER BY title COLLATE NOCASE"), None),
            Filter::Videos => (format!("SELECT {SONG_COLS} FROM tracks WHERE is_video = 1 AND {sc} ORDER BY path"), None),
            Filter::Favorites => (format!("SELECT {SONG_COLS} FROM tracks WHERE favorite = 1 AND {sc} ORDER BY title COLLATE NOCASE"), None),
            Filter::Recent => (format!("SELECT {SONG_COLS} FROM tracks WHERE {sc} ORDER BY added_at DESC, path LIMIT 200"), None),
            Filter::Album(a) => (
                format!("SELECT {SONG_COLS} FROM tracks WHERE album = ?1 AND {sc} ORDER BY track_no, title COLLATE NOCASE"),
                Some(a.clone()),
            ),
            Filter::Artist(a) => (
                format!("SELECT {SONG_COLS} FROM tracks WHERE artist = ?1 AND {sc} ORDER BY album COLLATE NOCASE, track_no, title"),
                Some(a.clone()),
            ),
            Filter::Search(q) => (
                format!(
                    "SELECT {SONG_COLS} FROM tracks
                     WHERE (title LIKE ?1 OR artist LIKE ?1 OR album LIKE ?1 OR path LIKE ?1) AND {sc}
                     ORDER BY title COLLATE NOCASE LIMIT 500"
                ),
                Some(format!("%{}%", q.trim())),
            ),
        };
        let Ok(mut st) = self.db.prepare(&sql) else { return Vec::new() };
        let rows = match &arg {
            Some(a) => st.query_map([a], song),
            None => st.query_map([], song),
        };
        let mut out: Vec<Song> = rows.map(|r| r.flatten().collect()).unwrap_or_default();
        if matches!(filter, Filter::Videos) {
            out.sort_by(|a, b| library::natural_cmp(&a.path.to_string_lossy(), &b.path.to_string_lossy()));
        }
        out
    }

    pub fn albums(&self) -> Vec<Album> {
        let Ok(mut st) = self.db.prepare(&format!(
            "SELECT album, MIN(artist) FROM tracks
             WHERE album != '' AND is_video = 0 AND {} GROUP BY album ORDER BY album COLLATE NOCASE",
            self.scope
        )) else {
            return Vec::new();
        };
        st.query_map([], |r| {
            Ok(Album { name: r.get(0)?, artist: r.get(1)? })
        })
        .map(|r| r.flatten().collect())
        .unwrap_or_default()
    }

    pub fn artists(&self) -> Vec<(String, usize)> {
        let Ok(mut st) = self.db.prepare(&format!(
            "SELECT artist, COUNT(*) FROM tracks WHERE artist != '' AND is_video = 0 AND {}
             GROUP BY artist ORDER BY artist COLLATE NOCASE",
            self.scope
        )) else {
            return Vec::new();
        };
        st.query_map([], |r| Ok((r.get(0)?, r.get::<_, i64>(1)? as usize)))
            .map(|r| r.flatten().collect())
            .unwrap_or_default()
    }

    pub fn is_favorite(&self, path: &Path) -> bool {
        self.db
            .query_row("SELECT favorite FROM tracks WHERE path = ?1", [path.to_string_lossy()], |r| r.get::<_, i64>(0))
            .optional()
            .ok()
            .flatten()
            .unwrap_or(0)
            != 0
    }

    /// Mark a track as a favourite. Files outside the library are added so the heart sticks.
    pub fn set_favorite(&self, path: &Path, fav: bool) {
        let p = path.to_string_lossy();
        let changed = self.db.execute("UPDATE tracks SET favorite = ?2 WHERE path = ?1", params![p, fav as i64]).unwrap_or(0);
        if changed == 0 && fav {
            let t = crate::meta::read_tags(path);
            let _ = self.db.execute(
                "INSERT INTO tracks (path, title, artist, album, track_no, duration, mtime, added_at, favorite, is_video)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 1, ?9)",
                params![p, t.title, t.artist, t.album, t.track_no, t.duration, mtime(path), now(), library::is_video(path) as i64],
            );
        }
    }

    /// Forget a track (after its file was moved to the Trash).
    pub fn remove(&self, path: &Path) {
        let _ = self.db.execute("DELETE FROM tracks WHERE path = ?1", [path.to_string_lossy()]);
    }

    pub fn record_play(&self, path: &Path) {
        let _ = self.db.execute(
            "UPDATE tracks SET play_count = play_count + 1, last_played = ?2 WHERE path = ?1",
            params![path.to_string_lossy(), now()],
        );
    }
}

/// Bring the index up to date with the library folders. Runs on a worker
/// thread with its own connection. Returns true if anything changed.
pub fn scan(on_progress: impl Fn(usize)) -> bool {
    let Ok(db) = open(&db_path()) else { return false };
    let dirs: Vec<PathBuf> = {
        let Ok(mut st) = db.prepare("SELECT DISTINCT path FROM session_dirs") else { return false };
        st.query_map([], |r| r.get::<_, String>(0)).map(|r| r.flatten().map(PathBuf::from).collect()).unwrap_or_default()
    };
    let known: HashMap<String, i64> = {
        let Ok(mut st) = db.prepare("SELECT path, mtime FROM tracks") else { return false };
        st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))
            .map(|r| r.flatten().collect())
            .unwrap_or_default()
    };
    let mut seen = HashSet::new();
    let mut changed = false;
    let mut done = 0usize;
    let added = now();
    for dir in &dirs {
        let files = library::scan(dir, true);
        let mut batch = Vec::new();
        for f in files {
            let key = f.to_string_lossy().into_owned();
            let m = mtime(&f);
            if known.get(&key) != Some(&m) {
                batch.push((key.clone(), f, m));
            }
            seen.insert(key);
        }
        for chunk in batch.chunks(64) {
            let rows: Vec<_> = chunk.iter().map(|(k, f, m)| (k, crate::meta::read_tags(f), *m, library::is_video(f))).collect();
            let Ok(tx) = db.unchecked_transaction() else { continue };
            for (k, t, m, video) in rows {
                let _ = tx.execute(
                    "INSERT INTO tracks (path, title, artist, album, track_no, duration, mtime, added_at, is_video)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                     ON CONFLICT(path) DO UPDATE SET title = excluded.title, artist = excluded.artist,
                         album = excluded.album, track_no = excluded.track_no, duration = excluded.duration,
                         mtime = excluded.mtime, is_video = excluded.is_video",
                    params![k, t.title, t.artist, t.album, t.track_no, t.duration, m, added, video as i64],
                );
            }
            let _ = tx.commit();
            changed = true;
            done += chunk.len();
            on_progress(done);
        }
    }
    // Forget files that are gone from library folders (favourites outside them stay).
    let gone: Vec<&String> = known
        .keys()
        .filter(|k| !seen.contains(*k) && dirs.iter().any(|d| Path::new(k).starts_with(d)))
        .collect();
    if !gone.is_empty() {
        if let Ok(tx) = db.unchecked_transaction() {
            for k in gone {
                let _ = tx.execute("DELETE FROM tracks WHERE path = ?1", [k]);
            }
            let _ = tx.commit();
            changed = true;
        }
    }
    // Album covers for the grid.
    if let Ok(mut st) = db.prepare("SELECT album, MIN(path) FROM tracks WHERE album != '' AND is_video = 0 GROUP BY album") {
        let albums: Vec<(String, String)> =
            st.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).map(|r| r.flatten().collect()).unwrap_or_default();
        for (album, sample) in albums {
            let out = thumbnail_path(&album);
            if !out.exists() && crate::meta::save_thumbnail(Path::new(&sample), &out, 200).is_some() {
                changed = true;
            }
        }
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn favorites_and_filters() {
        let dir = std::env::temp_dir().join(format!("nava-cat-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut cat = Catalog::open_at(&dir.join("c.db")).unwrap();
        for (p, title, album) in [("/m/a.mp3", "Alpha", "One"), ("/m/b.mp3", "Beta", "One"), ("/m/c.mkv", "Clip", "")] {
            cat.db
                .execute(
                    "INSERT INTO tracks (path, title, artist, album, track_no, duration, mtime, added_at, is_video)
                     VALUES (?1, ?2, 'X', ?3, 0, 60, 0, 0, ?4)",
                    params![p, title, album, p.ends_with("mkv") as i64],
                )
                .unwrap();
        }
        assert_eq!(cat.songs(&Filter::Songs).len(), 2);
        assert_eq!(cat.songs(&Filter::Videos).len(), 1);
        assert_eq!(cat.albums().len(), 1);
        assert_eq!(cat.songs(&Filter::Search("bet".into())).len(), 1);
        cat.set_favorite(Path::new("/m/b.mp3"), true);
        assert!(cat.is_favorite(Path::new("/m/b.mp3")));
        assert_eq!(cat.songs(&Filter::Favorites).len(), 1);
        cat.add_dir(Path::new("/m"));
        cat.remove_dir(Path::new("/m"));
        assert!(cat.dirs().is_empty());
        assert!(cat.is_empty());
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn each_session_sees_its_own_folders() {
        let dir = std::env::temp_dir().join(format!("nava-scope-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut cat = Catalog::open_at(&dir.join("c.db")).unwrap();
        for p in ["/music/a.mp3", "/music/b.mp3", "/films/x.mkv", "/music_old/c.mp3", "/it's/d.mp3"] {
            cat.db
                .execute(
                    "INSERT INTO tracks (path, title, artist, album, track_no, duration, mtime, added_at, is_video)
                     VALUES (?1, ?1, 'X', '', 0, 60, 0, 0, ?2)",
                    params![p, p.ends_with("mkv") as i64],
                )
                .unwrap();
        }
        cat.set_session("mu");
        assert!(cat.is_empty(), "a new session starts with an empty library");
        cat.add_dir(Path::new("/music"));
        assert_eq!(cat.songs(&Filter::Songs).len(), 2, "/music_old is not inside /music");
        assert!(cat.songs(&Filter::Videos).is_empty());
        cat.set_session("vido");
        cat.add_dir(Path::new("/films"));
        cat.add_dir(Path::new("/it's"));
        assert_eq!(cat.songs(&Filter::Videos).len(), 1);
        assert_eq!(cat.songs(&Filter::Songs).len(), 1, "quotes in folder names are fine");
        cat.set_session(MAIN_SESSION);
        assert_eq!(cat.dirs().len(), 3, "Main shows every folder");
        assert_eq!(cat.songs(&Filter::Songs).len(), 4);
        cat.forget_session("vido");
        assert_eq!(cat.all_dirs(), vec![PathBuf::from("/music")]);
        std::fs::remove_dir_all(dir).ok();
    }
}
