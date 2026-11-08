//! `rterm play <file>` — show images, animated GIFs and video inside rterm.
//!
//! * PNG / JPEG / GIF are written to the PTY as iTerm2 inline images
//!   (`OSC 1337`); rterm decodes and animates them, and they stay on screen
//!   after the command exits.
//! * Video is decoded by ffmpeg and streamed as raw RGBA frames over the
//!   local socket (`$RTERM_SOCK`). Without the socket but inside rterm the
//!   frames fall back to `OSC 1338` over the PTY.

use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{bail, Context, Result};
use base64::Engine;

use crate::socket::{self, Client};

pub struct Options {
    pub file: PathBuf,
    /// Target width in cells (socket mode only).
    pub cols: Option<usize>,
    /// Target height in cells (socket mode only).
    pub rows: Option<usize>,
    /// Explicit size in pixels (PTY fallback mode).
    pub pixel: Option<(u32, u32)>,
    pub fps: Option<u32>,
    pub r#loop: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            file: PathBuf::new(),
            cols: None,
            rows: None,
            pixel: None,
            fps: None,
            r#loop: false,
        }
    }
}

impl Options {
    pub fn parse(args: &[String]) -> Result<Self> {
        let mut opts = Self::default();
        let mut i = 0;
        while i < args.len() {
            let value = |i: &mut usize| -> Result<String> {
                *i += 1;
                let flag = &args[*i - 1];
                args.get(*i)
                    .cloned()
                    .ok_or_else(|| anyhow::anyhow!("{flag} needs a value"))
            };
            match args[i].as_str() {
                "-W" | "--cols" => {
                    opts.cols = Some(value(&mut i)?.parse().context("--cols must be a number")?);
                }
                "-H" | "--rows" => {
                    opts.rows = Some(value(&mut i)?.parse().context("--rows must be a number")?);
                }
                "--fps" => {
                    opts.fps = Some(value(&mut i)?.parse().context("--fps must be a number")?);
                }
                "--size" => {
                    let raw = value(&mut i)?;
                    let (w, h) = raw
                        .split_once('x')
                        .with_context(|| format!("--size wants WxH, got {raw}"))?;
                    opts.pixel = Some((
                        w.parse().context("--size width")?,
                        h.parse().context("--size height")?,
                    ));
                }
                "--loop" => opts.r#loop = true,
                other if opts.file.as_os_str().is_empty() => opts.file = PathBuf::from(other),
                other => bail!("unknown play argument: {other}"),
            }
            i += 1;
        }
        if opts.file.as_os_str().is_empty() {
            bail!("play: a file is required (e.g. rterm play clip.mp4)");
        }
        Ok(opts)
    }
}

fn is_inline(head: &[u8]) -> bool {
    head.starts_with(b"\x89PNG\r\n\x1a\n")
        || head.starts_with(&[0xff, 0xd8, 0xff])
        || head.starts_with(b"GIF87a")
        || head.starts_with(b"GIF89a")
}

pub fn run(opts: Options) -> Result<()> {
    let mut head = [0u8; 16];
    let n = {
        let mut file = std::fs::File::open(&opts.file)
            .with_context(|| format!("open {}", opts.file.display()))?;
        file.read(&mut head)?
    };
    if is_inline(&head[..n]) {
        let data =
            std::fs::read(&opts.file).with_context(|| format!("read {}", opts.file.display()))?;
        return inline(&opts.file, &data);
    }
    stream_video(&opts)
}

/// Write the file as an iTerm2 inline image (`OSC 1337`).
fn inline(path: &Path, data: &[u8]) -> Result<()> {
    let name = path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "image".into());
    // `:` and `;` would collide with the argument/data separator.
    let name: String = name
        .chars()
        .map(|c| if ":;\"\r\n".contains(c) { '_' } else { c })
        .collect();
    let b64 = base64::engine::general_purpose::STANDARD.encode(data);
    let mut out = io::stdout().lock();
    write!(
        out,
        "\x1b]1337;File=name={name};inline=1;size={}:{}\x07",
        data.len(),
        b64
    )
    .context("write to terminal")?;
    out.flush().ok();
    Ok(())
}

/// Probe the source resolution with ffprobe (optional).
fn probe(path: &Path) -> Option<(u32, u32)> {
    let out = Command::new("ffprobe")
        .args(["-v", "error", "-select_streams", "v:0"])
        .args(["-show_entries", "stream=width,height", "-of", "csv=p=0"])
        .arg(path)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let (w, h) = text.trim().split_once(',')?;
    Some((w.parse().ok()?, h.parse().ok()?))
}

/// Largest even size that fits the box without upscaling (`src = None` fills it).
fn fit_even(src: Option<(u32, u32)>, box_w: u32, box_h: u32) -> (u32, u32) {
    let box_w = box_w.max(2) & !1;
    let box_h = box_h.max(2) & !1;
    let Some((sw, sh)) = src.filter(|&(w, h)| w > 0 && h > 0) else {
        return (box_w, box_h);
    };
    let scale = (box_w as f32 / sw as f32)
        .min(box_h as f32 / sh as f32)
        .min(1.0);
    let w = ((((sw as f32) * scale) as u32) & !1).max(2);
    let h = ((((sh as f32) * scale) as u32) & !1).max(2);
    (w, h)
}

fn spawn_ffmpeg(path: &Path, w: u32, h: u32, fps: u32, lp: bool) -> Result<std::process::Child> {
    let mut cmd = Command::new("ffmpeg");
    cmd.arg("-v").arg("error");
    if lp {
        cmd.arg("-stream_loop").arg("-1");
    }
    cmd.arg("-i").arg(path);
    cmd.arg("-vf").arg(format!("scale={w}:{h}"));
    cmd.args([
        "-f",
        "rawvideo",
        "-pix_fmt",
        "rgba",
        "-r",
        &fps.to_string(),
        "pipe:1",
    ]);
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    cmd.spawn()
        .context("ffmpeg is required for video playback (install ffmpeg)")
}

