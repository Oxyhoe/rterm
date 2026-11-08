//! Built-in colour schemes cycled from the settings overlay (`presets` group).
//!
//! Applying a preset writes `[colors]` and forces `theme.mode = "manual"` so the
//! desktop scheme cannot immediately override the choice.

use crate::config::ColorsCfg;

pub struct Preset {
    pub name: &'static str,
    pub background: &'static str,
    pub foreground: &'static str,
    pub selection: &'static str,
    pub palette: [&'static str; 16],
}

impl Preset {
    pub fn colors(&self) -> ColorsCfg {
        let bg_light = luma(self.background) > luma(self.foreground);
        ColorsCfg {
            background: self.background.into(),
            foreground: self.foreground.into(),
            cursor: None,
            cursor_text: None,
            selection_background: Some(self.selection.into()),
            // Light schemes need dark text inside the (usually blue) selection.
            selection_foreground: Some(
                (if bg_light {
                    self.background
                } else {
                    self.foreground
                })
                .into(),
            ),
            palette: self.palette.iter().map(|s| s.to_string()).collect(),
        }
    }
}

pub const PRESETS: [Preset; 11] = [
    Preset {
        name: "tokyo night",
        background: "#16161e",
        foreground: "#c0caf5",
        selection: "#7aa2f7",
        palette: [
            "#1a1b26", "#f7768e", "#9ece6a", "#e0af68", "#7aa2f7", "#bb9af7", "#7dcfff", "#a9b1d6",
            "#414868", "#ff757f", "#c3e88d", "#ffc777", "#82aaff", "#c792ea", "#89ddff", "#c0caf5",
        ],
    },
    Preset {
        name: "gruvbox",
        background: "#282828",
        foreground: "#ebdbb2",
        selection: "#458588",
        palette: [
            "#282828", "#cc241d", "#98971a", "#d79921", "#4588b2", "#b16286", "#689d6a", "#a89984",
            "#928374", "#fb4934", "#b8bb26", "#fabd2f", "#83a598", "#d3869b", "#8ec07c", "#ebdbb2",
        ],
    },
    Preset {
        name: "catppuccin",
        background: "#1e1e2e",
        foreground: "#cdd6f4",
        selection: "#89b4fa",
        palette: [
            "#1e1e2e", "#f38ba8", "#a6e3a1", "#f9e2af", "#89b4fa", "#f5c2e7", "#94e2d5", "#bac2de",
            "#585b70", "#f38ba8", "#a6e3a1", "#f9e2af", "#89b4fa", "#f5c2e7", "#94e2d5", "#cdd6f4",
        ],
    },
    Preset {
        name: "nord",
        background: "#2e3440",
        foreground: "#d8dee9",
        selection: "#88c0d0",
        palette: [
            "#3b4252", "#bf616a", "#a3be8c", "#ebcb8b", "#81a1c1", "#b48ead", "#88c0d0", "#e5e9f0",
            "#4c566a", "#bf616a", "#a3be8c", "#ebcb8b", "#81a1c1", "#b48ead", "#88c0d0", "#eceff4",
        ],
    },
    Preset {
        name: "dracula",
        background: "#282a36",
        foreground: "#f8f8f2",
        selection: "#bd93f9",
        palette: [
            "#21222c", "#ff5555", "#50fa7b", "#f1fa8c", "#bd93f9", "#ff79c6", "#8be9fd", "#f8f8f2",
            "#6272a4", "#ff6e6e", "#69ff94", "#f4ffaa", "#d6acff", "#ff92df", "#a4ffff", "#f8f8f2",
        ],
    },
    Preset {
        name: "one dark",
        background: "#282c34",
        foreground: "#abb2bf",
        selection: "#61afef",
        palette: [
            "#282c34", "#e06c75", "#98c379", "#e5c07b", "#61afef", "#c678dd", "#56b6c2", "#abb2bf",
            "#5c6370", "#e06c75", "#98c379", "#e5c07b", "#61afef", "#c678dd", "#56b6c2", "#abb2bf",
        ],
    },
    Preset {
        name: "everforest",
        background: "#2d353b",
        foreground: "#d3c6aa",
        selection: "#a7c080",
        palette: [
            "#2d353b", "#e67e80", "#a7c080", "#dbbc7f", "#7fbbb3", "#d699b6", "#83c092", "#d3c6aa",
            "#56635f", "#e67e80", "#a7c080", "#dbbc7f", "#7fbbb3", "#d699b6", "#83c092", "#d3c6aa",
        ],
    },
    Preset {
        name: "rose pine",
        background: "#191724",
        foreground: "#e0def4",
        selection: "#9ccfd8",
        palette: [
            "#191724", "#eb6f92", "#31748f", "#f6c177", "#c4a7e7", "#9ccfd8", "#ebbcba", "#e0def4",
            "#26233a", "#eb6f92", "#31748f", "#f6c177", "#c4a7e7", "#9ccfd8", "#ebbcba", "#e0def4",
        ],
    },
    Preset {
        name: "monokai",
        background: "#272822",
        foreground: "#f8f8f2",
        selection: "#a6e22e",
        palette: [
            "#272822", "#f92672", "#a6e22e", "#e6db74", "#66d9ef", "#ae81ff", "#a1efe4", "#f8f8f2",
            "#75715e", "#f92672", "#a6e22e", "#e6db74", "#66d9ef", "#ae81ff", "#a1efe4", "#f8f8f2",
        ],
    },
    Preset {
        name: "solarized dark",
        background: "#002b36",
        foreground: "#93a1a1",
        selection: "#268bd2",
        palette: [
            "#073642", "#dc322f", "#859900", "#b58900", "#268bd2", "#d33682", "#2aa198", "#eee8d5",
            "#002b36", "#cb4b16", "#586e75", "#657b83", "#839496", "#6c71c4", "#93a1a1", "#fdf6e3",
        ],
    },
    Preset {
        name: "solarized light",
        background: "#fdf6e3",
        foreground: "#657b83",
        selection: "#268bd2",
        palette: [
            "#073642", "#dc322f", "#859900", "#b58900", "#268bd2", "#d33682", "#2aa198", "#eee8d5",
            "#002b36", "#cb4b16", "#586e75", "#657b83", "#839496", "#6c71c4", "#93a1a1", "#fdf6e3",
        ],
    },
];

/// Index of the preset `colors` matches, if any (background, foreground, palette).
pub fn index_of(colors: &ColorsCfg) -> Option<usize> {
    if colors.palette.len() != 16 {
        return None;
    }
    PRESETS.iter().position(|p| {
        colors.background.eq_ignore_ascii_case(p.background)
            && colors.foreground.eq_ignore_ascii_case(p.foreground)
            && colors
                .palette
                .iter()
                .zip(p.palette.iter())
                .all(|(a, b)| a.eq_ignore_ascii_case(b))
    })
}

pub fn name_of(colors: &ColorsCfg) -> Option<&'static str> {
    index_of(colors).map(|i| PRESETS[i].name)
}

