//! CPU renderer: background effects → text cells → media → cursor → post effects.
//!
//! Everything draws into a `Vec<u32>` (0x00RRGGBB) that is presented by
//! `softbuffer`. No GPU required; a full frame at 1080p costs a few ms.

use std::time::Duration;

use alacritty_terminal::ansi::{Color, CursorShape, NamedColor};
use alacritty_terminal::term::cell::{Cell, Flags};
use alacritty_terminal::term::RenderableContent;

use crate::config::Config;
use crate::font::{FontSystem, GlyphBitmap};
use crate::image_decode::{decode_any, Image};
use crate::media::{Kind, MediaItem, MediaStore};
use crate::palette::{self, Palette, Rgb8};

/// Per-row entrance effect for terminal cells. `FULL` means "already settled".
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RowFx {
    pub alpha: f32,
    pub dx: i32,
    pub dy: i32,
    /// Left-to-right reveal for `wipe` / `type` (1.0 = whole row).
    pub reveal: f32,
    /// Per-cell random reveal for `dissolve` (0.0 = off, 1.0 = whole row).
    pub dissolve: f32,
    /// Extra brightness while the row appears (0.0 = none).
    pub flash: f32,
}

impl RowFx {
    pub const FULL: RowFx = RowFx {
        alpha: 1.0,
        dx: 0,
        dy: 0,
        reveal: 1.0,
        dissolve: 0.0,
        flash: 0.0,
    };
}

impl Default for RowFx {
    fn default() -> Self {
        Self::FULL
    }
}

/// Grid geometry shared by input handling, media and rendering.
#[derive(Debug, Clone, Copy)]
pub struct Layout {
    pub origin_x: i32,
    pub origin_y: i32,
    pub cell_w: f32,
    pub cell_h: f32,
    pub cols: usize,
    pub rows: usize,
}

impl Layout {
    pub fn compute(
        w: u32,
        h: u32,
        metrics: &crate::font::CellMetrics,
        pad_x: u32,
        pad_y: u32,
    ) -> Self {
        let avail_w = (w as f32 - 2.0 * pad_x as f32).max(metrics.cell_w);
        let avail_h = (h as f32 - 2.0 * pad_y as f32).max(metrics.cell_h);
        let cols = ((avail_w / metrics.cell_w).floor() as usize).max(1);
        let rows = ((avail_h / metrics.cell_h).floor() as usize).max(1);
        let content_w = cols as f32 * metrics.cell_w;
        let content_h = rows as f32 * metrics.cell_h;
        Self {
            origin_x: ((w as f32 - content_w) / 2.0).round() as i32,
            origin_y: ((h as f32 - content_h) / 2.0).round() as i32,
            cell_w: metrics.cell_w,
            cell_h: metrics.cell_h,
            cols,
            rows,
        }
    }

    /// Padding value that makes alacritty's `SizeInfo` produce the same cols/rows.
    pub fn term_pad_x(&self) -> f32 {
        (self.cols as f32 * self.cell_w) / 2.0 - self.cell_w * 0.25
    }

    pub fn term_pad_y(&self) -> f32 {
        (self.rows as f32 * self.cell_h) / 2.0 - self.cell_h * 0.25
    }

    pub fn cell_at(&self, x: i32, y: i32) -> Option<(usize, usize)> {
        let col = (x - self.origin_x).div_euclid(self.cell_w as i32);
        let row = (y - self.origin_y).div_euclid(self.cell_h as i32);
        if col < 0 || row < 0 || col as usize >= self.cols || row as usize >= self.rows {
            return None;
        }
        Some((col as usize, row as usize))
    }

    pub fn cell_x(&self, col: usize) -> i32 {
        self.origin_x + (col as f32 * self.cell_w) as i32
    }

    pub fn cell_y(&self, row: usize) -> i32 {
        self.origin_y + (row as f32 * self.cell_h) as i32
    }
}

struct ImageCache {
    path: String,
    w: u32,
    h: u32,
    blur: u32,
    fit: String,
    rgba: Vec<u8>,
}

pub struct Renderer {
    pub fb: Vec<u32>,
    pub w: usize,
    pub h: usize,
    image_cache: Option<ImageCache>,
    plasma_scratch: Vec<u8>,
    /// Low-res bloom pyramid (rgb bytes), plus its blur scratch.
    bloom_a: Vec<u8>,
    bloom_tmp: Vec<u8>,
}

impl Default for Renderer {
    fn default() -> Self {
        Self::new()
    }
}

impl Renderer {
    pub fn new() -> Self {
        Self {
            fb: Vec::new(),
            w: 0,
            h: 0,
            image_cache: None,
            plasma_scratch: Vec::new(),
            bloom_a: Vec::new(),
            bloom_tmp: Vec::new(),
        }
    }

    pub fn ensure_size(&mut self, w: usize, h: usize) {
        if self.w != w || self.h != h {
            self.w = w;
            self.h = h;
            self.fb.clear();
            self.fb.resize(w * h, 0);
            self.image_cache = None;
        }
    }

    pub fn invalidate_bg(&mut self) {
        self.image_cache = None;
    }

    #[inline]
    fn put(&mut self, x: i32, y: i32, c: Rgb8) {
        if x < 0 || y < 0 || x as usize >= self.w || y as usize >= self.h {
            return;
        }
        let i = y as usize * self.w + x as usize;
        self.fb[i] = u32::from_be_bytes([0, c[0], c[1], c[2]]);
    }

    #[inline]
    fn put_blend(&mut self, x: i32, y: i32, c: Rgb8, a: u8) {
        if a == 0 || x < 0 || y < 0 || x as usize >= self.w || y as usize >= self.h {
            return;
        }
        let i = y as usize * self.w + x as usize;
        self.fb[i] = blend_u32(self.fb[i], c, a);
    }

    fn fill_rect(&mut self, x: i32, y: i32, rw: i32, rh: i32, c: Rgb8) {
        let x0 = x.max(0);
        let y0 = y.max(0);
        let x1 = (x + rw).min(self.w as i32);
        let y1 = (y + rh).min(self.h as i32);
        let packed = u32::from_be_bytes([0, c[0], c[1], c[2]]);
        for yy in y0..y1 {
            let row = yy as usize * self.w;
            for xx in x0..x1 {
                self.fb[row + xx as usize] = packed;
            }
        }
    }

