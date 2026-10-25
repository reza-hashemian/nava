//! Everything Nava remembers between runs, in one small SQLite file.

use std::path::PathBuf;

use rusqlite::{Connection, OptionalExtension, params};

#[derive(Clone, Debug, PartialEq)]
pub struct Session {
    pub queue: Vec<PathBuf>,
    pub index: usize,
    pub position: f64,
    pub volume: f64,
    pub shuffle: bool,
    /// 0 = off, 1 = all, 2 = one
    pub repeat: i32,
}

impl Default for Session {
    fn default() -> Self {
        Session { queue: Vec::new(), index: 0, position: 0.0, volume: 80.0, shuffle: false, repeat: 1 }
    }
}

pub struct Store {
    db: Connection,
}

pub const BUILT_IN_SESSIONS: [&str; 3] = ["Main", "Code", "Relax"];

impl Store {
    pub fn open() -> anyhow::Result<Store> {
        let dir = dirs::data_dir().unwrap_or_else(std::env::temp_dir).join("nava");
        std::fs::create_dir_all(&dir)?;
        Self::open_at(dir.join("nava.db"))
    }

    pub fn open_at(path: PathBuf) -> anyhow::Result<Store> {
        let db = Connection::open(path)?;
        db.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=NORMAL;
             CREATE TABLE IF NOT EXISTS settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
             CREATE TABLE IF NOT EXISTS sessions (
                 name TEXT PRIMARY KEY,
                 queue TEXT NOT NULL,
                 idx INTEGER NOT NULL,
                 position REAL NOT NULL,
                 volume REAL NOT NULL,
                 shuffle INTEGER NOT NULL,
                 repeat INTEGER NOT NULL,
                 updated_at INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS bookmarks (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 path TEXT NOT NULL,
                 position REAL NOT NULL,
                 note TEXT NOT NULL,
                 created INTEGER NOT NULL
             );
             CREATE INDEX IF NOT EXISTS bookmarks_path ON bookmarks(path);
             CREATE TABLE IF NOT EXISTS video_positions (
                 path TEXT PRIMARY KEY,
                 position REAL NOT NULL,
                 updated_at INTEGER NOT NULL
             );",
        )?;
        Ok(Store { db })
    }

    pub fn setting(&self, key: &str) -> Option<String> {
        self.db
            .query_row("SELECT value FROM settings WHERE key = ?1", [key], |r| r.get(0))
            .optional()
            .ok()
            .flatten()
    }

