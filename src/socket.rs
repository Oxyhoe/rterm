//! Local UNIX socket used by `rterm play` to stream video straight into the
//! terminal, bypassing the PTY (base64 through a pipe is ~30% slower and
//! wastes CPU on both ends).
//!
//! Framing: every message is `[u32 le length][payload]`; the first payload
//! byte is a tag — `0` for a text line, `1` for a raw RGBA frame
//! (`[1][u32 le id][rgba…]`).

use std::io::{self, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::mpsc::{channel, Sender};
use std::time::Duration;

use anyhow::{Context, Result};

use crate::app::EventLoopProxy;

/// Environment variable pointing children at this instance's socket.
pub const ENV: &str = "RTERM_SOCK";

/// Largest accepted message (4K RGBA frames fit comfortably).
const MAX_MSG: usize = 64 * 1024 * 1024;

/// Requests travelling from a connection thread to the event loop.
pub enum Msg {
    /// Client introduced itself; `reply` receives a `SIZE …` line.
    Hello { tty: String, reply: Sender<String> },
    /// Create/refresh an overlay at `col,row` sized `cols×rows` cells.
    Open {
        sid: usize,
        id: u64,
        col: usize,
        row: usize,
        cols: usize,
        rows: usize,
        w: u32,
        h: u32,
    },
    /// One decoded frame for overlay `id`.
    Frame { sid: usize, id: u64, rgba: Vec<u8> },
    /// Remove one overlay.
    Close { sid: usize, id: u64 },
    /// The client vanished: remove everything it opened.
    Closed { items: Vec<(usize, u64)> },
}

impl std::fmt::Debug for Msg {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Msg::Hello { tty, .. } => f.debug_struct("Hello").field("tty", tty).finish(),
            Msg::Open {
                sid,
                id,
                col,
                row,
                cols,
                rows,
                w,
                h,
            } => f
                .debug_struct("Open")
                .field("sid", sid)
                .field("id", id)
                .field("col", col)
                .field("row", row)
                .field("cols", cols)
                .field("rows", rows)
                .field("w", w)
                .field("h", h)
                .finish(),
            Msg::Frame { sid, id, rgba } => f
                .debug_struct("Frame")
                .field("sid", sid)
                .field("id", id)
                .field("bytes", &rgba.len())
                .finish(),
            Msg::Close { sid, id } => f
                .debug_struct("Close")
                .field("sid", sid)
                .field("id", id)
                .finish(),
            Msg::Closed { items } => f.debug_struct("Closed").field("items", items).finish(),
        }
    }
}

/// Manages the socket file; removed when the terminal exits.
pub struct Server {
    pub path: PathBuf,
}

impl Server {
    pub fn spawn(proxy: EventLoopProxy) -> Result<Self> {
        let path = socket_path();
        let _ = std::fs::remove_file(&path);
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
        }
        let listener =
            UnixListener::bind(&path).with_context(|| format!("bind {}", path.display()))?;
        // Children inherit this so `rterm play` finds us.
        std::env::set_var(ENV, &path);

        thread_spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { break };
                let proxy = proxy.clone();
                thread_spawn(move || serve(stream, proxy));
            }
        });
        Ok(Self { path })
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

fn thread_spawn(f: impl FnOnce() + Send + 'static) {
    let _ = std::thread::Builder::new()
        .name("rterm-sock".into())
        .spawn(f);
}

/// `$XDG_RUNTIME_DIR/rterm-<pid>.sock`, falling back to `/tmp`.
pub fn socket_path() -> PathBuf {
    use std::os::unix::fs::MetadataExt;
    let pid = std::process::id();
    if let Some(dir) = std::env::var_os("XDG_RUNTIME_DIR") {
        PathBuf::from(dir).join(format!("rterm-{pid}.sock"))
    } else {
        let uid = std::fs::metadata("/proc/self")
            .map(|m| m.uid())
            .unwrap_or(0);
        PathBuf::from(format!("/tmp/rterm-{uid}-{pid}.sock"))
    }
}

// --- framing -----------------------------------------------------------------

pub fn write_msg(w: &mut impl Write, payload: &[u8]) -> io::Result<()> {
    w.write_all(&(payload.len() as u32).to_le_bytes())?;
    w.write_all(payload)?;
    w.flush()
}

