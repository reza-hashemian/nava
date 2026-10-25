//! The equalizer: 10 bands, presets, a preamp, your own presets, and an Auto
//! mode that picks a preset for each song from its genre, or failing that
//! from what the file sounds like (mono, low bitrate, "mix" in the name…).

use std::path::Path;

use slint::{ModelRc, SharedString, VecModel};

use crate::meta::Tags;
use crate::{Player, library};

pub const BANDS: [u32; 10] = [31, 62, 125, 250, 500, 1000, 2000, 4000, 8000, 16000];

pub const PRESETS: &[(&str, [f32; 10])] = &[
    ("Flat", [0.0; 10]),
    ("Pop", [-1.0, 0.5, 2.0, 3.0, 2.0, 0.0, -0.5, 0.5, 1.5, 2.0]),
    ("Rock", [4.0, 3.0, 1.5, 0.0, -1.0, -0.5, 1.0, 2.5, 3.5, 4.0]),
    ("Hip-Hop", [5.0, 4.5, 2.5, 1.0, -0.5, -0.5, 1.0, 0.5, 1.5, 2.0]),
    ("EDM", [5.0, 4.0, 1.5, 0.0, -1.0, 1.0, 0.5, 2.0, 4.0, 4.5]),
    ("Deep House", [5.5, 4.5, 2.0, 0.5, -1.0, -1.5, -0.5, 1.0, 2.5, 3.0]),
    ("R&B", [3.0, 6.0, 4.5, 1.5, -1.5, -1.0, 1.5, 2.0, 2.5, 3.0]),
    ("Jazz", [3.0, 2.0, 1.0, 1.5, -1.0, -1.0, 0.0, 1.0, 2.0, 3.0]),
    ("Classical", [3.0, 2.5, 1.5, 1.0, -0.5, -0.5, 0.0, 1.5, 2.5, 3.0]),
    ("Acoustic", [3.0, 2.5, 2.0, 1.0, 1.5, 1.5, 2.5, 3.0, 2.5, 1.5]),
    ("Country", [2.0, 1.5, 0.5, 0.0, 1.0, 2.0, 2.5, 2.5, 2.0, 1.5]),
    ("Reggae", [0.0, 0.0, -0.5, -2.5, 0.0, 2.5, 2.5, 0.0, 0.0, 0.0]),
    ("Dream", [3.0, 2.0, 0.5, -1.0, -1.5, -1.0, 0.5, 2.0, 3.5, 4.5]),
    ("Vocal Boost", [-2.0, -2.0, -1.0, 1.0, 3.0, 4.0, 3.5, 1.5, 0.0, -1.0]),
    ("Podcast", [-4.0, -3.0, -1.0, 1.0, 3.0, 4.0, 3.0, 1.0, -1.0, -3.0]),
    ("Bass Boost", [6.0, 5.0, 4.0, 2.5, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0]),
    ("Treble Boost", [0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 2.5, 4.0, 5.0, 5.5]),
    ("Loudness", [5.0, 3.5, 0.0, 0.0, -1.0, 0.0, -0.5, 1.0, 4.0, 3.0]),
    ("Late Night", [-4.0, -3.0, -1.5, 0.0, 1.0, 2.0, 2.0, 1.0, 0.0, -1.0]),
];

pub fn preset(name: &str) -> Option<[f32; 10]> {
    PRESETS.iter().find(|(n, _)| *n == name).map(|(_, g)| *g)
}

#[derive(Clone, Debug)]
pub struct Eq {
    pub on: bool,
    pub auto: bool,
    pub gains: [f32; 10],
    pub preamp: f32,
    pub preset: String,
    /// What Auto picked for the current song.
    pub auto_choice: &'static str,
    pub custom: Vec<(String, [f32; 10])>,
}

impl Default for Eq {
    fn default() -> Self {
        Eq { on: false, auto: false, gains: [0.0; 10], preamp: 0.0, preset: "Flat".into(), auto_choice: "Flat", custom: Vec::new() }
    }
}

impl Eq {
    /// The gains actually in effect.
    pub fn effective(&self) -> [f32; 10] {
        if self.auto { preset(self.auto_choice).unwrap_or([0.0; 10]) } else { self.gains }
    }

