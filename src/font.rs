//! Font loading (`fontdb`) and glyph rasterization (`ab_glyph`) with a bounded
//! glyph cache — no GPU, no shaping: per-character rendering is what terminals
//! need and it keeps memory flat. Outlines are parsed lazily per glyph, unlike
//! fontdue which eagerly outlined every cmap-mapped glyph of every face.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use ab_glyph::{Font, FontArc, PxScale, ScaleFont};
use anyhow::{anyhow, Result};
use fontdb::{Family, Query, Source, Stretch, Style, Weight};

use crate::config::FontCfg;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FaceKind {
    Regular,
    Bold,
    Italic,
    BoldItalic,
}

struct Face {
    font: FontArc,
}

#[derive(Debug, Clone)]
pub struct CellMetrics {
    pub cell_w: f32,
    pub cell_h: f32,
    pub baseline: f32,
    pub underline_y: f32,
    pub double_underline_y: f32,
    pub strike_y: f32,
    pub size_px: f32,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct GlyphKey {
    face: FaceKind,
    ch: u32,
    px: u16,
}

#[derive(Debug)]
pub struct GlyphBitmap {
    pub w: u16,
    pub h: u16,
    /// Horizontal offset from the pen position.
    pub left: i32,
    /// Offset from baseline to the top edge (usually negative).
    pub top: i32,
    pub advance: f32,
    /// Coverage mask, `w * h` bytes, 0..=255.
    pub alpha: Vec<u8>,
}

#[derive(Default)]
struct GlyphCache {
    map: HashMap<GlyphKey, Arc<GlyphBitmap>>,
    order: std::collections::VecDeque<GlyphKey>,
    cap: usize,
}

impl GlyphCache {
    fn new(cap: usize) -> Self {
        Self {
            map: HashMap::new(),
            order: VecDeque::new(),
            cap,
        }
    }

    fn clear(&mut self) {
        self.map.clear();
        self.order.clear();
    }

