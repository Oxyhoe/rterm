//! In-terminal settings: the `F1` overlay and the `rterm config --edit` editor.
//!
//! Values preview live — the app re-applies the config on every step — while
//! writing to `config.toml` stays an explicit action (`Enter` / `S`).
//!
//! The list is long, so the panel windows it: `geometry` shows only the rows
//! that fit, centred on the selection. String rows (colours, paths, families,
//! title) open a tiny editor in place via [`EditState`].

use std::path::PathBuf;
use std::process::Command;

use crate::config::{ColorsCfg, Config};
use crate::font::FontSystem;
use crate::palette::{self, Rgb8};
use crate::presets;
use crate::render::{scale_rgb, Renderer};

/// One editable row of the settings list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    ProfileSelect,
    ProfileSave,
    ProfileDelete,
    Preset,
    Opacity,
    Title,
    FontFamily,
    BoldFamily,
    ItalicFamily,
    FontSize,
    OffsetX,
    OffsetY,
    SyntheticBold,
    Bg,
    Fg,
    CursorColor,
    SelBg,
    Ansi(u8),
    CursorStyle,
    CursorBlink,
    BlinkInterval,
    UnfocusedHollow,
    PaddingX,
    PaddingY,
    CopyOnSelect,
    MiddlePaste,
    ScrollLines,
    PanelAlpha,
    PanelBlur,
    PanelBlurRadius,
    TabBar,
    TabPosition,
    TabStyle,
    TabAlpha,
    TabClose,
    TabActiveBg,
    TabActiveFg,
    TabInactiveFg,
    TabBarBg,
    Suggest,
    SuggestColor,
    Scrollback,
    ThemeMode,
    Background,
    ImagePath,
    ImageFit,
    ImageDim,
    ImageBlur,
    GradientAnimated,
    GradientAngle,
    GradientSpeed,
    PlasmaSpeed,
    PlasmaScale,
    AuroraSpeed,
    Bloom,
    BloomIntensity,
    Grain,
    GrainIntensity,
    Scanlines,
    ScanlineIntensity,
    ScanlineSpeed,
    Glow,
    FadeIn,
    Vignette,
    AboveText,
    LoopGifs,
    MaxCacheMb,
    ShellProgram,
    PromptEnable,
    PromptDirStyle,
    PromptGit,
    PromptGitDirty,
    PromptStatus,
    PromptDuration,
    PromptTime,
    PromptSymDir,
    PromptSymGit,
    PromptSymPrompt,
    PromptColDir,
    PromptColGit,
    PromptColErr,
    PromptColDim,
    PromptColPrompt,
    AnimMs,
    AnimStyle,
    AnimPrompt,
    Glass,
    GlassStrength,
    GlassTint,
    PromptStyle,
    PromptTheme,
    PromptMultiline,
    PromptIcons,
    PromptUser,
    PromptVenv,
}

impl Field {
    pub const ALL: [Field; 110] = [
        Field::ProfileSelect,
        Field::ProfileSave,
        Field::ProfileDelete,
        Field::Preset,
        Field::Opacity,
        Field::Title,
        Field::FontFamily,
        Field::BoldFamily,
        Field::ItalicFamily,
        Field::FontSize,
        Field::OffsetX,
        Field::OffsetY,
        Field::SyntheticBold,
        Field::Bg,
        Field::Fg,
        Field::CursorColor,
        Field::SelBg,
        Field::Ansi(0),
        Field::Ansi(1),
        Field::Ansi(2),
        Field::Ansi(3),
        Field::Ansi(4),
        Field::Ansi(5),
        Field::Ansi(6),
        Field::Ansi(7),
        Field::Ansi(8),
        Field::Ansi(9),
        Field::Ansi(10),
        Field::Ansi(11),
        Field::Ansi(12),
        Field::Ansi(13),
        Field::Ansi(14),
        Field::Ansi(15),
        Field::CursorStyle,
        Field::CursorBlink,
        Field::BlinkInterval,
        Field::UnfocusedHollow,
        Field::PaddingX,
        Field::PaddingY,
        Field::CopyOnSelect,
        Field::MiddlePaste,
        Field::ScrollLines,
        Field::PanelAlpha,
        Field::PanelBlur,
        Field::PanelBlurRadius,
        Field::Glass,
        Field::GlassStrength,
        Field::GlassTint,
        Field::TabBar,
        Field::TabPosition,
        Field::TabStyle,
        Field::TabAlpha,
        Field::TabClose,
        Field::TabActiveBg,
        Field::TabActiveFg,
        Field::TabInactiveFg,
        Field::TabBarBg,
        Field::Suggest,
        Field::SuggestColor,
        Field::AnimMs,
        Field::AnimStyle,
        Field::AnimPrompt,
        Field::Scrollback,
        Field::ThemeMode,
        Field::Background,
        Field::ImagePath,
        Field::ImageFit,
        Field::ImageDim,
        Field::ImageBlur,
        Field::GradientAnimated,
        Field::GradientAngle,
        Field::GradientSpeed,
        Field::PlasmaSpeed,
        Field::PlasmaScale,
        Field::AuroraSpeed,
        Field::Bloom,
        Field::BloomIntensity,
        Field::Grain,
        Field::GrainIntensity,
        Field::Scanlines,
        Field::ScanlineIntensity,
        Field::ScanlineSpeed,
        Field::Glow,
        Field::FadeIn,
        Field::Vignette,
        Field::AboveText,
        Field::LoopGifs,
        Field::MaxCacheMb,
        Field::ShellProgram,
        Field::PromptEnable,
        Field::PromptDirStyle,
        Field::PromptGit,
        Field::PromptGitDirty,
        Field::PromptStatus,
        Field::PromptDuration,
        Field::PromptTime,
        Field::PromptSymDir,
        Field::PromptSymGit,
        Field::PromptSymPrompt,
        Field::PromptColDir,
        Field::PromptColGit,
        Field::PromptColErr,
        Field::PromptColDim,
        Field::PromptColPrompt,
        Field::PromptStyle,
        Field::PromptTheme,
        Field::PromptMultiline,
        Field::PromptIcons,
        Field::PromptUser,
        Field::PromptVenv,
    ];