    fn name_for(&self, g: &[f32; 10]) -> String {
        let same = |p: &[f32; 10]| p.iter().zip(g).all(|(a, b)| (a - b).abs() < 0.05);
        PRESETS
            .iter()
            .find(|(_, p)| same(p))
            .map(|(n, _)| n.to_string())
            .or_else(|| self.custom.iter().find(|(_, p)| same(p)).map(|(n, _)| n.clone()))
            .unwrap_or_else(|| "Custom".into())
    }

    /// Settings line: "2|on|auto|preamp|preset|g1,...,g10".
    fn encode(&self) -> String {
        let g: Vec<String> = self.gains.iter().map(|g| g.to_string()).collect();
        format!("2|{}|{}|{}|{}|{}", self.on as u8, self.auto as u8, self.preamp, self.preset, g.join(","))
    }

    fn decode(&mut self, v: &str) {
        let p: Vec<&str> = v.split('|').collect();
        let gains = |s: &str| -> Option<[f32; 10]> {
            let g: Vec<f32> = s.split(',').filter_map(|x| x.parse().ok()).collect();
            g.try_into().ok()
        };
        match p.as_slice() {
            ["2", on, auto, pre, name, g] => {
                self.on = *on == "1";
                self.auto = *auto == "1";
                self.preamp = pre.parse().unwrap_or(0.0);
                self.preset = name.to_string();
                if let Some(g) = gains(g) {
                    self.gains = g;
                }
            }
            // The first version: "on|preset|gains".
            [on, name, g] => {
                self.on = *on == "1";
                self.preset = name.to_string();
                if let Some(g) = gains(g) {
                    self.gains = g;
                }
            }
            _ => {}
        }
    }
}

/// Auto: the preset that suits this song.
pub fn choose(path: &Path, t: &Tags) -> &'static str {
    if library::is_video(path) {
        return "Flat";
    }
    let genre = t.genre.to_lowercase();
    let has = |words: &[&str]| words.iter().any(|w| genre.contains(w));
    if !genre.is_empty() {
        let pick = if has(&["podcast", "speech", "spoken", "audiobook", "talk", "lecture"]) {
            "Podcast"
        } else if has(&["hip", "rap", "trap", "drill"]) {
            "Hip-Hop"
        } else if has(&["house", "deep"]) {
            "Deep House"
        } else if has(&["electro", "edm", "dance", "techno", "trance", "dubstep"]) {
            "EDM"
        } else if has(&["r&b", "rnb", "soul"]) {
            "R&B"
        } else if has(&["rock", "metal", "punk", "grunge", "alternative"]) {
            "Rock"
        } else if has(&["jazz", "blues", "swing"]) {
            "Jazz"
        } else if has(&["classical", "orchestra", "symphony", "baroque", "opera", "instrumental", "piano"]) {
            "Classical"
        } else if has(&["acoustic", "folk", "singer"]) {
            "Acoustic"
        } else if has(&["country"]) {
            "Country"
        } else if has(&["reggae", "ska", "dub"]) {
            "Reggae"
        } else if has(&["ambient", "chill", "lo-fi", "lofi", "dream"]) {
            "Dream"
        } else if has(&["pop"]) {
            "Pop"
        } else {
            ""
        };
        if !pick.is_empty() {
            return pick;
        }
    }
    // No (useful) genre: listen to the file instead.
    let name = format!("{} {}", t.title, path.file_stem().map(|s| s.to_string_lossy()).unwrap_or_default()).to_lowercase();
    let spoken_words = ["voice", "podcast", "lecture", "جلسه", "سمینار", "کلاس", "درس", "سخنرانی", "پادکست", "audio 20"];
    let low_quality_speech = t.channels == 1 || (t.bitrate > 0 && t.bitrate <= 64);
    if low_quality_speech || spoken_words.iter().any(|w| name.contains(w)) {
        return "Podcast";
    }
    if ["mix", "dj ", "remix", "megamix", "set"].iter().any(|w| name.contains(w)) {
        return "EDM";
    }
    "Pop"
}

