//! Media protocols carried over the PTY:
//!
//!   - APC `ESC _ G ... ST`      — kitty graphics (transmit/print/delete/query)
//!   - OSC  `ESC ] 1337;... BEL` — iTerm2 inline images
//!   - OSC  `ESC ] 1338;... BEL` — rterm video overlays (`rterm play` frames)
//!
//! Parsing and routing happen on the event loop (they are cheap); actual image
//! decoding runs on a worker thread so a 20 MP JPEG never blocks a frame.

use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::thread;

use crate::image_decode::{self, decode_any, Frame, Image};
use crate::intercept::Captured;
use crate::iterm::{self, SizeHint};
use crate::kitty::{self, DeleteScope, Format, Op};
use crate::media::Source;

/// Cell rectangle a piece of media is anchored to.
///
/// `cols`/`rows` of 0 mean "derive from the pixel size when drawing", which is
/// what the renderer does for images whose cell size was never specified.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Place {
    pub col: usize,
    pub row: usize,
    pub cols: usize,
    pub rows: usize,
    pub z: i32,
}

/// Data that must survive the worker round-trip.
#[derive(Debug, Clone)]
pub struct Meta {
    pub source: Source,
    pub place: Option<Place>,
    /// Escape sequence to write back to the PTY once decoding finishes.
    pub reply: Option<String>,
    /// Protocol id of the video overlay this payload belongs to.
    pub video: Option<u64>,
    /// Session that owns this payload.
    pub sid: usize,
}

/// What the worker has to decode.
#[derive(Debug, Clone)]
pub enum Payload {
    /// png/jpeg/gif bytes; `inflate` when kitty zlib-compressed them.
    File { data: Vec<u8>, inflate: bool },
    /// Raw RGB/RGBA samples from kitty's f=24 / f=32 transmission.
    Raw {
        w: u32,
        h: u32,
        channels: u8,
        data: Vec<u8>,
    },
    /// A path handed over by kitty's file transmission medium.
    Path { path: PathBuf, delete: bool },
}

pub struct Job {
    pub meta: Meta,
    pub payload: Payload,
}

#[derive(Debug, Clone)]
pub enum Content {
    Static(Image),
    Anim(Vec<Frame>),
}

pub struct Done {
    pub meta: Meta,
    pub result: Result<Content, String>,
}

/// Action the event loop applies after a protocol message was parsed.
pub enum Effect {
    /// Send decode work to the worker thread.
    Job(Job),
    /// Write an escape-sequence reply to the PTY.
    Pty(String),
    /// Print a previously transmitted kitty image.
    Print {
        key: String,
        place: Place,
    },
    DeleteKey(String),
    /// Delete whatever is anchored on this cell.
    DeleteAt {
        col: usize,
        row: usize,
    },
    ClearMedia,
    VideoClose {
        id: u64,
    },
}

/// Snapshot of the terminal a protocol message may refer to.
#[derive(Debug, Clone)]
pub struct Ctx {
    /// Cursor position, viewport coordinates.
    pub col: usize,
    pub row: usize,
    /// Image ids currently held in the media store (for `i?` queries).
    pub kitty_keys: Vec<String>,
    /// Session the message came from, so replies and media land back there.
    pub sid: usize,
}

pub struct Protocol {
    kitty: kitty::TransmitBuffer,
    /// `a=p` requests for images that are still being decoded.
    deferred_prints: Vec<(String, Place)>,
}

impl Default for Protocol {
    fn default() -> Self {
        Self::new()
    }
}

impl Protocol {
    pub fn new() -> Self {
        Self {
            kitty: kitty::TransmitBuffer::default(),
            deferred_prints: Vec::new(),
        }
    }

    pub fn handle(&mut self, cap: Captured, ctx: &Ctx) -> Vec<Effect> {
        let mut out = Vec::new();
        match cap {
            Captured::KittyGfx(payload) => self.kitty(&payload, ctx, &mut out),
            Captured::Iterm1337(content) => Self::iterm(&content, ctx, &mut out),
            Captured::Media1338(content) => Self::video(&content, ctx, &mut out),
        }
        out
    }

    /// Park a print whose image is still decoding.
    pub fn defer_print(&mut self, key: String, place: Place) {
        // Keep at most a handful of pending prints; the oldest are the least interesting.
        if self.deferred_prints.len() >= 64 {
            self.deferred_prints.remove(0);
        }
        self.deferred_prints.push((key, place));
    }