    /// Section header shown above the first row of each group.
    pub fn group(self) -> &'static str {
        match self {
            Field::ProfileSelect | Field::ProfileSave | Field::ProfileDelete => "profiles",
            Field::Preset => "presets",
            Field::Opacity | Field::Title => "window",
            Field::FontFamily
            | Field::BoldFamily
            | Field::ItalicFamily
            | Field::FontSize
            | Field::OffsetX
            | Field::OffsetY
            | Field::SyntheticBold => "font",
            Field::Bg | Field::Fg | Field::CursorColor | Field::SelBg | Field::Ansi(_) => "colors",
            Field::CursorStyle
            | Field::CursorBlink
            | Field::BlinkInterval
            | Field::UnfocusedHollow => "cursor",
            Field::PaddingX
            | Field::PaddingY
            | Field::CopyOnSelect
            | Field::MiddlePaste
            | Field::ScrollLines
            | Field::PanelAlpha
            | Field::PanelBlur
            | Field::PanelBlurRadius
            | Field::Glass
            | Field::GlassStrength
            | Field::GlassTint => "ui",
            Field::TabBar
            | Field::TabPosition
            | Field::TabStyle
            | Field::TabAlpha
            | Field::TabClose
            | Field::TabActiveBg
            | Field::TabActiveFg
            | Field::TabInactiveFg
            | Field::TabBarBg => "tabs",
            Field::Suggest | Field::SuggestColor => "suggest",
            Field::AnimMs | Field::AnimStyle | Field::AnimPrompt => "anim",
            Field::Scrollback => "scrollback",
            Field::ThemeMode => "theme",
            Field::Background
            | Field::ImagePath
            | Field::ImageFit
            | Field::ImageDim
            | Field::ImageBlur
            | Field::GradientAnimated
            | Field::GradientAngle
            | Field::GradientSpeed
            | Field::PlasmaSpeed
            | Field::PlasmaScale
            | Field::AuroraSpeed
            | Field::Bloom
            | Field::BloomIntensity
            | Field::Grain
            | Field::GrainIntensity
            | Field::Scanlines
            | Field::ScanlineIntensity
            | Field::ScanlineSpeed
            | Field::Glow
            | Field::FadeIn
            | Field::Vignette => "effects",
            Field::AboveText | Field::LoopGifs | Field::MaxCacheMb => "media",
            Field::ShellProgram => "shell",
            Field::PromptEnable
            | Field::PromptDirStyle
            | Field::PromptGit
            | Field::PromptGitDirty
            | Field::PromptStatus
            | Field::PromptDuration
            | Field::PromptTime
            | Field::PromptSymDir
            | Field::PromptSymGit
            | Field::PromptSymPrompt
            | Field::PromptColDir
            | Field::PromptColGit
            | Field::PromptColErr
            | Field::PromptColDim
            | Field::PromptColPrompt
            | Field::PromptStyle
            | Field::PromptTheme
            | Field::PromptMultiline
            | Field::PromptIcons
            | Field::PromptUser
            | Field::PromptVenv => "prompt",
        }
    }

    pub fn label(self) -> String {
        match self {
            Field::ProfileSelect => "profile".into(),
            Field::ProfileSave => "save as".into(),
            Field::ProfileDelete => "delete".into(),
            Field::Preset => "preset".into(),
            Field::Opacity => "opacity".into(),
            Field::Title => "title".into(),
            Field::FontFamily => "family".into(),
            Field::BoldFamily => "bold family".into(),
            Field::ItalicFamily => "italic family".into(),
            Field::FontSize => "size".into(),
            Field::OffsetX => "offset x".into(),
            Field::OffsetY => "offset y".into(),
            Field::SyntheticBold => "synthetic bold".into(),
            Field::Bg => "background".into(),
            Field::Fg => "foreground".into(),
            Field::CursorColor => "cursor color".into(),
            Field::SelBg => "selection color".into(),
            Field::Ansi(i) => [
                "black",
                "red",
                "green",
                "yellow",
                "blue",
                "magenta",
                "cyan",
                "white",
                "bright black",
                "bright red",
                "bright green",
                "bright yellow",
                "bright blue",
                "bright magenta",
                "bright cyan",
                "bright white",
            ]
            .get(i as usize)
            .copied()
            .unwrap_or("colour")
            .to_string(),
            Field::CursorStyle => "style".into(),
            Field::CursorBlink => "blink".into(),
            Field::BlinkInterval => "blink interval".into(),
            Field::UnfocusedHollow => "hollow unfocused".into(),
            Field::PaddingX => "padding x".into(),
            Field::PaddingY => "padding y".into(),
            Field::CopyOnSelect => "copy on select".into(),
            Field::MiddlePaste => "middle-click paste".into(),
            Field::ScrollLines => "scroll lines".into(),
            Field::PanelAlpha => "panel alpha".into(),
            Field::PanelBlur => "panel blur".into(),
            Field::PanelBlurRadius => "blur radius".into(),
            Field::TabBar => "tab bar".into(),
            Field::TabPosition => "tab position".into(),
            Field::TabStyle => "tab style".into(),
            Field::TabAlpha => "tab alpha".into(),
            Field::TabClose => "close button".into(),
            Field::TabActiveBg => "active tab bg".into(),
            Field::TabActiveFg => "active tab fg".into(),
            Field::TabInactiveFg => "inactive tab fg".into(),
            Field::TabBarBg => "strip bg".into(),
            Field::Suggest => "autosuggest".into(),
            Field::SuggestColor => "suggest color".into(),
            Field::Scrollback => "history lines".into(),
            Field::ThemeMode => "mode".into(),
            Field::Background => "background".into(),
            Field::ImagePath => "image path".into(),
            Field::ImageFit => "image fit".into(),
            Field::ImageDim => "image dim".into(),
            Field::ImageBlur => "image blur".into(),
            Field::GradientAnimated => "gradient animate".into(),
            Field::GradientAngle => "gradient angle".into(),
            Field::GradientSpeed => "gradient speed".into(),
            Field::PlasmaSpeed => "plasma speed".into(),
            Field::PlasmaScale => "plasma scale".into(),
            Field::AuroraSpeed => "aurora speed".into(),
            Field::Bloom => "bloom".into(),
            Field::BloomIntensity => "bloom strength".into(),
            Field::Grain => "grain".into(),
            Field::GrainIntensity => "grain strength".into(),
            Field::Scanlines => "scanlines".into(),
            Field::ScanlineIntensity => "scanline strength".into(),
            Field::ScanlineSpeed => "scanline speed".into(),
            Field::Glow => "glow".into(),
            Field::FadeIn => "fade in".into(),
            Field::Vignette => "vignette".into(),
            Field::AboveText => "images above text".into(),
            Field::LoopGifs => "loop gifs".into(),
            Field::MaxCacheMb => "media cache".into(),
            Field::ShellProgram => "shell".into(),
            Field::PromptEnable => "enable".into(),
            Field::PromptDirStyle => "dir style".into(),
            Field::PromptGit => "git branch".into(),
            Field::PromptGitDirty => "git dirty".into(),
            Field::PromptStatus => "exit status".into(),
            Field::PromptDuration => "duration".into(),
            Field::PromptTime => "time".into(),
            Field::PromptSymDir => "dir symbol".into(),
            Field::PromptSymGit => "git symbol".into(),
            Field::PromptSymPrompt => "prompt symbol".into(),
            Field::PromptColDir => "dir color".into(),
            Field::PromptColGit => "git color".into(),
            Field::PromptColErr => "error color".into(),
            Field::PromptColDim => "dim color".into(),
            Field::PromptColPrompt => "prompt color".into(),
            Field::AnimMs => "duration ms".into(),
            Field::AnimStyle => "style".into(),
            Field::AnimPrompt => "animate prompt".into(),
            Field::Glass => "liquid glass".into(),
            Field::GlassStrength => "glass strength".into(),
            Field::GlassTint => "glass tint".into(),
            Field::PromptStyle => "layout".into(),
            Field::PromptTheme => "theme".into(),
            Field::PromptMultiline => "multiline".into(),
            Field::PromptIcons => "icons".into(),
            Field::PromptUser => "user".into(),
            Field::PromptVenv => "virtualenv".into(),
        }
    }

    pub fn value(self, cfg: &Config) -> String {
        match self {
            // Profile rows show the live pick from `SettingsView::profiles`;
            // this fallback keeps the standalone `value()` total for tests.
            Field::ProfileSelect | Field::ProfileDelete => "(none)".into(),
            Field::ProfileSave => "(new)".into(),
            Field::Preset => presets::name_of(&cfg.colors)
                .unwrap_or("custom")
                .to_string(),
            Field::Opacity => format!("{:.2}", cfg.window.opacity),
            Field::Title => cfg.window.title.clone(),
            Field::FontFamily => cfg.font.family.clone(),
            Field::BoldFamily => cfg
                .font
                .bold_family
                .clone()
                .unwrap_or_else(|| "(default)".into()),
            Field::ItalicFamily => cfg
                .font
                .italic_family
                .clone()
                .unwrap_or_else(|| "(default)".into()),
            Field::FontSize => size_text(cfg.font.size),
            Field::OffsetX => format!("{}", cfg.font.offset_x),
            Field::OffsetY => format!("{}", cfg.font.offset_y),
            Field::SyntheticBold => on_off(cfg.font.synthetic_bold).to_string(),
            Field::Bg => cfg.colors.background.clone(),
            Field::Fg => cfg.colors.foreground.clone(),
            Field::CursorColor => cfg.colors.cursor.clone().unwrap_or_else(|| "(auto)".into()),
            Field::SelBg => cfg
                .colors
                .selection_background
                .clone()
                .unwrap_or_else(|| "(auto)".into()),
            Field::Ansi(i) => cfg
                .colors
                .palette
                .get(i as usize)
                .cloned()
                .unwrap_or_else(|| ColorsCfg::default().palette[i as usize].clone()),
            Field::CursorStyle => cfg.cursor.style.clone(),
            Field::CursorBlink => on_off(cfg.cursor.blink).to_string(),
            Field::BlinkInterval => format!("{} ms", cfg.cursor.blink_interval_ms),
            Field::UnfocusedHollow => on_off(cfg.cursor.unfocused_hollow).to_string(),
            Field::PaddingX => format!("{}", cfg.ui.padding_x),
            Field::PaddingY => format!("{}", cfg.ui.padding_y),
            Field::CopyOnSelect => on_off(cfg.ui.copy_on_select).to_string(),
            Field::MiddlePaste => on_off(cfg.ui.middle_click_paste).to_string(),
            Field::ScrollLines => format!("{}", cfg.ui.scroll_lines),
            Field::PanelAlpha => format!("{:.2}", cfg.ui.panel_alpha),
            Field::PanelBlur => on_off(cfg.ui.panel_blur).to_string(),
            Field::PanelBlurRadius => format!("{} px", cfg.ui.panel_blur_radius),
            Field::TabBar => cfg.ui.tab_bar.clone(),
            Field::TabPosition => cfg.ui.tab_bar_position.clone(),
            Field::TabStyle => cfg.ui.tab_style.clone(),
            Field::TabAlpha => format!("{:.2}", cfg.ui.tab_bar_alpha),
            Field::TabClose => on_off(cfg.ui.tab_close).to_string(),
            Field::TabActiveBg => show_hex(&cfg.ui.tab_active_bg),
            Field::TabActiveFg => show_hex(&cfg.ui.tab_active_fg),
            Field::TabInactiveFg => show_hex(&cfg.ui.tab_inactive_fg),
            Field::TabBarBg => show_hex(&cfg.ui.tab_bar_bg),
            Field::Suggest => on_off(cfg.ui.suggest).to_string(),
            Field::SuggestColor => show_hex(&cfg.ui.suggest_color),
            Field::Scrollback => format!("{}", cfg.scrollback.lines),
            Field::ThemeMode => cfg.theme.mode.clone(),
            Field::Background => cfg.effects.background.clone(),
            Field::ImagePath => cfg.effects.image.clone().unwrap_or_else(|| "(none)".into()),
            Field::ImageFit => cfg.effects.image_fit.clone(),
            Field::ImageDim => format!("{:.2}", cfg.effects.image_dim),
            Field::ImageBlur => format!("{} px", cfg.effects.image_blur),
            Field::GradientAnimated => on_off(cfg.effects.gradient_animated).to_string(),
            Field::GradientAngle => format!("{}°", cfg.effects.gradient_angle.round() as i32),
            Field::GradientSpeed => format!("{:.2}", cfg.effects.gradient_speed),
            Field::PlasmaSpeed => format!("{:.2}", cfg.effects.plasma_speed),
            Field::PlasmaScale => format!("{:.1}", cfg.effects.plasma_scale),
            Field::AuroraSpeed => format!("{:.2}", cfg.effects.aurora_speed),
            Field::Bloom => on_off(cfg.effects.bloom).to_string(),
            Field::BloomIntensity => format!("{:.2}", cfg.effects.bloom_intensity),
            Field::Grain => on_off(cfg.effects.grain).to_string(),
            Field::GrainIntensity => format!("{:.2}", cfg.effects.grain_intensity),
            Field::Scanlines => on_off(cfg.effects.scanlines).to_string(),
            Field::ScanlineIntensity => format!("{:.2}", cfg.effects.scanline_intensity),
            Field::ScanlineSpeed => format!("{:.2}", cfg.effects.scanline_speed),
            Field::Glow => on_off(cfg.effects.glow).to_string(),
            Field::FadeIn => on_off(cfg.effects.fade_in).to_string(),
            Field::Vignette => on_off(cfg.effects.vignette).to_string(),
            Field::AboveText => on_off(cfg.media.above_text).to_string(),
            Field::LoopGifs => on_off(cfg.media.loop_gifs).to_string(),
            Field::MaxCacheMb => format!("{} MB", cfg.media.max_cache_mb),
            Field::ShellProgram => cfg
                .shell
                .program
                .clone()
                .unwrap_or_else(|| "(default)".into()),
            Field::PromptEnable => on_off(cfg.prompt.enable).to_string(),
            Field::PromptDirStyle => cfg.prompt.dir_style.clone(),
            Field::PromptGit => on_off(cfg.prompt.show_git).to_string(),
            Field::PromptGitDirty => on_off(cfg.prompt.git_dirty).to_string(),
            Field::PromptStatus => on_off(cfg.prompt.show_status).to_string(),
            Field::PromptDuration => on_off(cfg.prompt.show_duration).to_string(),
            Field::PromptTime => on_off(cfg.prompt.show_time).to_string(),
            Field::PromptSymDir => cfg.prompt.symbol_dir.clone(),
            Field::PromptSymGit => cfg.prompt.symbol_git.clone(),
            Field::PromptSymPrompt => cfg.prompt.symbol_prompt.clone(),
            Field::PromptColDir => cfg.prompt.color_dir.clone(),
            Field::PromptColGit => cfg.prompt.color_git.clone(),
            Field::PromptColErr => cfg.prompt.color_err.clone(),
            Field::PromptColDim => cfg.prompt.color_dim.clone(),
            Field::PromptColPrompt => cfg.prompt.color_prompt.clone(),
            Field::AnimMs => format!("{} ms", cfg.anim.ms),
            Field::AnimStyle => cfg.anim.style.clone(),
            Field::AnimPrompt => on_off(cfg.anim.prompt).to_string(),
            Field::Glass => on_off(cfg.ui.glass).to_string(),
            Field::GlassStrength => format!("{:.2}", cfg.ui.glass_strength),
            Field::GlassTint => show_hex(&cfg.ui.glass_tint),
            Field::PromptStyle => cfg.prompt.style.clone(),
            Field::PromptTheme => cfg.prompt.theme.clone(),
            Field::PromptMultiline => on_off(cfg.prompt.multiline).to_string(),
            Field::PromptIcons => on_off(cfg.prompt.icons).to_string(),
            Field::PromptUser => on_off(cfg.prompt.show_user).to_string(),
            Field::PromptVenv => on_off(cfg.prompt.show_venv).to_string(),
        }
    }

    /// One notch in `dir` (usually ±1, ±5 for the coarse keys); booleans toggle
    /// once and cycles advance by one regardless of the magnitude.
    pub fn adjust(self, cfg: &mut Config, families: &[String], dir: i32) {
        let step = if dir >= 0 { 1 } else { -1 };
        match self {
            Field::Preset => {
                let cur = presets::index_of(&cfg.colors);
                let n = presets::PRESETS.len() as i32;
                let next = match cur {
                    Some(i) => (i as i32 + step).rem_euclid(n),
                    None => {
                        if step > 0 {
                            0
                        } else {
                            n - 1
                        }
                    }
                };
                cfg.colors = presets::PRESETS[next as usize].colors();
            }
            Field::Opacity => {
                cfg.window.opacity = (cfg.window.opacity + 0.05 * dir as f32).clamp(0.2, 1.0);
            }
            Field::FontFamily => {
                if let Some(next) = cycle_family(Some(cfg.font.family.as_str()), families, step) {
                    cfg.font.family = next;
                }
            }
            Field::BoldFamily => {
                cfg.font.bold_family =
                    cycle_family(cfg.font.bold_family.as_deref(), families, step);
            }
            Field::ItalicFamily => {
                cfg.font.italic_family =
                    cycle_family(cfg.font.italic_family.as_deref(), families, step);
            }
            Field::FontSize => {
                cfg.font.size = (cfg.font.size + 0.5 * dir as f32).clamp(6.0, 72.0);
            }
            Field::OffsetX => {
                cfg.font.offset_x = (cfg.font.offset_x + dir).clamp(-10, 10);
            }
            Field::OffsetY => {
                cfg.font.offset_y = (cfg.font.offset_y + dir).clamp(-10, 10);
            }
            Field::SyntheticBold => cfg.font.synthetic_bold = !cfg.font.synthetic_bold,
            Field::CursorStyle => {
                const STYLES: [&str; 3] = ["block", "beam", "underline"];
                let current = STYLES
                    .iter()
                    .position(|s| *s == cfg.cursor.style)
                    .unwrap_or(0);
                let next = (current as i32 + step).rem_euclid(STYLES.len() as i32);
                cfg.cursor.style = STYLES[next as usize].into();
            }
            Field::CursorBlink => cfg.cursor.blink = !cfg.cursor.blink,
            Field::BlinkInterval => {
                cfg.cursor.blink_interval_ms = (cfg.cursor.blink_interval_ms as i64
                    + 100 * dir as i64)
                    .clamp(100, 10_000) as u64;
            }
            Field::UnfocusedHollow => cfg.cursor.unfocused_hollow = !cfg.cursor.unfocused_hollow,
            Field::PaddingX => {
                cfg.ui.padding_x = (cfg.ui.padding_x as i32 + dir).clamp(0, 100) as u32;
            }
            Field::PaddingY => {
                cfg.ui.padding_y = (cfg.ui.padding_y as i32 + dir).clamp(0, 100) as u32;
            }
            Field::CopyOnSelect => cfg.ui.copy_on_select = !cfg.ui.copy_on_select,
            Field::MiddlePaste => cfg.ui.middle_click_paste = !cfg.ui.middle_click_paste,
            Field::ScrollLines => {
                cfg.ui.scroll_lines = (cfg.ui.scroll_lines as i32 + dir).clamp(1, 100) as u32;
            }
            Field::PanelAlpha => {
                cfg.ui.panel_alpha = (cfg.ui.panel_alpha + 0.05 * dir as f32).clamp(0.2, 1.0);
            }
            Field::PanelBlur => cfg.ui.panel_blur = !cfg.ui.panel_blur,
            Field::PanelBlurRadius => {
                cfg.ui.panel_blur_radius =
                    (cfg.ui.panel_blur_radius as i32 + dir).clamp(0, 16) as u32;
            }
            Field::TabBar => {
                const MODES: [&str; 3] = ["auto", "on", "off"];
                let cur = MODES.iter().position(|m| *m == cfg.ui.tab_bar).unwrap_or(0);
                cfg.ui.tab_bar = MODES[(cur as i32 + step).rem_euclid(3) as usize].into();
            }
            Field::TabPosition => {
                const POS: [&str; 2] = ["top", "bottom"];
                let cur = POS
                    .iter()
                    .position(|p| *p == cfg.ui.tab_bar_position)
                    .unwrap_or(0);
                cfg.ui.tab_bar_position = POS[(cur as i32 + step).rem_euclid(2) as usize].into();
            }
            Field::TabStyle => {
                const STYLES: [&str; 3] = ["chip", "underline", "minimal"];
                let cur = STYLES
                    .iter()
                    .position(|s| *s == cfg.ui.tab_style)
                    .unwrap_or(0);
                cfg.ui.tab_style = STYLES[(cur as i32 + step).rem_euclid(3) as usize].into();
            }
            Field::TabAlpha => {
                cfg.ui.tab_bar_alpha = (cfg.ui.tab_bar_alpha + 0.1 * dir as f32).clamp(0.2, 1.0);
            }
            Field::TabClose => cfg.ui.tab_close = !cfg.ui.tab_close,
            Field::Suggest => cfg.ui.suggest = !cfg.ui.suggest,
            Field::Scrollback => {
                cfg.scrollback.lines =
                    (cfg.scrollback.lines as i64 + 1000 * dir as i64).clamp(0, 1_000_000) as usize;
            }
            Field::ThemeMode => {
                cfg.theme.mode = if cfg.theme.mode == "auto" {
                    "manual".into()
                } else {
                    "auto".into()
                };
            }
            Field::Background => {
                let mut modes = ["solid", "gradient", "plasma", "aurora"].to_vec();
                if cfg.effects.image.is_some() {
                    modes.push("image");
                }
                let current = modes
                    .iter()
                    .position(|m| *m == cfg.effects.background)
                    .unwrap_or(0);
                let next = (current as i32 + step).rem_euclid(modes.len() as i32);
                cfg.effects.background = modes[next as usize].into();
            }
            Field::ImageFit => {
                cfg.effects.image_fit = if cfg.effects.image_fit == "cover" {
                    "contain".into()
                } else {
                    "cover".into()
                };
            }
            Field::ImageDim => {
                cfg.effects.image_dim = (cfg.effects.image_dim + 0.02 * dir as f32).clamp(0.0, 1.0);
            }
            Field::ImageBlur => {
                cfg.effects.image_blur = (cfg.effects.image_blur as i32 + dir).clamp(0, 32) as u32;
            }
            Field::GradientAnimated => {
                cfg.effects.gradient_animated = !cfg.effects.gradient_animated
            }
            Field::GradientAngle => {
                cfg.effects.gradient_angle =
                    (cfg.effects.gradient_angle + 5.0 * dir as f32).rem_euclid(360.0);
            }
            Field::GradientSpeed => {
                cfg.effects.gradient_speed =
                    (cfg.effects.gradient_speed + 0.05 * dir as f32).clamp(0.0, 5.0);
            }
            Field::PlasmaSpeed => {
                cfg.effects.plasma_speed =
                    (cfg.effects.plasma_speed + 0.05 * dir as f32).clamp(0.0, 5.0);
            }
            Field::PlasmaScale => {
                cfg.effects.plasma_scale =
                    (cfg.effects.plasma_scale + 0.1 * dir as f32).clamp(0.1, 20.0);
            }
            Field::AuroraSpeed => {
                cfg.effects.aurora_speed =
                    (cfg.effects.aurora_speed + 0.05 * dir as f32).clamp(0.0, 5.0);
            }
            Field::Bloom => cfg.effects.bloom = !cfg.effects.bloom,
            Field::BloomIntensity => {
                cfg.effects.bloom_intensity =
                    (cfg.effects.bloom_intensity + 0.02 * dir as f32).clamp(0.0, 1.0);
            }
            Field::Grain => cfg.effects.grain = !cfg.effects.grain,
            Field::GrainIntensity => {
                cfg.effects.grain_intensity =
                    (cfg.effects.grain_intensity + 0.02 * dir as f32).clamp(0.0, 1.0);
            }
            Field::Scanlines => cfg.effects.scanlines = !cfg.effects.scanlines,
            Field::ScanlineIntensity => {
                cfg.effects.scanline_intensity =
                    (cfg.effects.scanline_intensity + 0.02 * dir as f32).clamp(0.0, 1.0);
            }
            Field::ScanlineSpeed => {
                cfg.effects.scanline_speed =
                    (cfg.effects.scanline_speed + 0.25 * dir as f32).clamp(0.0, 8.0);
            }
            Field::Glow => cfg.effects.glow = !cfg.effects.glow,
            Field::FadeIn => cfg.effects.fade_in = !cfg.effects.fade_in,
            Field::Vignette => cfg.effects.vignette = !cfg.effects.vignette,
            Field::AboveText => cfg.media.above_text = !cfg.media.above_text,
            Field::LoopGifs => cfg.media.loop_gifs = !cfg.media.loop_gifs,
            Field::MaxCacheMb => {
                cfg.media.max_cache_mb =
                    (cfg.media.max_cache_mb as i64 + 64 * dir as i64).clamp(16, 8192) as u64;
            }
            Field::PromptEnable => cfg.prompt.enable = !cfg.prompt.enable,
            Field::PromptGit => cfg.prompt.show_git = !cfg.prompt.show_git,
            Field::PromptGitDirty => cfg.prompt.git_dirty = !cfg.prompt.git_dirty,
            Field::PromptStatus => cfg.prompt.show_status = !cfg.prompt.show_status,
            Field::PromptDuration => cfg.prompt.show_duration = !cfg.prompt.show_duration,
            Field::PromptTime => cfg.prompt.show_time = !cfg.prompt.show_time,
            Field::PromptDirStyle => {
                const STYLES: [&str; 3] = ["full", "short", "last"];
                let current = STYLES
                    .iter()
                    .position(|s| *s == cfg.prompt.dir_style)
                    .unwrap_or(0);
                let next = (current as i32 + step).rem_euclid(STYLES.len() as i32);
                cfg.prompt.dir_style = STYLES[next as usize].into();
            }
            Field::AnimMs => {
                cfg.anim.ms = (cfg.anim.ms as i64 + 10 * dir as i64).clamp(0, 2000) as u64;
            }
            Field::AnimStyle => {
                const STYLES: [&str; 8] = [
                    "fade", "slide", "wipe", "rise", "drop", "type", "dissolve", "glow",
                ];
                let current = STYLES
                    .iter()
                    .position(|s| *s == cfg.anim.style)
                    .unwrap_or(0);
                let next = (current as i32 + step).rem_euclid(STYLES.len() as i32);
                cfg.anim.style = STYLES[next as usize].into();
            }
            Field::AnimPrompt => cfg.anim.prompt = !cfg.anim.prompt,
            Field::Glass => cfg.ui.glass = !cfg.ui.glass,
            Field::GlassStrength => {
                cfg.ui.glass_strength =
                    (cfg.ui.glass_strength + 0.05 * dir as f32).clamp(0.0, 1.0);
            }
            Field::PromptStyle => {
                const STYLES: [&str; 2] = ["plain", "powerline"];
                let current = STYLES
                    .iter()
                    .position(|s| *s == cfg.prompt.style)
                    .unwrap_or(0);
                let next = (current as i32 + step).rem_euclid(STYLES.len() as i32);
                cfg.prompt.style = STYLES[next as usize].into();
            }
            Field::PromptTheme => {
                const THEMES: [&str; 6] =
                    ["custom", "tokyo", "gruvbox", "dracula", "nord", "catppuccin"];
                let current = THEMES
                    .iter()
                    .position(|s| *s == cfg.prompt.theme)
                    .unwrap_or(0);
                let next = (current as i32 + step).rem_euclid(THEMES.len() as i32);
                cfg.prompt.theme = THEMES[next as usize].into();
            }
            Field::PromptMultiline => cfg.prompt.multiline = !cfg.prompt.multiline,
            Field::PromptIcons => cfg.prompt.icons = !cfg.prompt.icons,
            Field::PromptUser => cfg.prompt.show_user = !cfg.prompt.show_user,
            Field::PromptVenv => cfg.prompt.show_venv = !cfg.prompt.show_venv,
            // Rows edited as text never step from the wheel — and the profile
            // rows are driven by the app (load/save/delete), not by config.
            Field::Title
            | Field::Bg
            | Field::Fg
            | Field::CursorColor
            | Field::SelBg
            | Field::Ansi(_)
            | Field::ImagePath
            | Field::ShellProgram
            | Field::ProfileSelect
            | Field::ProfileSave
            | Field::ProfileDelete
            | Field::PromptSymDir
            | Field::PromptSymGit
            | Field::PromptSymPrompt
            | Field::PromptColDir
            | Field::PromptColGit
            | Field::PromptColErr
            | Field::PromptColDim
            | Field::PromptColPrompt
            | Field::TabActiveBg
            | Field::TabActiveFg
            | Field::TabInactiveFg
            | Field::TabBarBg
            | Field::SuggestColor
            | Field::GlassTint => {}
        }
    }

    /// Restore this one row to the built-in default.
    pub fn reset(self, cfg: &mut Config) {
        let def = Config::default();
        match self {
            // Profile rows manage files, not config values.
            Field::ProfileSelect | Field::ProfileSave | Field::ProfileDelete => {}
            Field::Preset => cfg.colors = def.colors,
            Field::Opacity => cfg.window.opacity = def.window.opacity,
            Field::Title => cfg.window.title = def.window.title,
            Field::FontFamily => cfg.font.family = def.font.family,
            Field::BoldFamily => cfg.font.bold_family = def.font.bold_family,
            Field::ItalicFamily => cfg.font.italic_family = def.font.italic_family,
            Field::FontSize => cfg.font.size = def.font.size,
            Field::OffsetX => cfg.font.offset_x = def.font.offset_x,
            Field::OffsetY => cfg.font.offset_y = def.font.offset_y,
            Field::SyntheticBold => cfg.font.synthetic_bold = def.font.synthetic_bold,
            Field::Bg => cfg.colors.background = def.colors.background,
            Field::Fg => cfg.colors.foreground = def.colors.foreground,
            Field::CursorColor => cfg.colors.cursor = def.colors.cursor,
            Field::SelBg => cfg.colors.selection_background = def.colors.selection_background,
            Field::Ansi(i) => {
                if cfg.colors.palette.len() != 16 {
                    cfg.colors.palette = def.colors.palette.clone();
                }
                cfg.colors.palette[i as usize] = def.colors.palette[i as usize].clone();
            }
            Field::CursorStyle => cfg.cursor.style = def.cursor.style,
            Field::CursorBlink => cfg.cursor.blink = def.cursor.blink,
            Field::BlinkInterval => cfg.cursor.blink_interval_ms = def.cursor.blink_interval_ms,
            Field::UnfocusedHollow => cfg.cursor.unfocused_hollow = def.cursor.unfocused_hollow,
            Field::PaddingX => cfg.ui.padding_x = def.ui.padding_x,
            Field::PaddingY => cfg.ui.padding_y = def.ui.padding_y,
            Field::CopyOnSelect => cfg.ui.copy_on_select = def.ui.copy_on_select,
            Field::MiddlePaste => cfg.ui.middle_click_paste = def.ui.middle_click_paste,
            Field::ScrollLines => cfg.ui.scroll_lines = def.ui.scroll_lines,
            Field::Scrollback => cfg.scrollback.lines = def.scrollback.lines,
            Field::ThemeMode => cfg.theme.mode = def.theme.mode,
            Field::Background => cfg.effects.background = def.effects.background,
            Field::ImagePath => cfg.effects.image = def.effects.image,
            Field::ImageFit => cfg.effects.image_fit = def.effects.image_fit,
            Field::ImageDim => cfg.effects.image_dim = def.effects.image_dim,
            Field::ImageBlur => cfg.effects.image_blur = def.effects.image_blur,
            Field::GradientAnimated => {
                cfg.effects.gradient_animated = def.effects.gradient_animated;
            }
            Field::GradientAngle => cfg.effects.gradient_angle = def.effects.gradient_angle,
            Field::GradientSpeed => cfg.effects.gradient_speed = def.effects.gradient_speed,
            Field::PlasmaSpeed => cfg.effects.plasma_speed = def.effects.plasma_speed,
            Field::PlasmaScale => cfg.effects.plasma_scale = def.effects.plasma_scale,
            Field::AuroraSpeed => cfg.effects.aurora_speed = def.effects.aurora_speed,
            Field::Bloom => cfg.effects.bloom = def.effects.bloom,
            Field::BloomIntensity => cfg.effects.bloom_intensity = def.effects.bloom_intensity,
            Field::Grain => cfg.effects.grain = def.effects.grain,
            Field::GrainIntensity => cfg.effects.grain_intensity = def.effects.grain_intensity,
            Field::Scanlines => cfg.effects.scanlines = def.effects.scanlines,
            Field::ScanlineIntensity => {
                cfg.effects.scanline_intensity = def.effects.scanline_intensity;
            }
            Field::ScanlineSpeed => cfg.effects.scanline_speed = def.effects.scanline_speed,
            Field::Glow => cfg.effects.glow = def.effects.glow,
            Field::FadeIn => cfg.effects.fade_in = def.effects.fade_in,
            Field::Vignette => cfg.effects.vignette = def.effects.vignette,
            Field::AboveText => cfg.media.above_text = def.media.above_text,
            Field::LoopGifs => cfg.media.loop_gifs = def.media.loop_gifs,
            Field::MaxCacheMb => cfg.media.max_cache_mb = def.media.max_cache_mb,
            Field::ShellProgram => cfg.shell.program = def.shell.program,
            Field::PanelAlpha => cfg.ui.panel_alpha = def.ui.panel_alpha,
            Field::PanelBlur => cfg.ui.panel_blur = def.ui.panel_blur,
            Field::PanelBlurRadius => cfg.ui.panel_blur_radius = def.ui.panel_blur_radius,
            Field::TabBar => cfg.ui.tab_bar = def.ui.tab_bar,
            Field::TabPosition => cfg.ui.tab_bar_position = def.ui.tab_bar_position,
            Field::TabStyle => cfg.ui.tab_style = def.ui.tab_style,
            Field::TabAlpha => cfg.ui.tab_bar_alpha = def.ui.tab_bar_alpha,
            Field::TabClose => cfg.ui.tab_close = def.ui.tab_close,
            Field::TabActiveBg => cfg.ui.tab_active_bg = def.ui.tab_active_bg,
            Field::TabActiveFg => cfg.ui.tab_active_fg = def.ui.tab_active_fg,
            Field::TabInactiveFg => cfg.ui.tab_inactive_fg = def.ui.tab_inactive_fg,
            Field::TabBarBg => cfg.ui.tab_bar_bg = def.ui.tab_bar_bg,
            Field::Suggest => cfg.ui.suggest = def.ui.suggest,
            Field::SuggestColor => cfg.ui.suggest_color = def.ui.suggest_color,
            Field::PromptEnable => cfg.prompt.enable = def.prompt.enable,
            Field::PromptDirStyle => cfg.prompt.dir_style = def.prompt.dir_style,
            Field::PromptGit => cfg.prompt.show_git = def.prompt.show_git,
            Field::PromptGitDirty => cfg.prompt.git_dirty = def.prompt.git_dirty,
            Field::PromptStatus => cfg.prompt.show_status = def.prompt.show_status,
            Field::PromptDuration => cfg.prompt.show_duration = def.prompt.show_duration,
            Field::PromptTime => cfg.prompt.show_time = def.prompt.show_time,
            Field::PromptSymDir => cfg.prompt.symbol_dir = def.prompt.symbol_dir,
            Field::PromptSymGit => cfg.prompt.symbol_git = def.prompt.symbol_git,
            Field::PromptSymPrompt => cfg.prompt.symbol_prompt = def.prompt.symbol_prompt,
            Field::PromptColDir => cfg.prompt.color_dir = def.prompt.color_dir,
            Field::PromptColGit => cfg.prompt.color_git = def.prompt.color_git,
            Field::PromptColErr => cfg.prompt.color_err = def.prompt.color_err,
            Field::PromptColDim => cfg.prompt.color_dim = def.prompt.color_dim,
            Field::PromptColPrompt => cfg.prompt.color_prompt = def.prompt.color_prompt,
            Field::AnimMs => cfg.anim.ms = def.anim.ms,
            Field::AnimStyle => cfg.anim.style = def.anim.style,
            Field::AnimPrompt => cfg.anim.prompt = def.anim.prompt,
            Field::Glass => cfg.ui.glass = def.ui.glass,
            Field::GlassStrength => cfg.ui.glass_strength = def.ui.glass_strength,
            Field::GlassTint => cfg.ui.glass_tint = def.ui.glass_tint,
            Field::PromptStyle => cfg.prompt.style = def.prompt.style,
            Field::PromptTheme => cfg.prompt.theme = def.prompt.theme,
            Field::PromptMultiline => cfg.prompt.multiline = def.prompt.multiline,
            Field::PromptIcons => cfg.prompt.icons = def.prompt.icons,
            Field::PromptUser => cfg.prompt.show_user = def.prompt.show_user,
            Field::PromptVenv => cfg.prompt.show_venv = def.prompt.show_venv,
        }
    }

    /// Colour rows (also drawn as a swatch); presets recolour everything too.
    pub fn is_color(self) -> bool {
        matches!(
            self,
            Field::Bg
                | Field::Fg
                | Field::CursorColor
                | Field::SelBg
                | Field::Ansi(_)
                | Field::Preset
        )
    }

    /// Rows whose change must be mirrored into `base_colors` (and force the
    /// theme back to manual so the desktop scheme cannot override them).
    pub fn affects_colors(self) -> bool {
        self.is_color()
    }

    /// Rows the colour picker can edit (the preset row is not one colour).
    pub fn is_pickable(self) -> bool {
        (self.is_color() && self != Field::Preset)
            || matches!(
                self,
                Field::PromptColDir
                    | Field::PromptColGit
                    | Field::PromptColErr
                    | Field::PromptColDim
                    | Field::PromptColPrompt
                    | Field::TabActiveBg
                    | Field::TabActiveFg
                    | Field::TabInactiveFg
                    | Field::TabBarBg
                    | Field::SuggestColor
                    | Field::GlassTint
            )
    }

    /// Rows that preview while the user is still typing (colours and numbers
    /// apply on every keystroke; font families and paths only on Enter so a
    /// half-typed name cannot spawn font loads or file reads).
    pub fn is_live(self) -> bool {
        !matches!(
            self,
            Field::FontFamily | Field::BoldFamily | Field::ItalicFamily | Field::ImagePath
        )
    }

    /// The hex behind a colour row, `None` for unset options or non-colours.
    pub fn color_hex(self, cfg: &Config) -> Option<&str> {
        match self {
            Field::Bg => Some(cfg.colors.background.as_str()),
            Field::Fg => Some(cfg.colors.foreground.as_str()),
            Field::CursorColor => cfg.colors.cursor.as_deref(),
            Field::SelBg => cfg.colors.selection_background.as_deref(),
            Field::Ansi(i) => cfg.colors.palette.get(i as usize).map(String::as_str),
            Field::PromptColDir => Some(cfg.prompt.color_dir.as_str()),
            Field::PromptColGit => Some(cfg.prompt.color_git.as_str()),
            Field::PromptColErr => Some(cfg.prompt.color_err.as_str()),
            Field::PromptColDim => Some(cfg.prompt.color_dim.as_str()),
            Field::PromptColPrompt => Some(cfg.prompt.color_prompt.as_str()),
            Field::TabActiveBg => Some(cfg.ui.tab_active_bg.as_str()),
            Field::TabActiveFg => Some(cfg.ui.tab_active_fg.as_str()),
            Field::TabInactiveFg => Some(cfg.ui.tab_inactive_fg.as_str()),
            Field::TabBarBg => Some(cfg.ui.tab_bar_bg.as_str()),
            Field::SuggestColor => Some(cfg.ui.suggest_color.as_str()),
            Field::GlassTint => Some(cfg.ui.glass_tint.as_str()),
            _ => None,
        }
    }

    /// Rows that open the in-place text editor with `Space` / `→`.
    pub fn is_editable(self) -> bool {
        matches!(
            self,
            Field::Title
                | Field::FontFamily
                | Field::BoldFamily
                | Field::ItalicFamily
                | Field::Bg
                | Field::Fg
                | Field::CursorColor
                | Field::SelBg
                | Field::Ansi(_)
                | Field::ImagePath
                | Field::ShellProgram
                | Field::PromptSymDir
                | Field::PromptSymGit
                | Field::PromptSymPrompt
                | Field::PromptColDir
                | Field::PromptColGit
                | Field::PromptColErr
                | Field::PromptColDim
                | Field::PromptColPrompt
                | Field::TabActiveBg
                | Field::TabActiveFg
                | Field::TabInactiveFg
                | Field::TabBarBg
                | Field::SuggestColor
                | Field::GlassTint
                | Field::ProfileSave
        )
    }

    /// Text the editor starts with, or `None` when the row is not editable.
    pub fn edit_prefill(self, cfg: &Config) -> Option<String> {
        Some(match self {
            Field::ProfileSave => String::new(), // the app refills with the picked name
            Field::Title => cfg.window.title.clone(),
            Field::FontFamily => cfg.font.family.clone(),
            Field::BoldFamily => cfg.font.bold_family.clone().unwrap_or_default(),
            Field::ItalicFamily => cfg.font.italic_family.clone().unwrap_or_default(),
            Field::Bg => cfg.colors.background.clone(),
            Field::Fg => cfg.colors.foreground.clone(),
            Field::CursorColor => cfg.colors.cursor.clone().unwrap_or_default(),
            Field::SelBg => cfg.colors.selection_background.clone().unwrap_or_default(),
            Field::Ansi(i) => cfg
                .colors
                .palette
                .get(i as usize)
                .cloned()
                .unwrap_or_default(),
            Field::ImagePath => cfg.effects.image.clone().unwrap_or_default(),
            Field::ShellProgram => cfg.shell.program.clone().unwrap_or_default(),
            Field::PromptSymDir => cfg.prompt.symbol_dir.clone(),
            Field::PromptSymGit => cfg.prompt.symbol_git.clone(),
            Field::PromptSymPrompt => cfg.prompt.symbol_prompt.clone(),
            Field::PromptColDir => cfg.prompt.color_dir.clone(),
            Field::PromptColGit => cfg.prompt.color_git.clone(),
            Field::PromptColErr => cfg.prompt.color_err.clone(),
            Field::PromptColDim => cfg.prompt.color_dim.clone(),
            Field::PromptColPrompt => cfg.prompt.color_prompt.clone(),
            Field::TabActiveBg => cfg.ui.tab_active_bg.clone(),
            Field::TabActiveFg => cfg.ui.tab_active_fg.clone(),
            Field::TabInactiveFg => cfg.ui.tab_inactive_fg.clone(),
            Field::TabBarBg => cfg.ui.tab_bar_bg.clone(),
            Field::SuggestColor => cfg.ui.suggest_color.clone(),
            Field::GlassTint => cfg.ui.glass_tint.clone(),
            _ => return None,
        })
    }

    /// Apply the typed text; `Err` keeps the editor open with the message.
    pub fn commit_edit(self, cfg: &mut Config, text: &str) -> Result<(), String> {
        match self {
            Field::Bg => cfg.colors.background = parse_hex(text)?,
            Field::Fg => cfg.colors.foreground = parse_hex(text)?,
            Field::Ansi(i) => {
                let hex = parse_hex(text)?;
                if cfg.colors.palette.len() != 16 {
                    let mut palette = ColorsCfg::default().palette;
                    for (slot, value) in cfg.colors.palette.iter().take(16).enumerate() {
                        palette[slot] = value.clone();
                    }
                    cfg.colors.palette = palette;
                }
                cfg.colors.palette[i as usize] = hex;
            }
            Field::CursorColor => {
                let trimmed = text.trim();
                cfg.colors.cursor = if trimmed.is_empty() {
                    None
                } else {
                    Some(parse_hex(trimmed)?)
                };
            }
            Field::SelBg => {
                let trimmed = text.trim();
                cfg.colors.selection_background = if trimmed.is_empty() {
                    None
                } else {
                    Some(parse_hex(trimmed)?)
                };
            }
            Field::Title => {
                let trimmed = text.trim();
                if trimmed.is_empty() {
                    return Err("title can't be empty".into());
                }
                cfg.window.title = trimmed.into();
            }
            Field::FontFamily => {
                let trimmed = text.trim();
                if trimmed.is_empty() {
                    return Err("family can't be empty".into());
                }
                cfg.font.family = trimmed.into();
            }
            Field::BoldFamily | Field::ItalicFamily => {
                let value = {
                    let trimmed = text.trim();
                    if trimmed.is_empty() {
                        None
                    } else {
                        Some(trimmed.to_string())
                    }
                };
                if matches!(self, Field::BoldFamily) {
                    cfg.font.bold_family = value;
                } else {
                    cfg.font.italic_family = value;
                }
            }
            Field::ImagePath => {
                let trimmed = text.trim();
                cfg.effects.image = if trimmed.is_empty() {
                    None
                } else {
                    Some(expand_home(trimmed))
                };
                // Pick the image mode once a path exists and nothing else is chosen.
                if cfg.effects.image.is_some() && cfg.effects.background == "solid" {
                    cfg.effects.background = "image".into();
                }
            }
            Field::ShellProgram => {
                let trimmed = text.trim();
                cfg.shell.program = if trimmed.is_empty() {
                    None
                } else {
                    Some(trimmed.into())
                };
            }
            Field::PromptSymDir => cfg.prompt.symbol_dir = text.to_string(),
            Field::PromptSymGit => cfg.prompt.symbol_git = text.to_string(),
            Field::PromptSymPrompt => cfg.prompt.symbol_prompt = text.to_string(),
            Field::PromptColDir => cfg.prompt.color_dir = parse_hex(text)?,
            Field::PromptColGit => cfg.prompt.color_git = parse_hex(text)?,
            Field::PromptColErr => cfg.prompt.color_err = parse_hex(text)?,
            Field::PromptColDim => cfg.prompt.color_dim = parse_hex(text)?,
            Field::PromptColPrompt => cfg.prompt.color_prompt = parse_hex(text)?,
            Field::TabActiveBg => cfg.ui.tab_active_bg = optional_hex(text)?,
            Field::TabActiveFg => cfg.ui.tab_active_fg = optional_hex(text)?,
            Field::TabInactiveFg => cfg.ui.tab_inactive_fg = optional_hex(text)?,
            Field::TabBarBg => cfg.ui.tab_bar_bg = optional_hex(text)?,
            Field::SuggestColor => cfg.ui.suggest_color = parse_hex(text)?,
            Field::GlassTint => cfg.ui.glass_tint = parse_hex(text)?,
            other => return Err(format!("{} is typed, not cycled", other.label())),
        }
        Ok(())
    }
}