    pub fn set_setting(&self, key: &str, value: &str) {
        let _ = self.db.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        );
    }

    /// Where you stopped watching a video, if somewhere in the middle.
    pub fn video_position(&self, path: &std::path::Path) -> Option<f64> {
        self.db
            .query_row("SELECT position FROM video_positions WHERE path = ?1", [path.to_string_lossy()], |r| r.get(0))
            .optional()
            .ok()
            .flatten()
    }

    /// Remember a video's position. Near the start or the end, forget it instead,
    /// so finished videos start over next time.
    pub fn set_video_position(&self, path: &std::path::Path, position: f64, duration: f64) {
        let p = path.to_string_lossy();
        if position < 15.0 || (duration > 0.0 && duration - position < 20.0) {
            let _ = self.db.execute("DELETE FROM video_positions WHERE path = ?1", [p]);
            return;
        }
        let _ = self.db.execute(
            "INSERT INTO video_positions (path, position, updated_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(path) DO UPDATE SET position = excluded.position, updated_at = excluded.updated_at",
            params![p, position, now()],
        );
    }

    /// Bookmarks on a file, in time order: (id, position, note).
    pub fn bookmarks(&self, path: &std::path::Path) -> Vec<(i64, f64, String)> {
        let Ok(mut st) = self.db.prepare("SELECT id, position, note FROM bookmarks WHERE path = ?1 ORDER BY position") else {
            return Vec::new();
        };
        st.query_map([path.to_string_lossy()], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .map(|r| r.flatten().collect())
            .unwrap_or_default()
    }

    pub fn add_bookmark(&self, path: &std::path::Path, position: f64, note: &str) -> i64 {
        let _ = self.db.execute(
            "INSERT INTO bookmarks (path, position, note, created) VALUES (?1, ?2, ?3, ?4)",
            params![path.to_string_lossy(), position, note, now()],
        );
        self.db.last_insert_rowid()
    }

    pub fn delete_bookmark(&self, id: i64) {
        let _ = self.db.execute("DELETE FROM bookmarks WHERE id = ?1", [id]);
    }

    pub fn load_session(&self, name: &str) -> Option<Session> {
        self.db
            .query_row(
                "SELECT queue, idx, position, volume, shuffle, repeat FROM sessions WHERE name = ?1",
                [name],
                |r| {
                    let queue: String = r.get(0)?;
                    Ok(Session {
                        queue: queue.lines().filter(|l| !l.is_empty()).map(PathBuf::from).collect(),
                        index: r.get::<_, i64>(1)?.max(0) as usize,
                        position: r.get(2)?,
                        volume: r.get(3)?,
                        shuffle: r.get::<_, i64>(4)? != 0,
                        repeat: r.get(5)?,
                    })
                },
            )
            .optional()
            .ok()
            .flatten()
    }

    /// Session names in sidebar order. The three built-in ones come first.
    pub fn session_names(&self) -> Vec<String> {
        let mut names: Vec<String> = BUILT_IN_SESSIONS.iter().map(|s| s.to_string()).collect();
        if let Some(list) = self.setting("sessions") {
            for n in list.lines().map(str::trim).filter(|n| !n.is_empty()) {
                if !names.iter().any(|x| x.eq_ignore_ascii_case(n)) {
                    names.push(n.to_string());
                }
            }
        }
        names
    }

    /// Add a session. Returns its name as stored (an existing one keeps its spelling).
    pub fn add_session(&self, name: &str) -> Option<String> {
        let name = name.trim();
        if name.is_empty() || name.contains('\n') {
            return None;
        }
        let names = self.session_names();
        if let Some(existing) = names.iter().find(|n| n.eq_ignore_ascii_case(name)) {
            return Some(existing.clone());
        }
        let custom: Vec<&str> = names.iter().skip(BUILT_IN_SESSIONS.len()).map(String::as_str).chain([name]).collect();
        self.set_setting("sessions", &custom.join("\n"));
        Some(name.to_string())
    }

    /// Forget a session you made (the built-in ones stay). Files are untouched.
    pub fn delete_session(&self, name: &str) {
        if BUILT_IN_SESSIONS.contains(&name) {
            return;
        }
        let custom: Vec<String> =
            self.session_names().into_iter().skip(BUILT_IN_SESSIONS.len()).filter(|n| n != name).collect();
        self.set_setting("sessions", &custom.join("\n"));
        let _ = self.db.execute("DELETE FROM sessions WHERE name = ?1", [name]);
    }

    pub fn save_session(&self, name: &str, s: &Session) {
        let queue = s.queue.iter().map(|p| p.to_string_lossy()).collect::<Vec<_>>().join("\n");
        let now = now();
        let _ = self.db.execute(
            "INSERT INTO sessions (name, queue, idx, position, volume, shuffle, repeat, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(name) DO UPDATE SET queue = excluded.queue, idx = excluded.idx,
                 position = excluded.position, volume = excluded.volume, shuffle = excluded.shuffle,
                 repeat = excluded.repeat, updated_at = excluded.updated_at",
            params![name, queue, s.index as i64, s.position, s.volume, s.shuffle as i64, s.repeat, now],
        );
    }
}

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_round_trip() {
        let dir = std::env::temp_dir().join(format!("nava-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let store = Store::open_at(dir.join("t.db")).unwrap();
        let s = Session {
            queue: vec!["/a/1.mp3".into(), "/a/دو.flac".into()],
            index: 1,
            position: 93.5,
            volume: 60.0,
            shuffle: true,
            repeat: 2,
        };
        store.save_session("code", &s);
        assert_eq!(store.load_session("code"), Some(s));
        store.set_video_position(std::path::Path::new("/v/a.mkv"), 600.0, 3600.0);
        assert_eq!(store.video_position(std::path::Path::new("/v/a.mkv")), Some(600.0));
        store.set_video_position(std::path::Path::new("/v/a.mkv"), 3590.0, 3600.0);
        assert_eq!(store.video_position(std::path::Path::new("/v/a.mkv")), None);
        let id = store.add_bookmark(std::path::Path::new("/v/a.mkv"), 75.0, "closures");
        store.add_bookmark(std::path::Path::new("/v/a.mkv"), 12.0, "");
        let marks = store.bookmarks(std::path::Path::new("/v/a.mkv"));
        assert_eq!(marks.len(), 2);
        assert_eq!(marks[1].2, "closures");
        store.delete_bookmark(id);
        assert_eq!(store.bookmarks(std::path::Path::new("/v/a.mkv")).len(), 1);
        store.set_setting("glass", "0.7");
        assert_eq!(store.setting("glass").as_deref(), Some("0.7"));
        assert_eq!(store.add_session("  Study ").as_deref(), Some("Study"));
        assert_eq!(store.add_session("study").as_deref(), Some("Study"));
        assert_eq!(store.add_session("code").as_deref(), Some("Code"));
        assert_eq!(store.session_names(), ["Main", "Code", "Relax", "Study"]);
        store.save_session("Study", &Session::default());
        store.delete_session("Study");
        store.delete_session("Main");
        assert_eq!(store.session_names(), ["Main", "Code", "Relax"]);
        assert_eq!(store.load_session("Study"), None);
        std::fs::remove_dir_all(dir).ok();
    }
}
