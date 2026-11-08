//! Follow the desktop when `theme.mode = "auto"`.
//!
//! Two things are detected: whether the desktop is dark or light (so the
//! terminal can swap to a light palette), and the accent colour the compositor
//! draws its focus ring with — selection, active tab and the pane focus border
//! then match the niri theme.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::config::ColorsCfg;

/// Colours to render with: the user's own palette, adjusted for the detected
/// scheme and the compositor accent.
pub fn effective(base: ColorsCfg) -> ColorsCfg {
    let mut colors = base;
    if detect_dark() == Some(false) {
        colors = light_colors(colors);
    }
    if let Some(accent) = detect_accent() {
        colors.selection_background = Some(accent);
    }
    colors
}

/// `$XDG_CONFIG_HOME`, else `$HOME/.config`.
fn config_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("XDG_CONFIG_HOME") {
        let path = PathBuf::from(dir);
        if path.is_absolute() {
            return path;
        }
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    home.join(".config")
}

/// `Some(true)` for a dark desktop, `Some(false)` for light, `None` unknown.
pub fn detect_dark() -> Option<bool> {
    if let Ok(value) = std::env::var("RTERM_THEME") {
        match value.trim().to_ascii_lowercase().as_str() {
            "dark" => return Some(true),
            "light" => return Some(false),
            _ => {}
        }
    }
    let dir = config_dir();
    detect_dark_in(&dir).or_else(gsettings_dark_cached)
}

/// Cheap file-only detection; safe to re-run while the app is up.
fn detect_dark_in(dir: &Path) -> Option<bool> {
    for name in ["gtk-4.0/settings.ini", "gtk-3.0/settings.ini"] {
        if let Ok(text) = std::fs::read_to_string(dir.join(name)) {
            if let Some(dark) = gtk_dark(&text) {
                return Some(dark);
            }
        }
    }
    std::fs::read_to_string(dir.join("kdeglobals"))
        .ok()
        .as_deref()
        .and_then(kde_dark)
}

/// `gsettings` spawns a process, so the answer is cached for a minute.
fn gsettings_dark_cached() -> Option<bool> {
    const TTL: Duration = Duration::from_secs(60);
    static CACHE: Mutex<Option<(Instant, Option<bool>)>> = Mutex::new(None);
    let mut cache = CACHE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some((at, value)) = *cache {
        if at.elapsed() < TTL {
            return value;
        }
    }
    let value = gsettings_dark();
    *cache = Some((Instant::now(), value));
    value
}

fn gsettings_dark() -> Option<bool> {
    let out = std::process::Command::new("gsettings")
        .args(["get", "org.gnome.desktop.interface", "color-scheme"])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout).to_ascii_lowercase();
    if text.contains("prefer-dark") {
        return Some(true);
    }
    if text.contains("prefer-light") {
        return Some(false);
    }
    None
}

/// A GTK `settings.ini`: the explicit flag wins, the theme name only hints.
fn gtk_dark(text: &str) -> Option<bool> {
    let mut hint = None;
    for line in text.lines() {
        let line = line.trim();
        if let Some(value) = line.strip_prefix("gtk-application-preferred-dark-theme=") {
            match value.trim().to_ascii_lowercase().as_str() {
                "true" | "1" => return Some(true),
                "false" | "0" => return Some(false),
                _ => {}
            }
        }
        if let Some(value) = line.strip_prefix("gtk-theme-name=") {
            let value = value.trim().to_ascii_lowercase();
            if value.contains("dark") {
                hint = Some(true);
            } else if value.contains("light") {
                hint = Some(false);
            }
        }
    }
    hint
}

/// KDE's `kdeglobals`: `[General] ColorSchemeName=BreezeDark`.
fn kde_dark(text: &str) -> Option<bool> {
    for line in text.lines() {
        let line = line.trim();
        let Some(value) = line
            .strip_prefix("ColorSchemeName=")
            .or_else(|| line.strip_prefix("ColorScheme="))
        else {
            continue;
        };
        let value = value.to_ascii_lowercase();
        if value.contains("dark") {
            return Some(true);
        }
        if value.contains("light") {
            return Some(false);
        }
    }
    None
}

/// The accent colour from the running niri configuration.
pub fn detect_accent() -> Option<String> {
    let text = std::fs::read_to_string(config_dir().join("niri").join("config.kdl")).ok()?;
    accent_from_str(&text)
}

/// `focus-ring` first (it usually carries `active-gradient from="..."`), then
/// `border`, then any `active-color` in the file.
pub fn accent_from_str(kdl: &str) -> Option<String> {
    for block in ["focus-ring", "border"] {
        if let Some(section) = block_section(kdl, block) {
            if let Some(color) = color_after(section, "from=") {
                return Some(color);
            }
            if let Some(color) = color_after(section, "active-color") {
                return Some(color);
            }
        }
    }
    color_after(kdl, "active-color")
}

