//! Track tags, cover art, the colours we pull out of it, and the blurred
//! backdrop the glass sits on. All of this runs on worker threads and hands
//! plain pixel buffers back to the UI thread.

use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

use image::{DynamicImage, RgbaImage, imageops::FilterType};
use lofty::config::ParseOptions;
use lofty::prelude::*;
use lofty::probe::Probe;
use slint::{Rgba8Pixel, SharedPixelBuffer};

#[derive(Clone, Debug, Default)]
pub struct Tags {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration: f64,
    /// Position on the album, 0 when unknown.
    pub track_no: u32,
    pub genre: String,
    /// 1 for mono. 0 when unknown.
    pub channels: u8,
    /// kbit/s, 0 when unknown.
    pub bitrate: u32,
}

/// Read tags without cover art. Falls back to guessing from the file name.
pub fn read_tags(path: &Path) -> Tags {
    let mut t = Tags::default();
    if crate::library::is_link(path) {
        // The real title arrives from yt-dlp once the link loads.
        let (url, _) = crate::library::link_target(path);
        t.title = url.split("://").nth(1).unwrap_or(&url).to_string();
        return t;
    }
    let parsed = Probe::open(path)
        .and_then(|p| p.options(ParseOptions::new().read_cover_art(false)).read());
    if let Ok(file) = parsed {
        let props = file.properties();
        t.duration = props.duration().as_secs_f64();
        t.channels = props.channels().unwrap_or(0);
        t.bitrate = props.audio_bitrate().unwrap_or(0);
        if let Some(tag) = file.primary_tag().or_else(|| file.first_tag()) {
            t.title = tag.title().map(|s| s.trim().to_string()).unwrap_or_default();
            t.artist = tag.artist().map(|s| s.trim().to_string()).unwrap_or_default();
            t.album = tag.album().map(|s| s.trim().to_string()).unwrap_or_default();
            t.track_no = tag.track().unwrap_or(0);
            t.genre = tag.genre().map(|s| s.trim().to_string()).unwrap_or_default();
        }
    }
    if t.title.is_empty() || t.artist.is_empty() {
        let (artist, title) = guess_from_name(path);
        if t.title.is_empty() {
            t.title = title;
        }
        if t.artist.is_empty() {
            t.artist = artist;
        }
    }
    t
}

/// "Artist - Title.mp3" → ("Artist", "Title"). Without a dash, the artist is empty.
fn guess_from_name(path: &Path) -> (String, String) {
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("").replace('_', " ");
    for sep in [" - ", " – ", " — ", " ~ "] {
        if let Some((a, b)) = stem.split_once(sep) {
            let (a, b) = (a.trim(), b.trim());
            if !a.is_empty() && !b.is_empty() {
                return (a.to_string(), b.to_string());
            }
        }
    }
    (String::new(), stem.trim().to_string())
}

/// Colours taken from the artwork, as 0xRRGGBB.
#[derive(Clone, Copy, Debug)]
pub struct Palette {
    /// The most vivid colour, used for the accent.
    pub accent: u32,
    /// The overall average, used to tint the glass.
    pub mood: u32,
    /// The colours of the four corners of the artwork (top-left, top-right,
    /// bottom-left, bottom-right), softened. The window's glow is painted from them.
    pub glow: [u32; 4],
}

/// Everything the UI needs to show a track's artwork.
pub struct Art {
    /// Large cover with rounded corners baked in.
    pub cover: SharedPixelBuffer<Rgba8Pixel>,
    /// Small cover with rounder corners, for the mini player and lists.
    pub small: SharedPixelBuffer<Rgba8Pixel>,
    pub palette: Palette,
    /// True when no real artwork was found and we drew a gradient instead.
    pub generated: bool,
    /// A small JPEG copy for the desktop's "now playing" widget.
    pub file: Option<PathBuf>,
}

/// Where cover copies for the desktop widget go. Cleared on every start.
pub fn cover_cache_dir() -> PathBuf {
    dirs::cache_dir().unwrap_or_else(std::env::temp_dir).join("nava").join("covers")
}

fn save_cover_copy(path: &Path, cover: &RgbaImage) -> Option<PathBuf> {
    let dir = cover_cache_dir();
    std::fs::create_dir_all(&dir).ok()?;
    let mut h = std::collections::hash_map::DefaultHasher::new();
    path.hash(&mut h);
    let file = dir.join(format!("{:016x}.jpg", h.finish()));
    if !file.exists() {
        let small = image::imageops::thumbnail(cover, 300, 300);
        DynamicImage::ImageRgba8(small).to_rgb8().save(&file).ok()?;
    }
    Some(file)
}