    fn draw_gradient(&mut self, cfg: &Config, elapsed: Duration, opacity: f32) {
        let stops: Vec<Rgb8> = cfg
            .effects
            .gradient_colors
            .iter()
            .map(|s| palette::parse_hex_or(s, [0x20, 0x20, 0x20]))
            .collect();
        let phase = if cfg.effects.gradient_animated {
            elapsed.as_secs_f32() * cfg.effects.gradient_speed
        } else {
            0.0
        };
        // Per-frame 256-entry LUT along the gradient axis.
        let mut lut = [[0u8; 3]; 256];
        let n = stops.len();
        for (i, slot) in lut.iter_mut().enumerate() {
            let mut t = i as f32 / 255.0 + phase;
            t = t - t.floor();
            let pos = t * n as f32;
            let idx = (pos as usize).min(n - 1);
            let frac = pos - idx as f32;
            let a = stops[idx];
            let b = stops[(idx + 1) % n];
            *slot = mix_rgb(a, b, frac);
        }

        let rad = cfg.effects.gradient_angle.to_radians();
        let (sin_a, cos_a) = (rad.sin(), rad.cos());
        let w = self.w as f32;
        let h = self.h as f32;
        let range = (w * cos_a.abs() + h * sin_a.abs()).max(1.0);
        let cx = w / 2.0;
        let cy = h / 2.0;
        let op = u32::from((opacity * 255.0) as u8);

        for y in 0..self.h {
            let dy = y as f32 - cy;
            let row = y * self.w;
            for x in 0..self.w {
                let dx = x as f32 - cx;
                let t = ((dx * cos_a + dy * sin_a) / range + 0.5).clamp(0.0, 1.0);
                let c = lut[(t * 255.0) as usize];
                self.fb[row + x] = u32::from_be_bytes([
                    0,
                    (u32::from(c[0]) * op / 255) as u8,
                    (u32::from(c[1]) * op / 255) as u8,
                    (u32::from(c[2]) * op / 255) as u8,
                ]);
            }
        }
    }

    fn draw_plasma(&mut self, cfg: &Config, elapsed: Duration, opacity: f32) {
        let c0 = palette::parse_hex_or(
            cfg.effects
                .plasma_colors
                .first()
                .map(String::as_str)
                .unwrap_or("#000000"),
            [0, 0, 0],
        );
        let c1 = palette::parse_hex_or(
            cfg.effects
                .plasma_colors
                .get(1)
                .map(String::as_str)
                .unwrap_or("#ffffff"),
            [255, 255, 255],
        );
        const DS: usize = 6; // downscale factor: keeps plasma cheap even at 4K
        let pw = self.w.div_ceil(DS).max(2);
        let ph = self.h.div_ceil(DS).max(2);
        if self.plasma_scratch.len() != pw * ph * 3 {
            self.plasma_scratch.resize(pw * ph * 3, 0);
        }
        let t = elapsed.as_secs_f32() * cfg.effects.plasma_speed;
        let scale = cfg.effects.plasma_scale;
        let aspect = pw as f32 / ph as f32;

        for py in 0..ph {
            let v = py as f32 / ph as f32 * 6.0 * scale;
            for px in 0..pw {
                let u = px as f32 / pw as f32 * 6.0 * scale * aspect;
                let val = (u + t).sin()
                    + (v * 1.3 - t * 0.7).sin()
                    + ((u + v) * 0.7 + t * 0.5).sin()
                    + (((u - 3.0 * aspect).powi(2) + (v - 3.0).powi(2)).sqrt() * 1.7 - t * 1.3)
                        .sin();
                let t01 = (val * 0.125 + 0.5).clamp(0.0, 1.0);
                let c = mix_rgb(c0, c1, t01);
                let i = (py * pw + px) * 3;
                self.plasma_scratch[i] = c[0];
                self.plasma_scratch[i + 1] = c[1];
                self.plasma_scratch[i + 2] = c[2];
            }
        }

        self.upscale_scratch(pw, ph, opacity);
    }

    /// Bilinear upscale of `plasma_scratch` (pw×ph×3) over the frame.
    fn upscale_scratch(&mut self, pw: usize, ph: usize, opacity: f32) {
        if self.w == 0 || self.h == 0 || pw == 0 || ph == 0 {
            return;
        }
        let sxr = (pw - 1) as f32 / (self.w - 1).max(1) as f32;
        let syr = (ph - 1) as f32 / (self.h - 1).max(1) as f32;
        let op = u32::from((opacity * 255.0) as u8);
        for y in 0..self.h {
            let fy = y as f32 * syr;
            let y0 = fy as usize;
            let y1 = (y0 + 1).min(ph - 1);
            let wy = fy - y0 as f32;
            let row = y * self.w;
            for x in 0..self.w {
                let fx = x as f32 * sxr;
                let x0 = fx as usize;
                let x1 = (x0 + 1).min(pw - 1);
                let wx = fx - x0 as f32;
                let mut c = [0u8; 3];
                for ch in 0..3 {
                    let p00 = self.plasma_scratch[(y0 * pw + x0) * 3 + ch];
                    let p10 = self.plasma_scratch[(y0 * pw + x1) * 3 + ch];
                    let p01 = self.plasma_scratch[(y1 * pw + x0) * 3 + ch];
                    let p11 = self.plasma_scratch[(y1 * pw + x1) * 3 + ch];
                    let top = p00 as f32 + (p10 as f32 - p00 as f32) * wx;
                    let bot = p01 as f32 + (p11 as f32 - p01 as f32) * wx;
                    c[ch] = (top + (bot - top) * wy) as u8;
                }
                self.fb[row + x] = u32::from_be_bytes([
                    0,
                    (u32::from(c[0]) * op / 255) as u8,
                    (u32::from(c[1]) * op / 255) as u8,
                    (u32::from(c[2]) * op / 255) as u8,
                ]);
            }
        }
    }

    /// Drifting soft blobs over the base color — the "aurora" background.
    fn draw_aurora(&mut self, cfg: &Config, elapsed: Duration, opacity: f32) {
        let base = palette::parse_hex_or(&cfg.colors.background, [11, 16, 38]);
        let colors: Vec<Rgb8> = cfg
            .effects
            .aurora_colors
            .iter()
            .map(|s| palette::parse_hex_or(s, [0x7a, 0xa2, 0xf7]))
            .collect();
        if colors.is_empty() || self.w == 0 || self.h == 0 {
            let c = scale_rgb(base, opacity);
            self.fb.fill(pack(c));
            return;
        }

        const DS: usize = 4;
        let pw = self.w.div_ceil(DS).max(2);
        let ph = self.h.div_ceil(DS).max(2);
        if self.plasma_scratch.len() != pw * ph * 3 {
            self.plasma_scratch.resize(pw * ph * 3, 0);
        }

        let t = elapsed.as_secs_f32() * cfg.effects.aurora_speed;
        let aspect = pw as f32 / ph as f32;
        for py in 0..ph {
            let v = (py as f32 + 0.5) / ph as f32;
            for px in 0..pw {
                let u = (px as f32 + 0.5) / pw as f32;
                let mut acc = [0f32; 3];
                let mut glow = 0f32;
                for (i, color) in colors.iter().enumerate() {
                    let fi = i as f32;
                    let cx = 0.5 + 0.42 * (t * (0.13 + 0.05 * fi) + fi * 1.9).sin();
                    let cy = 0.5 + 0.36 * (t * (0.11 + 0.04 * fi) + fi * 2.7).cos();
                    let dx = (u - cx) * aspect;
                    let dy = v - cy;
                    let d = (dx * dx + dy * dy).sqrt();
                    let radius = 0.42 + 0.1 * (fi * 0.7).sin();
                    let mut f = 1.0 - d / radius;
                    if f <= 0.0 {
                        continue;
                    }
                    f = f * f * f;
                    glow += f;
                    for (slot, channel) in acc.iter_mut().zip(color.iter()) {
                        *slot += f * f32::from(*channel);
                    }
                }
                let k = (glow * 0.5).min(1.0);
                let i = (py * pw + px) * 3;
                for ch in 0..3 {
                    let value = f32::from(base[ch]) * (1.0 - k) + acc[ch] * 0.6;
                    self.plasma_scratch[i + ch] = value.min(255.0) as u8;
                }
            }
        }
        self.upscale_scratch(pw, ph, opacity);
    }