/// The body of the `{ ... }` block that follows `name`.
fn block_section<'a>(text: &'a str, name: &str) -> Option<&'a str> {
    let bytes = text.as_bytes();
    let mut search = 0usize;
    while let Some(found) = text[search..].find(name) {
        let name_start = search + found;
        let after_name = name_start + name.len();
        let mut open = after_name;
        while open < bytes.len() && bytes[open].is_ascii_whitespace() {
            open += 1;
        }
        if open < bytes.len() && bytes[open] == b'{' {
            let body = open + 1;
            let mut depth = 1usize;
            for (offset, byte) in bytes[body..].iter().enumerate() {
                match byte {
                    b'{' => depth += 1,
                    b'}' => {
                        depth -= 1;
                        if depth == 0 {
                            return Some(&text[body..body + offset]);
                        }
                    }
                    _ => {}
                }
            }
            return None;
        }
        search = after_name;
    }
    None
}

/// First `#rrggbb` after `key`; the key must start at a word boundary so
/// `inactive-color` never satisfies a search for `active-color`.
fn color_after(text: &str, key: &str) -> Option<String> {
    let mut search = 0usize;
    let pos = loop {
        let found = text[search..].find(key)?;
        let abs = search + found;
        if abs == 0 || text.as_bytes()[abs - 1].is_ascii_whitespace() {
            break abs;
        }
        search = abs + 1;
    };
    let tail = &text[pos + key.len()..];
    let hash = tail.find('#')?;
    let hex: String = tail[hash + 1..]
        .chars()
        .take_while(char::is_ascii_hexdigit)
        .collect();
    if hex.len() >= 6 {
        return Some(format!("#{}", &hex[..6]));
    }
    None
}

/// A calm light scheme, used when the desktop reports a light theme.
pub fn light_colors(mut colors: ColorsCfg) -> ColorsCfg {
    colors.background = "#ebe8e1".into();
    colors.foreground = "#26262c".into();
    colors.cursor = Some("#26262c".into());
    colors.cursor_text = Some("#ebe8e1".into());
    colors.selection_background = Some("#7aa2f7".into());
    colors.selection_foreground = Some("#26262c".into());
    colors.palette = ["#26262c", "#c14c4c", "#4a8f4a", "#b08a28"]
        .iter()
        .chain(["#3f6fb5", "#9258a8", "#2f8f8f", "#8a8a92"].iter())
        .chain(["#5a5a62", "#d76b6b", "#6aab6a", "#c9a84a"].iter())
        .chain(["#5d8fd6", "#b07ac9", "#4fb0b0", "#f4f2ec"].iter())
        .map(|color| (*color).to_string())
        .collect();
    colors
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accent_comes_from_the_focus_ring_gradient() {
        let kdl = "focus-ring {\n    active-gradient from=\"#b3a081\" to=\"#c9884f\"\n    inactive-color \"#2a211a\"\n}";
        assert_eq!(accent_from_str(kdl).as_deref(), Some("#b3a081"));
    }

    #[test]
    fn accent_falls_back_to_border_active_color() {
        let kdl = "focus-ring { }\nborder {\n    active-color \"#7aa2f7\"\n}";
        assert_eq!(accent_from_str(kdl).as_deref(), Some("#7aa2f7"));
    }

    #[test]
    fn inactive_color_is_never_the_accent() {
        let kdl = "focus-ring {\n    inactive-color \"#2a211a\"\n}";
        assert_eq!(accent_from_str(kdl), None);
    }

    #[test]
    fn alpha_is_stripped_from_the_accent() {
        let kdl = "focus-ring { active-color \"#b3a08166\" }";
        assert_eq!(accent_from_str(kdl).as_deref(), Some("#b3a081"));
    }

    #[test]
    fn gtk_flag_beats_the_theme_name() {
        let ini = "gtk-theme-name=Adwaita\ngtk-application-preferred-dark-theme=true\n";
        assert_eq!(gtk_dark(ini), Some(true));
        let ini = "gtk-theme-name=Adwaita-dark\ngtk-application-preferred-dark-theme=false\n";
        assert_eq!(gtk_dark(ini), Some(false));
    }

    #[test]
    fn gtk_theme_name_only_hints() {
        assert_eq!(gtk_dark("gtk-theme-name=Adwaita-dark\n"), Some(true));
        assert_eq!(gtk_dark("gtk-theme-name=Adwaita\n"), None);
    }

    #[test]
    fn kde_scheme_name_reports_dark() {
        assert_eq!(
            kde_dark("[General]\nColorSchemeName=BreezeDark\n"),
            Some(true)
        );
        assert_eq!(
            kde_dark("[General]\nColorSchemeName=BreezeLight\n"),
            Some(false)
        );
    }

    #[test]
    fn light_scheme_replaces_the_palette() {
        let base = ColorsCfg::default();
        let light = light_colors(base.clone());
        assert_ne!(light.background, base.background);
        assert_eq!(light.palette.len(), 16);
        assert!(light.selection_background.is_some());
    }
}
