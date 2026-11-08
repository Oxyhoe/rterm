//! The built-in prompt: rterm generates a bash rc file that installs a
//! starship-style prompt (segments: user, virtualenv, directory, git, exit
//! status, duration, time) driven entirely by [`PromptCfg`] - edited from the
//! F1 menu.
//!
//! Shells started by rterm get `bash --rcfile ~/.config/rterm/prompt.bash`,
//! which sources the system and user rc files first and only then installs
//! the generated prompt, so aliases/keys from `~/.bashrc` keep working and
//! shells outside rterm are never touched.
//!
//! Two layouts are supported: `plain` (coloured text, the classic look) and
//! `powerline` (filled segments with Nerd Font arrows). A theme name swaps the
//! five colours without touching the rest of the config.

use std::fs;
use std::path::PathBuf;

use crate::config::{Config, PromptCfg};
use crate::palette::{self, Rgb8};

/// Where the generated rc file lives.
pub fn rc_path() -> PathBuf {
    Config::config_dir().join("prompt.bash")
}

/// Rewrite the rc file from the current config (best effort: a failed write
/// only means new shells keep the previous prompt).
pub fn sync(cfg: &PromptCfg) {
    let path = rc_path();
    if let Some(dir) = path.parent() {
        let _ = fs::create_dir_all(dir);
    }
    let _ = fs::write(&path, bash_rc(cfg));
}

/// Extra argv for `bash --rcfile ...` when the built-in prompt applies to
/// `program` (or `$SHELL` when the config does not override it).
pub fn rcfile_args(program: Option<&str>, args: &[String]) -> Option<Vec<String>> {
    // $SHELL is a full path ("/bin/bash"); compare the basename in both arms.
    let raw = match program {
        Some(p) => p.to_string(),
        None => std::env::var("SHELL").ok()?,
    };
    let name = raw.rsplit('/').next().unwrap_or(&raw);
    if name != "bash" {
        return None;
    }
    let mut out = vec!["--rcfile".to_string(), rc_path().display().to_string()];
    out.extend(args.iter().cloned());
    Some(out)
}

/// Colours resolved from the prompt theme: dir, git, error, dim, prompt.
fn colors(cfg: &PromptCfg) -> [String; 5] {
    let themed = match cfg.theme.as_str() {
        "tokyo" => ["#7aa2f7", "#9ece6a", "#f7768e", "#565f89", "#bb9af7"],
        "gruvbox" => ["#83a598", "#b8bb26", "#fb4934", "#928374", "#fabd2f"],
        "dracula" => ["#8be9fd", "#50fa7b", "#ff5555", "#6272a4", "#bd93f9"],
        "nord" => ["#88c0d0", "#a3be8c", "#bf616a", "#4c566a", "#b48ead"],
        "catppuccin" => ["#89b4fa", "#a6e3a1", "#f38ba8", "#585b70", "#cba6f7"],
        _ => {
            return [
                cfg.color_dir.clone(),
                cfg.color_git.clone(),
                cfg.color_err.clone(),
                cfg.color_dim.clone(),
                cfg.color_prompt.clone(),
            ]
        }
    };
    themed.map(str::to_string)
}

/// Split a `#rrggbb` colour into bytes with the same fallback palette uses.
fn rgb(hex: &str) -> Rgb8 {
    palette::parse_hex_or(hex, [0xc0, 0xca, 0xf5])
}

/// A darkened copy of `hex`, used as the powerline segment background.
fn darken(hex: &str, k: f32) -> Rgb8 {
    let c = rgb(hex);
    let k = k.clamp(0.0, 1.0);
    [
        (f32::from(c[0]) * k) as u8,
        (f32::from(c[1]) * k) as u8,
        (f32::from(c[2]) * k) as u8,
    ]
}