/// Rec. 709 luma of a `#rrggbb` string (0.0 black .. 1.0 white).
fn luma(hex: &str) -> f32 {
    let v = u32::from_str_radix(hex.trim_start_matches('#'), 16).unwrap_or(0);
    let (r, g, b) = ((v >> 16) & 0xff, (v >> 8) & 0xff, v & 0xff);
    (0.2126 * r as f32 + 0.7152 * g as f32 + 0.0722 * b as f32) / 255.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokyo_night_is_the_default_palette() {
        let colors = ColorsCfg::default();
        assert_eq!(name_of(&colors), Some("tokyo night"));
    }

    #[test]
    fn every_preset_builds_a_valid_palette() {
        for preset in &PRESETS {
            let colors = preset.colors();
            assert_eq!(colors.palette.len(), 16, "{}", preset.name);
            assert!(colors.selection_background.is_some(), "{}", preset.name);
            assert!(colors.background.starts_with('#'), "{}", preset.name);
            // The preset must be recognised again after building.
            assert_eq!(name_of(&colors), Some(preset.name), "{}", preset.name);
        }
    }

    #[test]
    fn names_are_unique() {
        for (i, a) in PRESETS.iter().enumerate() {
            for b in &PRESETS[i + 1..] {
                assert_ne!(a.name, b.name);
            }
        }
    }

    #[test]
    fn unknown_colors_match_no_preset() {
        let mut colors = ColorsCfg::default();
        colors.background = "#010203".into();
        assert_eq!(index_of(&colors), None);
    }
}