fn on_off(flag: bool) -> &'static str {
    if flag {
        "on"
    } else {
        "off"
    }
}

/// Value shown for an optional colour: the hex, or "(theme)" when the row
/// inherits from the theme (empty string in the config).
fn show_hex(value: &str) -> String {
    if value.is_empty() {
        "(theme)".into()
    } else {
        value.to_string()
    }
}

/// Parse a colour row that may also be emptied to fall back to the theme.
fn optional_hex(text: &str) -> Result<String, String> {
    if text.trim().is_empty() {
        Ok(String::new())
    } else {
        parse_hex(text)
    }
}

/// `14` for whole sizes, `13.5` otherwise.
fn size_text(size: f32) -> String {
    if (size - size.round()).abs() < 0.01 {
        format!("{}", size.round() as i32)
    } else {
        format!("{size:.1}")
    }
}

fn parse_hex(text: &str) -> Result<String, String> {
    let trimmed = text.trim();
    let body = trimmed
        .strip_prefix('#')
        .filter(|b| b.len() == 6 && b.bytes().all(|c| c.is_ascii_hexdigit()))
        .ok_or_else(|| "use #rrggbb".to_string())?;
    Ok(format!("#{}", body.to_ascii_lowercase()))
}

/// Next family in `list` around `current`; `None` starts at the near end.
fn cycle_family(current: Option<&str>, list: &[String], step: i32) -> Option<String> {
    if list.is_empty() {
        return None;
    }
    let found = current.and_then(|c| list.iter().position(|f| f.eq_ignore_ascii_case(c)));
    let next = match found {
        Some(i) => (i as i32 + step).rem_euclid(list.len() as i32),
        None if step >= 0 => 0,
        None => list.len() as i32 - 1,
    };
    Some(list[next as usize].clone())
}

