//! Color handling: hex parsing, the 256-color palette and resolution of
//! `alacritty_terminal` colors into RGB.

use alacritty_terminal::ansi::{Color, NamedColor};
use alacritty_terminal::term::color::{Colors, Rgb};

use crate::config::ColorsCfg;

pub type Rgb8 = [u8; 3];

pub fn parse_hex(s: &str) -> Option<Rgb8> {
    let t = s.trim().trim_start_matches('#');
    if t.len() == 3 {
        let r = u8::from_str_radix(&t[0..1], 16).ok()?;
        let g = u8::from_str_radix(&t[1..2], 16).ok()?;
        let b = u8::from_str_radix(&t[2..3], 16).ok()?;
        return Some([r * 17, g * 17, b * 17]);
    }
    if t.len() != 6 {
        return None;
    }
    let v = u32::from_str_radix(t, 16).ok()?;
    Some([(v >> 16) as u8, (v >> 8) as u8, v as u8])
}

pub fn parse_hex_or(s: &str, fallback: Rgb8) -> Rgb8 {
    parse_hex(s).unwrap_or(fallback)
}

/// `#rrggbb` for an RGB triple (what the settings rows and config store).
pub fn to_hex(c: Rgb8) -> String {
    format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2])
}

/// RGB → HSL with hue in `0..360`, saturation/lightness in `0..100`.
pub fn rgb_to_hsl(c: Rgb8) -> (f32, f32, f32) {
    let r = f32::from(c[0]) / 255.0;
    let g = f32::from(c[1]) / 255.0;
    let b = f32::from(c[2]) / 255.0;
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) / 2.0;
    if (max - min).abs() < f32::EPSILON {
        return (0.0, 0.0, l * 100.0);
    }
    let d = max - min;
    let s = if l > 0.5 {
        d / (2.0 - max - min)
    } else {
        d / (max + min)
    };
    let h = if max == r {
        ((g - b) / d) % 6.0
    } else if max == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    };
    let h = (h * 60.0).rem_euclid(360.0);
    (h, s * 100.0, l * 100.0)
}

/// HSL → RGB; hue wraps, s/l clamp to `0..100`.
pub fn hsl_to_rgb(mut h: f32, s: f32, l: f32) -> Rgb8 {
    h = h.rem_euclid(360.0);
    let s = s.clamp(0.0, 100.0) / 100.0;
    let l = l.clamp(0.0, 100.0) / 100.0;
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let x = c * (1.0 - (((h / 60.0) % 2.0) - 1.0).abs());
    let m = l - c / 2.0;
    let (r, g, b) = match (h / 60.0) as i32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    [
        ((r + m) * 255.0).round().clamp(0.0, 255.0) as u8,
        ((g + m) * 255.0).round().clamp(0.0, 255.0) as u8,
        ((b + m) * 255.0).round().clamp(0.0, 255.0) as u8,
    ]
}

/// Full 256-entry palette: 16 theme colors + xterm cube + grays.
pub struct Palette {
    pub colors: [Rgb8; 256],
}

impl Palette {
    pub fn from_cfg(cfg: &ColorsCfg) -> Self {
        let mut colors = [[0u8; 3]; 256];
        for (i, spec) in cfg.palette.iter().enumerate() {
            colors[i] = parse_hex_or(spec, colors[i]);
        }
        // 16..232: 6x6x6 color cube.
        let levels = [0u8, 95, 135, 175, 215, 255];
        for i in 0..216 {
            let (r, g, b) = (i / 36, (i / 6) % 6, i % 6);
            colors[16 + i] = [levels[r], levels[g], levels[b]];
        }
        // 232..256: grayscale ramp.
        for i in 0..24 {
            let v = (8 + i * 10) as u8;
            colors[232 + i] = [v, v, v];
        }
        Self { colors }
    }

    pub fn get(&self, i: usize) -> Rgb8 {
        self.colors[i & 0xff]
    }
}

/// Linear blend of two colors.
pub fn mix(a: Rgb8, b: Rgb8, t: f32) -> Rgb8 {
    let t = t.clamp(0.0, 1.0);
    let step = |x: u8, y: u8| (f32::from(x) + (f32::from(y) - f32::from(x)) * t) as u8;
    [step(a[0], b[0]), step(a[1], b[1]), step(a[2], b[2])]
}

pub fn dim(c: Rgb8) -> Rgb8 {
    let step = |v: u8| ((v as u16 * 2) / 3) as u8;
    [step(c[0]), step(c[1]), step(c[2])]
}

