//! Finding playable files in a folder and choosing which of them to queue.

use std::cmp::Ordering;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

const AUDIO_EXT: &[&str] = &[
    "mp3", "flac", "wav", "ogg", "oga", "opus", "m4a", "m4b", "aac", "wma", "aiff", "aif", "ape", "wv", "mka", "alac",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pick {
    First,
    Random,
    Newest,
}

impl Pick {
    pub fn parse(s: &str) -> Option<Pick> {
        match s.to_ascii_lowercase().as_str() {
            "first" => Some(Pick::First),
            "random" => Some(Pick::Random),
            "newest" => Some(Pick::Newest),
            _ => None,
        }
    }
    pub fn from_index(i: i32) -> Pick {
        match i {
            1 => Pick::Random,
            2 => Pick::Newest,
            _ => Pick::First,
        }
    }
}

const VIDEO_EXT: &[&str] = &[
    "mp4", "m4v", "mkv", "webm", "avi", "mov", "wmv", "flv", "ts", "m2ts", "mts", "mpg", "mpeg", "3gp", "ogv",
];

fn ext_in(p: &Path, list: &[&str]) -> bool {
    p.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| list.contains(&e.to_ascii_lowercase().as_str()))
}

/// Links play through yt-dlp. "audio+https://…" marks a link to play as audio only.
pub const AUDIO_LINK: &str = "audio+";

pub fn is_link(p: &Path) -> bool {
    let s = p.to_string_lossy();
    s.starts_with("http://") || s.starts_with("https://") || s.starts_with(AUDIO_LINK)
}

/// The address to give mpv, and whether to skip the picture.
pub fn link_target(p: &Path) -> (String, bool) {
    let s = p.to_string_lossy();
    match s.strip_prefix(AUDIO_LINK) {
        Some(url) => (url.to_string(), true),
        None => (s.into_owned(), false),
    }
}

pub fn is_video(p: &Path) -> bool {
    if is_link(p) {
        return !p.to_string_lossy().starts_with(AUDIO_LINK);
    }
    ext_in(p, VIDEO_EXT)
}

pub fn is_playable(p: &Path) -> bool {
    ext_in(p, AUDIO_EXT) || ext_in(p, VIDEO_EXT)
}

/// Like `is_playable`, but also looks inside ".ts"-style files: most of them
/// are TypeScript ("index.d.ts"), not MPEG transport streams.
pub fn is_media_file(p: &Path) -> bool {
    if !is_playable(p) {
        return false;
    }
    if !ext_in(p, &["ts", "mts", "m2ts"]) {
        return true;
    }
    if p.to_string_lossy().to_ascii_lowercase().ends_with(".d.ts") {
        return false;
    }
    // A transport stream starts with the 0x47 sync byte, every 188 bytes
    // (or 192 with a 4-byte timestamp in front, as on Blu-ray).
    use std::io::Read;
    let mut head = [0u8; 384];
    let Ok(n) = std::fs::File::open(p).and_then(|mut f| f.read(&mut head)) else { return false };
    let synced = |start: usize, step: usize| start + step < n && head[start] == 0x47 && head[start + step] == 0x47;
    synced(0, 188) || synced(4, 192)
}

/// All playable files under `dir`, in natural order.
pub fn scan(dir: &Path, recursive: bool) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else { continue };
        for e in entries.flatten() {
            let p = e.path();
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_dir() {
                let hidden = p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with('.'));
                if recursive && !hidden {
                    stack.push(p);
                }
            } else if is_media_file(&p) {
                out.push(p);
            }
        }
    }
    out.sort_by(|a, b| natural_cmp(&a.to_string_lossy(), &b.to_string_lossy()));
    out
}

/// Apply a pick mode and an optional limit to a scanned list.
pub fn choose(mut files: Vec<PathBuf>, pick: Pick, limit: Option<usize>) -> Vec<PathBuf> {
    match pick {
        Pick::First => {}
        Pick::Random => fastrand::shuffle(&mut files),
        Pick::Newest => {
            let mtime = |p: &PathBuf| std::fs::metadata(p).and_then(|m| m.modified()).unwrap_or(SystemTime::UNIX_EPOCH);
            files.sort_by_cached_key(|p| std::cmp::Reverse(mtime(p)));
        }
    }
    if let Some(n) = limit {
        files.truncate(n);
    }
    files
}

/// Compare strings so that "Track 2" comes before "Track 10".
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let (mut a, mut b) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (a.peek().copied(), b.peek().copied()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let take = |it: &mut std::iter::Peekable<std::str::Chars>| {
                    let mut s = String::new();
                    while let Some(c) = it.peek().copied().filter(char::is_ascii_digit) {
                        s.push(c);
                        it.next();
                    }
                    s
                };
                let (na, nb) = (take(&mut a), take(&mut b));
                let (ta, tb) = (na.trim_start_matches('0'), nb.trim_start_matches('0'));
                let ord = ta.len().cmp(&tb.len()).then_with(|| ta.cmp(tb));
                if ord != Ordering::Equal {
                    return ord;
                }
            }
            (Some(x), Some(y)) => {
                let ord = x.to_lowercase().cmp(y.to_lowercase());
                if ord != Ordering::Equal {
                    return ord;
                }
                a.next();
                b.next();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn natural_order() {
        let mut v = vec!["Track 10.mp3", "track 2.mp3", "Track 1.mp3", "Intro.mp3"];
        v.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(v, ["Intro.mp3", "Track 1.mp3", "track 2.mp3", "Track 10.mp3"]);
    }

    #[test]
    fn links() {
        assert!(is_link(Path::new("https://youtu.be/x")));
        assert!(is_video(Path::new("https://youtu.be/x")));
        assert!(!is_video(Path::new("audio+https://youtu.be/x")));
        assert_eq!(link_target(Path::new("audio+https://youtu.be/x")), ("https://youtu.be/x".to_string(), true));
    }

    #[test]
    fn typescript_is_not_video() {
        let dir = std::env::temp_dir().join(format!("nava-ts-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let code = dir.join("utc.ts");
        std::fs::write(&code, "export const x = 1;\n".repeat(30)).unwrap();
        let mut stream = vec![0u8; 188 * 3];
        stream[0] = 0x47;
        stream[188] = 0x47;
        stream[376] = 0x47;
        let video = dir.join("clip.ts");
        std::fs::write(&video, &stream).unwrap();
        assert!(!is_media_file(&code));
        assert!(!is_media_file(Path::new("/x/isYesterday.d.ts")));
        assert!(is_media_file(&video));
        assert_eq!(scan(&dir, false), vec![video]);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn limit_applies() {
        let files: Vec<PathBuf> = (0..10).map(|i| PathBuf::from(format!("{i}.mp3"))).collect();
        assert_eq!(choose(files.clone(), Pick::First, Some(3)).len(), 3);
        assert_eq!(choose(files, Pick::Random, None).len(), 10);
    }
}