/// `~/wall.png` -> `$HOME/wall.png` (the image loader takes literal paths).
fn expand_home(path: &str) -> String {
    if path == "~" {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home).display().to_string();
        }
    } else if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home).join(rest).display().to_string();
        }
    }
    path.to_string()
}

/// One drawn line: a section header or an editable row.
enum Line {
    Header(&'static str),
    Field(usize),
}

/// Where the panel sits and how pointer coordinates map onto rows.
pub struct Geometry {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    row_x: i32,
    row_y: i32,
    row_w: i32,
    row_h: i32,
    lines: Vec<Line>,
}

impl Geometry {
    /// Top pixel of the row showing `index`; the colour-picker card is
    /// anchored under the selected row, so clicks and drawing use this.
    pub fn row_of(&self, index: usize) -> i32 {
        let line = self
            .lines
            .iter()
            .position(|l| matches!(l, Line::Field(i) if *i == index))
            .unwrap_or(0);
        self.row_y + line as i32 * self.row_h.max(1)
    }

    /// The field under `(px, py)`, if any. A section header points at its
    /// first row, so clicking a collapsed `▸ group` opens it.
    pub fn hit(&self, px: i32, py: i32) -> Option<usize> {
        if px < self.row_x || px >= self.row_x + self.row_w || py < self.row_y {
            return None;
        }
        let line = (py - self.row_y) / self.row_h.max(1);
        match self.lines.get(line as usize) {
            Some(Line::Field(index)) => Some(*index),
            Some(Line::Header(group)) => Field::ALL.iter().position(|f| f.group() == *group),
            _ => None,
        }
    }
}

/// Interactive HSL colour picker for a colour row.
///
/// The value is pushed to the live config on every step, so the terminal
/// recolours while the sliders move. `orig` is the hex to restore when the
/// picker is cancelled with `Esc` (`"   "` clears an optional row).
#[derive(Debug, Clone, PartialEq)]
pub struct Pick {
    pub field: Field,
    pub orig: String,
    pub hue: f32,
    pub sat: f32,
    pub light: f32,
    /// 0 = hue, 1 = saturation, 2 = lightness.
    pub channel: usize,
}

impl Pick {
    /// Start from the row's current colour (or the foreground for "auto" rows).
    pub fn open(field: Field, current: &str, fallback: &str) -> Self {
        let rgb = palette::parse_hex(current)
            .or_else(|| palette::parse_hex(fallback))
            .unwrap_or([0x7a, 0xa2, 0xf7]);
        let (hue, sat, light) = palette::rgb_to_hsl(rgb);
        Self {
            field,
            orig: current.to_string(),
            hue,
            sat,
            light,
            channel: 0,
        }
    }

