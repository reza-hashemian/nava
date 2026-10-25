//! Small listening tools: sleep timer, playback speed (remembered per folder),
//! A-B loop, and bookmarks with notes that export to Markdown.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use slint::{ModelRc, SharedString, TimerMode, VecModel};

use crate::{Bookmark, Player, TrackOption, fmt_time, with};

const SPEEDS: [f64; 8] = [0.5, 0.75, 1.0, 1.1, 1.25, 1.5, 1.75, 2.0];
const FADE: f64 = 30.0;

#[derive(Clone, Copy, Debug)]
pub enum Sleep {
    At(Instant),
    EndOfTrack,
}

/// The folder a speed is remembered for.
fn speed_key(path: &Path) -> String {
    format!("speed:{}", path.parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default())
}

/// Where exported notes go: next to the video, or in Documents if that's not writable.
fn notes_path(video: &Path) -> PathBuf {
    let stem = video.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "video".into());
    let beside = video.with_file_name(format!("{stem}.notes.md"));
    let writable = video.parent().is_some_and(|d| {
        std::fs::metadata(d).map(|m| !m.permissions().readonly()).unwrap_or(false)
    });
    if writable {
        beside
    } else {
        dirs::document_dir().unwrap_or_else(std::env::temp_dir).join("Nava Notes").join(format!("{stem}.notes.md"))
    }
}

/// Notes as Markdown. Times link back to the file, which most players understand.
pub fn notes_markdown(title: &str, video: &Path, marks: &[(i64, f64, String)]) -> String {
    let mut md = format!("# {title}\n\n");
    for (_, pos, note) in marks {
        let note = if note.trim().is_empty() { "—" } else { note.trim() };
        md.push_str(&format!("- **{}** {}\n", fmt_time(*pos), note));
    }
    md.push_str(&format!("\n_From `{}`, saved by Nava._\n", video.display()));
    md
}

impl Player {
    // ---------- sleep timer ----------

    pub fn sleep_label(&self) -> String {
        match self.sleep {
            Some(Sleep::At(t)) => {
                let mins = (t.saturating_duration_since(Instant::now()).as_secs_f64() / 60.0).ceil() as u64;
                if mins >= 60 { format!("{} hr {} min", mins / 60, mins % 60) } else { format!("{mins} min") }
            }
            Some(Sleep::EndOfTrack) => "end of track".into(),
            None => String::new(),
        }
    }

    /// "15", "30", "45", "60" minutes, "end" or "off".
    pub fn set_sleep(&mut self, arg: &str) {
        self.sleep_timer.stop();
        self.sleep_label_timer.stop();
        self.sleep = match arg.trim() {
            "end" => Some(Sleep::EndOfTrack),
            "off" | "" => None,
            m => m.parse::<f64>().ok().filter(|m| *m > 0.0).map(|m| Sleep::At(Instant::now() + Duration::from_secs_f64(m * 60.0))),
        };
        if let Some(Sleep::At(at)) = self.sleep {
            // Start fading 30 s before the end.
            let wait = at.saturating_duration_since(Instant::now()).saturating_sub(Duration::from_secs_f64(FADE));
            self.sleep_timer.start(TimerMode::SingleShot, wait, || {
                with(|p| p.sleep_fade_out());
            });
            self.sleep_label_timer.start(TimerMode::Repeated, Duration::from_secs(30), || {
                with(|p| p.refresh_tool_menus());
            });
        }
        self.refresh_tool_menus();
    }

    fn sleep_fade_out(&mut self) {
        let start = Instant::now();
        let from = self.volume;
        self.sleep_timer.start(TimerMode::Repeated, Duration::from_millis(200), move || {
            with(|p| {
                let k = (start.elapsed().as_secs_f64() / FADE).min(1.0);
                p.mpv.set_double("volume", from * (1.0 - k));
                if k >= 1.0 {
                    p.sleep_timer.stop();
                    if p.playing {
                        p.play_pause();
                    }
                    // Back to the normal volume for next time.
                    p.mpv.set_double("volume", p.volume);
                    p.set_sleep("off");
                    p.save();
                }
            });
        });
    }

    // ---------- speed ----------

    pub fn set_speed(&mut self, v: f64) {
        self.speed = v.clamp(0.25, 4.0);
        self.mpv.set_double("speed", self.speed);
        if let Some(path) = &self.current_path {
            self.store.set_setting(&speed_key(path), &self.speed.to_string());
        }
        self.refresh_tool_menus();
    }

    // ---------- A-B loop ----------

    /// First press marks A, second marks B and loops, third clears.
    pub fn ab_loop(&mut self) {
        if self.current_path.is_none() {
            return;
        }
        // The scrubber's position can be a second old; ask mpv for the exact time.
        let now = if self.loaded { self.mpv.get_double("time-pos").unwrap_or(self.position) } else { self.position };
        let (a, b) = self.ab;
        self.ab = if a < 0.0 {
            (now, -1.0)
        } else if b < 0.0 && now > a + 0.5 {
            (a, now)
        } else {
            (-1.0, -1.0)
        };
        self.apply_ab();
        // Once both ends are set, start the loop right away.
        if self.ab.1 >= 0.0 {
            self.seek(self.ab.0);
        }
    }

