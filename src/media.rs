//! Media registry: still images, animated GIFs and video overlays with a
//! memory budget and frame scheduling.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::image_decode::{Frame, Image};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    Kitty(String),
    Iterm,
    Socket(u64),
}

#[derive(Debug, Clone)]
pub enum Kind {
    Static(Image),
    Anim {
        frames: Vec<Frame>,
        idx: usize,
        elapsed: Duration,
        playing: bool,
    },
    /// Live frames pushed over the socket / OSC 1338.
    Video {
        current: Arc<Vec<u8>>,
    },
}

pub struct MediaItem {
    pub id: u64,
    pub kind: Kind,
    pub source: Source,
    /// Anchor in viewport cells (col, row).
    pub col: usize,
    pub row: usize,
    /// Size in cells (0 → derived from pixels at render time).
    pub cols: usize,
    pub rows: usize,
    pub px_w: u32,
    pub px_h: u32,
    pub z: i32,
    pub created: Instant,
    bytes: usize,
}

impl MediaItem {
    pub fn static_image(&self) -> Option<&Image> {
        match &self.kind {
            Kind::Static(img) => Some(img),
            Kind::Anim { frames, idx, .. } => Some(&frames[*idx].image),
            Kind::Video { .. } => None,
        }
    }

    pub fn video_frame(&self) -> Option<&Arc<Vec<u8>>> {
        match &self.kind {
            Kind::Video { current } => Some(current),
            _ => None,
        }
    }
}

pub struct MediaStore {
    items: BTreeMap<u64, MediaItem>,
    next_id: u64,
    bytes: usize,
    max_bytes: usize,
    pub loop_gifs: bool,
}

impl MediaStore {
    pub fn new(max_mb: u64, loop_gifs: bool) -> Self {
        Self {
            items: BTreeMap::new(),
            next_id: 1,
            bytes: 0,
            max_bytes: (max_mb as usize) * 1024 * 1024,
            loop_gifs,
        }
    }