    pub fn hex(&self) -> String {
        palette::to_hex(palette::hsl_to_rgb(self.hue, self.sat, self.light))
    }

    pub fn channel_name(&self) -> &'static str {
        ["hue", "saturation", "lightness"][self.channel]
    }

    pub fn channel_value(&self) -> f32 {
        [self.hue, self.sat, self.light][self.channel]
    }

    /// Nudge the active slider (`dir` −1/+1, `step` for coarse Shift steps).
    pub fn step(&mut self, dir: i32, step: f32) {
        let delta = dir as f32 * step;
        match self.channel {
            0 => self.hue = (self.hue + delta).rem_euclid(360.0),
            1 => self.sat = (self.sat + delta).clamp(0.0, 100.0),
            _ => self.light = (self.light + delta).clamp(0.0, 100.0),
        }
    }

    pub fn switch_channel(&mut self, dir: i32) {
        self.channel = (self.channel as i32 + dir).rem_euclid(3) as usize;
    }

    /// Rotate the hue wheel (the circular part of the picker).
    pub fn rotate_hue(&mut self, dir: i32, step: f32) {
        self.hue = (self.hue + dir as f32 * step).rem_euclid(360.0);
    }

    /// Move along the radius of the wheel (saturation).
    pub fn adjust_sat(&mut self, dir: i32, step: f32) {
        self.sat = (self.sat + dir as f32 * step).clamp(0.0, 100.0);
    }

