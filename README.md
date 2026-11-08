# rterm
<img width="1920" height="1062" alt="Screenshot from 2026-11-08 23-42-38" src="https://github.com/user-attachments/assets/bfeb80c5-b256-40eb-8dc7-de9fad2ffb73" />
A fast, low-memory terminal emulator with first-class image, GIF and video
support, a built-in starship-style prompt and a live settings panel.

- **Media**: kitty graphics, iTerm2 inline images and rterm's own OSC 1338
  video overlays — decoded on worker threads so a 20 MP JPEG never blocks a
  frame.
- **Look**: 11 color presets, animated backgrounds (gradient / plasma /
  aurora), bloom, grain, scanlines, real window transparency and a liquid-glass
  settings panel (frosted, saturated, sheen + inner rim).
- **Feel**: tabs, splits, SGR mouse, scrollback, copy-on-select, per-line
  entrance animations (fade / slide / wipe) and a native powerline bash prompt
  with themes, icons, git, exit status and command duration.

## Build

```sh
cargo build --release
cp target/release/rterm ~/.local/bin/
```

`softbuffer` is patched locally (see `Cargo.toml`) so surfaces are presented
with an alpha channel — that is what makes window transparency work. The
patched crate lives in `vendor/`.

## Usage

```
rterm [options]                 start the terminal
rterm config [--init] [--edit] [--path <file>]  manage the config file

-e, --command <cmd...>    run <cmd> instead of $SHELL (rest of the args)
-d, --working-dir <dir>   start in <dir>
    --settings           start with the settings panel open
-h, --help                print this help
-V, --version             print the version
```

Config commands:

```sh
rterm config --init          # write the default config and print its path
rterm config --edit          # open the editor window (Enter saves, Esc quits)
rterm config --path <file>   # use <file> instead of the default path
```

## Configuration

Everything lives in `~/.config/rterm/config.toml` (or
`$XDG_CONFIG_HOME/rterm/config.toml`). The file is **watched**: edits are
applied live, and a broken file falls back to defaults with a warning.
Regenerate a commented default with `rterm config --init`.

Main sections: `[window]`, `[font]`, `[colors]`, `[theme]`, `[cursor]`,
`[effects]`, `[media]`, `[ui]`, `[scrollback]`, `[shell]`, `[prompt]`, `[anim]`.

Notable options:

| Option | What it does |
| --- | --- |
| `window.opacity` | window alpha (0.0–1.0), blended by the compositor |
| `ui.panel_alpha` | settings panel opacity |
| `ui.panel_blur` / `panel_blur_radius` | blur the terminal content behind the panel |
| `ui.tab_bar` | tab strip visibility: `auto` (only with 2+ tabs), `on`, `off` |
| `ui.tab_bar_position` | strip edge: `top` or `bottom` |
| `ui.tab_style` | strip design: `chip`, `underline` or `minimal` |
| `ui.tab_bar_alpha` | strip background opacity (0.2–1.0) |
| `ui.tab_close` | the ✕ button on every tab (click it to close that tab) |
| `ui.tab_active_bg` / `tab_active_fg` | colours of the selected tab (empty = theme) |
| `ui.tab_inactive_fg` / `tab_bar_bg` | colours of the other tabs and of the strip |
| `ui.suggest` | grey autosuggestion of an older command after the cursor |
| `ui.suggest_color` | colour of that suggestion (default `#7d8590`) |
| `ui.glass` | liquid-glass settings panel instead of flat frost |
| `ui.glass_strength` / `glass_tint` | glass intensity (0–1) and its tint colour |
| `anim.ms` | line entrance duration in ms (`0` disables it) |
| `anim.style` | `fade`, `slide` or `wipe` |
| `anim.prompt` | animate the live prompt/command line too |
| `effects.background` | `solid`, `gradient`, `plasma`, `aurora` or `image` |
| `effects.gradient_animated` etc. | animated backgrounds and post effects |
| `theme.mode` | `manual`, or `auto` to follow GNOME/KDE dark/light |
| `prompt.enable` | built-in prompt for new bash shells |
| `prompt.style` | `plain` or `powerline` |
| `prompt.theme` | `custom`, `tokyo`, `gruvbox`, `dracula`, `nord`, `catppuccin` |
| `prompt.icons` / `multiline` / `user` / `venv` | Nerd Font icons, two-line layout, user@host and virtualenv segments |
| `media.max_cache_mb` | budget for decoded images/GIFs/video frames |

