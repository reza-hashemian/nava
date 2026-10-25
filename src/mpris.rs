//! MPRIS on Linux: media keys, the GNOME/KDE "now playing" widget and
//! `playerctl` all talk to Nava through this D-Bus service.
//!
//! zbus only wakes up when a message arrives, so this costs nothing while idle.

use std::collections::HashMap;
use std::time::Instant;

use zbus::blocking::Connection;
use zbus::blocking::object_server::InterfaceRef;
use zbus::interface;
use zbus::object_server::SignalEmitter;
use zbus::zvariant::{ObjectPath, OwnedValue, Value};

use crate::ipc::Cmd;
use crate::system::to_ui;

const PATH: &str = "/org/mpris/MediaPlayer2";

struct Root {
    run: fn(Cmd),
}

#[interface(name = "org.mpris.MediaPlayer2")]
impl Root {
    fn raise(&self) {
        to_ui(self.run, Cmd::Show);
    }
    fn quit(&self) {}
    #[zbus(property)]
    fn can_quit(&self) -> bool {
        false
    }
    #[zbus(property)]
    fn can_raise(&self) -> bool {
        true
    }
    #[zbus(property)]
    fn has_track_list(&self) -> bool {
        false
    }
    #[zbus(property)]
    fn identity(&self) -> &str {
        "Nava"
    }
    #[zbus(property)]
    fn desktop_entry(&self) -> &str {
        "nava"
    }
    #[zbus(property)]
    fn supported_uri_schemes(&self) -> Vec<String> {
        Vec::new()
    }
    #[zbus(property)]
    fn supported_mime_types(&self) -> Vec<String> {
        Vec::new()
    }
}

#[derive(Default)]
struct Track {
    title: String,
    artist: String,
    album: String,
    duration: f64,
    art: Option<String>,
}

struct Player {
    run: fn(Cmd),
    track: Option<Track>,
    playing: bool,
    /// Position at `since`; while playing, it moves on with the clock.
    position: f64,
    since: Instant,
}

impl Player {
    fn now(&self) -> f64 {
        if self.playing { self.position + self.since.elapsed().as_secs_f64() } else { self.position }
    }
}

fn owned(v: Value<'_>) -> OwnedValue {
    v.try_to_owned().expect("plain values always convert")
}

#[interface(name = "org.mpris.MediaPlayer2.Player")]
impl Player {
    fn play_pause(&self) {
        to_ui(self.run, Cmd::Toggle);
    }
    fn play(&self) {
        to_ui(self.run, Cmd::Play);
    }
    fn pause(&self) {
        to_ui(self.run, Cmd::Pause);
    }
    fn stop(&self) {
        to_ui(self.run, Cmd::Pause);
    }
    fn next(&self) {
        to_ui(self.run, Cmd::Next);
    }
    fn previous(&self) {
        to_ui(self.run, Cmd::Prev);
    }
    fn seek(&self, offset_us: i64) {
        to_ui(self.run, Cmd::SeekBy(offset_us as f64 / 1e6));
    }
    fn set_position(&self, _track: ObjectPath<'_>, position_us: i64) {
        to_ui(self.run, Cmd::SeekBy(position_us as f64 / 1e6 - self.now()));
    }
    fn open_uri(&self, _uri: &str) {}

    #[zbus(signal)]
    async fn seeked(emitter: &SignalEmitter<'_>, position: i64) -> zbus::Result<()>;

    #[zbus(property)]
    fn playback_status(&self) -> &str {
        match (&self.track, self.playing) {
            (None, _) => "Stopped",
            (Some(_), true) => "Playing",
            (Some(_), false) => "Paused",
        }
    }
    #[zbus(property(emits_changed_signal = "false"))]
    fn position(&self) -> i64 {
        (self.now() * 1e6) as i64
    }
    #[zbus(property)]
    fn metadata(&self) -> HashMap<String, OwnedValue> {
        let mut m = HashMap::new();
        let Some(t) = &self.track else { return m };
        let id = ObjectPath::try_from("/org/mpris/MediaPlayer2/Track/current").expect("valid path");
        m.insert("mpris:trackid".into(), owned(Value::from(id)));
        m.insert("xesam:title".into(), owned(Value::from(t.title.as_str())));
        if !t.artist.is_empty() {
            m.insert("xesam:artist".into(), owned(Value::from(vec![t.artist.as_str()])));
        }
        if !t.album.is_empty() {
            m.insert("xesam:album".into(), owned(Value::from(t.album.as_str())));
        }
        if t.duration > 0.0 {
            m.insert("mpris:length".into(), owned(Value::from((t.duration * 1e6) as i64)));
        }
        if let Some(a) = &t.art {
            m.insert("mpris:artUrl".into(), owned(Value::from(a.as_str())));
        }
        m
    }
    #[zbus(property)]
    fn rate(&self) -> f64 {
        1.0
    }
    #[zbus(property)]
    fn minimum_rate(&self) -> f64 {
        1.0
    }
    #[zbus(property)]
    fn maximum_rate(&self) -> f64 {
        1.0
    }
    #[zbus(property)]
    fn can_go_next(&self) -> bool {
        self.track.is_some()
    }
    #[zbus(property)]
    fn can_go_previous(&self) -> bool {
        self.track.is_some()
    }
    #[zbus(property)]
    fn can_play(&self) -> bool {
        self.track.is_some()
    }
    #[zbus(property)]
    fn can_pause(&self) -> bool {
        self.track.is_some()
    }
    #[zbus(property)]
    fn can_seek(&self) -> bool {
        self.track.is_some()
    }
    #[zbus(property)]
    fn can_control(&self) -> bool {
        true
    }
}

pub struct Mpris {
    _conn: Connection,
    player: InterfaceRef<Player>,
}

impl Mpris {
    pub fn new(run: fn(Cmd)) -> Option<Mpris> {
        let built = (|| -> zbus::Result<(Connection, InterfaceRef<Player>)> {
            let player = Player { run, track: None, playing: false, position: 0.0, since: Instant::now() };
            let conn = zbus::blocking::connection::Builder::session()?
                .name("org.mpris.MediaPlayer2.nava")?
                .serve_at(PATH, Root { run })?
                .serve_at(PATH, player)?
                .build()?;
            let iface = conn.object_server().interface::<_, Player>(PATH)?;
            Ok((conn, iface))
        })();
        match built {
            Ok((conn, player)) => Some(Mpris { _conn: conn, player }),
            Err(e) => {
                eprintln!("nava: media keys unavailable: {e}");
                None
            }
        }
    }

    pub fn set_track(&mut self, title: &str, artist: &str, album: &str, duration: f64, art: Option<String>) {
        let mut p = self.player.get_mut();
        p.track = Some(Track { title: title.into(), artist: artist.into(), album: album.into(), duration, art });
        let e = self.player.signal_emitter();
        let _ = zbus::block_on(async {
            p.metadata_changed(e).await?;
            p.can_play_changed(e).await?;
            p.playback_status_changed(e).await
        });
    }

    pub fn set_state(&mut self, playing: bool, position: f64, has_track: bool) {
        let mut p = self.player.get_mut();
        let jumped = (p.now() - position).abs() > 1.5;
        p.playing = playing;
        p.position = position;
        p.since = Instant::now();
        if !has_track {
            p.track = None;
        }
        let e = self.player.signal_emitter();
        let _ = zbus::block_on(async {
            p.playback_status_changed(e).await?;
            if jumped {
                Player::seeked(e, (position * 1e6) as i64).await?;
            }
            Ok::<(), zbus::Error>(())
        });
    }
}
