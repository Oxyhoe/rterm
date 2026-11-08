//! rterm configuration: everything is tunable through `~/.config/rterm/config.toml`.
//!
//! The file is watched; edits are applied live (falls back to defaults on parse errors).

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub window: WindowCfg,
    pub font: FontCfg,
    pub colors: ColorsCfg,
    pub theme: ThemeCfg,
    pub cursor: CursorCfg,
    pub effects: EffectsCfg,
    pub media: MediaCfg,
    pub ui: UiCfg,
    pub shell: ShellCfg,
    pub scrollback: ScrollbackCfg,
    pub prompt: PromptCfg,
    pub anim: AnimCfg,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct WindowCfg {
    pub width: u32,
    pub height: u32,
    pub title: String,
    /// Window opacity in 0.2..=1.0 (1.0 = fully opaque). Below 1 the desktop
    /// shows through the terminal where the compositor honours the alpha
    /// channel; otherwise the background fades towards black.
    pub opacity: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct FontCfg {
    /// Font family, e.g. "JetBrains Mono", "monospace" picks any system monospace font.
    pub family: String,
    pub bold_family: Option<String>,
    pub italic_family: Option<String>,
    /// Font size in logical pixels.
    pub size: f32,
    /// Horizontal shift of glyphs inside the cell.
    pub offset_x: i32,
    /// Vertical shift of glyphs inside the cell.
    pub offset_y: i32,
    /// Draw bold text with a 1px double-strike when no bold face is available.
    pub synthetic_bold: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ColorsCfg {
    /// Default cell background (used by `effects.background = "solid"` and as fallback).
    pub background: String,
    pub foreground: String,
    pub cursor: Option<String>,
    pub cursor_text: Option<String>,
    pub selection_background: Option<String>,
    pub selection_foreground: Option<String>,
    /// 16 ANSI colors, hex like "#7aa2f7".
    pub palette: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ThemeCfg {
    /// "manual" uses `[colors]` as written; "auto" follows the desktop
    /// dark/light scheme and takes the accent from the niri config
    /// (`focus-ring` / `border` active colour).
    pub mode: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CursorCfg {
    /// "block" | "beam" | "underline"
    pub style: String,
    pub blink: bool,
    pub blink_interval_ms: u64,
    /// Draw a hollow cursor when the window is not focused.
    pub unfocused_hollow: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct EffectsCfg {
    /// "solid" | "gradient" | "plasma" | "aurora" | "image"
    pub background: String,
    pub gradient_colors: Vec<String>,
    /// Gradient angle in degrees (0 = left→right).
    pub gradient_angle: f32,
    pub gradient_animated: bool,
    pub gradient_speed: f32,
    pub plasma_colors: Vec<String>,
    pub plasma_speed: f32,
    pub plasma_scale: f32,
    /// Path to a background image.
    pub image: Option<String>,
    /// "cover" | "contain"
    pub image_fit: String,
    /// Darken the background image, 0.0..=1.0.
    pub image_dim: f32,
    /// Box-blur radius for the background image in pixels (0 = off).
    pub image_blur: u32,
    /// Colors of the drifting aurora blobs (background = "aurora").
    pub aurora_colors: Vec<String>,
    /// How fast the aurora drifts, 0.0..=5.0.
    pub aurora_speed: f32,
    /// Soft halo around bright pixels.
    pub bloom: bool,
    /// Bloom strength, 0.0..=1.0.
    pub bloom_intensity: f32,
    /// Animated film grain.
    pub grain: bool,
    /// Grain strength, 0.0..=1.0.
    pub grain_intensity: f32,
    /// CRT scanlines overlay.
    pub scanlines: bool,
    pub scanline_intensity: f32,
    /// Scanline roll speed in lines per second (0 = static).
    pub scanline_speed: f32,
    /// Soft light around the cursor, the focused pane and the active tab.
    pub glow: bool,
    /// Fade the window in from black on startup.
    pub fade_in: bool,
    /// Darken corners.
    pub vignette: bool,
}

/// Entrance animation for freshly painted terminal lines.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AnimCfg {
    /// How long an appearing line takes to settle, in milliseconds (0 = off).
    pub ms: u64,
    /// "fade" | "slide" (drift in from the left) | "wipe" (uncover left→right).
    pub style: String,
    /// Animate the live prompt/command line too, not only output above it.
    pub prompt: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MediaCfg {
    /// Memory budget for decoded images/GIF frames, in megabytes.
    pub max_cache_mb: u64,
    /// Draw images above text (true) or below text (false).
    pub above_text: bool,
    /// Loop animated GIFs.
    pub loop_gifs: bool,
    /// Allow the local UNIX socket used by `rterm play` for video streaming.
    pub allow_socket: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct UiCfg {
    pub padding_x: u32,
    pub padding_y: u32,
    /// Copy selection to clipboard on mouse release.
    pub copy_on_select: bool,
    /// Lines per mouse wheel notch.
    pub scroll_lines: u32,
    /// Paste PRIMARY selection with middle click.
    pub middle_click_paste: bool,
    /// Settings panel opacity (0.2..=1.0): below 1 the terminal shows through.
    pub panel_alpha: f32,
    /// Frost the settings panel: blur the terminal content underneath.
    pub panel_blur: bool,
    /// Blur radius for the settings panel in pixels (0..=16).
    pub panel_blur_radius: u32,
    /// Tab strip visibility: "auto" (only with two or more tabs), "on", "off".
    /// Cycled live with Ctrl+Shift+B.
    pub tab_bar: String,
    /// Where the tab strip sits: "top" or "bottom".
    pub tab_bar_position: String,
    /// Strip design: "pill" (rounded, glossy active tab), "chip",
    /// "underline" or "minimal".
    pub tab_style: String,
    /// Strip background opacity in 0.2..=1.0 (below 1 the terminal shows through).
    pub tab_bar_alpha: f32,
    /// Draw a ✕ close button on every tab of the strip.
    pub tab_close: bool,
    /// Draw a "+" button after the last tab (click it to open a new tab).
    pub tab_new_button: bool,
    /// Maximum tab title width in characters before it is ellipsized.
    pub tab_max_width: u32,
    /// Active tab fill; "" = the accent colour.
    pub tab_active_bg: String,
    /// Active tab label; "" = the window background colour.
    pub tab_active_fg: String,
    /// Inactive tab label; "" = the terminal foreground.
    pub tab_inactive_fg: String,
    /// Strip background; "" = the terminal background colour.
    pub tab_bar_bg: String,
    /// Show a grey "ghost" of a history match after the cursor (autosuggest).
    pub suggest: bool,
    /// Colour of that ghost text.
    pub suggest_color: String,
    /// Liquid-glass finish for the settings panel (saturation + sheen + rim).
    pub glass: bool,
    /// Glass intensity, 0.0..=1.0.
    pub glass_strength: f32,
    /// Tint layered over the frosted panel, "#rrggbb".
    pub glass_tint: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ShellCfg {
    /// Override the login shell (default: $SHELL).
    pub program: Option<String>,
    pub args: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ScrollbackCfg {
    pub lines: usize,
}

/// The built-in prompt, drawn for new shells instead of starship / PS1.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PromptCfg {
    /// Install the prompt in new shells (bash only; off = shell default).
    pub enable: bool,
    pub show_dir: bool,
    /// "full" (`~/src/app`), "short" (`~/…/app`) or "last" (`app`).
    pub dir_style: String,
    pub show_git: bool,
    /// Append a `*` when the repository has uncommitted changes.
    pub git_dirty: bool,
    pub show_status: bool,
    /// Show how long the last command ran (seconds, when >= 2s).
    pub show_duration: bool,
    pub show_time: bool,
    pub symbol_dir: String,
    pub symbol_git: String,
    pub symbol_prompt: String,
    pub color_dir: String,
    pub color_git: String,
    pub color_err: String,
    pub color_dim: String,
    pub color_prompt: String,
    /// "plain" (coloured text) or "powerline" (filled segments with arrows).
    pub style: String,
    /// "custom" keeps the colours above; a theme name overrides them.
    pub theme: String,
    /// Draw segments on one line and the prompt character on a second one.
    pub multiline: bool,
    /// Use Nerd Font icons in the segment prefixes.
    pub icons: bool,
    /// Show the `user@host` segment.
    pub show_user: bool,
    /// Show the active Python virtualenv, when one is set.
    pub show_venv: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            window: WindowCfg::default(),
            font: FontCfg::default(),
            colors: ColorsCfg::default(),
            theme: ThemeCfg::default(),
            cursor: CursorCfg::default(),
            effects: EffectsCfg::default(),
            media: MediaCfg::default(),
            ui: UiCfg::default(),
            shell: ShellCfg::default(),
            scrollback: ScrollbackCfg::default(),
            prompt: PromptCfg::default(),
            anim: AnimCfg::default(),
        }
    }
}

impl Default for WindowCfg {
    fn default() -> Self {
        Self {
            width: 1100,
            height: 720,
            title: "rterm".into(),
            opacity: 1.0,
        }
    }
}

impl Default for FontCfg {
    fn default() -> Self {
        Self {
            family: "monospace".into(),
            bold_family: None,
            italic_family: None,
            size: 14.0,
            offset_x: 0,
            offset_y: 0,
            synthetic_bold: true,
        }
    }
}

impl Default for ColorsCfg {
    fn default() -> Self {
        Self {
            background: "#16161e".into(),
            foreground: "#c0caf5".into(),
            cursor: None,
            cursor_text: None,
            selection_background: None,
            selection_foreground: None,
            palette: [
                "#1a1b26", "#f7768e", "#9ece6a", "#e0af68", "#7aa2f7", "#bb9af7", "#7dcfff",
                "#a9b1d6", "#414868", "#ff757f", "#c3e88d", "#ffc777", "#82aaff", "#c792ea",
                "#89ddff", "#c0caf5",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect(),
        }
    }
}

impl Default for ThemeCfg {
    fn default() -> Self {
        Self {
            mode: "manual".into(),
        }
    }
}

impl Default for CursorCfg {
    fn default() -> Self {
        Self {
            style: "block".into(),
            blink: false,
            blink_interval_ms: 750,
            unfocused_hollow: true,
        }
    }
}

impl Default for EffectsCfg {
    fn default() -> Self {
        Self {
            background: "solid".into(),
            gradient_colors: ["#0f2027", "#203a43", "#2c5364"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
            gradient_angle: 135.0,
            gradient_animated: false,
            gradient_speed: 0.05,
            plasma_colors: ["#16161e", "#7aa2f7"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
            plasma_speed: 0.4,
            plasma_scale: 1.5,
            image: None,
            image_fit: "cover".into(),
            image_dim: 0.6,
            image_blur: 0,
            aurora_colors: ["#0b1026", "#7dcfff", "#bb9af7", "#9ece6a"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
            aurora_speed: 0.35,
            bloom: false,
            bloom_intensity: 0.4,
            grain: false,
            grain_intensity: 0.06,
            scanlines: false,
            scanline_intensity: 0.08,
            scanline_speed: 0.0,
            glow: true,
            fade_in: true,
            vignette: false,
        }
    }
}

impl Default for MediaCfg {
    fn default() -> Self {
        Self {
            max_cache_mb: 256,
            above_text: true,
            loop_gifs: true,
            allow_socket: true,
        }
    }
}

impl Default for UiCfg {
    fn default() -> Self {
        Self {
            padding_x: 6,
            padding_y: 4,
            copy_on_select: false,
            scroll_lines: 3,
            middle_click_paste: true,
            panel_alpha: 0.85,
            panel_blur: true,
            panel_blur_radius: 6,
            tab_bar: "auto".into(),
            tab_bar_position: "top".into(),
            tab_style: "pill".into(),
            tab_bar_alpha: 1.0,
            tab_close: true,
            tab_new_button: true,
            tab_max_width: 18,
            tab_active_bg: String::new(),
            tab_active_fg: String::new(),
            tab_inactive_fg: String::new(),
            tab_bar_bg: String::new(),
            suggest: true,
            suggest_color: "#7d8590".into(),
            glass: true,
            glass_strength: 0.6,
            glass_tint: "#ffffff".into(),
        }
    }
}

impl Default for ShellCfg {
    fn default() -> Self {
        Self {
            program: None,
            args: Vec::new(),
        }
    }
}

impl Default for ScrollbackCfg {
    fn default() -> Self {
        Self { lines: 10_000 }
    }
}

impl Default for PromptCfg {
    fn default() -> Self {
        Self {
            enable: true,
            show_dir: true,
            dir_style: "full".into(),
            show_git: true,
            git_dirty: true,
            show_status: true,
            show_duration: true,
            show_time: false,
            symbol_dir: "in".into(),
            symbol_git: "on".into(),
            symbol_prompt: "❯".into(),
            color_dir: "#7dcfff".into(),
            color_git: "#9ece6a".into(),
            color_err: "#f7768e".into(),
            color_dim: "#565f89".into(),
            color_prompt: "#bb9af7".into(),
            style: "powerline".into(),
            theme: "custom".into(),
            multiline: true,
            icons: true,
            show_user: false,
            show_venv: true,
        }
    }
}

impl Default for AnimCfg {
    fn default() -> Self {
        Self {
            ms: 140,
            style: "fade".into(),
            prompt: true,
        }
    }
}

/// Clamp everything into sane ranges so a bad config can never break the terminal.
impl Config {
    pub fn sanitize(&mut self) {
        self.window.width = self.window.width.clamp(200, 7680);
        self.window.height = self.window.height.clamp(100, 4320);
        self.window.opacity = self.window.opacity.clamp(0.2, 1.0);
        self.font.size = self.font.size.clamp(6.0, 72.0);
        self.font.offset_x = self.font.offset_x.clamp(-10, 10);
        self.font.offset_y = self.font.offset_y.clamp(-10, 10);
        if self.font.family.trim().is_empty() {
            self.font.family = "monospace".into();
        }
        self.theme.mode = self.theme.mode.trim().to_lowercase();
        if !matches!(self.theme.mode.as_str(), "manual" | "auto") {
            self.theme.mode = "manual".into();
        }
        self.cursor.style = self.cursor.style.to_lowercase();
        if !matches!(self.cursor.style.as_str(), "block" | "beam" | "underline") {
            self.cursor.style = "block".into();
        }
        self.cursor.blink_interval_ms = self.cursor.blink_interval_ms.clamp(100, 10_000);
        self.effects.background = self.effects.background.to_lowercase();
        if !matches!(
            self.effects.background.as_str(),
            "solid" | "gradient" | "plasma" | "aurora" | "image"
        ) {
            self.effects.background = "solid".into();
        }
        if self.effects.background == "image" && self.effects.image.is_none() {
            self.effects.background = "solid".into();
        }
        self.effects.gradient_angle = self.effects.gradient_angle.rem_euclid(360.0);
        self.effects.gradient_speed = self.effects.gradient_speed.clamp(0.0, 5.0);
        self.effects.plasma_speed = self.effects.plasma_speed.clamp(0.0, 5.0);
        self.effects.plasma_scale = self.effects.plasma_scale.clamp(0.1, 20.0);
        self.effects.image_dim = self.effects.image_dim.clamp(0.0, 1.0);
        self.effects.image_blur = self.effects.image_blur.min(32);
        self.effects.scanline_intensity = self.effects.scanline_intensity.clamp(0.0, 1.0);
        self.effects.scanline_speed = self.effects.scanline_speed.clamp(0.0, 8.0);
        self.effects.aurora_speed = self.effects.aurora_speed.clamp(0.0, 5.0);
        self.effects.bloom_intensity = self.effects.bloom_intensity.clamp(0.0, 1.0);
        self.effects.grain_intensity = self.effects.grain_intensity.clamp(0.0, 1.0);
        self.effects.image_fit = self.effects.image_fit.to_lowercase();
        if !matches!(self.effects.image_fit.as_str(), "cover" | "contain") {
            self.effects.image_fit = "cover".into();
        }
        if self.effects.gradient_colors.is_empty() {
            self.effects.gradient_colors = EffectsCfg::default().gradient_colors;
        }
        if self.effects.plasma_colors.is_empty() {
            self.effects.plasma_colors = EffectsCfg::default().plasma_colors;
        }
        if self.effects.aurora_colors.is_empty() {
            self.effects.aurora_colors = EffectsCfg::default().aurora_colors;
        }
        self.media.max_cache_mb = self.media.max_cache_mb.clamp(16, 8192);
        self.ui.padding_x = self.ui.padding_x.clamp(0, 100);
        self.ui.padding_y = self.ui.padding_y.clamp(0, 100);
        self.ui.scroll_lines = self.ui.scroll_lines.clamp(1, 100);
        self.ui.panel_alpha = self.ui.panel_alpha.clamp(0.2, 1.0);
        self.ui.panel_blur_radius = self.ui.panel_blur_radius.min(16);
        self.ui.tab_bar = self.ui.tab_bar.trim().to_lowercase();
        if !matches!(self.ui.tab_bar.as_str(), "auto" | "on" | "off") {
            self.ui.tab_bar = "auto".into();
        }
        self.ui.tab_bar_position = self.ui.tab_bar_position.trim().to_lowercase();
        if !matches!(self.ui.tab_bar_position.as_str(), "top" | "bottom") {
            self.ui.tab_bar_position = "top".into();
        }
        self.ui.tab_style = self.ui.tab_style.trim().to_lowercase();
        if !matches!(
            self.ui.tab_style.as_str(),
            "pill" | "chip" | "underline" | "minimal"
        ) {
            self.ui.tab_style = "pill".into();
        }
        self.ui.tab_bar_alpha = self.ui.tab_bar_alpha.clamp(0.2, 1.0);
        self.ui.tab_max_width = self.ui.tab_max_width.clamp(4, 64);
        // Optional colours: empty means "inherit from the theme", anything
        // that isn't a hex triple falls back to the same.
        for value in [
            &mut self.ui.tab_active_bg,
            &mut self.ui.tab_active_fg,
            &mut self.ui.tab_inactive_fg,
            &mut self.ui.tab_bar_bg,
        ] {
            let text = value.trim().to_string();
            *value = if text.is_empty() || crate::palette::parse_hex(&text).is_some() {
                text
            } else {
                String::new()
            };
        }
        let suggest_color = self.ui.suggest_color.trim().to_string();
        self.ui.suggest_color = if crate::palette::parse_hex(&suggest_color).is_some() {
            suggest_color
        } else {
            "#7d8590".into()
        };
        self.prompt.dir_style = self.prompt.dir_style.trim().to_lowercase();
        if !matches!(self.prompt.dir_style.as_str(), "full" | "short" | "last") {
            self.prompt.dir_style = "full".into();
        }
        self.prompt.style = self.prompt.style.trim().to_lowercase();
        if !matches!(self.prompt.style.as_str(), "plain" | "powerline") {
            self.prompt.style = "plain".into();
        }
        self.prompt.theme = self.prompt.theme.trim().to_lowercase();
        if !matches!(
            self.prompt.theme.as_str(),
            "custom" | "tokyo" | "gruvbox" | "dracula" | "nord" | "catppuccin"
        ) {
            self.prompt.theme = "custom".into();
        }
        self.ui.glass_strength = self.ui.glass_strength.clamp(0.0, 1.0);
        let glass_tint = self.ui.glass_tint.trim().to_string();
        self.ui.glass_tint = if crate::palette::parse_hex(&glass_tint).is_some() {
            glass_tint
        } else {
            "#ffffff".into()
        };
        self.anim.ms = self.anim.ms.min(2000);
        self.anim.style = self.anim.style.trim().to_lowercase();
        if !matches!(
            self.anim.style.as_str(),
            "fade" | "slide" | "wipe" | "rise" | "drop" | "type" | "dissolve" | "glow"
        ) {
            self.anim.style = "fade".into();
        }
        if self.colors.palette.len() != 16 {
            let mut p = ColorsCfg::default().palette;
            for (i, c) in self.colors.palette.iter().take(16).enumerate() {
                p[i] = c.clone();
            }
            self.colors.palette = p;
        }
        self.scrollback.lines = self.scrollback.lines.min(1_000_000);
    }

    pub fn config_dir() -> PathBuf {
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
            .unwrap_or_else(|| PathBuf::from(".config"));
        base.join("rterm")
    }

    pub fn config_path() -> PathBuf {
        Self::config_dir().join("config.toml")
    }

    /// Load config; missing file -> defaults, broken file -> defaults + warning.
    pub fn load() -> (Self, Option<PathBuf>) {
        match Self::load_from(&Self::config_path()) {
            Ok(Some(cfg)) => (cfg, Some(Self::config_path())),
            Ok(None) => (Self::default(), None),
            Err(err) => {
                eprintln!("rterm: config error: {err}; using defaults");
                (Self::default(), Some(Self::config_path()))
            }
        }
    }

    /// `Ok(None)` when the file does not exist.
    pub fn load_from(path: &Path) -> anyhow::Result<Option<Self>> {
        let text = match fs::read_to_string(path) {
            Ok(text) => text,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(err) => return Err(err.into()),
        };
        let mut cfg: Self = toml::from_str(&text)?;
        cfg.sanitize();
        Ok(Some(cfg))
    }

    /// Write a fully generated example config.
    pub fn write_default(path: &Path) -> anyhow::Result<()> {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        fs::write(path, Self::example_toml())?;
        Ok(())
    }

    /// Serialize the current config (sanitized) over `path`.
    pub fn save(path: &Path, cfg: &Config) -> anyhow::Result<()> {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let mut cfg = cfg.clone();
        cfg.sanitize();
        fs::write(path, toml::to_string_pretty(&cfg)?)?;
        Ok(())
    }

    pub fn example_toml() -> String {
        format!(
            "{}\n{}",
            EXAMPLE_HEADER,
            toml::to_string_pretty(&Config::default()).unwrap_or_default()
        )
    }

    pub fn mtime(path: &Path) -> Option<std::time::SystemTime> {
        fs::metadata(path).and_then(|m| m.modified()).ok()
    }
}

const EXAMPLE_HEADER: &str = "\
# rterm configuration — edit freely, changes are applied live.
# Regenerate with: rterm config --init
# Open in a window with: rterm config --edit   (or press F1 in the terminal)
#
# Background: solid | gradient | plasma | aurora | image
#   gradient/plasma/aurora animate with *_speed; image takes effects.image.
# Extra looks: bloom (halo on bright pixels), grain (film noise),
#   glow (cursor / focused pane / active tab light), fade_in,
#   scanlines + scanline_speed (rolling CRT lines), vignette.
# window.opacity is window transparency (0.2 .. 1.0): below 1 the desktop
#   shows through the terminal (Wayland alpha); on compositors that ignore
#   alpha the background simply fades towards black.
# ui.panel_alpha (0.2 .. 1.0) makes the F1 settings panel see-through and
#   ui.panel_blur frosts it (blur radius ui.panel_blur_radius).
# ui.glass adds the liquid-glass finish on top: saturation, a glass_tint film,
#   a diagonal sheen and a bright inner rim (ui.glass_strength 0 .. 1).
# Text entrance animation: [anim] ms (0 = off, max 2000),
#   style = fade | slide | wipe | rise | drop | type | dissolve | glow,
#   prompt = animate the live prompt/command line too.
# ui.suggest shows the grey autosuggestion after the cursor (End or →
#   accepts it), ui.suggest_color tints that ghost text.
# Tabs: ui.tab_bar = auto | on | off, tab_style = pill | chip | underline |
#   minimal, tab_bar_position = top | bottom, tab_close adds ✕ to every tab,
#   tab_new_button adds a "+" that opens a tab, tab_max_width truncates titles.
# [prompt] is the built-in bash prompt (starship-style segments); set
#   prompt.enable = false to fall back to the shell's own prompt.
#   prompt.style = plain | powerline; prompt.theme = custom | tokyo | gruvbox |
#   dracula | nord | catppuccin; multiline splits segments from the ❯ line;
#   icons uses Nerd Font glyphs; user adds user@host; venv shows $VIRTUAL_ENV.
#
# theme.mode = auto     follows the desktop dark/light scheme (GTK/KDE settings)
#   and takes the accent colour from ~/.config/niri/config.kdl (focus-ring or
#   border active colour) for selection, tabs and the focus ring.
";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_roundtrip_through_toml() {
        let cfg = Config::default();
        let text = toml::to_string_pretty(&cfg).unwrap();
        let back: Config = toml::from_str(&text).unwrap();
        assert_eq!(cfg, back);
    }

    #[test]
    fn unknown_key_is_rejected() {
        assert!(toml::from_str::<Config>("[window]\nbogus = 1\n").is_err());
    }

    #[test]
    fn sanitize_fixes_bad_values() {
        let mut cfg: Config =
            toml::from_str("[window]\nwidth = 5\nopacity = 9.0\n[cursor]\nstyle = \"weird\"\n")
                .unwrap();
        cfg.sanitize();
        assert_eq!(cfg.window.width, 200);
        assert_eq!(cfg.window.opacity, 1.0);
        assert_eq!(cfg.cursor.style, "block");
    }

    #[test]
    fn example_parses_back() {
        let text = Config::example_toml();
        let body = text.strip_prefix(EXAMPLE_HEADER).unwrap();
        let mut cfg: Config = toml::from_str(body).unwrap();
        cfg.sanitize();
        assert_eq!(cfg, Config::default());
    }

    #[test]
    fn load_missing_returns_none() {
        let res = Config::load_from(Path::new("/nonexistent/rterm/config.toml")).unwrap();
        assert!(res.is_none());
    }
}