/// The response curve for drawing: a smooth line through the 10 points, and
/// the same shape closed off for the shaded area. Viewbox: 1000 × 200.
pub fn curve(g: &[f32; 10]) -> (String, String) {
    let pts: Vec<(f32, f32)> = g.iter().enumerate().map(|(i, v)| ((i as f32 + 0.5) * 100.0, 100.0 - v * 7.5)).collect();
    let mut all = vec![(0.0, pts[0].1)];
    all.extend(&pts);
    all.push((1000.0, pts[9].1));
    let mut d = format!("M {:.1} {:.1}", all[0].0, all[0].1);
    for i in 0..all.len() - 1 {
        let p0 = all[i.saturating_sub(1)];
        let (p1, p2) = (all[i], all[i + 1]);
        let p3 = all[(i + 2).min(all.len() - 1)];
        let c1 = (p1.0 + (p2.0 - p0.0) / 6.0, p1.1 + (p2.1 - p0.1) / 6.0);
        let c2 = (p2.0 - (p3.0 - p1.0) / 6.0, p2.1 - (p3.1 - p1.1) / 6.0);
        d.push_str(&format!(" C {:.1} {:.1} {:.1} {:.1} {:.1} {:.1}", c1.0, c1.1, c2.0, c2.1, p2.0, p2.1));
    }
    let fill = format!("{d} L 1000 200 L 0 200 Z");
    (d, fill)
}

impl Player {
    pub fn load_eq(&mut self) {
        if let Some(v) = self.store.setting("eq") {
            self.eq.decode(&v);
        }
        if let Some(c) = self.store.setting("eq_custom") {
            self.eq.custom = c
                .lines()
                .filter_map(|l| {
                    let (name, g) = l.split_once('|')?;
                    let g: Vec<f32> = g.split(',').filter_map(|x| x.parse().ok()).collect();
                    Some((name.to_string(), g.try_into().ok()?))
                })
                .collect();
        }
        let names: Vec<SharedString> = PRESETS.iter().map(|(n, _)| SharedString::from(*n)).collect();
        self.ui().set_eq_presets(ModelRc::new(VecModel::from(names)));
        self.refresh_custom();
        if self.eq.auto {
            self.eq_pick_for_current();
        }
        self.apply_eq();
    }

    fn refresh_custom(&self) {
        let names: Vec<SharedString> = self.eq.custom.iter().map(|(n, _)| SharedString::from(n.as_str())).collect();
        self.ui().set_eq_custom(ModelRc::new(VecModel::from(names)));
        let lines: Vec<String> = self
            .eq
            .custom
            .iter()
            .map(|(n, g)| format!("{n}|{}", g.iter().map(|x| x.to_string()).collect::<Vec<_>>().join(",")))
            .collect();
        self.store.set_setting("eq_custom", &lines.join("\n"));
    }

    pub fn apply_eq(&mut self) {
        let g = self.eq.effective();
        let ui = self.ui();
        ui.set_eq_on(self.eq.on);
        ui.set_eq_auto(self.eq.auto);
        ui.set_eq_auto_label(if self.eq.auto { format!("AUTO · {}", self.eq.auto_choice.to_uppercase()).into() } else { SharedString::new() });
        ui.set_eq_preset(if self.eq.auto { self.eq.auto_choice.into() } else { self.eq.preset.clone().into() });
        ui.set_eq_preamp(self.eq.preamp);
        ui.set_eq_gains(ModelRc::new(VecModel::from(g.to_vec())));
        let (line, fill) = curve(&g);
        ui.set_eq_curve(line.into());
        ui.set_eq_fill(fill.into());

        let flat = g.iter().all(|x| x.abs() < 0.05) && self.eq.preamp.abs() < 0.05;
        if !self.eq.on || flat {
            self.mpv.set_string("af", "");
        } else {
            let entries: Vec<String> = BANDS.iter().zip(g).map(|(f, v)| format!("entry({f},{v})")).collect();
            // Turn the signal down by the biggest boost so loud songs don't clip.
            let headroom = g.iter().cloned().fold(0.0f32, f32::max);
            let level = self.eq.preamp - headroom;
            let graph = format!("lavfi=[firequalizer=gain_entry='{}',volume={level}dB]", entries.join(";"));
            self.mpv.set_string("af", &graph);
        }
        self.store.set_setting("eq", &self.eq.encode());
    }

    pub fn eq_band(&mut self, band: usize, gain: f32) {
        if band >= 10 {
            return;
        }
        if self.eq.auto {
            // Editing by hand starts from what Auto was using.
            self.eq.gains = self.eq.effective();
            self.eq.auto = false;
        }
        self.eq.gains[band] = (gain * 2.0).round() / 2.0;
        self.eq.gains[band] = self.eq.gains[band].clamp(-12.0, 12.0);
        self.eq.on = true;
        self.eq.preset = self.eq.name_for(&self.eq.gains);
        self.apply_eq();
    }