/// The full rc file body for `cfg`.
pub fn bash_rc(cfg: &PromptCfg) -> String {
    let themed = colors(cfg);
    let mut out = String::new();
    out.push_str("# rterm built-in prompt - generated, your edits are overwritten.\n");
    out.push_str("for _rterm_rc in /etc/bash/bashrc /etc/bash.bashrc; do\n");
    out.push_str("  [ -r \"$_rterm_rc\" ] && . \"$_rterm_rc\"\n");
    out.push_str("done\nunset _rterm_rc\n");
    out.push_str("[ -r \"$HOME/.bashrc\" ] && . \"$HOME/.bashrc\"\n\n");

    // Duration: a DEBUG trap stamps SECONDS when the typed command starts
    // (PS0 would only be *displayed*, not executed). `rterm_after_prompt`
    // keeps the stamp tied to the first command after each prompt.
    if cfg.show_duration {
        out.push_str("rterm_after_prompt=1\n");
        out.push_str("rterm_t0=\"\"\n");
        out.push_str("rterm_dbg() {\n");
        out.push_str("  if [ \"$rterm_after_prompt\" = 1 ]; then\n");
        out.push_str("    rterm_t0=$SECONDS\n");
        out.push_str("    rterm_after_prompt=0\n");
        out.push_str("  fi\n");
        out.push_str("}\n");
        out.push_str("trap 'rterm_dbg' DEBUG\n");
    }

    out.push_str("rterm_prompt_cmd() {\n");
    out.push_str("  local st=$?\n");
    if cfg.show_duration {
        out.push_str("  local dur=\"\"\n");
        out.push_str("  if [ -n \"${rterm_t0-}\" ]; then\n");
        out.push_str("    local d=$((SECONDS - rterm_t0))\n");
        out.push_str("    if [ \"$d\" -ge 2 ]; then dur=\"${d}s\"; fi\n");
        out.push_str("  fi\n");
        out.push_str("  rterm_t0=\"\"\n");
        out.push_str("  rterm_durseg=\"\"\n");
        out.push_str("  [ -n \"$dur\" ] && rterm_durseg=\"$dur\"\n");
        out.push_str("  rterm_after_prompt=1\n");
    }
    if cfg.show_status {
        out.push_str("  rterm_stseg=\"\"\n");
        out.push_str("  if [ \"$st\" -ne 0 ]; then rterm_stseg=\"$st\"; fi\n");
    }
    if cfg.show_time {
        out.push_str("  rterm_timeseg=\"$(date +%H:%M)\"\n");
    }
    if cfg.show_user {
        out.push_str("  rterm_userseg=\"$USER\"\n");
    }
    if cfg.show_venv {
        out.push_str("  rterm_venvseg=\"\"\n");
        out.push_str("  if [ -n \"${VIRTUAL_ENV:-}\" ]; then\n");
        out.push_str("    rterm_venvseg=\"${VIRTUAL_ENV##*/}\"\n");
        out.push_str("  fi\n");
    }
    if cfg.show_dir {
        out.push_str(&dir_block(cfg));
    }
    if cfg.show_git {
        out.push_str(&git_block(cfg));
    }
    out.push_str("}\n");
    out.push_str("PROMPT_COMMAND=rterm_prompt_cmd\n");
    out.push_str(&format!(
        "PS1={}\n",
        single_quote(&ps1(cfg, &themed[4]))
    ));
    out
}

/// Directory segment; the variable holds plain text, styling lives in PS1
/// (bash decodes prompt escapes before expanding variables).
fn dir_block(cfg: &PromptCfg) -> String {
    match cfg.dir_style.as_str() {
        "last" => "  local b=${PWD##*/}\n  [ -z \"$b\" ] && b=/\n  rterm_dirseg=\"$b\"\n"
            .to_string(),
        "short" => "  local p=${PWD/#$HOME/~}\n  local IFS=/\n  local -a seg\n  \
                    read -r -a seg <<<\"$p\"\n  local n=${#seg[@]}\n  \
                    if [ \"$n\" -gt 3 ]; then\n    p=\"${seg[0]}/.../${seg[n-2]}/${seg[n-1]}\"\n  fi\n  \
                    rterm_dirseg=\"$p\"\n"
            .to_string(),
        _ => "  local p=${PWD/#$HOME/~}\n  rterm_dirseg=\"$p\"\n".to_string(),
    }
}

