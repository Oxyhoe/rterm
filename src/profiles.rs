//! Named profiles: full snapshots of `config.toml` stored next to it in
//! `~/.config/rterm/profiles/` and switched from the settings panel (F1).
//!
//! Switching loads a snapshot and re-applies it through the same path as an
//! external config edit, so the change is live — no restart, no re-login.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context};

use crate::config::Config;

/// Where saved profiles live.
pub fn dir() -> PathBuf {
    Config::config_dir().join("profiles")
}

/// A profile name doubles as its file name: keep it short and boring.
pub fn sanitize(raw: &str) -> Option<String> {
    let name = raw.trim();
    if name.is_empty() || name.len() > 48 || name.starts_with('.') {
        return None;
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    {
        return None;
    }
    Some(name.to_string())
}

/// Saved profile names, sorted.
pub fn list() -> Vec<String> {
    list_in(&dir())
}

/// Write `cfg` as the profile `name` (creating the directory if needed).
pub fn save(name: &str, cfg: &Config) -> anyhow::Result<()> {
    save_in(&dir(), name, cfg)
}

/// Load a profile; `Ok(None)` when it does not exist.
pub fn load(name: &str) -> anyhow::Result<Option<Config>> {
    load_in(&dir(), name)
}

/// Remove a profile; a missing file is not an error.
pub fn delete(name: &str) -> anyhow::Result<()> {
    delete_in(&dir(), name)
}

/// Resolve `name` inside `base`, rejecting anything that escapes the directory.
fn file(base: &Path, name: &str) -> anyhow::Result<PathBuf> {
    let Some(clean) = sanitize(name) else {
        bail!("invalid profile name: {name:?}");
    };
    Ok(base.join(format!("{clean}.toml")))
}

fn list_in(base: &Path) -> Vec<String> {
    let mut names = Vec::new();
    if let Ok(entries) = fs::read_dir(base) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("toml") {
                continue;
            }
            let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
            if let Some(name) = sanitize(stem) {
                names.push(name);
            }
        }
    }
    names.sort();
    names.dedup();
    names
}

fn save_in(base: &Path, name: &str, cfg: &Config) -> anyhow::Result<()> {
    Config::save(&file(base, name)?, cfg)
}

fn load_in(base: &Path, name: &str) -> anyhow::Result<Option<Config>> {
    Config::load_from(&file(base, name)?)
}

fn delete_in(base: &Path, name: &str) -> anyhow::Result<()> {
    let path = file(base, name)?;
    match fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err).with_context(|| format!("cannot remove {}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rterm-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn sanitize_accepts_simple_names_only() {
        assert_eq!(sanitize("  work-2 ").as_deref(), Some("work-2"));
        assert_eq!(sanitize("Dark.glaze").as_deref(), Some("Dark.glaze"));
        assert_eq!(sanitize(""), None);
        assert_eq!(sanitize("   "), None);
        assert_eq!(sanitize(".hidden"), None);
        assert_eq!(sanitize("../escape"), None);
        assert_eq!(sanitize("a b"), None);
        assert_eq!(sanitize("имя"), None);
        assert_eq!(sanitize(&"x".repeat(64)), None);
    }

    #[test]
    fn file_never_escapes_the_directory() {
        let base = Path::new("/profiles");
        assert_eq!(
            file(base, "../evil")
                .unwrap_err()
                .to_string()
                .contains("invalid"),
            true
        );
        assert_eq!(file(base, "work").unwrap(), base.join("work.toml"));
    }

    #[test]
    fn save_load_list_delete_roundtrip() {
        let dir = base("profiles");
        let mut cfg = Config::default();
        cfg.window.title = "profiled".into();

        assert!(list_in(&dir).is_empty());
        save_in(&dir, "work", &cfg).unwrap();
        save_in(&dir, "dark", &cfg).unwrap();
        assert_eq!(list_in(&dir), vec!["dark".to_string(), "work".to_string()]);

        let loaded = load_in(&dir, "work").unwrap().expect("saved profile loads");
        assert_eq!(loaded.window.title, "profiled");

        delete_in(&dir, "dark").unwrap();
        assert_eq!(list_in(&dir), vec!["work".to_string()]);
        assert!(load_in(&dir, "dark").unwrap().is_none());
        delete_in(&dir, "dark").unwrap(); // deleting twice is fine

        let _ = fs::remove_dir_all(&dir);
    }
}