    fn draw_image_bg(&mut self, cfg: &Config, opacity: f32) {
        let base = palette::parse_hex_or(&cfg.colors.background, [16, 16, 16]);
        let Some(path) = cfg.effects.image.clone() else {
            let c = scale_rgb(base, opacity);
            self.fb.fill(u32::from_be_bytes([0, c[0], c[1], c[2]]));
            return;
        };
        let (w, h) = (self.w as u32, self.h as u32);
        let blur = cfg.effects.image_blur;
        let fit = cfg.effects.image_fit.clone();
        let need_load = match &self.image_cache {
            Some(c) => c.path != path || c.w != w || c.h != h || c.blur != blur || c.fit != fit,
            None => true,
        };
        if need_load {
            let rgba = match std::fs::read(&path)
                .ok()
                .and_then(|bytes| decode_any(&bytes).ok())
            {
                Some(crate::image_decode::Decoded::Static(img)) => {
                    fit_image(&img, w, h, &fit, blur)
                }
                _ => Vec::new(),
            };
            self.image_cache = Some(ImageCache {
                path,
                w,
                h,
                blur,
                fit,
                rgba,
            });
        }
        let cache = self.image_cache.as_ref().unwrap();
        if cache.rgba.is_empty() {
            let c = scale_rgb(base, opacity);
            self.fb.fill(u32::from_be_bytes([0, c[0], c[1], c[2]]));
            return;
        }
        let dim = cfg.effects.image_dim.clamp(0.05, 1.0);
        let mul = dim * opacity;
        for (i, px) in cache.rgba.chunks_exact(4).enumerate() {
            let c = if px[3] == 0 {
                scale_rgb(base, opacity)
            } else {
                scale_rgb([px[0], px[1], px[2]], mul * (f32::from(px[3]) / 255.0))
            };
            self.fb[i] = u32::from_be_bytes([0, c[0], c[1], c[2]]);
        }
    }
}

/// Free helpers -----------------------------------------------------------------
#[inline]
fn unpack(p: u32) -> (u8, u8, u8) {
    ((p >> 16) as u8, (p >> 8) as u8, p as u8)
}

#[inline]
fn pack(c: Rgb8) -> u32 {
    u32::from_be_bytes([0, c[0], c[1], c[2]])
}

#[inline]
fn blend_u32(dst: u32, c: Rgb8, a: u8) -> u32 {
    if a == 255 {
        return pack(c);
    }
    let (dr, dg, db) = unpack(dst);
    let ia = 255 - u32::from(a);
    let a = u32::from(a);
    let r = (u32::from(c[0]) * a + u32::from(dr) * ia) / 255;
    let g = (u32::from(c[1]) * a + u32::from(dg) * ia) / 255;
    let b = (u32::from(c[2]) * a + u32::from(db) * ia) / 255;
    u32::from_be_bytes([0, r as u8, g as u8, b as u8])
}

/// Integer hash for grain; same inputs always give the same speckle.
#[inline]
fn hash3(x: u32, y: u32, seed: u32) -> u32 {
    let mut h = x.wrapping_mul(374_761_393)
        ^ y.wrapping_mul(668_265_263)
        ^ seed.wrapping_mul(1_013_904_223);
    h = (h ^ (h >> 13)).wrapping_mul(1_274_126_177);
    h ^ (h >> 16)
}

#[inline]
fn mix_rgb(a: Rgb8, b: Rgb8, t: f32) -> Rgb8 {
    let t = t.clamp(0.0, 1.0);
    [
        (f32::from(a[0]) + (f32::from(b[0]) - f32::from(a[0])) * t) as u8,
        (f32::from(a[1]) + (f32::from(b[1]) - f32::from(a[1])) * t) as u8,
        (f32::from(a[2]) + (f32::from(b[2]) - f32::from(a[2])) * t) as u8,
    ]
}

#[inline]
/// Multiply every channel by `k` (window opacity, image dimming).
pub fn scale_rgb(c: Rgb8, k: f32) -> Rgb8 {
    let k = k.clamp(0.0, 1.0);
    [
        (f32::from(c[0]) * k) as u8,
        (f32::from(c[1]) * k) as u8,
        (f32::from(c[2]) * k) as u8,
    ]
}

/// Fit (cover/contain) + optional box blur of a background image into `w*h`.
fn fit_image(img: &Image, w: u32, h: u32, fit: &str, blur: u32) -> Vec<u8> {
    if w == 0 || h == 0 {
        return Vec::new();
    }
    let sw = img.w.max(1) as f32;
    let sh = img.h.max(1) as f32;
    let scale = if fit == "contain" {
        (w as f32 / sw).min(h as f32 / sh)
    } else {
        (w as f32 / sw).max(h as f32 / sh)
    };
    let vw = w as f32 / scale;
    let vh = h as f32 / scale;
    let sx0 = (sw - vw) / 2.0;
    let sy0 = (sh - vh) / 2.0;
    let mut out = vec![0u8; (w * h * 4) as usize];
    for y in 0..h {
        let sy = (sy0 + y as f32 / scale).clamp(0.0, sh - 1.0) as u32;
        for x in 0..w {
            let sx = (sx0 + x as f32 / scale).clamp(0.0, sw - 1.0) as u32;
            let si = ((sy * img.w + sx) * 4) as usize;
            let di = ((y * w + x) * 4) as usize;
            if si + 4 <= img.rgba.len() {
                out[di..di + 4].copy_from_slice(&img.rgba[si..si + 4]);
            }
        }
    }
    if blur > 0 {
        box_blur_rgba(&mut out, w, h, blur);
    }
    out
}