/// Read one message; `Ok(None)` on a clean EOF.
pub fn read_msg(r: &mut impl Read) -> io::Result<Option<Vec<u8>>> {
    let mut len = [0u8; 4];
    match r.read_exact(&mut len) {
        Ok(()) => {}
        Err(err) if err.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(err) => return Err(err),
    }
    let n = u32::from_le_bytes(len) as usize;
    if n > MAX_MSG {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "message too large",
        ));
    }
    let mut buf = vec![0u8; n];
    r.read_exact(&mut buf)?;
    Ok(Some(buf))
}

pub fn write_line(w: &mut impl Write, line: &str) -> io::Result<()> {
    let mut payload = Vec::with_capacity(line.len() + 1);
    payload.push(0);
    payload.extend_from_slice(line.as_bytes());
    write_msg(w, &payload)
}

pub fn write_frame(w: &mut impl Write, id: u32, rgba: &[u8]) -> io::Result<()> {
    let mut payload = Vec::with_capacity(rgba.len() + 5);
    payload.push(1);
    payload.extend_from_slice(&id.to_le_bytes());
    payload.extend_from_slice(rgba);
    write_msg(w, &payload)
}

/// One connection: translates framed messages into event-loop [`Msg`]s.
fn serve(mut stream: UnixStream, proxy: EventLoopProxy) {
    let mut sid = usize::MAX;
    let mut opened: Vec<(usize, u64)> = Vec::new();
    while let Ok(Some(payload)) = read_msg(&mut stream) {
        match payload.first() {
            Some(0) => {
                let line = String::from_utf8_lossy(&payload[1..]).into_owned();
                let mut parts = line.split_whitespace();
                match parts.next() {
                    Some("HELLO") => {
                        let tty = parts.next().unwrap_or("").to_string();
                        let (tx, rx) = channel();
                        if proxy
                            .send_event(crate::app::UserEvent::Socket(Msg::Hello {
                                tty,
                                reply: tx,
                            }))
                            .is_err()
                        {
                            return;
                        }
                        // The event loop answers with `SIZE …` (or we time out).
                        let reply = rx
                            .recv_timeout(Duration::from_secs(2))
                            .unwrap_or_else(|_| "ERR busy".into());
                        if let Some(s) = reply
                            .split_whitespace()
                            .nth(6)
                            .and_then(|v| v.parse::<usize>().ok())
                        {
                            sid = s;
                        }
                        if write_line(&mut stream, &reply).is_err() {
                            return;
                        }
                    }
                    Some("OPEN") => {
                        let nums: Vec<usize> = parts
                            .by_ref()
                            .map(|p| p.parse::<usize>().unwrap_or(usize::MAX))
                            .collect();
                        if nums.len() != 8 || nums.iter().any(|&n| n == usize::MAX) {
                            continue;
                        }
                        let (s, id, col, row, cols, rows, w, h) = (
                            nums[0],
                            nums[1] as u64,
                            nums[2],
                            nums[3],
                            nums[4],
                            nums[5],
                            nums[6] as u32,
                            nums[7] as u32,
                        );
                        sid = s;
                        opened.retain(|&(_, existing)| existing != id);
                        opened.push((sid, id));
                        let _ = proxy.send_event(crate::app::UserEvent::Socket(Msg::Open {
                            sid,
                            id,
                            col,
                            row,
                            cols,
                            rows,
                            w,
                            h,
                        }));
                    }
                    Some("CLOSE") => {
                        if let Some(id) = parts.next().and_then(|v| v.parse::<u64>().ok()) {
                            opened.retain(|&(_, existing)| existing != id);
                            let _ = proxy
                                .send_event(crate::app::UserEvent::Socket(Msg::Close { sid, id }));
                        }
                    }
                    Some("BYE") => break,
                    _ => {}
                }
            }
            Some(1) if payload.len() >= 5 => {
                let id = u32::from_le_bytes(payload[1..5].try_into().unwrap()) as u64;
                let rgba = payload[5..].to_vec();
                let _ =
                    proxy.send_event(crate::app::UserEvent::Socket(Msg::Frame { sid, id, rgba }));
            }
            _ => {}
        }
    }
    if !opened.is_empty() {
        let _ = proxy.send_event(crate::app::UserEvent::Socket(Msg::Closed { items: opened }));
    }
}