    /// Move perpendicular to the wheel (lightness).
    pub fn adjust_light(&mut self, dir: i32, step: f32) {
        self.light = (self.light + dir as f32 * step).clamp(0.0, 100.0);
    }
}

/// In-place text editing for a string row: buffer plus caret.
pub struct EditState {
    pub field: Field,
    pub buf: String,
    /// Byte offset; always on a `char` boundary.
    pub pos: usize,
}

impl EditState {
    pub fn new(field: Field, buf: String) -> Self {
        let pos = buf.len();
        Self { field, buf, pos }
    }

    pub fn insert(&mut self, c: char) {
        if self.buf.len() < 4096 {
            self.buf.insert(self.pos, c);
            self.pos += c.len_utf8();
        }
    }

    pub fn backspace(&mut self) {
        if self.pos == 0 {
            return;
        }
        let prev = self.buf[..self.pos]
            .char_indices()
            .next_back()
            .map(|(i, _)| i)
            .unwrap_or(0);
        self.buf.remove(prev);
        self.pos = prev;
    }

    pub fn delete(&mut self) {
        if self.pos < self.buf.len() {
            self.buf.remove(self.pos);
        }
    }

    /// Walk the caret by `dir` characters (clamped to the buffer).
    pub fn move_caret(&mut self, dir: i32) {
        let steps = dir.unsigned_abs() as usize;
        if dir < 0 {
            for _ in 0..steps {
                if self.pos == 0 {
                    break;
                }
                self.pos = self.buf[..self.pos]
                    .char_indices()
                    .next_back()
                    .map(|(i, _)| i)
                    .unwrap_or(0);
            }
        } else if dir > 0 {
            for _ in 0..steps {
                if self.pos >= self.buf.len() {
                    break;
                }
                let width = self.buf[self.pos..]
                    .chars()
                    .next()
                    .map(char::len_utf8)
                    .unwrap_or(1);
                self.pos += width;
            }
        }
    }
}

/// Selection, transient status line and the font list used for cycling.
pub struct SettingsView {
    pub selected: usize,
    pub status: Option<String>,
    /// Full-window editor (`rterm config --edit`) instead of an overlay.
    pub full: bool,
    /// Non-`None` while a string row is being typed into.
    pub editing: Option<EditState>,
    /// Non-`None` while the colour picker owns the keys.
    pub picking: Option<Pick>,
    /// The one expanded section; every other group collapses to its header
    /// so the panel never shows a wall of a hundred rows.
    pub open_group: &'static str,
    families: Option<Vec<String>>,
    /// Saved profile names, listed on disk (`profiles` group reads them).
    pub profiles: Vec<String>,
    /// Position in `profiles`, shared by the select/save-as/delete rows.
    pub profile_idx: usize,
}

impl SettingsView {
    pub fn new(full: bool) -> Self {
        let mut view = Self {
            selected: 0,
            status: None,
            full,
            editing: None,
            picking: None,
            open_group: Field::ALL[0].group(),
            families: None,
            profiles: Vec::new(),
            profile_idx: 0,
        };
        view.refresh_profiles();
        view
    }

    /// Re-read `~/.config/rterm/profiles/` and clamp the pick to it.
    pub fn refresh_profiles(&mut self) {
        self.profiles = crate::profiles::list();
        if self.profile_idx >= self.profiles.len() {
            self.profile_idx = self.profiles.len().saturating_sub(1);
        }
    }

    /// The profile the select/save-as/delete rows currently point at.
    pub fn profile_name(&self) -> Option<&str> {
        self.profiles.get(self.profile_idx).map(String::as_str)
    }

    pub fn field(&self) -> Field {
        Field::ALL[self.selected.min(Field::ALL.len() - 1)]
    }

    pub fn move_selection(&mut self, delta: i32) {
        let last = Field::ALL.len() - 1;
        self.selected = (self.selected as i32 + delta).clamp(0, last as i32) as usize;
        self.status = None;
        self.editing = None;
        self.picking = None;
        // The section follows the selection (single-open accordion).
        self.open_group = self.field().group();
    }

    pub fn select(&mut self, index: usize) {
        self.selected = index.min(Field::ALL.len() - 1);
        self.status = None;
        self.editing = None;
        self.picking = None;
        self.open_group = self.field().group();
    }

    pub fn set_status(&mut self, text: impl Into<String>) {
        self.status = Some(text.into());
    }

    /// Installed font families, probed once through `fc-list`.
    pub fn families(&mut self) -> &[String] {
        if self.families.is_none() {
            self.families = Some(probe_families());
        }
        self.families.as_deref().unwrap_or(&[])
    }
}

/// Every family fontconfig knows about, sorted and deduplicated.
fn probe_families() -> Vec<String> {
    let Ok(output) = Command::new("fc-list")
        .arg("--format=%{family[0]}\n")
        .output()
    else {
        return Vec::new();
    };
    let text = String::from_utf8_lossy(&output.stdout);
    let mut families: Vec<String> = text.lines().map(str::to_string).collect();
    families.retain(|f| !f.trim().is_empty());
    families.sort();
    families.dedup();
    families
}

/// Panel geometry for the current config, font metrics and window size.
///
/// The list is windowed: only the rows that fit around the selection are
/// returned, so a ~70-row config never overflows the panel.
pub fn geometry(
    fonts: &mut FontSystem,
    cfg: &Config,
    view: &SettingsView,
    win_w: i32,
    win_h: i32,
) -> Geometry {
    let line_h = fonts.metrics.cell_h.max(18.0).round() as i32;
    let pad = (fonts.metrics.cell_w * 2.0).round().max(16.0) as i32;
    let gap = (fonts.metrics.cell_w * 2.0).round() as i32;
    let title_h = line_h * 2;
    let foot_h = line_h * 2;
    let cell_w = fonts.metrics.cell_w;

    // Full list first: the panel width stays stable while scrolling.
    // Only the open section lists its rows; the rest collapse to headers.
    let mut all: Vec<Line> = Vec::new();
    let mut group = "";
    let mut body_w = 0i32;
    for (index, field) in Field::ALL.iter().enumerate() {
        if field.group() != group {
            group = field.group();
            let marker = if group == view.open_group {
                "▾ "
            } else {
                "▸ "
            };
            let text = format!("{marker}{group}");
            all.push(Line::Header(group));
            body_w = body_w.max(fonts.text_width(&text) as i32 + pad * 2);
        }
        if group != view.open_group {
            continue;
        }
        all.push(Line::Field(index));
        let label = field.label();
        let value = row_value(*field, cfg, view);
        let width = fonts.text_width(&label) as i32 + gap + fonts.text_width(&value) as i32;
        body_w = body_w.max(width + pad * 2);
    }

    // The hint always counts towards the width: entering the editor or
    // showing a status line must not make the panel jump sideways.
    let footer = footer_text(view);
    let hint = if view.full { FULL_HINT } else { OVERLAY_HINT };
    let foot_w = fonts.text_width(&footer).max(fonts.text_width(hint)).ceil() as i32 + pad * 2;
    let w = body_w.max(foot_w).max(cell_w as i32 * 24).max(360);
    let w = w.min((win_w - 32).max(360));
    let total = title_h + all.len() as i32 * line_h + foot_h;
    let h = total.min((win_h - 32).max(240));
    let fit = ((h - title_h - foot_h) / line_h).max(1) as usize;

    // Centre the window on the selected row and clamp to the list ends.
    let sel_line = all
        .iter()
        .position(|l| matches!(l, Line::Field(i) if *i == view.selected))
        .unwrap_or(0);
    let start = sel_line
        .saturating_sub(fit / 2)
        .min(all.len().saturating_sub(fit));
    let lines = all.into_iter().skip(start).take(fit).collect();

    let x = ((win_w - w) / 2).max(0);
    let y = ((win_h - h) / 2).max(0);

    Geometry {
        x,
        y,
        w,
        h,
        row_x: x + pad,
        row_y: y + title_h,
        row_w: w - pad * 2,
        row_h: line_h,
        lines,
    }
}

/// Geometry of the colour-picker card, shared by the draw path and the click
/// hit-test so both always agree on where the wheel, the swatch and the
/// lightness slider sit.
#[derive(Debug, Clone, Copy)]
pub struct PickerGeo {
    pub rect: [i32; 4],
    /// Wheel centre and outer radius.
    pub cx: f32,
    pub cy: f32,
    pub ring: f32,
    /// Radius of the inner saturation disc.
    pub disc: f32,
    pub swatch: [i32; 4],
    /// Top pixel of the hex line, the RGB line and the slider's label line.
    pub text: [i32; 3],
    pub slider: [i32; 4],
}

impl PickerGeo {
    pub fn contains(&self, px: i32, py: i32) -> bool {
        let [x, y, w, h] = self.rect;
        px >= x && px < x + w && py >= y && py < y + h
    }

    pub fn in_slider(&self, px: i32, py: i32) -> bool {
        let [x, y, w, h] = self.slider;
        px >= x - 4 && px < x + w + 4 && py >= y - 6 && py < y + h + 6
    }
}

/// Lay the card out under (or above) the colour row at `row_y`.
///
/// The right column gets one full line per readout — swatch, hex, RGB,
/// slider label — so nothing can ever be printed over anything else, and the
/// card is grown until everything fits.
pub fn picker_geometry(geo: &Geometry, row_y: i32) -> PickerGeo {
    let line_h = geo.row_h.max(16);
    let box_w = geo.w - 24;
    let sw_h = (line_h * 3 / 2).max(26);
    // Swatch + hex + RGB + label + slider, each on its own line, versus the
    // room the wheel beside them needs: take the taller of the two.
    let box_h = (line_h * 9 / 2 + 48).max(line_h * 6 + 10);
    // Under the row, unless that would fall out of the panel: then above.
    let mut box_y = row_y + line_h;
    if box_y + box_h > geo.y + geo.h - line_h {
        box_y = row_y - box_h;
    }
    let box_y = box_y.max(geo.y + 6);
    let box_x = geo.x + 12;

    let avail = (box_h - 16).max(16);
    let ring = avail as f32 / 2.0;
    let cx = box_x as f32 + 12.0 + ring;
    let cy = box_y as f32 + (box_h - avail) as f32 / 2.0 + ring;
    let thickness = (ring * 0.22).max(6.0);
    let disc = (ring - thickness - 2.0).max(4.0);

    let right_x = (cx + ring + 16.0).round() as i32;
    let right_w = (box_x + box_w - 12 - right_x).max(48);
    let swatch_y = box_y + 10;
    let hex_y = swatch_y + sw_h + 4;
    let rgb_y = hex_y + line_h;
    let label_y = rgb_y + line_h + 4;
    let swatch = [right_x, swatch_y, right_w, sw_h];
    let text = [hex_y, rgb_y, label_y];
    let slider = [right_x, label_y + line_h + 2, right_w, 10];

    PickerGeo {
        rect: [box_x, box_y, box_w, box_h],
        cx,
        cy,
        ring,
        disc,
        swatch,
        text,
        slider,
    }
}