/// Resolve an alacritty color, honoring OSC overrides, against our palette/theme.
pub fn resolve(color: Color, overrides: &Colors, pal: &Palette, cfg: &ColorsCfg) -> Rgb8 {
    match color {
        Color::Named(named) => resolve_named(named, overrides, pal, cfg),
        Color::Indexed(i) => match overrides[i as usize] {
            Some(rgb) => [rgb.r, rgb.g, rgb.b],
            None => pal.get(i as usize),
        },
        Color::Spec(rgb) => [rgb.r, rgb.g, rgb.b],
    }
}

fn resolve_named(named: NamedColor, overrides: &Colors, pal: &Palette, cfg: &ColorsCfg) -> Rgb8 {
    if let Some(rgb) = overrides[named] {
        return [rgb.r, rgb.g, rgb.b];
    }
    match named {
        NamedColor::Foreground => parse_hex_or(&cfg.foreground, [0xcc, 0xcc, 0xcc]),
        NamedColor::BrightForeground => parse_hex_or(&cfg.foreground, [0xee, 0xee, 0xee]),
        NamedColor::Background => parse_hex_or(&cfg.background, [0x10, 0x10, 0x10]),
        NamedColor::Cursor => cfg
            .cursor
            .as_deref()
            .and_then(parse_hex)
            .unwrap_or_else(|| parse_hex_or(&cfg.foreground, [0xff, 0xff, 0xff])),
        NamedColor::DimForeground => dim(parse_hex_or(&cfg.foreground, [0xcc, 0xcc, 0xcc])),
        // Ansi names 0..=15 map directly onto the theme palette.
        other => pal.get(other as usize),
    }
}

/// Convert our Rgb8 to alacritty's Rgb (for OSC color replies).
pub fn to_alacritty(c: Rgb8) -> Rgb {
    Rgb {
        r: c[0],
        g: c[1],
        b: c[2],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hex_forms() {
        assert_eq!(parse_hex("#fff"), Some([255, 255, 255]));
        assert_eq!(parse_hex("000000"), Some([0, 0, 0]));
        assert_eq!(parse_hex("#1a2b3c"), Some([0x1a, 0x2b, 0x3c]));
        assert_eq!(parse_hex("nope"), None);
    }

    #[test]
    fn cube_and_gray_ramp() {
        let pal = Palette::from_cfg(&ColorsCfg::default());
        assert_eq!(pal.get(16), [0, 0, 0]);
        assert_eq!(pal.get(231), [255, 255, 255]);
        assert_eq!(pal.get(232), [8, 8, 8]);
        assert_eq!(pal.get(255), [238, 238, 238]);
        // First 16 come from the theme.
        assert_eq!(pal.get(1), parse_hex("#f7768e").unwrap());
    }

    #[test]
    fn named_foreground_from_theme() {
        let cfg = ColorsCfg::default();
        let pal = Palette::from_cfg(&cfg);
        let overrides = Colors::default();
        let got = resolve(Color::Named(NamedColor::Foreground), &overrides, &pal, &cfg);
        assert_eq!(got, parse_hex(&cfg.foreground).unwrap());
    }

    #[test]
    fn hsl_roundtrip_is_stable() {
        for hex in [
            "#ff0000", "#00ff00", "#0000ff", "#16161e", "#7aa2f7", "#c0caf5",
        ] {
            let c = parse_hex(hex).unwrap();
            let (h, s, l) = rgb_to_hsl(c);
            let back = hsl_to_rgb(h, s, l);
            for i in 0..3 {
                let diff = (c[i] as i32 - back[i] as i32).abs();
                assert!(diff <= 2, "{hex}: {:?} vs {:?}", c, back);
            }
        }
    }

    #[test]
    fn hsl_anchors() {
        assert_eq!(hsl_to_rgb(0.0, 100.0, 50.0), [255, 0, 0]);
        assert_eq!(hsl_to_rgb(120.0, 100.0, 50.0), [0, 255, 0]);
        assert_eq!(hsl_to_rgb(240.0, 100.0, 50.0), [0, 0, 255]);
        assert_eq!(hsl_to_rgb(0.0, 0.0, 0.0), [0, 0, 0]);
        assert_eq!(hsl_to_rgb(360.0, 0.0, 100.0), [255, 255, 255]);
        // Hue wraps, s/l clamp.
        assert_eq!(
            hsl_to_rgb(480.0, 100.0, 50.0),
            hsl_to_rgb(120.0, 100.0, 50.0)
        );
        assert_eq!(hsl_to_rgb(0.0, 999.0, -5.0), [0, 0, 0]);
        assert_eq!(to_hex([0x7a, 0xa2, 0xf7]), "#7aa2f7");
    }
}