// --- client ------------------------------------------------------------------

/// Geometry of the pane that owns the connection.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Size {
    pub cols: usize,
    pub rows: usize,
    pub cell_w: f32,
    pub cell_h: f32,
    pub col: usize,
    pub row: usize,
    pub sid: usize,
}

/// Parse the server's `SIZE …` reply.
pub fn parse_size(line: &str) -> Option<Size> {
    let mut f = line.split_whitespace();
    if f.next()? != "SIZE" {
        return None;
    }
    Some(Size {
        cols: f.next()?.parse().ok()?,
        rows: f.next()?.parse().ok()?,
        cell_w: f.next()?.parse().ok()?,
        cell_h: f.next()?.parse().ok()?,
        col: f.next()?.parse().ok()?,
        row: f.next()?.parse().ok()?,
        sid: f.next()?.parse().ok()?,
    })
}

pub struct Client {
    stream: UnixStream,
    pub size: Size,
}

impl Client {
    /// Connect to `$RTERM_SOCK` and handshake; `None` outside rterm.
    pub fn connect() -> Option<Self> {
        let path = std::env::var(ENV).ok()?;
        let mut stream = UnixStream::connect(&path).ok()?;
        stream.set_read_timeout(Some(Duration::from_secs(3))).ok()?;
        let tty = std::fs::read_link("/proc/self/fd/0")
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        write_line(&mut stream, &format!("HELLO {tty}")).ok()?;
        let reply = read_msg(&mut stream).ok()??;
        let text = if reply.first() == Some(&0) {
            String::from_utf8_lossy(&reply[1..]).into_owned()
        } else {
            String::from_utf8_lossy(&reply).into_owned()
        };
        let size = parse_size(&text)?;
        Some(Self { stream, size })
    }

    pub fn open(
        &mut self,
        id: u32,
        col: usize,
        row: usize,
        cols: usize,
        rows: usize,
        w: u32,
        h: u32,
    ) -> io::Result<()> {
        write_line(
            &mut self.stream,
            &format!(
                "OPEN {} {id} {col} {row} {cols} {rows} {w} {h}",
                self.size.sid
            ),
        )
    }

    pub fn frame(&mut self, id: u32, rgba: &[u8]) -> io::Result<()> {
        write_frame(&mut self.stream, id, rgba)
    }

    pub fn close(&mut self, id: u32) -> io::Result<()> {
        write_line(&mut self.stream, &format!("CLOSE {id}"))
    }

    pub fn bye(mut self) {
        let _ = write_line(&mut self.stream, "BYE");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn framing_roundtrip() {
        let (mut a, mut b) = UnixStream::pair().unwrap();
        write_line(&mut a, "HELLO /dev/pts/7").unwrap();
        let msg = read_msg(&mut b).unwrap().unwrap();
        assert_eq!(msg[0], 0);
        assert_eq!(&msg[1..], b"HELLO /dev/pts/7");
    }

    #[test]
    fn frame_roundtrip_keeps_id_and_pixels() {
        let (mut a, mut b) = UnixStream::pair().unwrap();
        let rgba = vec![9u8; 16];
        write_frame(&mut a, 42, &rgba).unwrap();
        let msg = read_msg(&mut b).unwrap().unwrap();
        assert_eq!(msg[0], 1);
        assert_eq!(u32::from_le_bytes(msg[1..5].try_into().unwrap()), 42);
        assert_eq!(&msg[5..], &rgba);
    }

    #[test]
    fn eof_ends_the_stream() {
        let (a, mut b) = UnixStream::pair().unwrap();
        drop(a);
        assert!(read_msg(&mut b).unwrap().is_none());
    }

    #[test]
    fn size_line_parses() {
        let size = parse_size("SIZE 120 34 9.00 18.00 3 4 1").unwrap();
        assert_eq!(size.cols, 120);
        assert_eq!(size.rows, 34);
        assert_eq!(size.cell_w, 9.0);
        assert_eq!(size.cell_h, 18.0);
        assert_eq!((size.col, size.row, size.sid), (3, 4, 1));
        assert!(parse_size("ERR busy").is_none());
    }
}