/// The hue ring and the saturation disc, painted per pixel with soft
/// antialiased rims: hue travels around the ring, saturation travels outwards
/// inside it, and the lightness slider below tints the whole disc.
fn draw_color_wheel(renderer: &mut Renderer, geo: &PickerGeo, pick: &Pick) {
    let hue = pick.hue.rem_euclid(360.0);
    let light = pick.light.clamp(0.0, 100.0);
    let sat = (pick.sat / 100.0).clamp(0.0, 1.0);
    let (cx, cy, ring, disc) = (geo.cx, geo.cy, geo.ring, geo.disc);
    let span = (ring * 2.0 + 3.0).ceil() as i32;
    let x0 = (cx - ring - 1.5).floor() as i32;
    let y0 = (cy - ring - 1.5).floor() as i32;
    renderer.plot(x0, y0, span, span, |px, py| {
        let dx = px as f32 + 0.5 - cx;
        let dy = py as f32 + 0.5 - cy;
        let d = (dx * dx + dy * dy).sqrt();
        let on_ring = ((ring + 0.5 - d).clamp(0.0, 1.0)
            * (d - disc + 0.5).clamp(0.0, 1.0)
            * 255.0) as u8;
        let on_disc = ((disc + 0.5 - d).clamp(0.0, 1.0) * 255.0) as u8;
        if on_disc >= on_ring && on_disc > 0 {
            let s = (d / disc).clamp(0.0, 1.0) * 100.0;
            Some((palette::hsl_to_rgb(hue, s, light), on_disc))
        } else if on_ring > 0 {
            let angle = dy.atan2(dx).to_degrees().rem_euclid(360.0);
            Some((palette::hsl_to_rgb(angle, 100.0, 50.0), on_ring))
        } else {
            None
        }
    });

    // Saturation knob: white dot at the current hue and radius.
    let rad = hue.to_radians();
    marker(renderer, cx + sat * disc * rad.cos(), cy + sat * disc * rad.sin(), 4.0);
    // Hue tick: a matching dot sitting on the ring itself.
    let mid = (ring + disc) / 2.0;
    marker(renderer, cx + mid * rad.cos(), cy + mid * rad.sin(), 3.5);
}

/// A white dot with a dark rim: the two handles of the picker.
fn marker(renderer: &mut Renderer, x: f32, y: f32, r: f32) {
    let span = (r * 2.0 + 4.0).ceil() as i32;
    let x0 = (x - r - 2.0).floor() as i32;
    let y0 = (y - r - 2.0).floor() as i32;
    renderer.plot(x0, y0, span, span, |px, py| {
        let d = ((px as f32 + 0.5 - x).powi(2) + (py as f32 + 0.5 - y).powi(2)).sqrt();
        let rim = (r + 1.5 - d).clamp(0.0, 1.0);
        let core = (r - d).clamp(0.0, 1.0);
        if rim <= 0.0 {
            return None;
        }
        if core > 0.0 {
            Some(([255, 255, 255], (core * 255.0) as u8))
        } else {
            Some(([12, 12, 16], (rim * 200.0) as u8))
        }
    });
}

/// Pop-out card: the wheel on the left, the live swatch with its hex and RGB
/// readout plus the lightness slider on the right.
#[allow(clippy::too_many_arguments)]
fn draw_picker(
    renderer: &mut Renderer,
    fonts: &mut FontSystem,
    geo: &Geometry,
    row_y: i32,
    pick: &Pick,
    accent: Rgb8,
    bg: Rgb8,
    fg: Rgb8,
    muted: Rgb8,
) {
    let g = picker_geometry(geo, row_y);
    let [bx, by, bw, bh] = g.rect;
    let surface = tint(bg, 24);
    renderer.fill_box(bx, by, bw, bh, surface);
    renderer.outline(bx, by, bw, bh, accent);

    draw_color_wheel(renderer, &g, pick);

    let rgb = palette::parse_hex_or(&pick.hex(), [0x33; 3]);
    let [sx, sy, sw, sh] = g.swatch;
    renderer.fill_box(sx, sy, sw, sh, rgb);
    renderer.outline(sx - 1, sy - 1, sw + 2, sh + 2, tint(rgb, 90));
    let hex = pick.hex().to_uppercase();
    let rgb_txt = format!("R{} G{} B{}", rgb[0], rgb[1], rgb[2]);
    let [hex_y, rgb_y, label_y] = g.text;
    renderer.label(
        fonts,
        sx,
        hex_y as f32 + fonts.metrics.baseline,
        &hex,
        fg,
    );
    renderer.label(
        fonts,
        sx,
        rgb_y as f32 + fonts.metrics.baseline,
        &rgb_txt,
        muted,
    );

    // Lightness slider: black through the colour to white, knob at `light`.
    let [lx, ly, lw, lh] = g.slider;
    renderer.plot(lx, ly, lw, lh, |px, _| {
        let t = ((px - lx) as f32 / lw.max(1) as f32).clamp(0.0, 1.0);
        Some((palette::hsl_to_rgb(pick.hue, pick.sat, t * 100.0), 255))
    });
    renderer.outline(lx - 1, ly - 1, lw + 2, lh + 2, tint(bg, 48));
    let knob = lx + (lw as f32 * (pick.light / 100.0)).round() as i32;
    renderer.fill_box(knob - 1, ly - 3, 3, lh + 6, [255, 255, 255]);
    // Label on its own line above the slider; the percentage is flush right,
    // so a long word never runs into the number.
    let baseline = label_y as f32 + fonts.metrics.baseline;
    renderer.label(fonts, lx, baseline, "lightness", muted);
    let pct = format!("{}%", pick.light.round());
    let pct_w = fonts.text_width(&pct).ceil() as i32;
    renderer.label(fonts, lx + lw - pct_w, baseline, &pct, fg);
}

const FULL_HINT: &str = "Enter save and quit · Esc quit without saving";
const OVERLAY_HINT: &str =
    "↑↓ rows · ←→ change · Enter colour picker · Space text edit · Home default · S save · Esc close";

fn footer_text(view: &SettingsView) -> String {
    if let Some(status) = &view.status {
        return status.clone();
    }
    if view.picking.is_some() {
        return "←→ hue · ↑↓ saturation · PgUp/PgDn light · Enter done · Esc cancel".into();
    }
    if view.editing.is_some() {
        return "typing applies live · Enter done · Esc cancel".into();
    }
    if view.full {
        FULL_HINT.into()
    } else {
        OVERLAY_HINT.into()
    }
}

/// Dim the frame and paint the panel on top of everything else.
/// Row text; the profile rows read the live pick from `view` instead of the
/// config (their `value()` fallback only exists so the standalone call is total).
pub fn row_value(field: Field, cfg: &Config, view: &SettingsView) -> String {
    match field {
        Field::ProfileSelect | Field::ProfileDelete => {
            view.profile_name().unwrap_or("(none)").to_string()
        }
        Field::ProfileSave => view.profile_name().unwrap_or("(new)").to_string(),
        other => other.value(cfg),
    }
}

pub fn draw(
    renderer: &mut Renderer,
    fonts: &mut FontSystem,
    cfg: &Config,
    view: &SettingsView,
    win_w: i32,
    win_h: i32,
) {
    let accent = palette::parse_hex_or(
        cfg.colors
            .selection_background
            .as_deref()
            .unwrap_or("#7aa2f7"),
        [0x7a, 0xa2, 0xf7],
    );
    let bg = palette::parse_hex_or(&cfg.colors.background, [0x16, 0x16, 0x1e]);
    let fg = palette::parse_hex_or(&cfg.colors.foreground, [0xc0, 0xca, 0xf5]);
    let muted = palette::dim(fg);
    let panel = tint(bg, 12);

    renderer.shade(3, 5);

    let geo = geometry(fonts, cfg, view, win_w, win_h);
    // Frost the panel: blur the shaded terminal content underneath it. Liquid
    // glass always frosts, even when the plain panel blur is off.
    let glass = cfg.ui.glass;
    let strength = cfg.ui.glass_strength.clamp(0.0, 1.0);
    if cfg.ui.panel_blur || glass {
        let radius = if glass {
            cfg.ui.panel_blur_radius.max(8)
        } else {
            cfg.ui.panel_blur_radius
        };
        renderer.blur_rect(geo.x, geo.y, geo.w, geo.h, radius);
    }
    // The panel blends over the shaded frame; its colour is pre-scaled by the
    // window opacity so it stays a true premultiplied source for the alpha
    // byte written at present time.
    let op = cfg.window.opacity.clamp(0.2, 1.0);
    let panel = scale_rgb(panel, op);
    let panel_a = (cfg.ui.panel_alpha * 255.0 + 0.5) as u8;
    renderer.fill_box_alpha(geo.x, geo.y, geo.w, geo.h, panel, panel_a);
    if glass {
        // Liquid glass: richer colours, a tinted film, a diagonal sheen and a
        // bright inner rim - blur alone reads as frosted plastic, not glass.
        renderer.saturate_rect(geo.x, geo.y, geo.w, geo.h, 1.0 + strength * 1.8);
        let tint = palette::parse_hex_or(&cfg.ui.glass_tint, [255, 255, 255]);
        let tint_a = (strength * 34.0 + 0.5) as u8;
        renderer.fill_box_alpha(geo.x, geo.y, geo.w, geo.h, scale_rgb(tint, op), tint_a);
        renderer.sheen(geo.x, geo.y, geo.w, geo.h, (strength * 95.0 + 0.5) as u8);
        renderer.glass_rim(
            geo.x,
            geo.y,
            geo.w,
            geo.h,
            (strength * 120.0 + 0.5) as u8,
            (strength * 55.0 + 0.5) as u8,
        );
    }
    renderer.outline(geo.x, geo.y, geo.w, geo.h, accent);

    let title = if view.full {
        "rterm settings editor"
    } else {
        "rterm settings"
    };
    renderer.label(
        fonts,
        geo.x + 16,
        geo.y as f32 + fonts.metrics.baseline + 6.0,
        title,
        accent,
    );

    let mut y = geo.row_y;
    let mut sel_y = None;
    for line in geo.lines.iter() {
        match line {
            Line::Header(text) => {
                let open = *text == view.open_group;
                let marker = if open { "▾ " } else { "▸ " };
                let colour = if open { accent } else { muted };
                renderer.label(
                    fonts,
                    geo.row_x,
                    y as f32 + fonts.metrics.baseline + 2.0,
                    &format!("{marker}{text}"),
                    colour,
                );
            }
            Line::Field(index) => {
                let field = Field::ALL[*index];
                let selected = *index == view.selected;
                if selected {
                    renderer.fill_box(geo.row_x - 6, y, geo.row_w + 12, geo.row_h, accent);
                    sel_y = Some(y);
                }
                let mut value = row_value(field, cfg, view);
                if let Some(edit) = &view.editing {
                    if edit.field == field {
                        value = format!("{}|{}", &edit.buf[..edit.pos], &edit.buf[edit.pos..]);
                    }
                }
                let label_color = if selected { bg } else { fg };
                let value_color = if selected { bg } else { accent };
                let baseline = y as f32 + fonts.metrics.baseline + 2.0;
                if let Some(hex) = field.color_hex(cfg) {
                    let sw = (geo.row_h - 8).max(10);
                    let value_w = fonts.text_width(&value).ceil() as i32;
                    let sx = geo.row_x + geo.row_w - value_w - sw - 8;
                    let sy = y + (geo.row_h - sw) / 2;
                    let chip = palette::parse_hex_or(hex, accent);
                    renderer.fill_box(sx, sy, sw, sw, chip);
                    // Contrast outline keeps light swatches visible on any row.
                    let edge = if selected { bg } else { tint(chip, 90) };
                    renderer.outline(sx - 1, sy - 1, sw + 2, sw + 2, edge);
                }
                renderer.label(fonts, geo.row_x, baseline, &field.label(), label_color);
                let value_w = fonts.text_width(&value).ceil() as i32;
                renderer.label(
                    fonts,
                    geo.row_x + geo.row_w - value_w,
                    baseline,
                    &value,
                    value_color,
                );
            }
        }
        y += geo.row_h;
    }

    // Colour picker: a pop-out card of HSL sliders that recolour live.
    if let (Some(pick), Some(row_y)) = (&view.picking, sel_y) {
        draw_picker(renderer, fonts, &geo, row_y, pick, accent, bg, fg, muted);
    }

    let footer = footer_text(view);
    let footer_y = geo.y + geo.h - fonts.metrics.cell_h as i32 - 8;
    renderer.label(
        fonts,
        geo.x + 16,
        footer_y as f32 + fonts.metrics.baseline,
        &footer,
        muted,
    );
}

