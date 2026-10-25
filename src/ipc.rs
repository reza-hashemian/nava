//! One Nava at a time, controllable from the terminal.
//!
//! The running instance listens on a local socket (a Unix socket, or a
//! loopback TCP port on Windows). Each connection sends one line and gets one
//! line back. A second `nava` launch hands its request to the running one and
//! exits.

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Duration;

/// A request to the running instance.
#[derive(Debug, Clone, PartialEq)]
pub enum Cmd {
    Toggle,
    Play,
    Pause,
    Next,
    Prev,
    /// Bring the window forward.
    Show,
    /// Switch to the Code session, go mini and play.
    Code,
    Mini,
    Full,
    ToggleMini,
    SeekBy(f64),
    Status(String),
    /// The screen locked (true) or unlocked (false).
    ScreenLocked(bool),
    /// "15", "30", ... minutes, "end" for end of track, "off".
    Sleep(String),
    Speed(f64),
    /// Bookmark the current moment with a note.
    Mark(String),
    Load { paths: Vec<PathBuf>, limit: Option<usize>, pick: String, recursive: bool },
}

impl Cmd {
    /// Wire format: words separated by tabs, one command per line.
    pub fn encode(&self) -> String {
        match self {
            Cmd::Toggle => "toggle".into(),
            Cmd::Play => "play".into(),
            Cmd::Pause => "pause".into(),
            Cmd::Next => "next".into(),
            Cmd::Prev => "prev".into(),
            Cmd::Show => "show".into(),
            Cmd::Code => "code".into(),
            Cmd::Mini => "mini".into(),
            Cmd::Full => "full".into(),
            Cmd::ToggleMini => "toggle-mini".into(),
            Cmd::SeekBy(s) => format!("seek\t{s}"),
            Cmd::Status(f) => format!("status\t{f}"),
            Cmd::ScreenLocked(l) => format!("locked\t{}", *l as u8),
            Cmd::Sleep(s) => format!("sleep\t{s}"),
            Cmd::Speed(s) => format!("speed\t{s}"),
            Cmd::Mark(n) => format!("mark\t{}", n.replace(['\t', '\n'], " ")),
            Cmd::Load { paths, limit, pick, recursive } => {
                let mut s = format!(
                    "load\t{}\t{pick}\t{}",
                    limit.map(|n| n.to_string()).unwrap_or_default(),
                    *recursive as u8
                );
                for p in paths {
                    s.push('\t');
                    s.push_str(&p.to_string_lossy());
                }
                s
            }
        }
    }

    pub fn decode(line: &str) -> Option<Cmd> {
        let mut parts = line.trim_end_matches(['\r', '\n']).split('\t');
        Some(match parts.next()? {
            "toggle" => Cmd::Toggle,
            "play" => Cmd::Play,
            "pause" => Cmd::Pause,
            "next" => Cmd::Next,
            "prev" => Cmd::Prev,
            "show" => Cmd::Show,
            "code" => Cmd::Code,
            "mini" => Cmd::Mini,
            "full" => Cmd::Full,
            "toggle-mini" => Cmd::ToggleMini,
            "seek" => Cmd::SeekBy(parts.next()?.parse().ok()?),
            "status" => Cmd::Status(parts.next().unwrap_or("").to_string()),
            "locked" => Cmd::ScreenLocked(parts.next()? == "1"),
            "sleep" => Cmd::Sleep(parts.next()?.to_string()),
            "speed" => Cmd::Speed(parts.next()?.parse().ok()?),
            "mark" => Cmd::Mark(parts.next().unwrap_or("").to_string()),
            "load" => {
                let limit = parts.next()?.parse().ok();
                let pick = parts.next()?.to_string();
                let recursive = parts.next()? == "1";
                let paths = parts.filter(|p| !p.is_empty()).map(PathBuf::from).collect();
                Cmd::Load { paths, limit, pick, recursive }
            }
            _ => return None,
        })
    }
}

#[cfg(unix)]
mod transport {
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::PathBuf;

    pub type Stream = UnixStream;
    pub type Listener = UnixListener;

    fn path() -> PathBuf {
        if let Some(p) = std::env::var_os("NAVA_SOCKET") {
            return p.into();
        }
        let dir = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
        let user = std::env::var("USER").unwrap_or_default();
        dir.join(format!("nava-{user}.sock"))
    }

    pub fn connect() -> std::io::Result<Stream> {
        UnixStream::connect(path())
    }

    pub fn listen() -> std::io::Result<Listener> {
        let p = path();
        // Only called after connect() failed, so a leftover file is stale.
        let _ = std::fs::remove_file(&p);
        UnixListener::bind(p)
    }

    pub fn cleanup() {
        let _ = std::fs::remove_file(path());
    }
}

#[cfg(not(unix))]
mod transport {
    use std::net::{Ipv4Addr, TcpListener, TcpStream};

    pub type Stream = TcpStream;
    pub type Listener = TcpListener;
    const PORT: u16 = 47_823;

    pub fn connect() -> std::io::Result<Stream> {
        TcpStream::connect_timeout(&(Ipv4Addr::LOCALHOST, PORT).into(), std::time::Duration::from_millis(300))
    }

    pub fn listen() -> std::io::Result<Listener> {
        TcpListener::bind((Ipv4Addr::LOCALHOST, PORT))
    }

    pub fn cleanup() {}
}

/// Send a command to the running instance. `Err` means none is running.
pub fn send(cmd: &Cmd) -> std::io::Result<String> {
    let mut s = transport::connect()?;
    s.set_read_timeout(Some(Duration::from_secs(3)))?;
    writeln!(s, "{}", cmd.encode())?;
    let mut reply = String::new();
    BufReader::new(s).read_line(&mut reply)?;
    Ok(reply.trim_end().to_string())
}

/// Start serving. `handle` runs on the UI thread and returns the reply line.
pub fn serve(handle: fn(Cmd) -> String) -> std::io::Result<()> {
    let listener = transport::listen()?;
    std::thread::spawn(move || {
        for conn in listener.incoming().flatten() {
            let _ = conn.set_read_timeout(Some(Duration::from_secs(2)));
            let mut line = String::new();
            let mut reader = BufReader::new(&conn);
            if reader.read_line(&mut line).is_err() {
                continue;
            }
            let reply = match Cmd::decode(&line) {
                Some(cmd) => {
                    let (tx, rx) = mpsc::channel();
                    let queued = slint::invoke_from_event_loop(move || {
                        let _ = tx.send(handle(cmd));
                    });
                    match queued {
                        Ok(()) => rx.recv_timeout(Duration::from_secs(2)).unwrap_or_default(),
                        Err(_) => break,
                    }
                }
                None => "error: unknown command".to_string(),
            };
            let mut w = &conn;
            let _ = writeln!(w, "{reply}");
        }
    });
    Ok(())
}

pub fn cleanup() {
    transport::cleanup();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        let cmds = [
            Cmd::Toggle,
            Cmd::Code,
            Cmd::SeekBy(-5.0),
            Cmd::Status("{title} · {pos}".into()),
            Cmd::Sleep("30".into()),
            Cmd::Speed(1.5),
            Cmd::Mark("closures are neat".into()),
            Cmd::Load {
                paths: vec!["/a/b c.mp3".into(), "/موزیک".into()],
                limit: Some(20),
                pick: "random".into(),
                recursive: true,
            },
            Cmd::Load { paths: vec![], limit: None, pick: "first".into(), recursive: false },
        ];
        for c in cmds {
            assert_eq!(Cmd::decode(&c.encode()), Some(c));
        }
    }
}