/// Read exactly one frame; `Ok(false)` on a clean/partial EOF.
fn next_frame(r: &mut impl Read, buf: &mut [u8]) -> io::Result<bool> {
    let mut filled = 0;
    while filled < buf.len() {
        match r.read(&mut buf[filled..])? {
            0 => return Ok(false),
            n => filled += n,
        }
    }
    Ok(true)
}

fn stream_video(opts: &Options) -> Result<()> {
    let src = probe(&opts.file);
    let mut client = Client::connect();
    if client.is_none() && std::env::var("RTERM").is_err() {
        bail!(
            "video playback needs rterm: {} is not set (socket disabled?)",
            socket::ENV
        );
    }
    let fps = opts.fps.unwrap_or(if client.is_some() { 30 } else { 15 });

    // Target pixel size: fit the pane (socket) or use --size (PTY fallback).
    let (w, h) = match &client {
        Some(c) => {
            let s = c.size;
            let avail_cols = opts.cols.unwrap_or(s.cols.saturating_sub(1).min(80)).max(2);
            let avail_rows = opts.rows.unwrap_or(s.rows.saturating_sub(1).min(40)).max(2);
            let box_w = (avail_cols as f32 * s.cell_w) as u32;
            let box_h = (avail_rows as f32 * s.cell_h) as u32;
            fit_even(src, box_w, box_h)
        }
        None => {
            let (w, h) = opts.pixel.unwrap_or((320, 180));
            ((w.max(2) & !1), (h.max(2) & !1))
        }
    };

    let mut child = spawn_ffmpeg(&opts.file, w, h, fps, opts.r#loop)?;
    let mut pipe = child.stdout.take().context("ffmpeg stdout")?;
    let frame_len = w as usize * h as usize * 4;
    let mut buf = vec![0u8; frame_len];
    let id = std::process::id();
    let mut out = io::stdout().lock();

    eprintln!("rterm play: {w}x{h} @ {fps} fps — Ctrl+C to stop");

    // Anchor the overlay before the first frame arrives.
    if let Some(c) = client.as_mut() {
        let s = c.size;
        let cols = (w as f32 / s.cell_w).ceil() as usize;
        let rows = (h as f32 / s.cell_h).ceil() as usize;
        c.open(id, s.col, s.row, cols, rows.max(1), w, h)
            .context("socket open")?;
    }

    let mut frames = 0u64;
    loop {
        match next_frame(&mut pipe, &mut buf) {
            Ok(true) => {}
            Ok(false) => break,
            Err(err) => return Err(err).context("read ffmpeg frame"),
        }
        match client.as_mut() {
            Some(c) => {
                if let Err(err) = c.frame(id, &buf) {
                    if err.kind() == io::ErrorKind::BrokenPipe {
                        break; // terminal went away
                    }
                    return Err(err).context("socket frame");
                }
            }
            None => {
                let mut packet = Vec::with_capacity(12 + frame_len);
                packet.extend_from_slice(b"RTF1");
                packet.extend_from_slice(&w.to_le_bytes());
                packet.extend_from_slice(&h.to_le_bytes());
                packet.extend_from_slice(&buf);
                let b64 = base64::engine::general_purpose::STANDARD.encode(&packet);
                write!(out, "\x1b]1338;{id},-1,-1,0,0;{b64}\x07").context("write frame")?;
                out.flush().ok();
            }
        }
        frames += 1;
    }

    match client {
        Some(mut c) => {
            let _ = c.close(id);
            c.bye();
        }
        None => {
            write!(out, "\x1b]1338;{id},-1,-1,0,0;\x07").ok();
            out.flush().ok();
        }
    }
    let _ = child.kill();
    let _ = child.wait();
    if frames == 0 {
        bail!(
            "ffmpeg produced no frames (is {} a video?)",
            opts.file.display()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_inline_formats() {
        assert!(is_inline(b"\x89PNG\r\n\x1a\n...."));
        assert!(is_inline(&[0xff, 0xd8, 0xff, 0xe0]));
        assert!(is_inline(b"GIF89a.."));
        assert!(!is_inline(b"\x00\x00\x00 ftypisom"));
    }

    #[test]
    fn fits_preserve_aspect_and_evenness() {
        assert_eq!(fit_even(Some((1920, 1080)), 800, 600), (800, 450));
        assert_eq!(fit_even(Some((100, 400)), 800, 600), (100, 400));
        // Never upscales the source.
        assert_eq!(fit_even(Some((64, 32)), 800, 600), (64, 32));
        // No source: fill the box, even.
        assert_eq!(fit_even(None, 801, 601), (800, 600));
    }

    #[test]
    fn parses_options() {
        let args: Vec<String> = ["clip.mp4", "-W", "40", "--fps", "24", "--loop"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let opts = Options::parse(&args).unwrap();
        assert_eq!(opts.file, PathBuf::from("clip.mp4"));
        assert_eq!(opts.cols, Some(40));
        assert_eq!(opts.fps, Some(24));
        assert!(opts.r#loop);
        assert!(Options::parse(&[]).is_err());
    }

    #[test]
    fn sizes_are_even_and_at_least_two() {
        assert_eq!((640, 360), fit_even(Some((640, 360)), 640, 360));
        assert_eq!((636, 358), fit_even(Some((640, 360)), 639, 359));
    }
}