/// Git segment; `git_dirty` adds the `*` marker for a dirty worktree.
fn git_block(cfg: &PromptCfg) -> String {
    let dirty = if cfg.git_dirty {
        "if [ -n \"$(git status --porcelain 2>/dev/null)\" ]; then dirty=\" *\"; fi; "
    } else {
        ""
    };
    let dirty_var = if cfg.git_dirty { "$dirty" } else { "" };
    format!(
        "  rterm_gitseg=\"\"\n  \
         if command -v git >/dev/null 2>&1 && git rev-parse --is-inside-work-tree >/dev/null 2>&1; then\n    \
         local br=\"\" dirty=\"\"\n    \
         br=$(git symbolic-ref --short HEAD 2>/dev/null) || br=$(git rev-parse --short HEAD 2>/dev/null)\n    \
         [ -n \"$br\" ] && {{ {dirty}rterm_gitseg=\"$br{dirty_var}\"; }}\n  \
         fi\n"
    )
}

fn dir_icon(cfg: &PromptCfg) -> String {
    if cfg.icons {
        "\u{f07b}".to_string()
    } else {
        cfg.symbol_dir.clone()
    }
}

fn git_icon(cfg: &PromptCfg) -> String {
    if cfg.icons {
        "\u{e725}".to_string()
    } else {
        cfg.symbol_git.clone()
    }
}

fn user_icon(cfg: &PromptCfg) -> String {
    if cfg.icons {
        "\u{f007}".to_string()
    } else {
        String::new()
    }
}

fn venv_icon(cfg: &PromptCfg) -> String {
    if cfg.icons {
        "\u{e73c}".to_string()
    } else {
        String::new()
    }
}

fn cross_icon(cfg: &PromptCfg) -> String {
    if cfg.icons {
        "\u{f00d}".to_string()
    } else {
        String::new()
    }
}

fn clock_icon(cfg: &PromptCfg) -> String {
    if cfg.icons {
        "\u{f017}".to_string()
    } else {
        String::new()
    }
}

fn calendar_icon(cfg: &PromptCfg) -> String {
    if cfg.icons {
        "\u{f073}".to_string()
    } else {
        String::new()
    }
}

/// The PS1 value: one `${var:+...}` conditional per enabled segment, an
/// optional newline, then the prompt character. All escape sequences are
/// literal here because bash decodes PS1 *before* expanding variables.
fn ps1(cfg: &PromptCfg, prompt_hex: &str) -> String {
    let colors = colors(cfg);
    let mut out = String::new();
    let segments: [(&str, &str, String); 7] = [
        (
            "rterm_userseg",
            &colors[1],
            user_icon(cfg),
        ),
        (
            "rterm_venvseg",
            &colors[4],
            venv_icon(cfg),
        ),
        ("rterm_dirseg", &colors[0], dir_icon(cfg)),
        ("rterm_gitseg", &colors[1], git_icon(cfg)),
        ("rterm_stseg", &colors[2], cross_icon(cfg)),
        ("rterm_durseg", &colors[3], clock_icon(cfg)),
        ("rterm_timeseg", &colors[3], calendar_icon(cfg)),
    ];
    for (var, hex, icon) in segments {
        if var_enabled(cfg, var) {
            out.push_str(&segment(cfg, var, hex, &icon));
        }
    }
    if cfg.multiline {
        out.push_str("\\n");
    } else if !out.is_empty() {
        out.push(' ');
    }
    out.push_str(&plain(prompt_hex, &cfg.symbol_prompt));
    out.push(' ');
    out
}

/// Whether a generated segment variable exists at all, so PS1 never expands
/// an unset name (which would print a blank but still pad powerline rows).
fn var_enabled(cfg: &PromptCfg, var: &str) -> bool {
    match var {
        "rterm_userseg" => cfg.show_user,
        "rterm_venvseg" => cfg.show_venv,
        "rterm_dirseg" => cfg.show_dir,
        "rterm_gitseg" => cfg.show_git,
        "rterm_stseg" => cfg.show_status,
        "rterm_durseg" => cfg.show_duration,
        "rterm_timeseg" => cfg.show_time,
        _ => false,
    }
}

