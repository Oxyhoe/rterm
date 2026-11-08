//! One terminal: emulator, PTY and the media/protocol state it owns.

use std::collections::HashMap;
use std::io::Read;
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::thread;
use std::time::Instant;

use alacritty_terminal::ansi::{CursorShape, Processor};
use alacritty_terminal::grid::Scroll;
use alacritty_terminal::index::{Column, Line, Point, Side};
use alacritty_terminal::term::{SizeInfo, Term};
use anyhow::{Context, Result};

use crate::anim::TextFx;
use crate::app::TermProxy;
use crate::config::{Config, ShellCfg};
use crate::image_decode::scale_nearest;
use crate::intercept::{Captured, Interceptor};
use crate::media::{MediaStore, Source};
use crate::protocol::{self, Content, Ctx, Effect, Meta, Pipeline};
use crate::pty::{self, Pty};
use crate::render::Layout;
use crate::suggest::{self, Typing};

pub enum PtyMsg {
    Data(Vec<u8>),
    Eof,
}

/// What happened while draining the PTY.
#[derive(Default)]
pub struct Drain {
    pub fed: bool,
    pub eof: bool,
}

pub struct Session {
    pub id: usize,
    pub term: Term<TermProxy>,
    pub parser: Processor,
    pub interceptor: Interceptor,
    pub protocol: protocol::Protocol,
    pub media: MediaStore,
    pub videos: HashMap<u64, u64>,
    pub pty: Option<Pty>,
    pub killer: Option<Box<dyn portable_pty::ChildKiller + Send + Sync>>,
    /// PID of the shell process (for matching `rterm play` clients to a pane).
    pub pid: Option<u32>,
    pty_tx: Sender<PtyMsg>,
    pty_rx: Receiver<PtyMsg>,
    pub title: String,
    /// What the shell prompt has been fed since the last Enter (autosuggest).
    pub typing: Typing,
    /// Commands the grey suggestion searches: the shell's own history plus
    /// what was typed in this session, newest first.
    pub history: Vec<String>,
    /// Per-row entrance animation state (see [`crate::anim`]).
    pub fx: TextFx,
}

impl Session {
    pub fn new(
        id: usize,
        term_cfg: &alacritty_terminal::config::Config<()>,
        size: SizeInfo,
        proxy: TermProxy,
        media_cfg: &crate::config::MediaCfg,
    ) -> Self {
        let media = MediaStore::new(media_cfg.max_cache_mb, media_cfg.loop_gifs);
        let (pty_tx, pty_rx) = channel();
        Self {
            id,
            term: Term::new(term_cfg, size, proxy),
            parser: Processor::new(),
            interceptor: Interceptor::new(),
            protocol: protocol::Protocol::new(),
            media,
            videos: HashMap::new(),
            pty: None,
            killer: None,
            pid: None,
            pty_tx,
            pty_rx,
            title: String::new(),
            typing: Typing::new(),
            history: suggest::load_history(&suggest::history_path()),
            fx: TextFx::new(),
        }
    }

    /// Refresh the per-row entrance animation from the visible grid.
    pub fn refresh_fx(&mut self, layout: &Layout, cfg: &Config, now: Instant) {
        let mut content = self.term.renderable_content();
        self.fx.refresh(&mut content, layout, cfg, now);
    }

    /// Enter reached the shell: remember the command for future suggestions.
    pub fn commit_typed(&mut self) {
        if let Some(cmd) = self.typing.commit() {
            self.history.insert(0, cmd);
            self.history.truncate(suggest::HISTORY);
        }
    }

    /// The grey tail to draw after the cursor: its text and the column it
    /// starts at.
    ///
    /// `None` unless suggestions are on, the line is still tracked, the
    /// screen in front of the cursor really ends with what was typed, and the
    /// cells ahead are empty — a suggestion never paints over real content.
    pub fn suggestion(&self, layout: &Layout, enabled: bool) -> Option<(String, usize)> {
        if !enabled || !self.typing.trusted() {
            return None;
        }
        let typed = self.typing.text();
        if typed.is_empty() {
            return None;
        }
        let tail = suggest::match_history(&self.history, typed)?.to_string();
        let content = self.term.renderable_content();
        if content.cursor.shape == CursorShape::Hidden {
            return None;
        }
        let col = content.cursor.point.column.0;
        let row = &self.term.grid()[content.cursor.point.line];
        let head: String = (0..col).map(|c| row[Column(c)].c).collect();
        if !head.ends_with(typed) {
            return None;
        }
        for i in 0..tail.chars().count() {
            if col + i >= layout.cols {
                return None;
            }
            if row[Column(col + i)].c != ' ' {
                return None;
            }
        }
        Some((tail, col))
    }