pub fn load_art(path: &Path) -> Art {
    let img = if crate::library::is_link(path) {
        link_thumbnail(path)
    } else {
        embedded_cover(path).or_else(|| folder_cover(path))
    };
    let generated = img.is_none();
    let img = img.unwrap_or_else(|| generated_cover(path));
    let mut cover = img.resize_to_fill(560, 560, FilterType::Triangle).to_rgba8();
    let file = if generated { None } else { save_cover_copy(path, &cover) };
    round_corners(&mut cover, 22.0);
    let mut small = img.resize_to_fill(120, 120, FilterType::Triangle).to_rgba8();
    round_corners(&mut small, 16.0);
    let tiny = img.resize_exact(24, 24, FilterType::Triangle).to_rgba8();
    Art { file, cover: to_buffer(&cover), small: to_buffer(&small), palette: palette(&tiny), generated }
}

/// Make the corners transparent, with a soft (anti-aliased) edge. Done on the
/// pixels because Slint's software renderer can't clip images to rounded shapes.
pub fn round_corners(img: &mut RgbaImage, radius: f32) {
    let (w, h) = (img.width() as f32, img.height() as f32);
    for (x, y, p) in img.enumerate_pixels_mut() {
        let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
        let cx = if fx < radius { radius } else if fx > w - radius { w - radius } else { continue };
        let cy = if fy < radius { radius } else if fy > h - radius { h - radius } else { continue };
        let d = ((fx - cx).powi(2) + (fy - cy).powi(2)).sqrt();
        let coverage = (radius + 0.5 - d).clamp(0.0, 1.0);
        p.0[3] = (p.0[3] as f32 * coverage) as u8;
    }
}

/// A small square cover for album grids, saved as PNG with rounded corners. `None` without artwork.
pub fn save_thumbnail(track: &Path, out: &Path, size: u32) -> Option<()> {
    let img = embedded_cover(track).or_else(|| folder_cover(track))?;
    let mut thumb = img.resize_to_fill(size, size, FilterType::Triangle).to_rgba8();
    round_corners(&mut thumb, size as f32 * 0.07);
    if let Some(dir) = out.parent() {
        std::fs::create_dir_all(dir).ok()?;
    }
    thumb.save(out).ok()
}

/// A link's thumbnail, fetched with yt-dlp into the cache.
fn link_thumbnail(path: &Path) -> Option<DynamicImage> {
    let (url, _) = crate::library::link_target(path);
    let mut h = std::collections::hash_map::DefaultHasher::new();
    url.hash(&mut h);
    let dir = cover_cache_dir().join("links");
    let base = dir.join(format!("{:016x}", h.finish()));
    let jpg = base.with_extension("jpg");
    if !jpg.exists() {
        std::fs::create_dir_all(&dir).ok()?;
        let status = std::process::Command::new("yt-dlp")
            .args(["--skip-download", "--no-playlist", "--write-thumbnail", "--convert-thumbnails", "jpg", "-q", "-o"])
            .arg(&base)
            .arg(&url)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .ok()?;
        if !status.success() {
            return None;
        }
    }
    image::open(jpg).ok()
}

fn embedded_cover(path: &Path) -> Option<DynamicImage> {
    let file = Probe::open(path).ok()?.read().ok()?;
    let tag = file.primary_tag().or_else(|| file.first_tag())?;
    let pic = tag
        .pictures()
        .iter()
        .find(|p| p.pic_type() == lofty::picture::PictureType::CoverFront)
        .or_else(|| tag.pictures().first())?;
    image::load_from_memory(pic.data()).ok()
}

fn folder_cover(path: &Path) -> Option<DynamicImage> {
    let dir = path.parent()?;
    let names = ["cover", "folder", "front", "album", "artwork"];
    let exts = ["jpg", "jpeg", "png", "webp"];
    let mut found: Option<PathBuf> = None;
    for e in std::fs::read_dir(dir).ok()?.flatten() {
        let p = e.path();
        let (Some(stem), Some(ext)) = (
            p.file_stem().and_then(|s| s.to_str()).map(str::to_lowercase),
            p.extension().and_then(|s| s.to_str()).map(str::to_lowercase),
        ) else {
            continue;
        };
        if exts.contains(&ext.as_str()) && names.contains(&stem.as_str()) {
            found = Some(p);
            break;
        }
    }
    image::open(found?).ok()
}

/// A soft two-colour gradient, seeded by the folder and title so every track
/// without artwork still gets its own colours.
fn generated_cover(path: &Path) -> DynamicImage {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    path.parent().hash(&mut h);
    let seed = h.finish();
    let h1 = (seed % 360) as f32;
    let h2 = (h1 + 40.0 + ((seed >> 16) % 80) as f32) % 360.0;
    let (a, b) = (hsv_to_rgb(h1, 0.62, 0.92), hsv_to_rgb(h2, 0.70, 0.62));
    let n = 256u32;
    let img = RgbaImage::from_fn(n, n, |x, y| {
        let t = (x as f32 + y as f32) / (2.0 * n as f32);
        let mix = |i: usize| (a[i] as f32 * (1.0 - t) + b[i] as f32 * t) as u8;
        image::Rgba([mix(0), mix(1), mix(2), 255])
    });
    DynamicImage::ImageRgba8(img)
}

