//! Updates and the occasional "come say hi" card.
//!
//! The update check asks GitHub for the latest release, at most once a day and
//! quietly; it only lights a dot on the update button. The community card
//! shows up rarely and backs off for good once it has been answered.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use slint::{ComponentHandle, Timer};
use slint::winit_030::WinitWindowAccessor;

use crate::{Player, with};

pub const GITHUB_URL: &str = "https://github.com/reza-hashemian/nava";
pub const RELEASES_URL: &str = "https://github.com/reza-hashemian/nava/releases/latest";
pub const TELEGRAM_URL: &str = "https://t.me/RezaUplink";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

const LATEST_API: &str = "https://api.github.com/repos/reza-hashemian/nava/releases/latest";
const DAY: u64 = 24 * 60 * 60;

pub fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Open a web page in the default browser.
pub fn open_url(url: &str) {
    #[cfg(target_os = "macos")]
    let r = std::process::Command::new("open").arg(url).spawn();
    #[cfg(target_os = "windows")]
    let r = {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        std::process::Command::new("cmd").args(["/C", "start", "", url]).creation_flags(CREATE_NO_WINDOW).spawn()
    };
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let r = std::process::Command::new("xdg-open").arg(url).spawn();
    if let Err(e) = r {
        eprintln!("nava: can't open {url}: {e}");
    }
}

/// The newest released version on GitHub, like "0.1.6". Blocks; run it off the UI thread.
pub fn latest_version() -> Option<String> {
    let mut cmd = std::process::Command::new("curl");
    cmd.args([
        "-sL",
        "--max-time",
        "8",
        "-H",
        "Accept: application/vnd.github+json",
        "-H",
        &format!("User-Agent: Nava/{VERSION}"),
        LATEST_API,
    ]);
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    let out = cmd.output().ok()?;
    if !out.status.success() {
        return None;
    }
    let body = String::from_utf8_lossy(&out.stdout);
    let tag = json_string(&body, "tag_name")?;
    let v = tag.trim().trim_start_matches(['v', 'V']).to_string();
    parse(&v).map(|_| v)
}

fn parse(v: &str) -> Option<Vec<u64>> {
    // "1.2.3-beta" → [1, 2, 3]; anything else isn't a version.
    let core = v.split(['-', '+']).next()?;
    let parts: Option<Vec<u64>> = core.split('.').map(|p| p.parse().ok()).collect();
    parts.filter(|p| !p.is_empty())
}

/// Whether `latest` is a newer version than `current`.
pub fn is_newer(latest: &str, current: &str) -> bool {
    match (parse(latest), parse(current)) {
        (Some(mut a), Some(mut b)) => {
            let n = a.len().max(b.len());
            a.resize(n, 0);
            b.resize(n, 0);
            a > b
        }
        _ => false,
    }
}

/// Whether it's time for the daily update check.
pub fn update_check_due(last_check: u64, now: u64) -> bool {
    now.saturating_sub(last_check) >= DAY
}

/// What we remember about the community card.
#[derive(Default, Debug, Clone, Copy, PartialEq)]
pub struct Nudge {
    pub launches: u64,
    pub first_run: u64,
    pub last_shown: u64,
    pub times_shown: u64,
    /// Turned off for good: "Don't show again", or they already followed a link.
    pub off: bool,
}

impl Nudge {
    /// Polite by design: never in the first days or first launches, then at most
    /// once every three weeks, and only a few times ever.
    pub fn due(&self, now: u64) -> bool {
        !self.off
            && self.launches >= 5
            && now.saturating_sub(self.first_run) >= 3 * DAY
            && now.saturating_sub(self.last_shown) >= 21 * DAY
            && self.times_shown < 4
    }
}

impl Nudge {
    fn load(text: &str) -> Nudge {
        let v: Vec<u64> = text.split(',').map(|x| x.trim().parse().unwrap_or(0)).collect();
        let at = |i: usize| v.get(i).copied().unwrap_or(0);
        Nudge { launches: at(0), first_run: at(1), last_shown: at(2), times_shown: at(3), off: at(4) == 1 }
    }

    fn encode(&self) -> String {
        format!("{},{},{},{},{}", self.launches, self.first_run, self.last_shown, self.times_shown, self.off as u8)
    }
}

impl Player {
    fn nudge(&self) -> Nudge {
        Nudge::load(&self.store.setting("community").unwrap_or_default())
    }

    fn save_nudge(&self, n: &Nudge) {
        self.store.set_setting("community", &n.encode());
    }

    /// At startup: count the launch, light the update dot from the last check,
    /// and schedule the quiet daily check and (maybe) the community card.
    pub(crate) fn start_community(&mut self, relaunched: bool) {
        let ui = self.ui();
        ui.set_app_version(VERSION.into());
        let latest = self.store.setting("update_latest").unwrap_or_default();
        if is_newer(&latest, VERSION) {
            ui.set_update_latest(latest.into());
            ui.set_update_status("available".into());
        }

        let now = now();
        let mut n = self.nudge();
        if !relaunched {
            n.launches += 1;
        }
        if n.first_run == 0 {
            n.first_run = now;
        }
        self.save_nudge(&n);

        let last_check = self.store.setting("update_checked").and_then(|v| v.parse().ok()).unwrap_or(0);
        if update_check_due(last_check, now) {
            Timer::single_shot(Duration::from_secs(20), || {
                with(|p| p.check_updates(false));
            });
        }
        if n.due(now) {
            // Not at startup: let them settle in first.
            Timer::single_shot(Duration::from_secs(150), || {
                with(|p| p.offer_community(0));
            });
        }
    }