/// Foreground-only escape wrapper (literal text in PS1, prompt character).
fn plain(hex: &str, body: &str) -> String {
    let [r, g, b] = rgb(hex);
    format!("\\[\\e[38;2;{r};{g};{b}m\\]{body}\\[\\e[0m\\]")
}

/// One segment as a PS1 conditional: hidden while the variable is empty, so a
/// clean git tree or a zero exit status never leaves a blank powerline block.
fn segment(cfg: &PromptCfg, var: &str, hex: &str, icon: &str) -> String {
    let [r, g, b] = rgb(hex);
    let prefix = if icon.is_empty() {
        String::new()
    } else {
        format!("{icon} ")
    };
    if cfg.style == "powerline" {
        let bg = darken(hex, 0.24);
        let arrow = '\u{e0b0}';
        format!(
            "${{{var}:+\\[\\e[48;2;{};{};{}m\\]\\[\\e[38;2;{r};{g};{b}m\\] {prefix}${{{var}}} \
             \\[\\e[0m\\]\\[\\e[38;2;{};{};{}m\\]{arrow}\\[\\e[0m\\]}}",
            bg[0], bg[1], bg[2], bg[0], bg[1], bg[2]
        )
    } else {
        format!(
            "${{{var}:+\\[\\e[38;2;{r};{g};{b}m\\] {prefix}${{{var}}}\\[\\e[0m\\]}}"
        )
    }
}