/// Two-pass separable box blur with edge replication (O(w*h) per pass).
fn box_blur_rgba(buf: &mut [u8], w: u32, h: u32, radius: u32) {
    let (w, h) = (w as usize, h as usize);
    if w == 0 || h == 0 {
        return;
    }
    let rx = (radius as usize).min(w - 1);
    let ry = (radius as usize).min(h - 1);
    let mut tmp = vec![0u8; buf.len()];

    // Horizontal pass.
    for y in 0..h {
        let row = y * w;
        for ch in 0..4 {
            let first = u32::from(buf[(row) * 4 + ch]);
            let mut sum = first * rx as u32;
            for i in 0..=rx {
                sum += u32::from(buf[(row + i) * 4 + ch]);
            }
            let count = (2 * rx + 1) as u32;
            for x in 0..w {
                tmp[(row + x) * 4 + ch] = (sum / count) as u8;
                let lo = x.saturating_sub(rx);
                sum -= u32::from(buf[(row + lo) * 4 + ch]);
                let hi = (x + rx + 1).min(w - 1);
                sum += u32::from(buf[(row + hi) * 4 + ch]);
            }
        }
    }

    // Vertical pass.
    for x in 0..w {
        for ch in 0..4 {
            let first = u32::from(tmp[x * 4 + ch]);
            let mut sum = first * ry as u32;
            for i in 0..=ry {
                sum += u32::from(tmp[(i * w + x) * 4 + ch]);
            }
            let count = (2 * ry + 1) as u32;
            for y in 0..h {
                buf[(y * w + x) * 4 + ch] = (sum / count) as u8;
                let lo = y.saturating_sub(ry);
                sum -= u32::from(tmp[(lo * w + x) * 4 + ch]);
                let hi = (y + ry + 1).min(h - 1);
                sum += u32::from(tmp[(hi * w + x) * 4 + ch]);
            }
        }
    }
}

/// Horizontal box blur over packed `0x00RRGGBB` pixels (row-major `w`).
fn blur_axis_h(src: &[u32], dst: &mut [u32], w: usize, h: usize, r: usize) {
    let count = (2 * r + 1) as u32;
    for y in 0..h {
        let row = y * w;
        let first = src[row];
        let mut sr = ((first >> 16) & 0xff) * r as u32;
        let mut sg = ((first >> 8) & 0xff) * r as u32;
        let mut sb = (first & 0xff) * r as u32;
        for i in 0..=r {
            let p = src[row + i];
            sr += (p >> 16) & 0xff;
            sg += (p >> 8) & 0xff;
            sb += p & 0xff;
        }
        for x in 0..w {
            dst[row + x] = pack([(sr / count) as u8, (sg / count) as u8, (sb / count) as u8]);
            let lo = x.saturating_sub(r);
            let p = src[row + lo];
            sr -= (p >> 16) & 0xff;
            sg -= (p >> 8) & 0xff;
            sb -= p & 0xff;
            let hi = (x + r + 1).min(w - 1);
            let p = src[row + hi];
            sr += (p >> 16) & 0xff;
            sg += (p >> 8) & 0xff;
            sb += p & 0xff;
        }
    }
}

/// Vertical box blur over packed `0x00RRGGBB` pixels (row-major `w`).
fn blur_axis_v(src: &[u32], dst: &mut [u32], w: usize, h: usize, r: usize) {
    let count = (2 * r + 1) as u32;
    for x in 0..w {
        let first = src[x];
        let mut sr = ((first >> 16) & 0xff) * r as u32;
        let mut sg = ((first >> 8) & 0xff) * r as u32;
        let mut sb = (first & 0xff) * r as u32;
        for i in 0..=r {
            let p = src[i * w + x];
            sr += (p >> 16) & 0xff;
            sg += (p >> 8) & 0xff;
            sb += p & 0xff;
        }
        for y in 0..h {
            dst[y * w + x] = pack([(sr / count) as u8, (sg / count) as u8, (sb / count) as u8]);
            let lo = y.saturating_sub(r);
            let p = src[lo * w + x];
            sr -= (p >> 16) & 0xff;
            sg -= (p >> 8) & 0xff;
            sb -= p & 0xff;
            let hi = (y + r + 1).min(h - 1);
            let p = src[hi * w + x];
            sr += (p >> 16) & 0xff;
            sg += (p >> 8) & 0xff;
            sb += p & 0xff;
        }
    }
}

impl Renderer {
    /// Draw the visible grid of cells (backgrounds, glyphs, selection).
    pub fn draw_cells(
        &mut self,
        content: &mut RenderableContent<'_>,
        fonts: &mut FontSystem,
        pal: &Palette,
        cfg: &Config,
        layout: &Layout,
        row_fx: &[RowFx],
    ) {
        let offset = content.display_offset as i32;
        let metrics_baseline = fonts.metrics.baseline;
        let underline_y = fonts.metrics.underline_y as i32;
        let strike_y = fonts.metrics.strike_y as i32;
        let default_fg = palette::parse_hex_or(&cfg.colors.foreground, [0xcc, 0xcc, 0xcc]);
        let default_bg = palette::parse_hex_or(&cfg.colors.background, [0, 0, 0]);
        let sel_bg = cfg
            .colors
            .selection_background
            .as_deref()
            .and_then(palette::parse_hex)
            .unwrap_or(default_fg);
        let sel_fg = cfg
            .colors
            .selection_foreground
            .as_deref()
            .and_then(palette::parse_hex)
            .unwrap_or(default_bg);
        let synthetic_bold = cfg.font.synthetic_bold && !fonts.has_bold();
        let cw = layout.cell_w as i32;
        let chh = layout.cell_h as i32;

        while let Some(indexed) = content.display_iter.next() {
            let cell: &Cell = indexed.cell;
            let row = indexed.point.line.0 + offset;
            let col = indexed.point.column.0;
            if row < 0 || row >= layout.rows as i32 || col >= layout.cols {
                continue;
            }
            if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
                continue;
            }
            let fx = row_fx.get(row as usize).copied().unwrap_or(RowFx::FULL);
            // `wipe` uncovers the row column by column; the half-cell margin
            // keeps the leading edge from popping in too early.
            if (col as f32 + 0.5) > fx.reveal * layout.cols as f32 {
                continue;
            }
            // `dissolve` reveals cells in a fixed pseudo-random order.
            if fx.dissolve > 0.0 {
                let threshold = ((1.0 - fx.dissolve.clamp(0.0, 1.0)) * 255.0) as u32;
                let noise = hash3(col as u32, row as u32, 0x9e37) & 0xff;
                if noise < threshold {
                    continue;
                }
            }
            let x = layout.cell_x(col) + fx.dx;
            let y = layout.cell_y(row as usize) + fx.dy;
            let baseline = y as f32 + metrics_baseline;
            let a = (fx.alpha.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;

            let selected = content
                .selection
                .map(|s| s.contains(indexed.point))
                .unwrap_or(false);

            let mut fg = cell.fg;
            let mut bg = cell.bg;
            if cell.flags.contains(Flags::INVERSE) {
                std::mem::swap(&mut fg, &mut bg);
            }
            let is_default_bg = bg == Color::Named(NamedColor::Background);
            let mut fg_rgb = palette::resolve(fg, content.colors, pal, &cfg.colors);
            let mut bg_rgb = palette::resolve(bg, content.colors, pal, &cfg.colors);
            if cell.flags.contains(Flags::DIM) {
                fg_rgb = palette::dim(fg_rgb);
            }
            if selected {
                fg_rgb = sel_fg;
                bg_rgb = sel_bg;
            }
            if fx.flash > 0.0 {
                // `glow`: fresh text flashes towards white, then settles.
                fg_rgb = mix_rgb(fg_rgb, [255, 255, 255], (fx.flash * 0.75).clamp(0.0, 1.0));
            }

            if !is_default_bg || selected {
                if a == 255 {
                    self.fill_rect(x, y, cw, chh, bg_rgb);
                } else {
                    self.fill_box_alpha(x, y, cw, chh, bg_rgb, a);
                }
            }

            if cell.c != ' ' && !cell.flags.contains(Flags::HIDDEN) {
                let bold = cell.flags.contains(Flags::BOLD);
                let italic = cell.flags.contains(Flags::ITALIC);
                if let Some(glyph) = fonts.glyph(cell.c, bold, italic) {
                    self.blit_glyph(x, baseline, &glyph, fg_rgb, a);
                    if bold && synthetic_bold {
                        self.blit_glyph(x + 1, baseline, &glyph, fg_rgb, a.saturating_mul(110) / 255);
                    }
                    if let Some(zw) = cell.zerowidth() {
                        let zx = x + glyph.advance as i32;
                        for &zc in zw {
                            if let Some(g2) = fonts.glyph(zc, bold, italic) {
                                self.blit_glyph(zx, baseline, &g2, fg_rgb, a);
                            }
                        }
                    }
                }
            }

            if cell.flags.contains(Flags::UNDERLINE) {
                self.fill_box_alpha(x, y + underline_y, cw, 1, fg_rgb, a);
            }
            if cell.flags.contains(Flags::DOUBLE_UNDERLINE) {
                self.fill_box_alpha(x, y + underline_y, cw, 1, fg_rgb, a);
                self.fill_box_alpha(x, y + chh - 2, cw, 1, fg_rgb, a);
            }
            if cell.flags.contains(Flags::STRIKEOUT) {
                self.fill_box_alpha(x, y + strike_y, cw, 1, fg_rgb, a);
            }
        }
    }