    fn insert(&mut self, key: GlyphKey, bmp: Arc<GlyphBitmap>) -> Arc<GlyphBitmap> {
        while self.map.len() >= self.cap {
            match self.order.pop_front() {
                Some(old) => {
                    self.map.remove(&old);
                }
                None => break,
            }
        }
        self.order.push_back(key.clone());
        self.map.insert(key, bmp.clone());
        bmp
    }
}

pub struct FontSystem {
    faces: [Option<Face>; 4],
    fallbacks: Vec<Face>,
    fallback_paths: Vec<std::path::PathBuf>,
    pub metrics: CellMetrics,
    cache: GlyphCache,
}

/// `PxScale` that makes ab_glyph produce fontdue's px-per-em scaling.
fn em_scale(font: &FontArc, px: f32) -> PxScale {
    let height = font.height_unscaled();
    let upem = font.units_per_em().unwrap_or(height);
    if height > 0.0 && upem > 0.0 {
        PxScale::from(px * height / upem)
    } else {
        PxScale::from(px)
    }
}

fn query_family(
    db: &fontdb::Database,
    name: &str,
    style: Style,
    weight: Weight,
) -> Option<fontdb::ID> {
    let family = if name == "monospace" {
        Family::Monospace
    } else {
        Family::Name(name)
    };
    db.query(&Query {
        families: &[family],
        style,
        weight,
        stretch: Stretch::Normal,
    })
    .or_else(|| {
        db.query(&Query {
            families: &[Family::Monospace],
            style,
            weight,
            stretch: Stretch::Normal,
        })
    })
    .or_else(|| {
        db.query(&Query {
            families: &[Family::SansSerif],
            style,
            weight,
            stretch: Stretch::Normal,
        })
    })
}

fn face_data(db: &fontdb::Database, id: fontdb::ID) -> Option<Arc<[u8]>> {
    let face = db.face(id)?;
    match &face.source {
        Source::Binary(bin) => Some(Arc::from(bin.as_ref().as_ref())),
        Source::File(path) => std::fs::read(path).ok().map(Arc::from),
        Source::SharedFile(_, bin) => Some(Arc::from(bin.as_ref().as_ref())),
    }
}

fn load_face(db: &fontdb::Database, id: fontdb::ID) -> Option<Face> {
    let data = face_data(db, id)?;
    let font = FontArc::try_from_vec(data.to_vec()).ok()?;
    Some(Face { font })
}

fn load_face_path(path: &std::path::Path) -> Option<Face> {
    let data = std::fs::read(path).ok()?;
    let font = FontArc::try_from_vec(data).ok()?;
    Some(Face { font })
}

/// Ask fontconfig for the best file matching a pattern like `JetBrains Mono:style=Bold`.
fn fc_match(pattern: &str) -> Option<std::path::PathBuf> {
    let out = std::process::Command::new("fc-match")
        .arg("-f")
        .arg("%{file}")
        .arg(pattern)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if s.is_empty() {
        return None;
    }
    let path = std::path::PathBuf::from(s);
    path.is_file().then_some(path)
}

impl FontSystem {
    pub fn new(cfg: &FontCfg, scale: f32) -> Result<Self> {
        let mut sys = Self {
            faces: [None, None, None, None],
            fallbacks: Vec::new(),
            fallback_paths: Vec::new(),
            metrics: CellMetrics {
                cell_w: 8.0,
                cell_h: 16.0,
                baseline: 12.0,
                underline_y: 13.0,
                double_underline_y: 15.0,
                strike_y: 8.0,
                size_px: 14.0,
            },
            cache: GlyphCache::new(4096),
        };
        // Fast path: ask fontconfig which files to open (avoids scanning GBs of fonts).
        if !sys.load_via_fontconfig(cfg) {
            sys.load_via_fontdb(cfg)?;
        }
        if sys.faces[FaceKind::Regular as usize].is_none() {
            return Err(anyhow!("no usable font found for family `{}`", cfg.family));
        }
        sys.recompute_metrics(cfg, scale);
        Ok(sys)
    }

    /// Resolve faces through `fc-match`. Returns false when fontconfig is unavailable.
    fn load_via_fontconfig(&mut self, cfg: &FontCfg) -> bool {
        let Some(regular_path) = fc_match(&cfg.family) else {
            return false;
        };
        let bold_family = cfg.bold_family.as_deref().unwrap_or(&cfg.family);
        let italic_family = cfg.italic_family.as_deref().unwrap_or(&cfg.family);

        let Some(regular) = load_face_path(&regular_path) else {
            return false;
        };
        self.faces[FaceKind::Regular as usize] = Some(regular);

        let pick = |spec: String, regular: &std::path::Path| -> Option<Face> {
            let path = fc_match(&spec)?;
            if path == regular {
                // fontconfig fell back to the base face: style is not available.
                return None;
            }
            load_face_path(&path)
        };
        self.faces[FaceKind::Bold as usize] =
            pick(format!("{bold_family}:style=Bold"), &regular_path);
        self.faces[FaceKind::Italic as usize] =
            pick(format!("{italic_family}:style=Italic"), &regular_path);
        let bold_path = fc_match(&format!("{bold_family}:style=Bold"));
        self.faces[FaceKind::BoldItalic as usize] =
            pick(format!("{italic_family}:style=Bold Italic"), &regular_path)
                .filter(|_| bold_path.is_some() && bold_path.as_ref() != Some(&regular_path));

        // Symbol/emoji fallbacks for glyphs missing in the primary font.
        self.fallbacks.clear();
        self.fallback_paths.clear();
        let specs = [
            "Symbols Nerd Font Mono",
            "Symbols Nerd Font",
            "Noto Sans Symbols 2",
            "Noto Sans Symbols",
            "DejaVu Sans",
        ];
        for spec in specs {
            if self.fallbacks.len() >= 3 {
                break;
            }
            if let Some(path) = fc_match(spec) {
                if path == regular_path || self.fallback_paths.contains(&path) {
                    continue;
                }
                if let Some(face) = load_face_path(&path) {
                    self.fallback_paths.push(path);
                    self.fallbacks.push(face);
                }
            }
        }
        true
    }