    pub fn eq_choose(&mut self, name: &str) {
        let g = preset(name).or_else(|| self.eq.custom.iter().find(|(n, _)| n == name).map(|(_, g)| *g));
        if let Some(g) = g {
            self.eq.gains = g;
            self.eq.preset = name.to_string();
            self.eq.auto = false;
            self.eq.on = true;
            self.apply_eq();
        }
    }

    pub fn eq_set_auto(&mut self, auto: bool) {
        self.eq.auto = auto;
        if auto {
            self.eq.on = true;
            self.eq_pick_for_current();
        }
        self.apply_eq();
    }

    pub fn eq_set_on(&mut self, on: bool) {
        self.eq.on = on;
        self.apply_eq();
    }

    pub fn eq_set_preamp(&mut self, db: f32) {
        self.eq.preamp = ((db * 2.0).round() / 2.0).clamp(-12.0, 12.0);
        self.apply_eq();
    }

    pub fn eq_save_custom(&mut self, name: &str) {
        let name = name.trim().replace(['|', '\n'], " ");
        let name = if name.is_empty() { format!("My preset {}", self.eq.custom.len() + 1) } else { name };
        let g = self.eq.effective();
        self.eq.custom.retain(|(n, _)| *n != name);
        self.eq.custom.push((name.clone(), g));
        self.eq.gains = g;
        self.eq.auto = false;
        self.eq.preset = name;
        self.refresh_custom();
        self.apply_eq();
    }

    pub fn eq_delete_custom(&mut self, i: usize) {
        if i < self.eq.custom.len() {
            self.eq.custom.remove(i);
            self.refresh_custom();
            self.eq.preset = self.eq.name_for(&self.eq.gains);
            self.apply_eq();
        }
    }

    fn eq_pick_for_current(&mut self) {
        let Some(item) = self.index.and_then(|i| self.items.get(i)) else { return };
        let tags = item.tags.clone().unwrap_or_default();
        self.eq.auto_choice = choose(&item.path, &tags);
    }

    /// A new song: Auto picks again.
    pub fn eq_track_changed(&mut self) {
        if self.eq.auto {
            let before = self.eq.auto_choice;
            self.eq_pick_for_current();
            if before != self.eq.auto_choice {
                self.apply_eq();
            } else {
                self.ui().set_eq_auto_label(format!("AUTO · {}", self.eq.auto_choice.to_uppercase()).into());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tags(genre: &str, channels: u8, bitrate: u32, title: &str) -> Tags {
        Tags { genre: genre.into(), channels, bitrate, title: title.into(), ..Default::default() }
    }

    #[test]
    fn auto_picks() {
        let p = Path::new("/m/a.mp3");
        assert_eq!(choose(p, &tags("Hip Hop", 2, 320, "x")), "Hip-Hop");
        assert_eq!(choose(p, &tags("Deep House", 2, 320, "x")), "Deep House");
        assert_eq!(choose(p, &tags("", 1, 48, "x")), "Podcast");
        assert_eq!(choose(Path::new("/t/جلسه دوم سمینار.m4a"), &tags("", 2, 128, "")), "Podcast");
        assert_eq!(choose(p, &tags("", 2, 320, "Mini mix 9")), "EDM");
        assert_eq!(choose(p, &tags("", 2, 320, "Negaram")), "Pop");
        assert_eq!(choose(Path::new("/v/a.mkv"), &tags("Rock", 2, 0, "")), "Flat");
    }

    #[test]
    fn settings_round_trip() {
        let mut e = Eq { on: true, auto: true, preamp: -2.5, preset: "Rock".into(), ..Default::default() };
        e.gains[3] = 4.0;
        let mut back = Eq::default();
        back.decode(&e.encode());
        assert!(back.on && back.auto);
        assert_eq!(back.preamp, -2.5);
        assert_eq!(back.gains[3], 4.0);
        let mut old = Eq::default();
        old.decode("1|Bass|6,5,4,2,0.5,0,0,0,0,0");
        assert!(old.on && old.gains[0] == 6.0);
    }

    #[test]
    fn curve_has_ten_segments_plus_edges() {
        let (line, fill) = curve(&[0.0; 10]);
        assert_eq!(line.matches(" C ").count(), 11);
        assert!(fill.ends_with('Z'));
    }
}