    #[inline]
    fn blit_glyph(&mut self, x: i32, baseline: f32, g: &GlyphBitmap, color: Rgb8, a_mul: u8) {
        if g.w == 0 || g.h == 0 {
            return;
        }
        let x0 = x + g.left;
        let y0 = baseline as i32 + g.top;
        for row in 0..i32::from(g.h) {
            let py = y0 + row;
            if py < 0 || py as usize >= self.h {
                continue;
            }
            let dst_row = py as usize * self.w;
            for col in 0..i32::from(g.w) {
                let a = u32::from(g.alpha[(row as u16 * g.w + col as u16) as usize]);
                if a == 0 {
                    continue;
                }
                let px = x0 + col;
                if px < 0 || px as usize >= self.w {
                    continue;
                }
                let a = (a * u32::from(a_mul) / 255) as u8;
                let idx = dst_row + px as usize;
                self.fb[idx] = blend_u32(self.fb[idx], color, a);
            }
        }
    }

    /// Draw the cursor on top of the cells (media may still cover it).
    #[allow(clippy::too_many_arguments)]
    pub fn draw_cursor(
        &mut self,
        layout: &Layout,
        cfg: &Config,
        pal: &Palette,
        content_colors: &alacritty_terminal::term::color::Colors,
        fonts: &mut FontSystem,
        shape: CursorShape,
        col: usize,
        row: usize,
        ch: char,
        visible: bool,
        focused: bool,
    ) {
        if !visible || row >= layout.rows || col >= layout.cols {
            return;
        }
        let cursor_rgb = palette::resolve(
            Color::Named(NamedColor::Cursor),
            content_colors,
            pal,
            &cfg.colors,
        );
        let cursor_text = cfg
            .colors
            .cursor_text
            .as_deref()
            .and_then(palette::parse_hex)
            .unwrap_or_else(|| palette::parse_hex_or(&cfg.colors.background, [0, 0, 0]));
        let x = layout.cell_x(col);
        let y = layout.cell_y(row);
        let cw = layout.cell_w as i32;
        let chh = layout.cell_h as i32;
        let hollow = !focused && cfg.cursor.unfocused_hollow;

        match shape {
            CursorShape::Block => {
                if hollow {
                    self.hollow_rect(x, y, cw, chh, cursor_rgb);
                } else {
                    self.fill_rect(x, y, cw, chh, cursor_rgb);
                    if ch != ' ' {
                        if let Some(glyph) = fonts.glyph(ch, false, false) {
                            let baseline = y as f32 + fonts.metrics.baseline;
                            self.blit_glyph(x, baseline, &glyph, cursor_text, 255);
                        }
                    }
                }
            }
            CursorShape::Underline => {
                let th = ((layout.cell_h * 0.15).round() as i32).max(2);
                self.fill_rect(x, y + chh - th, cw, th, cursor_rgb);
            }
            CursorShape::Beam => {
                let tw = ((layout.cell_w * 0.15).round() as i32).max(2);
                self.fill_rect(x, y, tw, chh, cursor_rgb);
            }
            CursorShape::HollowBlock => {
                self.hollow_rect(x, y, cw, chh, cursor_rgb);
            }
            CursorShape::Hidden => {}
        }

        if cfg.effects.glow {
            let cx = x as f32 + cw as f32 * 0.5;
            let cy = y as f32 + chh as f32 * 0.5;
            let radius = cw.max(chh) as f32 * 2.6;
            self.add_light(cx, cy, radius, cursor_rgb, 0.5);
        }
    }

    /// Solid rectangle (tab strips, selection blocks).
    pub fn fill_box(&mut self, x: i32, y: i32, w: i32, h: i32, c: Rgb8) {
        self.fill_rect(x, y, w, h, c);
    }

    /// Rectangle blended over the frame at alpha `a` (0..=255); used by the
    /// see-through settings panel.
    pub fn fill_box_alpha(&mut self, x: i32, y: i32, w: i32, h: i32, c: Rgb8, a: u8) {
        if a == 0 || self.w == 0 || self.h == 0 {
            return;
        }
        let x0 = x.max(0);
        let y0 = y.max(0);
        let x1 = (x + w).min(self.w as i32);
        let y1 = (y + h).min(self.h as i32);
        for yy in y0..y1 {
            let row = yy as usize * self.w;
            for xx in x0..x1 {
                let i = row + xx as usize;
                self.fb[i] = blend_u32(self.fb[i], c, a);
            }
        }
    }

    /// Rounded rectangle blended over the frame at alpha `a` (0..=255).
    /// The corners are anti-aliased, which is what makes the tab chips read
    /// as soft capsules rather than plain boxes.
    pub fn fill_round_rect_alpha(
        &mut self,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        radius: i32,
        c: Rgb8,
        a: u8,
    ) {
        if a == 0 || w <= 0 || h <= 0 || self.w == 0 || self.h == 0 {
            return;
        }
        let r = radius.clamp(0, w.min(h) / 2) as f32;
        let x0 = x.max(0);
        let y0 = y.max(0);
        let x1 = (x + w).min(self.w as i32);
        let y1 = (y + h).min(self.h as i32);
        for yy in y0..y1 {
            let py = yy as f32 + 0.5;
            let cy = py.clamp(y as f32 + r, y as f32 + h as f32 - r);
            let row = yy as usize * self.w;
            for xx in x0..x1 {
                let px = xx as f32 + 0.5;
                let cx = px.clamp(x as f32 + r, x as f32 + w as f32 - r);
                let dx = px - cx;
                let dy = py - cy;
                let d = (dx * dx + dy * dy).sqrt();
                // `+ 0.5` gives roughly one pixel of edge coverage.
                let cov = (r - d + 0.5).clamp(0.0, 1.0);
                if cov <= 0.0 {
                    continue;
                }
                let alpha = (f32::from(a) * cov).round() as u8;
                if alpha == 0 {
                    continue;
                }
                let i = row + xx as usize;
                self.fb[i] = blend_u32(self.fb[i], c, alpha);
            }
        }
    }