### Color presets

`preset` in the settings panel cycles 11 schemes: tokyo night, gruvbox,
catppuccin, nord, dracula, one dark, everforest, rose pine, monokai,
solarized dark, solarized light.

### Profiles

Named snapshots of the config, stored as plain files in
`~/.config/rterm/profiles/<name>.toml` and managed from the first group of
the settings panel (F1):

| Row | Keys | What it does |
| --- | --- | --- |
| `profile` | `←` `→` | cycle the saved profiles; each switch loads and applies it live |
| `save as` | `Space`, type a name, `Enter` | write the current settings (as previewed) under that name |
| `delete` | `→` | remove the selected profile (`←` only shows a hint) |

Names allow `A–Z a–z 0–9 - _ .` (max 48 chars). Loading a profile is
instant — no restart, no re-login — but it does **not** touch
`config.toml`: press `S` in the panel if the loaded profile should become
your on-disk config. The list is re-read from disk every time the panel
opens, so files can also be copied or edited by hand.

### Autosuggestions

While you type at a prompt, rterm looks for an older command that starts the
same way — in `~/.bash_history` (or `$HISTFILE`) plus everything typed in this
window — and draws the rest in grey right after the cursor. `End` or `→` types
it in; any key that can change the line behind rterm's back (Tab completion,
arrows, a paste, a shortcut) hides the hint until the next prompt, and before
drawing it rterm checks the text on screen, so the grey tail can never disagree
with what the shell actually has.

Toggle it with the `suggest` row of the settings panel or with
`ui.suggest = false`; `ui.suggest_color` tints the grey.

### Motion and liquid glass

New output lines fade in over `anim.ms` (default 140 ms, `0` turns the effect
off). `anim.style` picks the motion: `fade`, `slide` (drifts in from the left)
or `wipe` (uncovered left to right). A line is stamped only when it changes
from empty to non-empty, so typing never replays the prompt; `anim.prompt`
decides whether the live prompt/command line takes part at all. Everything is
frame-budgeted: the event loop asks for extra frames only while something is
actually moving.

The settings panel uses liquid glass by default. It keeps the panel blur but
adds a saturation boost, a `glass_tint` film, a diagonal sheen and a bright
inner rim, all scaled by `ui.glass_strength` (0–1). Set `ui.glass = false` to
go back to a flat frosted panel. Both effects are toggleable from the **anim**
and **ui** sections of the F1 panel.

## Settings panel (F1)
<img width="1920" height="1080" alt="Screenshot from 2026-11-08 23-44-27" src="https://github.com/user-attachments/assets/4ca63bcc-fc7f-4687-b4b9-fe81f3baaa3e" />
Press **F1** in any pane. Changes preview instantly; nothing touches the
config file until you save.
<img width="1917" height="1079" alt="Screenshot from 2026-11-08 23-48-16" src="https://github.com/user-attachments/assets/c4b736c3-480d-4bf1-a79a-d8e1522a19f2" />
| Key | Action |
| --- | --- |
| `↑` `↓` | select field |
| `←` `→` | change value / toggle |
| `PgUp` `PgDn` | step by 5 |
| `Home` | reset field to default |
| `Ctrl+Home` | jump to first field |
| `End` | jump to last field |
| `Space` | toggle / begin editing (hex colors, text) |
| `Enter` | apply edit · save panel |
| `S` | save config |
| `R` | revert to the config on disk |
| `Esc` / `F1` | close panel |

## Keybindings