    pub fn take_print(&mut self, key: &str) -> Option<Place> {
        let idx = self.deferred_prints.iter().position(|(k, _)| k == key)?;
        Some(self.deferred_prints.remove(idx).1)
    }

    fn kitty(&mut self, payload: &str, ctx: &Ctx, out: &mut Vec<Effect>) {
        let msg = kitty::parse(payload);
        let key = msg.get("i").unwrap_or("0").to_string();
        let quiet = msg.quiet();

        // Capability and id queries.
        if msg.has_query("a") {
            out.push(Effect::Pty("\x1b_Ga=t,T,p,d\x1b\\".into()));
            return;
        }
        if msg.has_query("i") || msg.has_query("p") {
            out.push(Effect::Pty(format!(
                "\x1b_Gi={}\x1b\\",
                ctx.kitty_keys.join(",")
            )));
            return;
        }

        // Remember whether this transmission also prints, before `interpret`
        // consumes the message.
        let print_here = msg.get("a") == Some("T");
        let place = Place {
            col: msg
                .get_u32("c")
                .map(|v| v.saturating_sub(1))
                .unwrap_or(ctx.col as u32) as usize,
            row: msg
                .get_u32("r")
                .map(|v| v.saturating_sub(1))
                .unwrap_or(ctx.row as u32) as usize,
            cols: msg.get_u32("w").unwrap_or(0) as usize,
            rows: msg.get_u32("h").unwrap_or(0) as usize,
            z: 0,
        };

        match kitty::interpret(&msg) {
            Op::Transmit(t) => {
                let format = t.format;
                let width = t.width;
                let height = t.height;
                let compression = t.compression;
                let medium = msg.get("t").unwrap_or("d").to_string();
                let place = print_here.then_some(place);
                match self.kitty.push(t) {
                    Err(err) => out.push(Effect::Pty(err_reply(&key, "EBADFMT", &err))),
                    Ok(None) => {}
                    Ok(Some(done)) => {
                        let payload = match medium.as_str() {
                            "f" | "t" => Payload::Path {
                                path: String::from_utf8_lossy(&done.payload).into_owned().into(),
                                delete: medium == "t",
                            },
                            _ => match raw_payload(format, width, height, &done.payload) {
                                Ok(Some(payload)) => payload,
                                Ok(None) => Payload::File {
                                    data: done.payload,
                                    inflate: compression == 32 || compression == 33,
                                },
                                Err(err) => {
                                    out.push(Effect::Pty(err_reply(&key, "EBADFMT", &err)));
                                    return;
                                }
                            },
                        };
                        let reply = (quiet == 0).then(|| ok_reply(&key));
                        out.push(Effect::Job(Job {
                            meta: Meta {
                                source: Source::Kitty(key.clone()),
                                place,
                                reply,
                                video: None,
                                sid: ctx.sid,
                            },
                            payload,
                        }));
                    }
                }
            }
            Op::Print {
                key,
                col,
                row,
                quiet: _,
            } => {
                let place = Place {
                    col: col.unwrap_or(ctx.col as u32) as usize,
                    row: row.unwrap_or(ctx.row as u32) as usize,
                    ..place
                };
                out.push(Effect::Print { key, place });
            }
            Op::Delete {
                scope,
                key,
                quiet: _,
            } => match scope {
                DeleteScope::Image => match key {
                    Some(k) => out.push(Effect::DeleteKey(k)),
                    None => out.push(Effect::ClearMedia),
                },
                DeleteScope::CursorCell => out.push(Effect::DeleteAt {
                    col: ctx.col,
                    row: ctx.row,
                }),
                DeleteScope::All => out.push(Effect::ClearMedia),
            },
            Op::Query { quiet: _ } => out.push(Effect::Pty(format!(
                "\x1b_Gi={}\x1b\\",
                ctx.kitty_keys.join(",")
            ))),
            Op::Unsupported { action, quiet } => {
                if quiet == 0 {
                    out.push(Effect::Pty(err_reply(
                        &key,
                        "EN",
                        &format!("unsupported action {action:?}"),
                    )));
                }
            }
        }
    }