    /// Opaque rounded rectangle.
    pub fn fill_round_rect(
        &mut self,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        radius: i32,
        c: Rgb8,
    ) {
        self.fill_round_rect_alpha(x, y, w, h, radius, c, 255);
    }

    /// Paint a region pixel by pixel: the callback answers `Some((colour, alpha))`
    /// for every pixel it covers and `None` to leave the background alone.
    /// Gradients (the colour picker's wheel and sliders) need per-pixel control,
    /// which the box primitives cannot express.
    pub fn plot<F>(&mut self, x: i32, y: i32, w: i32, h: i32, mut f: F)
    where
        F: FnMut(i32, i32) -> Option<(Rgb8, u8)>,
    {
        if self.w == 0 || self.h == 0 || w <= 0 || h <= 0 {
            return;
        }
        let x0 = x.max(0);
        let y0 = y.max(0);
        let x1 = (x + w).min(self.w as i32);
        let y1 = (y + h).min(self.h as i32);
        for yy in y0..y1 {
            let row = yy as usize * self.w;
            for xx in x0..x1 {
                let Some((c, a)) = f(xx, yy) else {
                    continue;
                };
                if a == 0 {
                    continue;
                }
                let i = row + xx as usize;
                self.fb[i] = if a == 255 {
                    pack(c)
                } else {
                    blend_u32(self.fb[i], c, a)
                };
            }
        }
    }

    /// Boost (or mute) the saturation of a rectangle in place. `amount` is a
    /// multiplier: 1.0 leaves the pixels alone, >1 makes colours richer.
    pub fn saturate_rect(&mut self, x: i32, y: i32, w: i32, h: i32, amount: f32) {
        if self.w == 0 || self.h == 0 || w <= 0 || h <= 0 {
            return;
        }
        let amount = amount.max(0.0);
        let x0 = x.max(0);
        let y0 = y.max(0);
        let x1 = (x + w).min(self.w as i32);
        let y1 = (y + h).min(self.h as i32);
        for yy in y0..y1 {
            let row = yy as usize * self.w;
            for xx in x0..x1 {
                let i = row + xx as usize;
                let (r, g, b) = unpack(self.fb[i]);
                let luma = 0.2126 * f32::from(r) + 0.7152 * f32::from(g) + 0.0722 * f32::from(b);
                let mix = |v: u8| {
                    (luma + (f32::from(v) - luma) * amount)
                        .clamp(0.0, 255.0)
                        .round() as u8
                };
                self.fb[i] = pack([mix(r), mix(g), mix(b)]);
            }
        }
    }

    /// Diagonal light sweep across a rectangle, the "wet" part of liquid glass.
    /// `strength` is the alpha at the top-left corner (0..=255).
    pub fn sheen(&mut self, x: i32, y: i32, w: i32, h: i32, strength: u8) {
        if strength == 0 || w <= 0 || h <= 0 {
            return;
        }
        let (fw, fh) = (w as f32, h as f32);
        self.plot(x, y, w, h, |xx, yy| {
            let u = (xx - x) as f32 / fw;
            let v = (yy - y) as f32 / fh;
            let t = ((u + v) * 0.5).clamp(0.0, 1.0);
            let a = f32::from(strength) * (1.0 - t).powi(2);
            (a >= 1.0).then_some(([255, 255, 255], a as u8))
        });
    }

    /// Inner light rim on the top/left and a soft dark edge on the bottom/right,
    /// so a frosted panel reads as a physical sheet of glass.
    pub fn glass_rim(&mut self, x: i32, y: i32, w: i32, h: i32, light: u8, dark: u8) {
        if w <= 0 || h <= 0 {
            return;
        }
        self.fill_box_alpha(x, y, w, 1, [255, 255, 255], light);
        self.fill_box_alpha(x, y, 1, h, [255, 255, 255], light);
        self.fill_box_alpha(x, y + h - 1, w, 1, [0, 0, 0], dark);
        self.fill_box_alpha(x + w - 1, y, 1, h, [0, 0, 0], dark);
    }

    /// Box-blur the pixels inside a rectangle (frosted-glass settings panel).
    /// Two separable passes with edge clamping keep the region self-contained.
    pub fn blur_rect(&mut self, x: i32, y: i32, w: i32, h: i32, radius: u32) {
        if self.w == 0 || self.h == 0 || w <= 0 || h <= 0 || radius == 0 {
            return;
        }
        let x0 = x.max(0);
        let y0 = y.max(0);
        let x1 = (x + w).min(self.w as i32);
        let y1 = (y + h).min(self.h as i32);
        let rw = (x1 - x0) as usize;
        let rh = (y1 - y0) as usize;
        if rw < 3 || rh < 3 {
            return;
        }
        let rx = (radius as usize).min(rw - 1);
        let ry = (radius as usize).min(rh - 1);
        let mut reg = Vec::with_capacity(rw * rh);
        for yy in y0..y1 {
            let row = yy as usize * self.w;
            reg.extend_from_slice(&self.fb[row + x0 as usize..row + x1 as usize]);
        }
        let mut tmp = vec![0u32; rw * rh];
        for _ in 0..2 {
            blur_axis_h(&reg, &mut tmp, rw, rh, rx);
            blur_axis_v(&tmp, &mut reg, rw, rh, ry);
        }
        for (i, yy) in (y0..y1).enumerate() {
            let row = yy as usize * self.w;
            self.fb[row + x0 as usize..row + x1 as usize]
                .copy_from_slice(&reg[i * rw..(i + 1) * rw]);
        }
    }

    /// One-pixel frame (focused pane border).
    pub fn outline(&mut self, x: i32, y: i32, w: i32, h: i32, c: Rgb8) {
        self.hollow_rect(x, y, w, h, c);
    }

    /// Darken the whole frame in place (settings overlay backdrop).
    pub fn shade(&mut self, num: u32, den: u32) {
        for pixel in self.fb.iter_mut() {
            let value = *pixel;
            let r = ((value >> 16) & 0xff) * num / den;
            let g = ((value >> 8) & 0xff) * num / den;
            let b = (value & 0xff) * num / den;
            *pixel = (r << 16) | (g << 8) | b;
        }
    }

    /// Plain single-line text (tab labels).
    pub fn label(
        &mut self,
        fonts: &mut FontSystem,
        x: i32,
        baseline: f32,
        text: &str,
        color: Rgb8,
    ) {
        let mut pen = x as f32;
        for ch in text.chars() {
            if let Some(glyph) = fonts.glyph(ch, false, false) {
                self.blit_glyph(pen.round() as i32, baseline, &glyph, color, 255);
                pen += glyph.advance;
            }
        }
    }