/// Lighten `c` towards white by `amount` (0..=64) for panel surfaces.
fn tint(c: Rgb8, amount: u8) -> Rgb8 {
    let mix = |v: u8| v.saturating_add(amount);
    [mix(c[0]), mix(c[1]), mix(c[2])]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn system() -> Option<FontSystem> {
        match FontSystem::new(&Config::default().font, 1.0) {
            Ok(sys) => Some(sys),
            Err(_) => None, // environment without fonts
        }
    }

    /// The panel draws one header per switch of `group`; a field added at the
    /// end of `ALL` but belonging to an earlier group would create a duplicate
    /// section, which is exactly how the new anim/glass rows first landed.
    #[test]
    fn groups_form_contiguous_sections() {
        let mut seen: Vec<&str> = Vec::new();
        let mut current = None;
        for field in Field::ALL {
            let group = field.group();
            if current != Some(group) {
                assert!(
                    !seen.contains(&group),
                    "group {group:?} is split into more than one section"
                );
                seen.push(group);
                current = Some(group);
            }
        }
    }


    #[test]
    fn every_field_has_a_label_group_and_value() {
        for field in Field::ALL {
            assert!(!field.label().is_empty(), "{field:?}");
            assert!(!field.group().is_empty(), "{field:?}");
            let value = field.value(&Config::default());
            assert!(!value.is_empty(), "{field:?}");
            assert!(value.len() < 40, "{field:?} value too long: {value}");
        }
        for (i, a) in Field::ALL.iter().enumerate() {
            for b in &Field::ALL[i + 1..] {
                assert_ne!(a, b, "duplicate row in Field::ALL");
            }
        }
    }

    #[test]
    fn font_size_steps_by_half() {
        let mut cfg = Config::default();
        Field::FontSize.adjust(&mut cfg, &[], 1);
        assert_eq!(cfg.font.size, 14.5);
        assert_eq!(Field::FontSize.value(&cfg), "14.5");
        Field::FontSize.adjust(&mut cfg, &[], 1);
        assert_eq!(cfg.font.size, 15.0);
        assert_eq!(Field::FontSize.value(&cfg), "15");
        Field::FontSize.adjust(&mut cfg, &[], -5);
        assert_eq!(cfg.font.size, 12.5);
        // never below the clamp
        for _ in 0..40 {
            Field::FontSize.adjust(&mut cfg, &[], -1);
        }
        assert_eq!(cfg.font.size, 6.0);
    }

    #[test]
    fn coarse_steps_jump_and_booleans_toggle_once() {
        let mut cfg = Config::default();
        Field::Opacity.adjust(&mut cfg, &[], -5);
        assert!((cfg.window.opacity - 0.75).abs() < 1e-6, "{}", cfg.window.opacity);
        Field::Scrollback.adjust(&mut cfg, &[], 5);
        assert_eq!(cfg.scrollback.lines, 15_000);
        Field::MaxCacheMb.adjust(&mut cfg, &[], -5);
        assert_eq!(cfg.media.max_cache_mb, 16); // clamps at the floor
        // A boolean stays a single toggle even for a coarse step.
        let before = cfg.effects.bloom;
        Field::Bloom.adjust(&mut cfg, &[], 5);
        assert_ne!(cfg.effects.bloom, before);
    }

    #[test]
    fn preset_cycles_wrap_and_round_trip() {
        let mut cfg = Config::default();
        assert_eq!(presets::name_of(&cfg.colors), Some("tokyo night"));
        Field::Preset.adjust(&mut cfg, &[], 1);
        assert_eq!(presets::name_of(&cfg.colors), Some("gruvbox"));
        for _ in 0..presets::PRESETS.len() {
            Field::Preset.adjust(&mut cfg, &[], -1);
        }
        assert_eq!(presets::name_of(&cfg.colors), Some("gruvbox"));
        Field::Preset.adjust(&mut cfg, &[], -1);
        assert_eq!(presets::name_of(&cfg.colors), Some("tokyo night"));
    }

    #[test]
    fn commit_edit_validates_colours() {
        let mut cfg = Config::default();
        Field::Bg.commit_edit(&mut cfg, "#FF8800").unwrap();
        assert_eq!(cfg.colors.background, "#ff8800");
        assert!(Field::Bg.commit_edit(&mut cfg, "red").is_err());
        assert!(Field::Bg.commit_edit(&mut cfg, "").is_err());
        assert!(Field::Bg.commit_edit(&mut cfg, "#fff").is_err());

        cfg.colors.cursor = Some("#123456".into());
        Field::CursorColor.commit_edit(&mut cfg, "   ").unwrap();
        assert!(cfg.colors.cursor.is_none());
        assert!(Field::CursorColor.commit_edit(&mut cfg, "zzz").is_err());

        Field::Ansi(1).commit_edit(&mut cfg, "#AABBCC").unwrap();
        assert_eq!(cfg.colors.palette[1], "#aabbcc");
        assert_eq!(cfg.colors.palette[0], "#1a1b26");
    }

    #[test]
    fn commit_edit_sets_string_rows() {
        let mut cfg = Config::default();
        Field::ShellProgram.commit_edit(&mut cfg, "").unwrap();
        assert!(cfg.shell.program.is_none());
        Field::ShellProgram.commit_edit(&mut cfg, "/bin/zsh").unwrap();
        assert_eq!(cfg.shell.program.as_deref(), Some("/bin/zsh"));

        Field::Title.commit_edit(&mut cfg, "   ").unwrap_err();
        Field::Title.commit_edit(&mut cfg, "my term").unwrap();
        assert_eq!(cfg.window.title, "my term");

        Field::BoldFamily.commit_edit(&mut cfg, "").unwrap();
        assert!(cfg.font.bold_family.is_none());

        Field::ImagePath.commit_edit(&mut cfg, "~/x.png").unwrap();
        assert!(cfg.effects.image.as_deref().unwrap().ends_with("/x.png"));
        assert_eq!(cfg.effects.background, "image");
    }

    #[test]
    fn reset_restores_the_row_default() {
        let mut cfg = Config::default();
        cfg.font.size = 40.0;
        cfg.effects.grain_intensity = 0.9;
        cfg.colors.background = "#000000".into();
        Field::FontSize.reset(&mut cfg);
        Field::GrainIntensity.reset(&mut cfg);
        Field::Bg.reset(&mut cfg);
        assert_eq!(cfg.font.size, 14.0);
        assert_eq!(cfg.effects.grain_intensity, 0.06);
        assert_eq!(cfg.colors.background, "#16161e");
    }

    #[test]
    fn editing_tracks_character_boundaries() {
        let mut edit = EditState::new(Field::ImagePath, "~/фото.png".into());
        assert_eq!(edit.pos, edit.buf.len());
        edit.move_caret(-1);
        edit.insert('X');
        assert_eq!(edit.buf, "~/фото.pnXg");
        edit.backspace();
        assert_eq!(edit.buf, "~/фото.png");
        edit.move_caret(-1);
        assert_eq!(edit.pos, "~/фото.p".len());
        edit.move_caret(1);
        assert_eq!(edit.pos, "~/фото.png".len() - 1);
        edit.move_caret(-100);
        assert_eq!(edit.pos, 0);
        edit.delete();
        assert_eq!(edit.buf, "/фото.png");
    }

    /// The picker card keeps its widgets inside the panel: the wheel on the
    /// left with the saturation disc strictly inside the ring, the swatch and
    /// the lightness slider within the card — so clicks land exactly where
    /// the drawing put them.
    #[test]
    fn picker_geometry_keeps_widgets_inside_the_card() {
        let Some(mut fonts) = system() else { return };
        let cfg = Config::default();
        let view = SettingsView::new(false);
        let geo = geometry(&mut fonts, &cfg, &view, 1100, 700);
        let g = picker_geometry(&geo, geo.row_of(0));

        assert!(g.ring > 8.0, "the wheel must be visible");
        assert!(g.disc > 0.0 && g.disc < g.ring, "disc inside the ring");
        let [x, y, w, h] = g.rect;
        let [sx, sy, sw, sh] = g.swatch;
        assert!(sx >= x && sx + sw <= x + w && sy >= y && sy + sh <= y + h);
        // Each readout owns its own line: swatch, hex, RGB, slider label,
        // slider — stacked in that order, so no two can overlap.
        let [hex_y, rgb_y, label_y] = g.text;
        assert!(sy + sh <= hex_y, "hex line starts under the swatch");
        assert!(hex_y < rgb_y && rgb_y < label_y, "one line per readout");
        assert!(label_y < y + h, "the label stays inside the card");
        let [lx, ly, lw, lh] = g.slider;
        assert!(lx >= x && lx + lw <= x + w && ly >= y && ly + lh <= y + h);
        assert!(label_y < ly, "label line above the slider");
        assert!(g.contains(g.cx as i32, g.cy as i32), "centre is clickable");
        assert!(g.in_slider(lx + lw / 2, ly + lh / 2), "slider is clickable");
        assert!(
            !g.in_slider(x + w - 1, y + h - 1),
            "the card corner is not the slider"
        );
    }

    #[test]
    fn geometry_windows_the_list_around_the_selection() {
        let Some(mut fonts) = system() else { return };
        let cfg = Config::default();
        let mut view = SettingsView::new(false);
        view.select(Field::ALL.len() - 1);

        let geo = geometry(&mut fonts, &cfg, &view, 1100, 300);
        assert!(
            geo.lines.len() < Field::ALL.len(),
            "a short window must scroll: {} of {} lines",
            geo.lines.len(),
            Field::ALL.len()
        );
        assert!(
            geo.lines
                .iter()
                .any(|l| matches!(l, Line::Field(i) if *i == Field::ALL.len() - 1)),
            "the selected row must be visible"
        );
        // Rows keep absolute indices, so a click still maps back correctly.
        if let Some(li) = geo.lines.iter().position(|l| matches!(l, Line::Field(_))) {
            let py = geo.row_y + li as i32 * geo.row_h + 1;
            assert!(geo.hit(geo.row_x + 1, py).is_some());
        }

        // A tall window shows every section header plus the fields of the one
        // open section — the accordion keeps the panel short.
        let tall = geometry(&mut fonts, &cfg, &view, 1100, 4000);
        let open_group = view.field().group();
        let expected = Field::ALL
            .iter()
            .map(|f| f.group())
            .collect::<Vec<_>>()
            .windows(2)
            .filter(|w| w[0] != w[1])
            .count() // one header per group
            + 1 // header of the last group
            + Field::ALL.iter().filter(|f| f.group() == open_group).count();
        assert_eq!(tall.lines.len(), expected);
        assert!(tall
            .lines
            .iter()
            .any(|l| matches!(l, Line::Header(g) if *g == open_group)));
        assert!(tall.lines.iter().any(|l| matches!(
            l,
            Line::Field(i) if *i == Field::ALL.len() - 1
        )));
    }
}