    fn iterm(content: &str, ctx: &Ctx, out: &mut Vec<Effect>) {
        let Some(file) = iterm::parse(content) else {
            return;
        };
        // Without `inline=1` the file is only referenced, never drawn.
        if !file.inline {
            return;
        }
        let place = Place {
            col: ctx.col,
            row: ctx.row,
            cols: match file.width {
                Some(SizeHint::Cells(n)) => n as usize,
                _ => 0,
            },
            rows: match file.height {
                Some(SizeHint::Cells(n)) => n as usize,
                _ => 0,
            },
            z: 0,
        };
        out.push(Effect::Job(Job {
            meta: Meta {
                source: Source::Iterm,
                place: Some(place),
                reply: None,
                video: None,
                sid: ctx.sid,
            },
            payload: Payload::File {
                data: file.data,
                inflate: false,
            },
        }));
    }

    /// `OSC 1338;<id>,<col>,<row>,<cols>,<rows>;<base64 frame>`.
    ///
    /// An empty frame body is an "open"/keep-alive marker: the overlay appears
    /// with the first frame that carries pixels.
    fn video(content: &str, ctx: &Ctx, out: &mut Vec<Effect>) {
        let (head, body) = match content.split_once(';') {
            Some(pair) => pair,
            None => return,
        };
        let mut nums = head.split(',').filter_map(|n| n.trim().parse::<i64>().ok());
        let (Some(id), Some(col), Some(row), Some(cols), Some(rows)) = (
            nums.next(),
            nums.next(),
            nums.next(),
            nums.next(),
            nums.next(),
        ) else {
            return;
        };
        if id < 0 || col < -1 || row < -1 {
            return;
        }
        if body.is_empty() {
            out.push(Effect::VideoClose { id: id as u64 });
            return;
        }
        let Ok(data) = b64(body) else { return };
        out.push(Effect::Job(Job {
            meta: Meta {
                source: Source::Socket(id as u64),
                sid: ctx.sid,
                place: Some(Place {
                    // `-1` anchors the overlay where the cursor is now — the
                    // client has no way to know the cursor position.
                    col: if col < 0 { ctx.col } else { col as usize },
                    row: if row < 0 { ctx.row } else { row as usize },
                    cols: cols.max(0) as usize,
                    rows: rows.max(0) as usize,
                    z: 0,
                }),
                reply: None,
                video: Some(id as u64),
            },
            payload: Payload::File {
                data,
                inflate: false,
            },
        }));
    }
}

/// `f=24`/`f=32` carry raw samples instead of an encoded image.
fn raw_payload(
    format: Format,
    width: u32,
    height: u32,
    data: &[u8],
) -> Result<Option<Payload>, String> {
    let channels = match format {
        Format::Rgb => 3,
        Format::Rgba => 4,
        Format::Png => return Ok(None),
    };
    if width == 0 || height == 0 {
        return Err("raw transmission is missing s=/v= dimensions".into());
    }
    let expected = (width as usize)
        .checked_mul(height as usize)
        .and_then(|px| px.checked_mul(channels as usize))
        .ok_or_else(|| "raw transmission dimensions overflow".to_string())?;
    if data.len() < expected {
        return Err(format!(
            "raw transmission too small ({} < {})",
            data.len(),
            expected
        ));
    }
    Ok(Some(Payload::Raw {
        w: width,
        h: height,
        channels,
        data: data[..expected].to_vec(),
    }))
}

pub fn ok_reply(key: &str) -> String {
    format!("\x1b_Gi={key};OK\x1b\\")
}

pub fn err_reply(key: &str, code: &str, message: &str) -> String {
    format!("\x1b_Gi={key};{code};{}\x1b\\", sanitize(message))
}

fn sanitize(message: &str) -> String {
    message
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || " .,:;=-_/".contains(c) {
                c
            } else {
                '?'
            }
        })
        .take(96)
        .collect()
}

fn b64(text: &str) -> Result<Vec<u8>, String> {
    use base64::Engine;
    let engine = base64::engine::general_purpose::STANDARD;
    engine
        .decode(text.trim())
        .or_else(|_| base64::engine::general_purpose::STANDARD_NO_PAD.decode(text.trim()))
        .map_err(|e| e.to_string())
}

/// Magic prefix of a raw RGBA frame: `RTF1` + `u32le width` + `u32le height`.
const RAW_FRAME_MAGIC: &[u8; 4] = b"RTF1";
/// Hard caps so a hostile payload cannot ask for an unbounded allocation.
const MAX_PIXELS: u64 = 100_000_000;