    pub fn set_budget(&mut self, max_mb: u64) {
        self.max_bytes = (max_mb as usize) * 1024 * 1024;
        self.evict();
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn bytes_used(&self) -> usize {
        self.bytes
    }

    pub fn get(&self, id: u64) -> Option<&MediaItem> {
        self.items.get(&id)
    }

    pub fn get_mut(&mut self, id: u64) -> Option<&mut MediaItem> {
        self.items.get_mut(&id)
    }

    pub fn iter(&self) -> impl Iterator<Item = &MediaItem> {
        self.items.values()
    }

    fn make(&mut self, kind: Kind, source: Source, bytes: usize, w: u32, h: u32) -> MediaItem {
        let id = self.next_id;
        self.next_id += 1;
        MediaItem {
            id,
            kind,
            source,
            col: 0,
            row: 0,
            cols: 0,
            rows: 0,
            px_w: w,
            px_h: h,
            z: 0,
            created: Instant::now(),
            bytes,
        }
    }

    fn track(&mut self, item: MediaItem) -> u64 {
        self.bytes += item.bytes;
        let id = item.id;
        self.items.insert(id, item);
        self.evict();
        id
    }

    /// Drop least-recently-used items until we fit the budget (never the newest one).
    fn evict(&mut self) {
        while self.bytes > self.max_bytes && self.items.len() > 1 {
            let oldest = self
                .items
                .iter()
                .min_by_key(|(_, it)| it.created)
                .map(|(id, _)| *id);
            match oldest {
                Some(id) => {
                    if let Some(it) = self.items.remove(&id) {
                        self.bytes -= it.bytes;
                    }
                }
                None => break,
            }
        }
    }

    pub fn insert_static(&mut self, img: Image, source: Source) -> u64 {
        let (bytes, w, h) = (img.bytes(), img.w, img.h);
        let item = self.make(Kind::Static(img), source, bytes, w, h);
        self.track(item)
    }

    pub fn insert_anim(&mut self, frames: Vec<Frame>, source: Source) -> u64 {
        let bytes: usize = frames.iter().map(|f| f.image.bytes()).sum();
        let (w, h) = frames
            .first()
            .map(|f| (f.image.w, f.image.h))
            .unwrap_or((0, 0));
        let kind = Kind::Anim {
            frames,
            idx: 0,
            elapsed: Duration::ZERO,
            playing: true,
        };
        let item = self.make(kind, source, bytes, w, h);
        self.track(item)
    }

    /// Store/replacement a keyed kitty image without placing it yet.
    pub fn put_kitty_image(&mut self, key: &str, img: Image) -> u64 {
        let old = self.find_kitty(key);
        if let Some(id) = old {
            self.remove(id);
        }
        self.insert_static(img, Source::Kitty(key.to_string()))
    }

    pub fn find_kitty(&self, key: &str) -> Option<u64> {
        self.items
            .iter()
            .find(|(_, it)| it.source == Source::Kitty(key.to_string()))
            .map(|(id, _)| *id)
    }

    pub fn place(&mut self, id: u64, col: usize, row: usize, cols: usize, rows: usize, z: i32) {
        if let Some(it) = self.items.get_mut(&id) {
            it.col = col;
            it.row = row;
            it.cols = cols;
            it.rows = rows;
            it.z = z;
            it.created = Instant::now(); // placement refreshes LRU
        }
    }

    pub fn remove(&mut self, id: u64) {
        if let Some(it) = self.items.remove(&id) {
            self.bytes = self.bytes.saturating_sub(it.bytes);
        }
    }

    pub fn remove_by_source(&mut self, pred: impl Fn(&Source) -> bool) {
        let ids: Vec<u64> = self
            .items
            .iter()
            .filter(|(_, it)| pred(&it.source))
            .map(|(id, _)| *id)
            .collect();
        for id in ids {
            self.remove(id);
        }
    }

    pub fn clear(&mut self) {
        self.items.clear();
        self.bytes = 0;
    }

    /// Create a video overlay; returns its id.
    pub fn video_open(
        &mut self,
        px_w: u32,
        px_h: u32,
        cols: usize,
        rows: usize,
        source: Source,
    ) -> u64 {
        let bytes = (px_w as usize) * (px_h as usize) * 4;
        let kind = Kind::Video {
            current: Arc::new(Vec::new()),
        };
        let mut item = self.make(kind, source, bytes, px_w, px_h);
        item.bytes = 0; // counted when frames arrive
        item.cols = cols;
        item.rows = rows;
        self.track(item)
    }

    /// Push a frame into a video overlay. Returns true when something changed.
    pub fn video_frame(&mut self, id: u64, rgba: Vec<u8>) -> bool {
        self.video_frame_arc(id, Arc::new(rgba))
    }

    /// Same as [`Self::video_frame`] but keeps the buffer shared with the decoder.
    pub fn video_frame_arc(&mut self, id: u64, rgba: Arc<Vec<u8>>) -> bool {
        let Some(item) = self.items.get_mut(&id) else {
            return false;
        };
        let new_bytes = rgba.len();
        if let Kind::Video { current } = &mut item.kind {
            self.bytes = self.bytes.saturating_sub(item.bytes);
            item.bytes = new_bytes;
            self.bytes += new_bytes;
            *current = rgba;
            item.created = Instant::now();
            true
        } else {
            false
        }
    }

    /// Advance GIF playback; returns true if a visible frame changed.
    pub fn advance(&mut self, now: Instant) -> bool {
        let mut changed = false;
        for item in self.items.values_mut() {
            if let Kind::Anim {
                frames,
                idx,
                elapsed,
                playing,
            } = &mut item.kind
            {
                if !*playing || frames.len() < 2 {
                    continue;
                }
                let dt = now.duration_since(item.created);
                item.created = now;
                *elapsed += dt;
                let n = frames.len();
                let mut guard = 0;
                while *elapsed >= frames[*idx].delay && guard < n * 2 {
                    *elapsed -= frames[*idx].delay;
                    *idx += 1;
                    if *idx >= n {
                        if self.loop_gifs {
                            *idx = 0;
                        } else {
                            *idx = n - 1;
                            *playing = false;
                            break;
                        }
                    }
                    changed = true;
                    guard += 1;
                }
            }
        }
        changed
    }

    /// Next moment a GIF frame flips, for redraw scheduling.
    pub fn next_deadline(&self, now: Instant) -> Option<Instant> {
        let mut best: Option<Duration> = None;
        for item in self.items.values() {
            if let Kind::Anim {
                frames,
                idx,
                elapsed,
                playing,
            } = &item.kind
            {
                if *playing && frames.len() > 1 {
                    let remaining = frames[*idx].delay.saturating_sub(*elapsed);
                    best = Some(best.map_or(remaining, |b: Duration| b.min(remaining)));
                }
            }
        }
        best.map(|d| now + d.max(Duration::from_millis(5)))
    }

    pub fn any_animating(&self) -> bool {
        self.items.values().any(|it| {
            matches!(
                it.kind,
                Kind::Anim { ref frames, playing: true, .. } if frames.len() > 1
            )
        }) || self
            .items
            .values()
            .any(|it| matches!(it.kind, Kind::Video { .. }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn img(w: u32, h: u32) -> Image {
        Image::from_rgba(w, h, vec![0u8; (w * h * 4) as usize])
    }

    fn frame(delay_ms: u64) -> Frame {
        Frame {
            image: img(2, 2),
            delay: Duration::from_millis(delay_ms),
        }
    }

    #[test]
    fn budget_evicts_oldest() {
        let mut store = MediaStore::new(16, true);
        // Each 64x64 RGBA image = 16 KiB; budget 16 MiB → ~1024 images.
        for _ in 0..1100 {
            store.insert_static(img(64, 64), Source::Iterm);
        }
        assert!(store.bytes_used() <= 16 * 1024 * 1024);
        assert!(store.len() < 1100);
        assert!(store.len() > 900);
    }

    #[test]
    fn gif_advances_and_wraps() {
        let mut store = MediaStore::new(16, true);
        store.insert_anim(vec![frame(10), frame(10)], Source::Iterm);
        let now = Instant::now();
        let id = 1;
        let Kind::Anim { idx, .. } = &store.get(id).unwrap().kind else {
            panic!()
        };
        assert_eq!(*idx, 0);
        // Jump time forward by mutating created.
        store.get_mut(id).unwrap().created = now - Duration::from_millis(15);
        assert!(store.advance(now));
        let Kind::Anim { idx, .. } = &store.get(id).unwrap().kind else {
            panic!()
        };
        assert_eq!(*idx, 1); // 15ms → one 10ms frame consumed
    }

    #[test]
    fn gif_no_loop_stops_at_last() {
        let mut store = MediaStore::new(16, false);
        store.insert_anim(vec![frame(10), frame(10)], Source::Iterm);
        let now = Instant::now();
        store.get_mut(1).unwrap().created = now - Duration::from_millis(1000);
        store.advance(now);
        let Kind::Anim { idx, playing, .. } = &store.get(1).unwrap().kind else {
            panic!()
        };
        assert_eq!(*idx, 1);
        assert!(!playing);
    }

    #[test]
    fn video_replaces_frames() {
        let mut store = MediaStore::new(16, true);
        let id = store.video_open(4, 4, 2, 2, Source::Socket(1));
        assert!(store.video_frame(id, vec![7u8; 64]));
        assert_eq!(store.get(id).unwrap().video_frame().unwrap().len(), 64);
        assert!(store.video_frame(id, vec![9u8; 64]));
        assert_eq!(store.get(id).unwrap().video_frame().unwrap()[0], 9);
        assert_eq!(store.bytes_used(), 64);
        store.remove(id);
        assert_eq!(store.bytes_used(), 0);
    }

    #[test]
    fn kitty_key_replacement_does_not_leak() {
        let mut store = MediaStore::new(16, true);
        store.put_kitty_image("1", img(32, 32));
        store.put_kitty_image("1", img(32, 32));
        assert_eq!(store.len(), 1);
        assert_eq!(store.bytes_used(), 32 * 32 * 4);
    }

    #[test]
    fn deadline_scheduled_for_next_frame() {
        let mut store = MediaStore::new(16, true);
        store.insert_anim(vec![frame(50), frame(50)], Source::Iterm);
        let now = Instant::now();
        let dl = store.next_deadline(now).unwrap();
        assert!(dl <= now + Duration::from_millis(50));
    }
}
