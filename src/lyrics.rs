//! Lyrics: from a `.lrc` file next to the song, the file's own tags, or
//! LRCLIB (a free online lyrics database). Timed (LRC) lyrics follow the music.

use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Lyrics {
    /// (seconds, line). Times are -1 for lyrics without timing.
    pub lines: Vec<(f64, String)>,
    pub synced: bool,
    pub source: &'static str,
}

impl Lyrics {
    /// The line being sung at `t`, for timed lyrics.
    pub fn line_at(&self, t: f64) -> Option<usize> {
        if !self.synced {
            return None;
        }
        // A little early feels right: the eye should get there as the voice does.
        let t = t + 0.25;
        match self.lines.iter().rposition(|(start, _)| *start <= t) {
            Some(i) => Some(i),
            None => None,
        }
    }
}

/// Parse LRC (`[mm:ss.xx] words`) or plain text.
pub fn parse(text: &str) -> Lyrics {
    let mut offset = 0.0;
    let mut timed = Vec::new();
    let mut plain = Vec::new();
    for raw in text.lines() {
        let mut line = raw.trim();
        if let Some(v) = line.strip_prefix("[offset:").and_then(|r| r.strip_suffix(']')) {
            offset = v.trim().parse::<f64>().unwrap_or(0.0) / 1000.0;
            continue;
        }
        let mut times = Vec::new();
        while let Some(rest) = line.strip_prefix('[') {
            let Some(end) = rest.find(']') else { break };
            let tag = &rest[..end];
            match parse_time(tag) {
                Some(t) => times.push(t),
                None if tag.contains(':') => {} // [ar:...], [ti:...] and friends
                None => break,
            }
            line = rest[end + 1..].trim_start();
        }
        if times.is_empty() {
            if !raw.trim_start().starts_with('[') {
                plain.push(raw.trim().to_string());
            }
        } else {
            for t in times {
                timed.push(((t - offset).max(0.0), line.trim().to_string()));
            }
        }
    }
    if timed.len() >= 3 {
        timed.sort_by(|a, b| a.0.total_cmp(&b.0));
        Lyrics { lines: timed, synced: true, source: "" }
    } else {
        // Drop blank lines at the ends, keep the ones between verses.
        while plain.first().is_some_and(|l| l.is_empty()) {
            plain.remove(0);
        }
        while plain.last().is_some_and(|l| l.is_empty()) {
            plain.pop();
        }
        Lyrics { lines: plain.into_iter().map(|l| (-1.0, l)).collect(), synced: false, source: "" }
    }
}

fn parse_time(tag: &str) -> Option<f64> {
    let (m, s) = tag.split_once(':')?;
    let m: f64 = m.trim().parse().ok()?;
    let s: f64 = s.trim().replace(',', ".").parse().ok()?;
    Some(m * 60.0 + s)
}

/// Look for lyrics, in order: `song.lrc`, the file's tags, then online.
/// Runs on a worker thread.
pub fn find(path: &Path, title: &str, artist: &str, album: &str, duration: f64, online: bool) -> Option<Lyrics> {
    if !crate::library::is_link(path) {
        let lrc = path.with_extension("lrc");
        if let Ok(text) = std::fs::read_to_string(&lrc) {
            let mut l = parse(&text);
            if !l.lines.is_empty() {
                l.source = "file";
                return Some(l);
            }
        }
        if let Some(text) = embedded(path) {
            let mut l = parse(&text);
            if !l.lines.is_empty() {
                l.source = "tags";
                return Some(l);
            }
        }
    }
    if online && !title.trim().is_empty() && !artist.trim().is_empty() {
        let mut l = parse(&online_lookup(title, artist, album, duration)?);
        if !l.lines.is_empty() {
            l.source = "LRCLIB";
            return Some(l);
        }
    }
    None
}

fn embedded(path: &Path) -> Option<String> {
    use lofty::prelude::*;
    let file = lofty::probe::Probe::open(path).ok()?.read().ok()?;
    let tag = file.primary_tag().or_else(|| file.first_tag())?;
    tag.get_string(lofty::tag::ItemKey::Lyrics).map(str::to_string)
}