fn raw_frame(data: &[u8]) -> Option<Image> {
    if data.len() < 12 || &data[..4] != RAW_FRAME_MAGIC {
        return None;
    }
    let w = u32::from_le_bytes(data[4..8].try_into().ok()?);
    let h = u32::from_le_bytes(data[8..12].try_into().ok()?);
    let px = u64::from(w) * u64::from(h);
    if w == 0 || h == 0 || px > MAX_PIXELS {
        return None;
    }
    let need = (px * 4) as usize;
    if data.len() < 12 + need {
        return None;
    }
    Some(Image::from_rgba(w, h, data[12..12 + need].to_vec()))
}

fn run(payload: Payload) -> Result<Content, String> {
    match payload {
        Payload::Raw {
            w,
            h,
            channels,
            data,
        } => {
            let px = u64::from(w) * u64::from(h);
            if px > MAX_PIXELS {
                return Err("raw image too large".into());
            }
            let mut rgba = Vec::with_capacity((px * 4) as usize);
            match channels {
                3 => {
                    for px in data.chunks_exact(3) {
                        rgba.extend_from_slice(&[px[0], px[1], px[2], 255]);
                    }
                }
                4 => rgba.extend_from_slice(&data),
                other => return Err(format!("unsupported channel count {other}")),
            }
            Ok(Content::Static(Image::from_rgba(w, h, rgba)))
        }
        Payload::File { data, inflate } => {
            let data = if inflate {
                image_decode::inflate(&data).map_err(|e| format!("inflate: {e}"))?
            } else {
                data
            };
            decode_bytes(&data)
        }
        Payload::Path { path, delete } => {
            let data = std::fs::read(&path).map_err(|e| format!("read {}: {e}", path.display()))?;
            if delete {
                let _ = std::fs::remove_file(&path);
            }
            decode_bytes(&data)
        }
    }
}

fn decode_bytes(data: &[u8]) -> Result<Content, String> {
    if let Some(image) = raw_frame(data) {
        return Ok(Content::Static(image));
    }
    match decode_any(data) {
        Ok(image_decode::Decoded::Static(image)) => Ok(Content::Static(image)),
        Ok(image_decode::Decoded::Animated(frames)) => Ok(Content::Anim(frames)),
        Err(err) => Err(err.to_string()),
    }
}

/// Decode worker: one thread, FIFO order, results pushed back over an mpsc.
pub struct Pipeline {
    jobs: Sender<Job>,
    done: Receiver<Done>,
}

impl Pipeline {
    pub fn spawn(proxy: crate::app::EventLoopProxy) -> Self {
        let (job_tx, job_rx) = channel::<Job>();
        let (done_tx, done_rx) = channel::<Done>();
        thread::Builder::new()
            .name("rterm-decoder".into())
            .spawn(move || {
                while let Ok(job) = job_rx.recv() {
                    let result = run(job.payload);
                    if done_tx
                        .send(Done {
                            meta: job.meta,
                            result,
                        })
                        .is_err()
                    {
                        break;
                    }
                    let _ = proxy.send_event(crate::app::UserEvent::MediaReady);
                }
            })
            .expect("spawn decoder thread");
        Self {
            jobs: job_tx,
            done: done_rx,
        }
    }

    pub fn submit(&self, job: Job) {
        // The event loop owns the receiver; a closed channel just means we are quitting.
        let _ = self.jobs.send(job);
    }