    /// Start the shell and the reader thread feeding [`PtyMsg`]s into this session.
    pub fn spawn_shell(
        &mut self,
        shell: &ShellCfg,
        cols: u16,
        rows: u16,
        cwd: Option<PathBuf>,
        proxy: crate::app::EventLoopProxy,
    ) -> Result<()> {
        let mut env = vec![
            ("TERM".to_string(), "xterm-256color".to_string()),
            ("COLORTERM".to_string(), "truecolor".to_string()),
            ("RTERM".to_string(), "1".to_string()),
        ];
        // Let `rterm play` find this instance's streaming socket.
        if let Ok(sock) = std::env::var(crate::socket::ENV) {
            env.push((crate::socket::ENV.to_string(), sock));
        }
        let spawned = pty::spawn(shell, cols, rows, &env, cwd)?;
        self.pid = spawned.pid;

        let tx = self.pty_tx.clone();
        let mut reader = spawned.reader;
        thread::Builder::new()
            .name(format!("rterm-pty-{}", self.id))
            .spawn(move || {
                let mut buf = vec![0u8; 64 * 1024];
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => {
                            if tx.send(PtyMsg::Data(buf[..n].to_vec())).is_err() {
                                return;
                            }
                            let _ = proxy.send_event(crate::app::UserEvent::Wake);
                        }
                        Err(err) if err.kind() == std::io::ErrorKind::Interrupted => continue,
                        Err(_) => break,
                    }
                }
                let _ = tx.send(PtyMsg::Eof);
                let _ = proxy.send_event(crate::app::UserEvent::Wake);
            })
            .context("spawn pty reader")?;

        self.pty = Some(spawned.pty);
        self.killer = Some(spawned.killer);
        Ok(())
    }

    /// Terminal-side clear (`Ctrl+Shift+K`): viewport, scrollback and overlays.
    pub fn clear_all(&mut self) {
        self.term.scroll_display(Scroll::Bottom);
        // CUP home, erase display, erase scrollback — what `clear` emits.
        for &b in b"\x1b[H\x1b[2J\x1b[3J" {
            self.parser.advance(&mut self.term, b);
        }
        self.media.clear();
        self.videos.clear();
    }

    pub fn shutdown(&mut self) {
        if let Some(mut killer) = self.killer.take() {
            let _ = killer.kill();
        }
        self.pty = None;
    }

    pub fn resize(&mut self, cols: u16, rows: u16, size: SizeInfo) {
        self.term.resize(size);
        if let Some(pty) = self.pty.as_mut() {
            let _ = pty.resize(cols, rows);
        }
    }

    pub fn pty_write(&mut self, data: &[u8]) {
        if crate::app::tracing() {
            eprintln!("[pty{}<] {:?}", self.id, String::from_utf8_lossy(data));
        }
        if let Some(pty) = self.pty.as_mut() {
            if let Err(err) = pty.write(data) {
                eprintln!("rterm: pty write failed: {err}");
            }
        }
    }

    /// Everything the protocol layer needs to answer/position media.
    pub fn ctx(&self, layout: &Layout) -> Ctx {
        let offset = self.term.grid().display_offset() as i32;
        let cursor = self.term.renderable_content().cursor.point;
        let col = (cursor.column.0).clamp(0, layout.cols.saturating_sub(1));
        let row = (cursor.line.0 + offset).clamp(0, layout.rows.saturating_sub(1) as i32);
        let kitty_keys = self
            .media
            .iter()
            .filter_map(|item| match &item.source {
                Source::Kitty(key) => Some(key.clone()),
                _ => None,
            })
            .collect();
        Ctx {
            col,
            row: row as usize,
            kitty_keys,
            sid: self.id,
        }
    }

    /// Feed raw PTY bytes: media sequences are split off, the rest is parsed.
    pub fn feed(&mut self, bytes: &[u8], layout: &Layout, pipeline: &mut Pipeline) {
        if bytes.is_empty() {
            return;
        }
        if crate::app::tracing() {
            eprintln!("[pty{}>] {:?}", self.id, String::from_utf8_lossy(bytes));
        }
        let mut plain = Vec::with_capacity(bytes.len());
        let mut captured = Vec::new();
        self.interceptor
            .feed_marked(bytes, &mut plain, &mut captured);

        // Replay the original interleaving so media lands at the cursor
        // position it was emitted at, not after the trailing text.
        let mut taken = 0;
        for (at, cap) in captured {
            let at = at.min(plain.len());
            self.feed_parsed(&plain[taken..at]);
            taken = at;
            self.dispatch(cap, layout, pipeline);
        }
        self.feed_parsed(&plain[taken..]);
    }

    pub fn feed_parsed(&mut self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        for &byte in bytes {
            self.parser.advance(&mut self.term, byte);
        }
    }

    pub fn dispatch(&mut self, cap: Captured, layout: &Layout, pipeline: &mut Pipeline) {
        let ctx = self.ctx(layout);
        for effect in self.protocol.handle(cap, &ctx) {
            self.apply(effect, layout, pipeline);
        }
    }

    pub fn apply(&mut self, effect: Effect, layout: &Layout, pipeline: &mut Pipeline) {
        match effect {
            Effect::Job(job) => pipeline.submit(job),
            Effect::Pty(seq) => self.pty_write(seq.as_bytes()),
            Effect::Print { key, place } => match self.media.find_kitty(&key) {
                Some(id) => {
                    self.media
                        .place(id, place.col, place.row, place.cols, place.rows, place.z);
                }
                None => self.protocol.defer_print(key, place),
            },
            Effect::DeleteKey(key) => {
                let wanted = key;
                self.media
                    .remove_by_source(|src| matches!(src, Source::Kitty(k) if *k == wanted));
            }
            Effect::DeleteAt { col, row } => {
                let hits: Vec<u64> = self
                    .media
                    .iter()
                    .filter(|item| cell_rect_contains(item, col, row, layout))
                    .map(|item| item.id)
                    .collect();
                for id in hits {
                    self.media.remove(id);
                }
            }
            Effect::ClearMedia => {
                self.media.clear();
                self.videos.clear();
            }
            Effect::VideoClose { id } => self.close_video(id),
        }
    }

    /// Create (or reuse) a video overlay keyed by the protocol id.
    #[allow(clippy::too_many_arguments)]
    pub fn open_video(
        &mut self,
        key: u64,
        w: u32,
        h: u32,
        cols: usize,
        rows: usize,
        col: usize,
        row: usize,
    ) {
        let id = match self.videos.get(&key) {
            Some(id) if self.media.get(*id).is_some() => *id,
            _ => {
                let id = self.media.video_open(w, h, cols, rows, Source::Socket(key));
                self.videos.insert(key, id);
                id
            }
        };
        self.media.place(id, col, row, cols, rows, 0);
    }

    /// Push one decoded frame into the overlay; `true` when pixels changed.
    pub fn push_video_frame(&mut self, key: u64, rgba: Vec<u8>) -> bool {
        let Some(&id) = self.videos.get(&key) else {
            return false;
        };
        self.media.video_frame(id, rgba)
    }

    /// Remove the overlay for `key` (protocol id).
    pub fn close_video(&mut self, key: u64) {
        self.videos.remove(&key);
        self.media
            .remove_by_source(|src| matches!(src, Source::Socket(id) if *id == key));
    }

    /// Pull everything the reader thread has queued; `eof` means the shell exited.
    pub fn drain_pty(&mut self, layout: &Layout, pipeline: &mut Pipeline) -> Drain {
        let mut out = Drain::default();
        while let Ok(msg) = self.pty_rx.try_recv() {
            match msg {
                PtyMsg::Data(bytes) => {
                    self.feed(&bytes, layout, pipeline);
                    out.fed = true;
                }
                PtyMsg::Eof => {
                    let mut tail = Vec::new();
                    self.interceptor.flush_all(&mut tail);
                    self.feed_parsed(&tail);
                    out.eof = true;
                }
            }
        }
        out
    }

    /// Finish an escape sequence left hanging across an idle period.
    pub fn flush_stale(&mut self) {
        if !self.interceptor.in_sequence() {
            return;
        }
        let mut flushed = Vec::new();
        self.interceptor.flush_stale(&mut flushed);
        if !flushed.is_empty() {
            self.feed_parsed(&flushed);
        }
    }

    pub fn point_at(&self, col: usize, row: usize, _layout: &Layout) -> Point {
        let offset = self.term.grid().display_offset() as i32;
        Point {
            line: Line(row as i32 - offset),
            column: Column(col),
        }
    }

    pub fn side_at(&self, col: usize, px: f64, layout: &Layout) -> Side {
        let left = f64::from(layout.cell_x(col));
        if px >= left + f64::from(layout.cell_w) / 2.0 {
            Side::Right
        } else {
            Side::Left
        }
    }

    /// Store a decoded payload; returns `true` when the content changed.
    pub fn on_media_done(&mut self, done: protocol::Done) {
        let meta = done.meta;
        match done.result {
            Ok(content) => {
                if let Some(protocol_id) = meta.video {
                    self.store_video(protocol_id, &meta, content);
                } else {
                    let id = match content {
                        Content::Static(image) => {
                            self.media.insert_static(image, meta.source.clone())
                        }
                        Content::Anim(frames) => {
                            self.media.insert_anim(frames, meta.source.clone())
                        }
                    };
                    if let Some(place) = meta.place {
                        self.media
                            .place(id, place.col, place.row, place.cols, place.rows, place.z);
                    }
                    if let Source::Kitty(key) = &meta.source {
                        if let Some(place) = self.protocol.take_print(key) {
                            self.media
                                .place(id, place.col, place.row, place.cols, place.rows, place.z);
                        }
                    }
                }
                if let Some(reply) = &meta.reply {
                    self.pty_write(reply.as_bytes());
                }
            }
            Err(err) => {
                if let Source::Kitty(key) = &meta.source {
                    let reply = protocol::err_reply(key, "EDECODE", &err);
                    self.pty_write(reply.as_bytes());
                } else {
                    eprintln!("rterm: media decode failed: {err}");
                }
            }
        }
    }

    fn store_video(&mut self, protocol_id: u64, meta: &Meta, content: Content) {
        let image = match content {
            Content::Static(image) => image,
            Content::Anim(frames) => match frames.into_iter().next() {
                Some(frame) => frame.image,
                None => return,
            },
        };
        let place = meta.place;
        let cols = place.map_or(0, |p| p.cols);
        let rows = place.map_or(0, |p| p.rows);

        let id = match self.videos.get(&protocol_id) {
            Some(id) if self.media.get(*id).is_some() => *id,
            _ => {
                let id = self
                    .media
                    .video_open(image.w, image.h, cols, rows, meta.source.clone());
                self.videos.insert(protocol_id, id);
                id
            }
        };
        let (pw, ph) = self
            .media
            .get(id)
            .map(|item| (item.px_w, item.px_h))
            .unwrap_or((image.w, image.h));
        let frame = if (pw, ph) == (image.w, image.h) {
            image
        } else {
            scale_nearest(&image, pw, ph)
        };
        if let Some(p) = place {
            self.media.place(id, p.col, p.row, p.cols, p.rows, p.z);
        }
        self.media.video_frame_arc(id, frame.rgba);
    }
}

/// Does the item's cell rectangle (derived size included) cover this cell?
pub fn cell_rect_contains(
    item: &crate::media::MediaItem,
    col: usize,
    row: usize,
    layout: &Layout,
) -> bool {
    let cols = if item.cols > 0 {
        item.cols
    } else {
        ((f64::from(item.px_w) / f64::from(layout.cell_w.max(1.0))).ceil() as usize).max(1)
    };
    let rows = if item.rows > 0 {
        item.rows
    } else {
        ((f64::from(item.px_h) / f64::from(layout.cell_h.max(1.0))).ceil() as usize).max(1)
    };
    col >= item.col && col < item.col + cols && row >= item.row && row < item.row + rows
}