/// Single-quote `s` for use inside a shell script.
fn single_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> PromptCfg {
        PromptCfg::default()
    }

    #[test]
    fn rc_installs_prompt_after_user_rc() {
        let rc = bash_rc(&cfg());
        assert!(rc.contains("[ -r \"$HOME/.bashrc\" ] && . \"$HOME/.bashrc\""));
        assert!(rc.contains("PROMPT_COMMAND=rterm_prompt_cmd"));
        assert!(rc.contains("PS1='"));
        assert!(rc.contains("rterm_prompt_cmd()"));
        // The function must capture $? before anything else.
        let body = rc.split("rterm_prompt_cmd() {").nth(1).unwrap();
        assert!(body.trim_start().starts_with("local st=$?"));
    }

    #[test]
    fn powerline_uses_segment_backgrounds_and_arrows() {
        let rc = bash_rc(&cfg());
        assert!(rc.contains("\\e[48;2;"), "powerline needs a background");
        assert!(rc.contains('\u{e0b0}'), "powerline needs its arrow");
        assert!(rc.contains("rterm_dirseg"));
        assert!(rc.contains("rterm_gitseg"));
        assert!(rc.contains("rterm_stseg"));
        assert!(rc.contains("\\n"));
        assert!(rc.contains("❯"));
        assert!(rc.contains("trap 'rterm_dbg' DEBUG"));
        assert!(rc.contains("rterm_after_prompt=1"));
    }

    #[test]
    fn plain_style_drops_backgrounds_but_keeps_colours() {
        let mut c = cfg();
        c.style = "plain".into();
        let rc = bash_rc(&c);
        assert!(rc.contains("\\e[38;2;"));
        assert!(!rc.contains("\\e[48;2;"));
        assert!(!rc.contains('\u{e0b0}'));
    }

    #[test]
    fn themes_override_the_custom_colours() {
        let mut c = cfg();
        c.theme = "nord".into();
        let rc = bash_rc(&c);
        assert!(rc.contains("\\e[38;2;136;192;208m"), "nord dir colour");
        c.theme = "custom".into();
        assert!(bash_rc(&c).contains("\\e[38;2;125;207;255m"));
    }

    #[test]
    fn disabled_segments_leave_no_trace() {
        let mut c = cfg();
        c.show_git = false;
        c.show_status = false;
        c.show_duration = false;
        c.show_time = false;
        c.show_venv = false;
        let rc = bash_rc(&c);
        assert!(!rc.contains("rterm_gitseg"));
        assert!(!rc.contains("git status"));
        assert!(!rc.contains("rterm_stseg"));
        assert!(!rc.contains("rterm_dbg"));
        assert!(!rc.contains("trap "));
        assert!(!rc.contains("rterm_timeseg"));
        assert!(!rc.contains("rterm_venvseg"));
        assert!(rc.contains("rterm_dirseg"));
    }

    #[test]
    fn user_segment_is_optional() {
        let mut c = cfg();
        assert!(!bash_rc(&c).contains("rterm_userseg"));
        c.show_user = true;
        assert!(bash_rc(&c).contains("rterm_userseg"));
    }

    #[test]
    fn multiline_can_be_turned_off() {
        let mut c = cfg();
        c.multiline = false;
        let rc = bash_rc(&c);
        let ps1 = rc.split("PS1=").nth(1).unwrap();
        assert!(!ps1.contains("\\n"));
        c.multiline = true;
        let rc = bash_rc(&c);
        let ps1 = rc.split("PS1=").nth(1).unwrap();
        assert!(ps1.contains("\\n"));
    }

    #[test]
    fn icons_can_be_swapped_for_the_old_symbols() {
        let mut c = cfg();
        c.icons = false;
        c.symbol_dir = "in".into();
        let rc = bash_rc(&c);
        assert!(rc.contains(" in ${rterm_dirseg}"), "old symbol text should survive");
        assert!(!rc.contains('\u{f07b}'));
        c.icons = true;
        assert!(bash_rc(&c).contains('\u{f07b}'));
    }

    #[test]
    fn dir_styles_bake_the_right_snippet() {
        let mut c = cfg();
        c.dir_style = "last".into();
        assert!(bash_rc(&c).contains("local b=${PWD##*/}"));
        c.dir_style = "short".into();
        let rc = bash_rc(&c);
        assert!(rc.contains("read -r -a seg"));
        assert!(rc.contains("seg[n-2]"));
        c.dir_style = "full".into();
        let rc = bash_rc(&c);
        assert!(rc.contains("local p=${PWD/#$HOME/~}"));
        assert!(!rc.contains("seg[n-2]"));
    }

    #[test]
    fn git_dirty_marker_is_optional() {
        let mut c = cfg();
        c.git_dirty = false;
        let rc = bash_rc(&c);
        assert!(rc.contains("$br"));
        assert!(!rc.contains("git status --porcelain"));
        c.git_dirty = true;
        assert!(bash_rc(&c).contains("git status --porcelain"));
    }

    #[test]
    fn rcfile_args_only_for_bash() {
        let args: Vec<String> = Vec::new();
        let out = rcfile_args(Some("/bin/bash"), &args).expect("bash");
        assert_eq!(out[0], "--rcfile");
        assert!(out[1].ends_with("prompt.bash"));
        assert!(rcfile_args(Some("/bin/zsh"), &args).is_none());
        assert!(rcfile_args(Some("fish"), &args).is_none());
    }

    #[test]
    fn rcfile_args_shell_env_matches_on_basename() {
        if let Ok(sh) = std::env::var("SHELL") {
            let out = rcfile_args(None, &[]);
            let expect_bash = sh.rsplit('/').next() == Some("bash");
            assert_eq!(out.is_some(), expect_bash, "SHELL={sh}");
            if let Some(o) = out {
                assert_eq!(o[0], "--rcfile");
                assert!(o[1].ends_with("prompt.bash"));
            }
        }
    }

    #[test]
    fn generated_rc_is_valid_bash() {
        let mut c = cfg();
        c.show_user = true;
        c.show_venv = true;
        let body = bash_rc(&c);
        let path = std::env::temp_dir().join(format!(
            "rterm-prompt-test-{}.bash",
            std::process::id()
        ));
        std::fs::write(&path, body).expect("write");
        let status = std::process::Command::new("bash")
            .arg("-n")
            .arg(&path)
            .status()
            .expect("bash");
        let _ = std::fs::remove_file(&path);
        assert!(status.success(), "generated prompt.bash does not parse");
    }

    #[test]
    fn quotes_are_escaped() {
        assert_eq!(single_quote("it's"), "'it'\\''s'");
        let mut c = cfg();
        c.icons = false;
        c.symbol_prompt = "o'clock".into();
        assert!(bash_rc(&c).contains("o'\\''clock"));
    }
}