    fn hollow_rect(&mut self, x: i32, y: i32, w: i32, h: i32, c: Rgb8) {
        self.fill_rect(x, y, w, 1, c);
        self.fill_rect(x, y + h - 1, w, 1, c);
        self.fill_rect(x, y, 1, h, c);
        self.fill_rect(x + w - 1, y, 1, h, c);
    }

    /// Paint the full-window backdrop; cells are composited on top of it.
    pub fn draw_background(&mut self, cfg: &Config, elapsed: Duration) {
        if self.w == 0 || self.h == 0 {
            return;
        }
        let opacity = cfg.window.opacity.clamp(0.2, 1.0);
        match cfg.effects.background.as_str() {
            "gradient" => self.draw_gradient(cfg, elapsed, opacity),
            "plasma" => self.draw_plasma(cfg, elapsed, opacity),
            "aurora" => self.draw_aurora(cfg, elapsed, opacity),
            "image" => self.draw_image_bg(cfg, opacity),
            _ => {
                let base = palette::parse_hex_or(&cfg.colors.background, [0x16, 0x16, 0x1e]);
                self.fb.fill(pack(scale_rgb(base, opacity)));
            }
        }
    }

    /// Draw all media overlays (call before cells for `above_text = false`).
    pub fn draw_media(&mut self, media: &MediaStore, layout: &Layout) {
        let mut items: Vec<&MediaItem> = media.iter().collect();
        items.sort_by_key(|item| (item.z, item.id));
        for item in items {
            let cols = if item.cols > 0 {
                item.cols
            } else {
                ((item.px_w as f32 / layout.cell_w).ceil() as usize).max(1)
            };
            let rows = if item.rows > 0 {
                item.rows
            } else {
                ((item.px_h as f32 / layout.cell_h).ceil() as usize).max(1)
            };
            let x = layout.cell_x(item.col);
            let y = layout.cell_y(item.row);
            let rw = layout.cell_x(item.col + cols) - x;
            let rh = layout.cell_y(item.row + rows) - y;
            if rw <= 0 || rh <= 0 {
                continue;
            }

            let (data, pw, ph): (&[u8], u32, u32) = match &item.kind {
                Kind::Static(img) => (&img.rgba, img.w, img.h),
                Kind::Anim { frames, idx, .. } => {
                    let img = &frames[*idx].image;
                    (&img.rgba, img.w, img.h)
                }
                Kind::Video { current } => {
                    if current.len() != (item.px_w * item.px_h * 4) as usize {
                        continue; // no frame yet
                    }
                    (current, item.px_w, item.px_h)
                }
            };
            if pw == 0 || ph == 0 || data.len() < (pw * ph * 4) as usize {
                continue;
            }
            for yy in 0..rh {
                let py = (yy as u64 * u64::from(ph) / rh as u64) as u32;
                let dy = y + yy;
                if dy < 0 || dy as usize >= self.h {
                    continue;
                }
                let dst_row = dy as usize * self.w;
                for xx in 0..rw {
                    let px = (xx as u64 * u64::from(pw) / rw as u64) as u32;
                    let di = ((py * pw + px) * 4) as usize;
                    let a = data[di + 3];
                    if a == 0 {
                        continue;
                    }
                    let dx = x + xx;
                    if dx < 0 || dx as usize >= self.w {
                        continue;
                    }
                    let idx = dst_row + dx as usize;
                    self.fb[idx] =
                        blend_u32(self.fb[idx], [data[di], data[di + 1], data[di + 2]], a);
                }
            }
        }
    }

    /// CRT scanlines + vignette.
    pub fn post_effects(&mut self, cfg: &Config, elapsed: Duration) {
        if self.w == 0 || self.h == 0 {
            return;
        }
        if cfg.effects.scanlines {
            let k = (255.0 * (1.0 - cfg.effects.scanline_intensity.clamp(0.0, 1.0))) as u32;
            let roll = if cfg.effects.scanline_speed > 0.0 {
                (elapsed.as_secs_f32() * cfg.effects.scanline_speed) as i32 % 2
            } else {
                0
            };
            for y in ((1 + roll)..self.h as i32).step_by(2) {
                let y = y as usize;
                let row = y * self.w;
                for x in 0..self.w {
                    let (r, g, b) = unpack(self.fb[row + x]);
                    self.fb[row + x] = u32::from_be_bytes([
                        0,
                        (u32::from(r) * k / 255) as u8,
                        (u32::from(g) * k / 255) as u8,
                        (u32::from(b) * k / 255) as u8,
                    ]);
                }
            }
        }
        if cfg.effects.vignette {
            let w = self.w as f32;
            let h = self.h as f32;
            for y in 0..self.h {
                let dy = (y as f32 - h / 2.0) / (h / 2.0);
                let row = y * self.w;
                for x in 0..self.w {
                    let dx = (x as f32 - w / 2.0) / (w / 2.0);
                    let d2 = (dx * dx + dy * dy).min(2.0);
                    let k = (((1.0 - 0.35 * d2).max(0.3)) * 255.0) as u32;
                    let (r, g, b) = unpack(self.fb[row + x]);
                    self.fb[row + x] = u32::from_be_bytes([
                        0,
                        (u32::from(r) * k / 255) as u8,
                        (u32::from(g) * k / 255) as u8,
                        (u32::from(b) * k / 255) as u8,
                    ]);
                }
            }
        }
        if cfg.effects.bloom && cfg.effects.bloom_intensity > 0.0 {
            self.apply_bloom(cfg.effects.bloom_intensity);
        }
        if cfg.effects.grain && cfg.effects.grain_intensity > 0.0 {
            self.apply_grain(cfg.effects.grain_intensity, elapsed.as_millis() as u32);
        }
        if cfg.effects.fade_in {
            let k = (elapsed.as_secs_f32() / 0.7).min(1.0);
            if k < 1.0 {
                let factor = (k * 255.0) as u32;
                for pixel in self.fb.iter_mut() {
                    let value = *pixel;
                    let r = ((value >> 16) & 0xff) * factor / 255;
                    let g = ((value >> 8) & 0xff) * factor / 255;
                    let b = (value & 0xff) * factor / 255;
                    *pixel = (r << 16) | (g << 8) | b;
                }
            }
        }
    }