    /// Ask GitHub for the newest version. `manual` = they pressed the button.
    pub(crate) fn check_updates(&mut self, manual: bool) {
        if manual {
            self.ui().set_update_status("checking".into());
        }
        std::thread::spawn(move || {
            let latest = latest_version();
            let _ = slint::invoke_from_event_loop(move || {
                with(|p| p.apply_update(latest, manual));
            });
        });
    }

    fn apply_update(&mut self, latest: Option<String>, manual: bool) {
        let ui = self.ui();
        match latest {
            Some(v) => {
                self.store.set_setting("update_checked", &now().to_string());
                self.store.set_setting("update_latest", &v);
                if is_newer(&v, VERSION) {
                    ui.set_update_latest(v.into());
                    ui.set_update_status("available".into());
                } else if manual {
                    ui.set_update_status("latest".into());
                } else {
                    ui.set_update_status("".into());
                }
            }
            // A quiet check that fails says nothing; it tries again next launch.
            None if manual => ui.set_update_status("failed".into()),
            None => {}
        }
    }

    /// Show the community card, but only when it won't get in the way:
    /// not over a video, a sheet, the mini player or a hidden window.
    fn offer_community(&mut self, tries: u32) {
        let ui = self.ui();
        let fullscreen = ui.window().with_winit_window(|w| w.fullscreen().is_some()).unwrap_or(false);
        let visible = ui.window().with_winit_window(|w| w.is_visible().unwrap_or(true)).unwrap_or(true);
        let busy = self.mini
            || fullscreen
            || !visible
            || (self.is_video && self.playing)
            || ui.get_show_settings()
            || ui.get_show_open_sheet()
            || ui.get_show_equalizer()
            || ui.get_show_link_sheet()
            || ui.get_show_delete_sheet()
            || ui.get_show_update_sheet();
        if busy {
            if tries < 6 {
                Timer::single_shot(Duration::from_secs(300), move || {
                    with(|p| p.offer_community(tries + 1));
                });
            }
            return;
        }
        let mut n = self.nudge();
        n.last_shown = now();
        n.times_shown += 1;
        self.save_nudge(&n);
        ui.set_show_community(true);
        // It leaves on its own if they don't answer.
        Timer::single_shot(Duration::from_secs(45), || {
            with(|p| {
                if p.ui().get_show_community() {
                    p.community_closed("later");
                }
            });
        });
    }

    pub(crate) fn community_closed(&mut self, how: &str) {
        self.ui().set_show_community(false);
        if how == "never" || how == "followed" {
            let mut n = self.nudge();
            n.off = true;
            self.save_nudge(&n);
        }
    }
}

/// Pull one string field out of a JSON object, without a JSON library.
fn json_string(json: &str, key: &str) -> Option<String> {
    let start = json.find(&format!("\"{key}\""))? + key.len() + 2;
    let rest = json[start..].trim_start().strip_prefix(':')?.trim_start();
    let rest = rest.strip_prefix('"')?;
    Some(rest[..rest.find('"')?].to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        assert!(is_newer("0.1.6", "0.1.5"));
        assert!(is_newer("0.2", "0.1.9"));
        assert!(is_newer("1.0.0", "0.99.99"));
        assert!(!is_newer("0.1.5", "0.1.5"));
        assert!(!is_newer("0.1.5", "0.1.10"));
        assert!(!is_newer("garbage", "0.1.5"));
        assert!(!is_newer("0.1.5-beta", "0.1.5"));
    }

    #[test]
    fn tag_from_json() {
        let body = r#"{"url":"x","tag_name": "v0.1.6","name":"Nava 0.1.6"}"#;
        assert_eq!(json_string(body, "tag_name").as_deref(), Some("v0.1.6"));
    }

    #[test]
    fn nudge_round_trip() {
        let n = Nudge { launches: 7, first_run: 11, last_shown: 22, times_shown: 2, off: true };
        assert_eq!(Nudge::load(&n.encode()), n);
        assert_eq!(Nudge::load(""), Nudge::default());
    }

    #[test]
    fn nudge_is_rare() {
        let t = 100 * DAY;
        let fresh = Nudge { launches: 1, first_run: t, ..Default::default() };
        assert!(!fresh.due(t));
        let settled = Nudge { launches: 6, first_run: t - 4 * DAY, ..Default::default() };
        assert!(settled.due(t));
        assert!(!Nudge { last_shown: t - 10 * DAY, ..settled }.due(t));
        assert!(Nudge { last_shown: t - 22 * DAY, times_shown: 1, ..settled }.due(t));
        assert!(!Nudge { times_shown: 4, ..settled }.due(t));
        assert!(!Nudge { off: true, ..settled }.due(t));
        assert!(!Nudge { launches: 3, ..settled }.due(t));
    }
}
