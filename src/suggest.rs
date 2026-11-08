//! Grey autosuggestions (`ui.suggest`): the dim tail of an earlier command
//! drawn after the cursor; `End` or `→` types it in.
//!
//! The command being typed is tracked from the keystrokes rterm forwards to
//! the PTY. Any key that can rewrite the line behind rterm's back — Tab
//! completion, cursor motion, a paste, a shortcut — marks the buffer
//! untrusted until the next Enter, so a half-tracked line can never offer a
//! wrong suggestion. On top of that the draw path re-checks the buffer
//! against the text the screen really shows in front of the cursor.

use std::fs;
use std::path::{Path, PathBuf};

/// How many history entries are kept for matching.
pub const HISTORY: usize = 1000;

/// Characters typed since the last Enter, plus how far rterm may trust them.
#[derive(Debug, Clone, PartialEq)]
pub struct Typing {
    text: String,
    trusted: bool,
}

impl Default for Typing {
    fn default() -> Self {
        Self::new()
    }
}

impl Typing {
    pub fn new() -> Self {
        Self {
            text: String::new(),
            trusted: true,
        }
    }

    /// A printable character reached the shell.
    pub fn push(&mut self, c: char) {
        if self.trusted && self.text.len() < 4096 {
            self.text.push(c);
        }
    }

    /// The shell deleted the last character.
    pub fn backspace(&mut self) {
        if self.trusted {
            self.text.pop();
        }
    }

    /// Something rterm cannot model touched the line: stop suggesting (and
    /// stop recording history) until the next prompt.
    pub fn desync(&mut self) {
        self.trusted = false;
        self.text.clear();
    }

    /// Enter was pressed: hand the command back for the history and arm the
    /// next prompt. `None` when the line was never tracked faithfully.
    pub fn commit(&mut self) -> Option<String> {
        let cmd = if self.trusted && !self.text.is_empty() {
            Some(std::mem::take(&mut self.text))
        } else {
            self.text.clear();
            None
        };
        self.trusted = true;
        cmd
    }

    /// The accepted tail of a suggestion is now part of the line.
    pub fn accept(&mut self, tail: &str) {
        if self.trusted {
            self.text.push_str(tail);
        }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn trusted(&self) -> bool {
        self.trusted
    }
}

/// Where rterm looks for the shell's own history (`$HISTFILE`, else bash's).
pub fn history_path() -> PathBuf {
    match std::env::var("HISTFILE") {
        Ok(path) if !path.is_empty() => PathBuf::from(path),
        _ => PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".bash_history"),
    }
}

/// Newest-first copy of the shell history; blank lines and bash's `#…`
/// timestamp records are dropped.
pub fn load_history(path: &Path) -> Vec<String> {
    let Ok(text) = fs::read_to_string(path) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for line in text.lines().rev() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        out.push(line.to_string());
        if out.len() >= HISTORY {
            break;
        }
    }
    out
}

/// The most recent entry that continues `prefix`: only **the rest of it**.
///
/// The ghost is painted right where the cursor sits, so returning the whole
/// entry would show `git git push origin` and `End` would type the command
/// twice. `starts_with` holds for the first `prefix.len()` bytes, so that
/// offset is always a char boundary — even mid-CJK or mid-emoji.
pub fn match_history<'a>(history: &'a [String], prefix: &str) -> Option<&'a str> {
    if prefix.is_empty() {
        return None;
    }
    let entry = history
        .iter()
        .find(|entry| entry.len() > prefix.len() && entry.starts_with(prefix))?;
    Some(&entry[prefix.len()..])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hist(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn match_picks_the_newest_longer_entry() {
        // Newest first, the order `load_history` and `commit_typed` keep.
        let h = hist(&["git push origin", "git status", "ls -la"]);
        assert_eq!(match_history(&h, "git "), Some("push origin"));
        assert_eq!(match_history(&h, "git s"), Some("tatus"));
        // An exact hit is not a suggestion (nothing left to add).
        assert_eq!(match_history(&h, "git status"), None);
        assert_eq!(match_history(&h, "make"), None);
        assert_eq!(match_history(&h, ""), None);
    }

    /// The ghost is drawn over the cells right after the cursor, so what it
    /// offers must complete the line and nothing more — otherwise the screen
    /// reads `git git push origin` and accepting types the command twice.
    #[test]
    fn the_tail_always_finishes_the_line_it_belongs_to() {
        let h = hist(&["git push origin", "cd ~/фото", "ls -la"]);
        for prefix in ["g", "git ", "git p", "c", "cd ~/ф", "ls -", "l"] {
            let tail = match_history(&h, prefix).expect("prefix is a prefix");
            let entry = h
                .iter()
                .find(|e| e.starts_with(prefix))
                .expect("the entry behind the match");
            assert_eq!(
                format!("{prefix}{tail}"),
                *entry,
                "prefix + ghost must rebuild the history line"
            );
            assert!(
                !tail.is_empty() && !tail.starts_with(prefix),
                "the ghost repeats what was already typed: {prefix:?} + {tail:?}"
            );
        }
    }

    #[test]
    fn typing_follows_keystrokes_and_refuses_unknown_ones() {
        let mut t = Typing::new();
        for c in "git che".chars() {
            t.push(c);
        }
        assert_eq!(t.text(), "git che");
        t.backspace();
        assert_eq!(t.text(), "git ch");
        t.push('e');
        assert_eq!(t.text(), "git che");
        assert!(t.trusted());
        // Tab completes the line somewhere rterm cannot see.
        t.desync();
        assert!(!t.trusted());
        assert_eq!(t.text(), "");
        // Nothing typed while untrusted may leak into the history.
        t.push('x');
        assert_eq!(t.commit(), None);
        assert!(t.trusted(), "Enter arms the next prompt");
        t.push('l');
        assert_eq!(t.commit().as_deref(), Some("l"));
        assert_eq!(t.text(), "");
    }

    #[test]
    fn accepted_tail_becomes_part_of_the_line() {
        let mut t = Typing::new();
        for c in "git ".chars() {
            t.push(c);
        }
        t.accept("status");
        assert_eq!(t.text(), "git status");
        assert_eq!(t.commit().as_deref(), Some("git status"));
    }

    #[test]
    fn history_load_drops_comments_and_keeps_newest_first() {
        let path = std::env::temp_dir().join(format!("rterm-hist-{}.txt", std::process::id()));
        fs::write(&path, "#1700000000\n# comment\ncmake --build target\n\nls\n").unwrap();
        let h = load_history(&path);
        let _ = fs::remove_file(&path);
        assert_eq!(h, vec!["ls".to_string(), "cmake --build target".to_string()]);
        assert!(load_history(Path::new("/nonexistent/rterm-hist")).is_empty());
    }
}