    /// Slow fallback: index every system font with fontdb (can take minutes
    /// with very large font collections; used only when fontconfig is absent).
    fn load_via_fontdb(&mut self, cfg: &FontCfg) -> Result<()> {
        let mut db = fontdb::Database::new();
        db.load_system_fonts();
        if db.faces().next().is_none() {
            return Err(anyhow!("no fonts found on the system"));
        }
        let bold_family = cfg
            .bold_family
            .clone()
            .unwrap_or_else(|| cfg.family.clone());
        let italic_family = cfg
            .italic_family
            .clone()
            .unwrap_or_else(|| cfg.family.clone());

        let specs = [
            (
                FaceKind::Regular,
                cfg.family.as_str(),
                Style::Normal,
                Weight::NORMAL,
            ),
            (
                FaceKind::Bold,
                bold_family.as_str(),
                Style::Normal,
                Weight::BOLD,
            ),
            (
                FaceKind::Italic,
                italic_family.as_str(),
                Style::Italic,
                Weight::NORMAL,
            ),
            (
                FaceKind::BoldItalic,
                italic_family.as_str(),
                Style::Italic,
                Weight::BOLD,
            ),
        ];
        for (kind, family, style, weight) in specs {
            let id = query_family(&db, family, style, weight);
            self.faces[kind as usize] = id.and_then(|id| load_face(&db, id));
        }
        if self.faces[FaceKind::Regular as usize].is_none() {
            return Err(anyhow!("font family `{}` not found", cfg.family));
        }

        // Fallback faces for glyphs missing in the primary font (symbols etc.).
        // Prefer patched "Nerd Font" families, then any other normal-weight face.
        self.fallbacks.clear();
        self.fallback_paths.clear();
        let mut candidates: Vec<&fontdb::FaceInfo> = db.faces().collect();
        candidates.sort_by_key(|f| {
            let name = f.families.first().map(|(n, _)| n.as_str()).unwrap_or("");
            if name.contains("Nerd") {
                0
            } else if name.contains("Symbols") || name.contains("Emoji") {
                1
            } else {
                2
            }
        });
        for info in candidates.into_iter().take(6) {
            if self.fallbacks.len() >= 3 {
                break;
            }
            if let Some(face) = load_face(&db, info.id) {
                self.fallbacks.push(face);
            }
        }

        Ok(())
    }

    fn face(&self, kind: FaceKind) -> &Face {
        self.faces[kind as usize]
            .as_ref()
            .or(self.faces[FaceKind::Regular as usize].as_ref())
            .expect("regular font face is mandatory")
    }

    pub fn recompute_metrics(&mut self, cfg: &FontCfg, scale: f32) {
        let px = (cfg.size * scale).round().max(1.0);
        let scale_factor = em_scale(&self.face(FaceKind::Regular).font, px);
        let scaled = self.face(FaceKind::Regular).font.as_scaled(scale_factor);
        let ascent = scaled.ascent();
        let descent = scaled.descent();
        let line_gap = scaled.line_gap();

        // Uniform cell width: widest of a few ASCII letters keeps text from overlapping
        // even when the fallback font is not strictly monospaced.
        let mut advance = 0.0f32;
        for ch in ['M', 'W', '@', 'x'] {
            advance = advance.max(scaled.h_advance(scaled.glyph_id(ch)));
        }
        let cell_w = (advance.ceil() + cfg.offset_x as f32).max(1.0);
        let cell_h = ((ascent - descent + line_gap).round() + cfg.offset_y as f32).max(1.0);
        let baseline = ascent.round() + cfg.offset_y as f32;
        self.metrics = CellMetrics {
            cell_w,
            cell_h,
            baseline,
            underline_y: (baseline + (px * 0.12).round()).min(cell_h - 1.0),
            double_underline_y: cell_h - 1.0,
            strike_y: (baseline - px * 0.3).round().max(1.0),
            size_px: px,
        };
        self.cache.clear();
    }