fn cache_path(title: &str, artist: &str) -> PathBuf {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (title.to_lowercase(), artist.to_lowercase()).hash(&mut h);
    dirs::cache_dir().unwrap_or_else(std::env::temp_dir).join("nava").join("lyrics").join(format!("{:016x}.lrc", h.finish()))
}

/// Ask LRCLIB, through `curl` so Nava needs no web stack of its own.
/// Answers (including "none") are cached so each song is asked about once.
fn online_lookup(title: &str, artist: &str, album: &str, duration: f64) -> Option<String> {
    let cache = cache_path(title, artist);
    if let Ok(text) = std::fs::read_to_string(&cache) {
        return (!text.is_empty()).then_some(text);
    }
    let mut url = format!(
        "https://lrclib.net/api/get?track_name={}&artist_name={}",
        encode(title.trim()),
        encode(artist.trim())
    );
    if !album.trim().is_empty() {
        url.push_str(&format!("&album_name={}", encode(album.trim())));
    }
    if duration > 0.0 {
        url.push_str(&format!("&duration={}", duration.round() as u64));
    }
    let out = std::process::Command::new("curl")
        .args(["-s", "--max-time", "8", "-H", "User-Agent: Nava (https://github.com/)", &url])
        .output()
        .ok()?;
    let body = String::from_utf8_lossy(&out.stdout);
    let text = json_string(&body, "syncedLyrics").or_else(|| json_string(&body, "plainLyrics")).unwrap_or_default();
    // Only remember real answers: a network error shouldn't hide lyrics forever.
    if out.status.success() && (body.contains("\"id\"") || body.contains("NotFound")) {
        if let Some(dir) = cache.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(&cache, &text);
    }
    (!text.is_empty()).then_some(text)
}

fn encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            b' ' => "+".into(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// Pull one string field out of a JSON object, without a JSON library.
fn json_string(json: &str, key: &str) -> Option<String> {
    let start = json.find(&format!("\"{key}\""))? + key.len() + 2;
    let rest = json[start..].trim_start().strip_prefix(':')?.trim_start();
    let mut chars = rest.strip_prefix('"')?.chars();
    let mut out = String::new();
    while let Some(c) = chars.next() {
        match c {
            '"' => return Some(out),
            '\\' => match chars.next()? {
                'n' => out.push('\n'),
                't' => out.push('\t'),
                'r' => {}
                'u' => {
                    let hex: String = chars.by_ref().take(4).collect();
                    let code = u32::from_str_radix(&hex, 16).ok()?;
                    // Surrogate pairs (emoji) come as two escapes.
                    if (0xD800..0xDC00).contains(&code) {
                        let rest: String = chars.by_ref().take(6).collect();
                        let low = u32::from_str_radix(rest.strip_prefix("\\u")?, 16).ok()?;
                        out.push(char::from_u32(0x10000 + ((code - 0xD800) << 10) + (low - 0xDC00))?);
                    } else {
                        out.push(char::from_u32(code)?);
                    }
                }
                other => out.push(other),
            },
            c => out.push(c),
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_lrc() {
        let l = parse("[ar:Someone]\n[00:01.00]one\n[00:05.50]two\n[00:03.00][00:09.00]three\n[offset:500]");
        assert!(l.synced);
        assert_eq!(l.lines.len(), 4);
        assert_eq!(l.lines[1], (3.0, "three".into()));
        assert_eq!(l.line_at(5.4), Some(2));
        assert_eq!(l.line_at(0.2), None);
    }

    #[test]
    fn plain_text() {
        let l = parse("\nسلام\n\nخداحافظ\n");
        assert!(!l.synced);
        assert_eq!(l.lines.len(), 3);
        assert_eq!(l.lines[0].1, "سلام");
    }

    #[test]
    fn json_fields() {
        let j = r#"{"id":1,"plainLyrics":"a \"b\"\nc é","syncedLyrics":null}"#;
        assert_eq!(json_string(j, "plainLyrics").as_deref(), Some("a \"b\"\nc é"));
        assert_eq!(json_string(j, "syncedLyrics"), None);
    }
}