    pub fn drain(&self) -> Vec<Done> {
        self.done.try_iter().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> Ctx {
        Ctx {
            col: 3,
            row: 4,
            kitty_keys: vec!["1".into()],
            sid: 0,
        }
    }

    fn effects(cap: Captured) -> Vec<Effect> {
        let mut protocol = Protocol::new();
        protocol.handle(cap, &ctx())
    }

    #[test]
    fn raw_frame_decodes_and_validates() {
        let mut data = b"RTF1".to_vec();
        data.extend_from_slice(&2u32.to_le_bytes());
        data.extend_from_slice(&2u32.to_le_bytes());
        data.extend_from_slice(&[1, 2, 3, 255, 4, 5, 6, 255, 7, 8, 9, 255, 10, 11, 12, 255]);

        let image = raw_frame(&data).expect("frame decodes");
        assert_eq!((image.w, image.h), (2, 2));
        assert_eq!(&image.rgba[..4], &[1, 2, 3, 255]);
        assert_eq!(&image.rgba[12..16], &[10, 11, 12, 255]);

        assert!(
            raw_frame(&data[..data.len() - 1]).is_none(),
            "truncated frame"
        );
        assert!(raw_frame(b"NOPE00000000000000000").is_none(), "wrong magic");
        assert!(
            raw_frame(b"RTF1\x00\x00\x00\x00\x00\x00\x00\x00").is_none(),
            "zero size"
        );
    }

    #[test]
    fn raw_rgb_is_promoted_to_opaque_rgba() {
        let content = run(Payload::Raw {
            w: 1,
            h: 2,
            channels: 3,
            data: vec![9, 8, 7, 1, 2, 3],
        })
        .expect("decode");
        let Content::Static(image) = content else {
            panic!("expected a still image")
        };
        assert_eq!((image.w, image.h), (1, 2));
        assert_eq!(&image.rgba[..], &[9, 8, 7, 255, 1, 2, 3, 255]);

        let err = run(Payload::Raw {
            w: 1,
            h: 1,
            channels: 5,
            data: vec![0; 5],
        })
        .expect_err("bad channel count");
        assert!(err.contains("channel"), "{err}");
    }

    #[test]
    fn kitty_transmit_and_print_queues_a_job() {
        let effects = effects(Captured::KittyGfx("a=T,i=9,f=32,s=1,v=1;AAAAAA==".into()));
        let [Effect::Job(job)] = effects.as_slice() else {
            panic!("expected a single decode job");
        };
        assert_eq!(job.meta.source, Source::Kitty("9".into()));
        assert_eq!(job.meta.reply, Some(ok_reply("9")));
        let place = job.meta.place.expect("print position");
        assert_eq!((place.col, place.row), (3, 4));
        assert!(matches!(
            job.payload,
            Payload::Raw {
                w: 1,
                h: 1,
                channels: 4,
                ..
            }
        ));
    }

    #[test]
    fn kitty_capability_query_answers_on_the_pty() {
        let effects = effects(Captured::KittyGfx("a?".into()));
        let [Effect::Pty(reply)] = effects.as_slice() else {
            panic!("expected a capability reply");
        };
        assert!(reply.starts_with("\x1b_Ga=t"), "{reply}");
    }

    #[test]
    fn kitty_delete_without_id_clears_everything() {
        assert!(matches!(
            effects(Captured::KittyGfx("a=d;".into())).as_slice(),
            [Effect::ClearMedia]
        ));
        assert!(matches!(
            effects(Captured::KittyGfx("a=d,d=c;".into())).as_slice(),
            [Effect::DeleteAt { col: 3, row: 4 }]
        ));
    }

    #[test]
    fn iterm_requires_inline_and_queues_a_job() {
        assert!(effects(Captured::Iterm1337("File=name=x;QQ==".into())).is_empty());
        assert!(effects(Captured::Iterm1337("inline=1;width=2cells:QQ==".into())).is_empty());
        let job_effects = effects(Captured::Iterm1337(
            "File=inline=1;width=2cells;height=1cells:QQ==".into(),
        ));
        let [Effect::Job(job)] = job_effects.as_slice() else {
            panic!("expected a decode job");
        };
        assert_eq!(job.meta.source, Source::Iterm);
        let place = job.meta.place.expect("position");
        assert_eq!((place.cols, place.rows), (2, 1));
    }

    #[test]
    fn osc1338_frames_and_closes_video_overlays() {
        let frame_effects = effects(Captured::Media1338("7,1,2,4,3;QUJD".into()));
        let [Effect::Job(job)] = frame_effects.as_slice() else {
            panic!("expected a decode job");
        };
        assert_eq!(job.meta.source, Source::Socket(7));
        assert_eq!(job.meta.video, Some(7));
        let place = job.meta.place.expect("position");
        assert_eq!((place.col, place.row, place.cols, place.rows), (1, 2, 4, 3));

        let close = effects(Captured::Media1338("7,0,0,0,0;".into()));
        assert!(matches!(close.as_slice(), [Effect::VideoClose { id: 7 }]));
    }

    #[test]
    fn replies_are_sanitized() {
        assert_eq!(ok_reply("3"), "\x1b_Gi=3;OK\x1b\\");
        let reply = err_reply("3", "EDECODE", "bad\r\nbytes\x07");
        assert!(!reply.contains('\r') && !reply.contains('\n') && !reply.contains('\x07'));
        assert!(reply.contains("EDECODE"));
    }
}