    /// Rasterize (or fetch) a glyph. Returns `None` for control characters.
    pub fn glyph(&mut self, ch: char, bold: bool, italic: bool) -> Option<Arc<GlyphBitmap>> {
        let kind = match (bold, italic) {
            (false, false) => FaceKind::Regular,
            (true, false) => FaceKind::Bold,
            (false, true) => FaceKind::Italic,
            (true, true) => FaceKind::BoldItalic,
        };
        let px16 = (self.metrics.size_px * 4.0).round() as u16;
        let key = GlyphKey {
            face: kind,
            ch: ch as u32,
            px: px16,
        };
        if let Some(bmp) = self.cache.map.get(&key) {
            return Some(bmp.clone());
        }

        // Pick a face that actually has this glyph.
        let mut use_kind = kind;
        if self.face(kind).font.glyph_id(ch).0 == 0 {
            if kind != FaceKind::Regular && self.face(FaceKind::Regular).font.glyph_id(ch).0 != 0 {
                use_kind = FaceKind::Regular;
            } else {
                for fb in &self.fallbacks {
                    if fb.font.glyph_id(ch).0 != 0 {
                        let bmp = rasterize(&fb.font, ch, self.metrics.size_px);
                        return Some(self.cache.insert(key, Arc::new(bmp)));
                    }
                }
                if ch.is_control() {
                    return None;
                }
            }
        }

        let bmp = rasterize(&self.face(use_kind).font, ch, self.metrics.size_px);
        Some(self.cache.insert(key, Arc::new(bmp)))
    }

    pub fn clear_cache(&mut self) {
        self.cache.clear();
    }

    /// Width of `text` at the current metrics (tab labels, gutters).
    pub fn text_width(&mut self, text: &str) -> f32 {
        let mut width = 0.0;
        for ch in text.chars() {
            if let Some(glyph) = self.glyph(ch, false, false) {
                width += glyph.advance;
            }
        }
        width
    }

    pub fn has_bold(&self) -> bool {
        self.faces[FaceKind::Bold as usize].is_some()
    }