    fn apply_ab(&mut self) {
        let (a, b) = self.ab;
        self.mpv.set_string("ab-loop-a", &if a >= 0.0 { format!("{a:.3}") } else { "no".into() });
        self.mpv.set_string("ab-loop-b", &if b >= 0.0 { format!("{b:.3}") } else { "no".into() });
        let ui = self.ui();
        ui.set_ab_a(a as f32);
        ui.set_ab_b(b as f32);
    }

    // ---------- bookmarks ----------

    pub fn add_bookmark(&mut self, note: &str) {
        let Some(path) = self.current_path.clone() else { return };
        if self.playing && self.loaded {
            if let Some(t) = self.mpv.get_double("time-pos") {
                self.position = t;
            }
        }
        self.store.add_bookmark(&path, self.position, note.trim());
        self.refresh_bookmarks();
    }

    pub fn seek_bookmark(&mut self, i: usize) {
        if let Some(&(_, pos)) = self.bookmark_ids.get(i) {
            self.seek(pos);
        }
    }

    pub fn delete_bookmark(&mut self, i: usize) {
        if let Some(&(id, _)) = self.bookmark_ids.get(i) {
            self.store.delete_bookmark(id);
            self.refresh_bookmarks();
        }
    }

    pub fn refresh_bookmarks(&mut self) {
        let marks = self.current_path.as_ref().map(|p| self.store.bookmarks(p)).unwrap_or_default();
        self.bookmark_ids = marks.iter().map(|(id, pos, _)| (*id, *pos)).collect();
        let rows: Vec<Bookmark> =
            marks.iter().map(|(_, pos, note)| Bookmark { time: fmt_time(*pos).into(), note: note.clone().into() }).collect();
        let ticks: Vec<f32> = if self.duration > 0.0 {
            marks.iter().map(|(_, pos, _)| (*pos / self.duration) as f32).collect()
        } else {
            Vec::new()
        };
        let ui = self.ui();
        ui.set_bookmarks(ModelRc::new(VecModel::from(rows)));
        ui.set_bookmark_positions(ModelRc::new(VecModel::from(ticks)));
    }

    pub fn export_notes(&mut self) {
        let Some(path) = self.current_path.clone() else { return };
        let marks = self.store.bookmarks(&path);
        let title = self.ui().get_track_title().to_string();
        let out = notes_path(&path);
        let saved = out
            .parent()
            .map(|d| std::fs::create_dir_all(d).is_ok())
            .unwrap_or(true)
            .then(|| std::fs::write(&out, notes_markdown(&title, &path, &marks)).ok())
            .flatten();
        self.show_note(match saved {
            Some(()) => format!("Notes saved to {}", out.display()),
            None => "Couldn't save the notes".into(),
        });
    }

    // ---------- shared ----------

    /// A new file: its folder's speed, no loop, its bookmarks.
    pub fn track_tools_changed(&mut self, path: &Path) {
        self.speed = self.store.setting(&speed_key(path)).and_then(|v| v.parse().ok()).unwrap_or(1.0);
        self.mpv.set_double("speed", self.speed);
        self.ab = (-1.0, -1.0);
        self.apply_ab();
        self.refresh_bookmarks();
        self.refresh_tool_menus();
    }

    /// Rebuild the speed and sleep menus and the sleep label.
    pub fn refresh_tool_menus(&mut self) {
        let speeds: Vec<TrackOption> = SPEEDS
            .iter()
            .map(|s| TrackOption {
                id: s.to_string().into(),
                label: if *s == 1.0 { "Normal".into() } else { format!("{s}×").into() },
                selected: (s - self.speed).abs() < 0.01,
            })
            .collect();
        let current = match self.sleep {
            Some(Sleep::EndOfTrack) => "end",
            Some(Sleep::At(_)) => "on",
            None => "off",
        };
        let sleeps: Vec<TrackOption> = [("off", "Off"), ("15", "15 minutes"), ("30", "30 minutes"), ("45", "45 minutes"), ("60", "1 hour"), ("end", "End of this track")]
            .iter()
            .map(|(id, label)| TrackOption {
                id: (*id).into(),
                label: if *id == "off" && current == "on" { format!("Off  ·  {} left", self.sleep_label()).into() } else { (*label).into() },
                selected: *id == current,
            })
            .collect();
        let ui = self.ui();
        ui.set_speed(self.speed as f32);
        ui.set_speed_options(ModelRc::new(VecModel::from(speeds)));
        ui.set_sleep_options(ModelRc::new(VecModel::from(sleeps)));
        ui.set_sleep_label(SharedString::from(self.sleep_label()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_lists_notes_in_order() {
        let md = notes_markdown("Rust 101", Path::new("/c/01.mp4"), &[(1, 75.0, "closures".into()), (2, 600.0, "".into())]);
        assert!(md.starts_with("# Rust 101"));
        assert!(md.contains("- **1:15** closures"));
        assert!(md.contains("- **10:00** —"));
    }
}