| Keys | Action |
| --- | --- |
| `Ctrl+Shift+C` / `V` | copy / paste |
| `Ctrl+Insert` / `Shift+Insert` | copy / paste (X11 style) |
| `Ctrl+Shift+T` / `W` | new / close tab |
| `Ctrl+Shift+1…9` | select tab |
| `Ctrl+Tab` / `Ctrl+Shift+Tab` | cycle tabs |
| `Ctrl+Shift+B` | cycle tab strip: auto → on → off |
| `End` / `→` | accept the grey command suggestion |
| `Ctrl+Shift+E` / `O` | split right / down |
| `Ctrl+Shift+X` | close pane |
| `Ctrl+Shift+←↑→↓` | focus pane |
| `Ctrl+Shift++` / `-` | font size in / out |
| `Ctrl+Shift+N` | new window |
| `Ctrl+Shift+K` | clear screen, scrollback and media |
| `Shift+PgUp` / `PgDn` | scroll the scrollback |
| `F1` | settings panel |

Mouse: SGR mouse reporting, wheel scroll, drag, **copy on select** and **middle-click paste** (both configurable in `[ui]`). Inside the settings panel, click a section header to open/close it, click a colour swatch to open the live HSL picker, and click a text row to edit it. In the tab strip, a click switches tabs and a click on the ✕ closes that tab.


In the colour picker (`Enter` on a colour row): `←`/`→` move the hue around the wheel, `↑`/`↓` move the marker along the wheel (saturation), `PgUp`/`PgDn` brighten/darken, `Shift` makes the steps bigger, `Enter` keeps the colour, `Esc` restores the original. The wheel shows Hue and Saturation, while the small colour square in the middle shows the current RGB values; the terminal recolours as you move the wheel.

## Images, GIF and video

rterm intercepts media sequences from the PTY stream before they reach the
VT emulator:

| Protocol | Sequence |
| --- | --- |
| kitty graphics | `ESC _ G … ST` (transmit / print / delete / query) |
| iTerm2 inline images | `OSC 1337;File=… BEL` |
| rterm video overlays | `OSC 1338;… BEL` (frames also stream over a local UNIX socket, `media.allow_socket`) |

Stills (PNG, JPEG, …) and animated GIFs are anchored to cell rectangles and
decoded on background threads; GIF looping is configurable
(`media.loop_gifs`). Decoded media is kept under a cache budget so memory
stays flat.

## Built-in prompt

For new bash shells rterm generates `~/.config/rterm/prompt.bash` and starts
the shell as `bash --rcfile <that file>`. The rc file sources the usual
system and user rc files first, so aliases and keybindings from `~/.bashrc`
keep working — and shells started outside rterm are never touched.

Segments (each toggleable in the panel): directory, git branch + dirty flag,
exit status, command duration, time, `user@host` and the active Python
virtualenv. The layout is `plain` (colored text) or `powerline` (filled
segments with arrows); `prompt.icons` uses Nerd Font glyphs instead of the
`symbol_dir` / `symbol_git` text, and `prompt.multiline = false` keeps
everything on one line. A named `prompt.theme` (`tokyo`, `gruvbox`, `dracula`,
`nord`, `catppuccin`) overrides the five `color_*` values without touching the
terminal palette, so starship/oh-my-zsh-style prompts work natively — no
plugin, no external binary, no shell framework. Other shells (zsh, fish) are
not supported yet.

## Compositor integration (optional)

Window transparency works anywhere; background blur behind the window is a
compositor feature. For example, in niri:

```kdl
match app-id="rterm" {
    opacity 0.72
    background-effect {
        blur true
        xray true
    }
    draw-border-with-background false
}
```

Wayland only; on other setups rterm still runs, just without the blur
pass-through.

## Development

```sh
cargo test          # unit tests
cargo clippy        # lints
cargo build --release
```

The code is organized as: `intercept` → `protocol` (media parsing),
`pty` (shell), `input` (keys/mouse), `render` (softbuffer painting),
`app` (event loop, panes/tabs), `settings`/`config` (UI + file),
`prompt` (rc generation), `theme` (desktop colors).

## License

MIT