    pub fn cache_len(&self) -> usize {
        self.cache.map.len()
    }
}

fn rasterize(font: &FontArc, ch: char, px: f32) -> GlyphBitmap {
    // The em size is relative to *this* face: fallback faces have their own
    // units-per-em, exactly like fontdue's per-font `rasterize(ch, px)`.
    let scale = em_scale(font, px);
    let id = font.glyph_id(ch);
    let advance = font.as_scaled(scale).h_advance(id);
    // Glyphs without outlines (spaces, control placeholders) keep a zero-size
    // bitmap; `draw_cells` skips them and only advances the pen.
    let Some(outlined) = font.outline_glyph(id.with_scale(scale)) else {
        return GlyphBitmap {
            w: 0,
            h: 0,
            left: 0,
            top: 0,
            advance,
            alpha: Vec::new(),
        };
    };
    // `px_bounds` is in pixel space with y growing downward: min.y is the top
    // edge relative to the baseline (negative above it), matching the old
    // fontdue mapping of `top = -ymin - height`.
    let bounds = outlined.px_bounds();
    let w = bounds.width().round().max(0.0) as u16;
    let h = bounds.height().round().max(0.0) as u16;
    let mut alpha = vec![0u8; usize::from(w) * usize::from(h)];
    if !alpha.is_empty() {
        outlined.draw(|x, y, coverage| {
            if let Some(slot) = alpha.get_mut(y as usize * w as usize + x as usize) {
                *slot = (coverage.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
            }
        });
    }
    GlyphBitmap {
        w,
        h,
        left: bounds.min.x.round() as i32,
        top: bounds.min.y.round() as i32,
        advance,
        alpha,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn system() -> Option<FontSystem> {
        match FontSystem::new(&FontCfg::default(), 1.0) {
            Ok(sys) => Some(sys),
            Err(_) => None, // environment without fonts
        }
    }

    #[test]
    fn loads_and_reports_metrics() {
        let Some(sys) = system() else { return };
        assert!(sys.metrics.cell_w >= 1.0);
        assert!(sys.metrics.cell_h >= 4.0);
        assert!(sys.metrics.baseline > 0.0);
        assert!(sys.metrics.underline_y < sys.metrics.cell_h);
    }

    #[test]
    fn rasterizes_ascii_and_caches() {
        let Some(mut sys) = system() else { return };
        let g1 = sys.glyph('A', false, false).expect("glyph A");
        assert!(g1.alpha.iter().any(|&a| a > 0));
        let len = sys.cache_len();
        let g2 = sys.glyph('A', false, false).unwrap();
        assert_eq!(len, sys.cache_len()); // cache hit
        assert!(Arc::ptr_eq(&g1, &g2));
        assert_eq!(g1.w, g2.w);
    }

    #[test]
    fn control_chars_have_no_glyph() {
        let Some(mut sys) = system() else { return };
        assert!(sys.glyph('\n', false, false).is_none());
    }

    #[test]
    fn cache_is_bounded() {
        let Some(mut sys) = system() else { return };
        for c in '\u{2500}'..='\u{27bf}' {
            let _ = sys.glyph(c, false, false);
        }
        assert!(sys.cache_len() <= 4096);
    }

    /// Fallback glyphs must be scaled by the fallback face's own em size,
    /// not by the primary font's (the two fonts have different units-per-em).
    #[test]
    fn fallback_glyphs_scaled_by_their_own_face() {
        let Some(mut sys) = system() else { return };
        let px = sys.metrics.size_px;
        let primary = |c: char| sys.face(FaceKind::Regular).font.glyph_id(c).0 == 0;
        let probe = ('\u{2000}'..='\u{2BFF}')
            .chain('\u{1F300}'..='\u{1F5FF}')
            .find(|&c| primary(c) && sys.fallbacks.iter().any(|f| f.font.glyph_id(c).0 != 0));
        let Some(ch) = probe else { return };
        // Independent expectation: outline with the fallback face's own scale.
        let (ew, eh, eleft, etop, eadv) = {
            let fb = sys
                .fallbacks
                .iter()
                .find(|f| f.font.glyph_id(ch).0 != 0)
                .expect("fallback has the glyph");
            let scale = em_scale(&fb.font, px);
            let id = fb.font.glyph_id(ch);
            let advance = fb.font.as_scaled(scale).h_advance(id);
            let outlined = fb
                .font
                .outline_glyph(id.with_scale(scale))
                .expect("has outline");
            let bounds = outlined.px_bounds();
            (
                bounds.width().round() as u16,
                bounds.height().round() as u16,
                bounds.min.x.round() as i32,
                bounds.min.y.round() as i32,
                advance,
            )
        };
        let got = sys.glyph(ch, false, false).expect("glyph rasterized");
        assert_eq!(got.w, ew, "width for {ch:?}");
        assert_eq!(got.h, eh, "height for {ch:?}");
        assert_eq!(got.left, eleft, "left for {ch:?}");
        assert_eq!(got.top, etop, "top for {ch:?}");
        assert!((got.advance - eadv).abs() < 0.01, "advance for {ch:?}");
        // If the primary font's em were used instead, the glyph would be
        // mis-sized; guard against that with a non-empty bitmap.
        assert!(got.w > 0 && got.h > 0 && got.alpha.iter().any(|&a| a > 0));
    }
}
