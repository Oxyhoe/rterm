//! Peak memory of face loading with ab_glyph (lazy outline parsing).
//! Modes: `a4` (Adwaita Mono Nerd, 4 faces), `d4` (DejaVu Sans Mono, 4 faces),
//! `a4g` = a4 plus a full 4096-entry glyph cache fill.
use std::fmt::Write as _;

use ab_glyph::{Font, FontArc, GlyphId, PxScale};

fn rss() -> String {
    let s = std::fs::read_to_string("/proc/self/status").unwrap();
    let mut out = String::new();
    for l in s.lines() {
        if l.starts_with("VmRSS") || l.starts_with("RssAnon") {
            let _ = write!(out, "{}; ", l);
        }
    }
    out
}

fn main() {
    let mode = std::env::args().nth(1).unwrap_or_default();
    let a = "/usr/share/fonts/NerdFonts/ttf/AdwaitaMonoNerdFont";
    let d = "/usr/share/fonts/TTF/DejaVuSansMono";
    let (base, styles): (&str, [&str; 4]) = if mode.starts_with('d') {
        (d, ["", "-Bold", "-Oblique", "-BoldOblique"])
    } else {
        (a, ["-Regular", "-Bold", "-Italic", "-BoldItalic"])
    };
    let paths: Vec<String> = styles.iter().map(|s| format!("{base}{s}.ttf")).collect();

    println!("start {}", rss());
    let mut fonts = Vec::new();
    for p in &paths {
        match std::fs::read(p)
            .ok()
            .and_then(|d| FontArc::try_from_vec(d).ok())
        {
            Some(f) => fonts.push(f),
            None => println!("  missing/err {p}"),
        }
    }
    println!("after load ({} faces, kept alive): {}", fonts.len(), rss());

    if mode.ends_with('g') {
        // Worst case: fill the bounded glyph cache (4096 entries, like rterm).
        let scale = PxScale::from(13.0);
        let mut cache: Vec<(u16, Vec<u8>)> = Vec::with_capacity(4096);
        'outer: for f in &fonts {
            for id in 0..f.glyph_count() as u16 {
                let Some(outlined) = f.outline_glyph(GlyphId(id).with_scale(scale)) else {
                    continue;
                };
                let b = outlined.px_bounds();
                let (w, h) = (b.width().round() as usize, b.height().round() as usize);
                let mut alpha = vec![0u8; w * h];
                outlined.draw(|x, y, c| {
                    if let Some(s) = alpha.get_mut(y as usize * w + x as usize) {
                        *s = (c.clamp(0.0, 1.0) * 255.0) as u8;
                    }
                });
                cache.push((id, alpha));
                if cache.len() >= 4096 {
                    break 'outer;
                }
            }
        }
        println!("after 4096 cached glyphs: {}", rss());
        std::hint::black_box(cache);
    }
    std::hint::black_box(fonts);
    println!("done  {}", rss());
}