fn palette(px: &RgbaImage) -> Palette {
    let mut bins = [(0f32, 0f32, 0f32, 0f32); 12];
    let (mut ar, mut ag, mut ab) = (0f32, 0f32, 0f32);
    for p in px.pixels() {
        let [r, g, b, _] = p.0;
        ar += r as f32;
        ag += g as f32;
        ab += b as f32;
        let (h, s, v) = rgb_to_hsv(r, g, b);
        // Favour colourful, mid-to-bright pixels.
        let w = s * s * v * (1.0 - (v - 0.7).abs());
        let bin = &mut bins[((h / 30.0) as usize).min(11)];
        bin.0 += r as f32 * w;
        bin.1 += g as f32 * w;
        bin.2 += b as f32 * w;
        bin.3 += w;
    }
    let n = (px.width() * px.height()) as f32;
    let mood = pack(ar / n, ag / n, ab / n);
    let best = bins.iter().copied().fold((0., 0., 0., 0.), |a, b| if b.3 > a.3 { b } else { a });
    let accent = if best.3 > 0.5 {
        let (h, s, _) = rgb_to_hsv((best.0 / best.3) as u8, (best.1 / best.3) as u8, (best.2 / best.3) as u8);
        // Keep the hue, but make it bright enough to work as a button colour.
        let [r, g, b] = hsv_to_rgb(h, s.clamp(0.45, 0.85), 0.88);
        pack(r as f32, g as f32, b as f32)
    } else {
        0x8a7cf0 // grey artwork: fall back to a soft violet
    };
    // The four corners, a little more colourful and kept away from black and white.
    let (w, h) = (px.width(), px.height());
    let quad = |x0: u32, y0: u32| {
        let (mut r, mut g, mut b, mut n) = (0f32, 0f32, 0f32, 0f32);
        for y in y0..y0 + h / 2 {
            for x in x0..x0 + w / 2 {
                let [pr, pg, pb, _] = px.get_pixel(x, y).0;
                r += pr as f32;
                g += pg as f32;
                b += pb as f32;
                n += 1.0;
            }
        }
        let (hh, ss, vv) = rgb_to_hsv((r / n) as u8, (g / n) as u8, (b / n) as u8);
        let [r, g, b] = hsv_to_rgb(hh, (ss * 1.3).min(0.85), vv.clamp(0.28, 0.9));
        pack(r as f32, g as f32, b as f32)
    };
    let glow = [quad(0, 0), quad(w / 2, 0), quad(0, h / 2), quad(w / 2, h / 2)];
    Palette { accent, mood, glow }
}

fn pack(r: f32, g: f32, b: f32) -> u32 {
    ((r as u32).min(255) << 16) | ((g as u32).min(255) << 8) | (b as u32).min(255)
}

fn to_buffer(img: &RgbaImage) -> SharedPixelBuffer<Rgba8Pixel> {
    SharedPixelBuffer::clone_from_slice(img.as_raw(), img.width(), img.height())
}

fn rgb_to_hsv(r: u8, g: u8, b: u8) -> (f32, f32, f32) {
    let (r, g, b) = (r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    let h = if d == 0.0 {
        0.0
    } else if max == r {
        60.0 * (((g - b) / d).rem_euclid(6.0))
    } else if max == g {
        60.0 * ((b - r) / d + 2.0)
    } else {
        60.0 * ((r - g) / d + 4.0)
    };
    let s = if max == 0.0 { 0.0 } else { d / max };
    (h, s, max)
}

fn hsv_to_rgb(h: f32, s: f32, v: f32) -> [u8; 3] {
    let c = v * s;
    let x = c * (1.0 - ((h / 60.0).rem_euclid(2.0) - 1.0).abs());
    let m = v - c;
    let (r, g, b) = match (h / 60.0) as u32 % 6 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    [((r + m) * 255.0) as u8, ((g + m) * 255.0) as u8, ((b + m) * 255.0) as u8]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guesses_artist_and_title() {
        let (a, t) = guess_from_name(Path::new("/x/Hojat Ashrafzade - Negaram.mp3"));
        assert_eq!((a.as_str(), t.as_str()), ("Hojat Ashrafzade", "Negaram"));
        let (a, t) = guess_from_name(Path::new("/x/Voice 033.m4a"));
        assert_eq!((a.as_str(), t.as_str()), ("", "Voice 033"));
    }

    #[test]
    fn hsv_round_trip() {
        let (h, s, v) = rgb_to_hsv(200, 80, 40);
        let [r, g, b] = hsv_to_rgb(h, s, v);
        assert!((r as i32 - 200).abs() <= 1 && (g as i32 - 80).abs() <= 1 && (b as i32 - 40).abs() <= 1);
    }
}