    /// Bright-pixel halo: downsample → threshold → box blur → additive.
    fn apply_bloom(&mut self, intensity: f32) {
        const DS: usize = 4;
        const THRESHOLD: f32 = 96.0;
        let bw = self.w.div_ceil(DS).max(1);
        let bh = self.h.div_ceil(DS).max(1);
        if self.bloom_a.len() != bw * bh * 3 {
            self.bloom_a.resize(bw * bh * 3, 0);
            self.bloom_tmp.resize(bw * bh * 3, 0);
        }

        for by in 0..bh {
            let y0 = by * DS;
            let y1 = (y0 + DS).min(self.h);
            for bx in 0..bw {
                let x0 = bx * DS;
                let x1 = (x0 + DS).min(self.w);
                let mut sum = [0u32; 3];
                let mut count = 0u32;
                for y in y0..y1 {
                    let row = y * self.w;
                    for x in x0..x1 {
                        let (r, g, b) = unpack(self.fb[row + x]);
                        sum[0] += u32::from(r);
                        sum[1] += u32::from(g);
                        sum[2] += u32::from(b);
                        count += 1;
                    }
                }
                if count == 0 {
                    continue;
                }
                let index = (by * bw + bx) * 3;
                for (ch, total) in sum.iter().enumerate() {
                    let avg = *total as f32 / count as f32;
                    let over = avg - THRESHOLD;
                    self.bloom_a[index + ch] = if over > 0.0 {
                        (over * 3.0).min(255.0) as u8
                    } else {
                        0
                    };
                }
            }
        }

        for _ in 0..2 {
            self.blur_bloom(bw, bh, 2, true);
            self.blur_bloom(bw, bh, 2, false);
        }

        for y in 0..self.h {
            let by = (y / DS).min(bh - 1);
            for x in 0..self.w {
                let index = (by * bw + (x / DS).min(bw - 1)) * 3;
                let br = self.bloom_a[index];
                let bg = self.bloom_a[index + 1];
                let bb = self.bloom_a[index + 2];
                if br == 0 && bg == 0 && bb == 0 {
                    continue;
                }
                let pixel = y * self.w + x;
                let (r, g, b) = unpack(self.fb[pixel]);
                let add =
                    |v: u8, glow: u8| (f32::from(v) + f32::from(glow) * intensity).min(255.0) as u8;
                self.fb[pixel] = pack([add(r, br), add(g, bg), add(b, bb)]);
            }
        }
    }

    /// One separable box-blur pass over the bloom pyramid.
    fn blur_bloom(&mut self, bw: usize, bh: usize, radius: i32, horizontal: bool) {
        let count_cells = bw * bh * 3;
        if self.bloom_tmp.len() != count_cells {
            self.bloom_tmp.resize(count_cells, 0);
        }
        for y in 0..bh {
            for x in 0..bw {
                let mut sum = [0u32; 3];
                let mut taps = 0u32;
                for step in -radius..=radius {
                    let (tx, ty) = if horizontal {
                        (x as i32 + step, y as i32)
                    } else {
                        (x as i32, y as i32 + step)
                    };
                    if tx < 0 || ty < 0 || tx as usize >= bw || ty as usize >= bh {
                        continue;
                    }
                    let src = ((ty as usize) * bw + tx as usize) * 3;
                    for (slot, value) in sum.iter_mut().zip(self.bloom_a[src..src + 3].iter()) {
                        *slot += u32::from(*value);
                    }
                    taps += 1;
                }
                let dst = (y * bw + x) * 3;
                let taps = taps.max(1);
                for (ch, total) in sum.iter().enumerate() {
                    self.bloom_tmp[dst + ch] = (*total / taps) as u8;
                }
            }
        }
        std::mem::swap(&mut self.bloom_a, &mut self.bloom_tmp);
    }

    /// Animated film grain over the finished frame.
    fn apply_grain(&mut self, intensity: f32, seed: u32) {
        let amplitude = intensity * 72.0;
        if amplitude <= 0.0 {
            return;
        }
        for y in 0..self.h {
            for x in 0..self.w {
                let noise = hash3(x as u32, y as u32, seed);
                let n = (f32::from((noise & 0xff) as u8) - 128.0) / 128.0 * amplitude;
                let index = y * self.w + x;
                let (r, g, b) = unpack(self.fb[index]);
                let adjust = |v: u8| (f32::from(v) + n).clamp(0.0, 255.0) as u8;
                self.fb[index] = pack([adjust(r), adjust(g), adjust(b)]);
            }
        }
    }

    /// Additive radial light (cursor halo and friends).
    pub fn add_light(&mut self, cx: f32, cy: f32, radius: f32, color: Rgb8, strength: f32) {
        if radius <= 0.0 || strength <= 0.0 || self.w == 0 || self.h == 0 {
            return;
        }
        let x0 = (cx - radius).floor().max(0.0) as usize;
        let y0 = (cy - radius).floor().max(0.0) as usize;
        let x1 = ((cx + radius).ceil() as usize + 1).min(self.w);
        let y1 = ((cy + radius).ceil() as usize + 1).min(self.h);
        for y in y0..y1 {
            for x in x0..x1 {
                let dx = x as f32 + 0.5 - cx;
                let dy = y as f32 + 0.5 - cy;
                let d = (dx * dx + dy * dy).sqrt() / radius;
                if d >= 1.0 {
                    continue;
                }
                let falloff = (1.0 - d) * (1.0 - d) * strength;
                let index = y * self.w + x;
                let (r, g, b) = unpack(self.fb[index]);
                let add = |v: u8, c: u8| (f32::from(v) + f32::from(c) * falloff).min(255.0) as u8;
                self.fb[index] = pack([add(r, color[0]), add(g, color[1]), add(b, color[2])]);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blur_rect_softens_only_its_region() {
        let mut r = Renderer::new();
        r.ensure_size(40, 40);
        r.fb.fill(0x00101010);
        for yy in 10..30 {
            for xx in 10..30 {
                let i = yy * r.w + xx;
                r.fb[i] = if yy < 20 {
                    if (xx - 10) % 2 == 0 {
                        0x00ff_ffff
                    } else {
                        0x0000_0000
                    }
                } else {
                    0x00ff_0000
                };
            }
        }
        r.blur_rect(10, 10, 20, 20, 3);
        let softened = r.fb[15 * 40 + 11];
        let sr = (softened >> 16) & 0xff;
        assert!(
            sr > 0 && sr < 255,
            "stripe pixel should blend, got {softened:#010x}"
        );
        assert_eq!(
            r.fb[5 * 40 + 5],
            0x00101010,
            "outside the region must not change"
        );
        assert_eq!(
            r.fb[29 * 40 + 25],
            0x00ff_0000,
            "solid area far from stripes stays pure"
        );
    }

    #[test]
    fn glass_primitives_stay_inside_their_region() {
        let mut r = Renderer::new();
        r.ensure_size(24, 24);
        r.fb.fill(0x0020_4080);
        r.saturate_rect(4, 4, 10, 10, 2.0);
        r.sheen(4, 4, 10, 10, 120);
        r.glass_rim(4, 4, 10, 10, 90, 40);
        // Outside the panel nothing moves.
        assert_eq!(r.fb[0], 0x0020_4080);
        assert_eq!(r.fb[23 * 24 + 23], 0x0020_4080);
        // The top-left rim is lightened (blended towards white).
        let rim = r.fb[4 * 24 + 4];
        assert!((rim >> 16) & 0xff > 0x20);
        // Saturation must keep the blue dominant but spread the channels.
        let mid = r.fb[9 * 24 + 9];
        let (r_, g_, b_) = unpack(mid);
        assert!(b_ >= r_ && b_ >= g_, "blue should stay dominant: {mid:#010x}");
    }
}
