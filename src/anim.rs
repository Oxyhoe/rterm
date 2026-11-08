//! Entrance animation for terminal lines.
//!
//! A line is stamped the moment it turns from empty to non-empty and then
//! settles over [`AnimCfg::ms`]. Typing does **not** restart a line that is
//! already on screen (otherwise every keystroke would replay the prompt), and
//! the live cursor row is skipped unless `anim.prompt` is on.

use std::time::Instant;

use alacritty_terminal::ansi::{Color, NamedColor};
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::RenderableContent;

use crate::config::Config;
use crate::render::{Layout, RowFx};

#[derive(Default)]
pub struct TextFx {
    /// Whether each visible row currently holds anything worth showing.
    filled: Vec<bool>,
    /// When a row appeared; `None` once it has settled or is empty.
    start: Vec<Option<Instant>>,
    /// The row finished its animation and must not be stamped again until it
    /// goes blank and comes back.
    settled: Vec<bool>,
    /// Latest effect per row, handed to the renderer.
    fx: Vec<RowFx>,
}

impl TextFx {
    pub fn new() -> Self {
        Self::default()
    }

    /// Recompute the visible rows from the terminal grid.
    pub fn refresh(
        &mut self,
        content: &mut RenderableContent<'_>,
        layout: &Layout,
        cfg: &Config,
        now: Instant,
    ) {
        let rows = layout.rows.max(1);
        if self.filled.len() != rows {
            self.filled.resize(rows, false);
            self.start.resize(rows, None);
            self.settled.resize(rows, false);
            self.fx.resize(rows, RowFx::FULL);
        }
        self.filled.iter_mut().for_each(|f| *f = false);

        let offset = content.display_offset as i32;
        for indexed in content.display_iter.by_ref() {
            let cell = indexed.cell;
            if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
                continue;
            }
            let row = indexed.point.line.0 + offset;
            if row < 0 || row as usize >= rows {
                continue;
            }
            let glyph = cell.c != ' ' && !cell.flags.contains(Flags::HIDDEN);
            let painted_bg = cell.bg != Color::Named(NamedColor::Background);
            if glyph || painted_bg {
                self.filled[row as usize] = true;
            }
        }

        let cursor_row = content.cursor.point.line.0 + offset;
        let skip_cursor = !cfg.anim.prompt;
        let style = cfg.anim.style.as_str();
        let ms = cfg.anim.ms.min(2000) as f32;

        for row in 0..rows {
            self.advance(
                row,
                self.filled[row],
                row as i32 == cursor_row,
                skip_cursor,
                style,
                ms,
                layout.cell_w,
                layout.cell_h,
                now,
            );
        }
    }

    /// Apply the state machine to one row. Split out so the rules (stamp once,
    /// never restart on change, skip the cursor row) are testable without a
    /// real terminal grid.
    #[allow(clippy::too_many_arguments)]
    fn advance(
        &mut self,
        row: usize,
        filled: bool,
        is_cursor: bool,
        skip_cursor: bool,
        style: &str,
        ms: f32,
        cell_w: f32,
        cell_h: f32,
        now: Instant,
    ) {
        if self.start.len() <= row {
            self.filled.resize(row + 1, false);
            self.start.resize(row + 1, None);
            self.settled.resize(row + 1, false);
            self.fx.resize(row + 1, RowFx::FULL);
        }
        if !filled {
            self.start[row] = None;
            self.settled[row] = false;
            self.fx[row] = RowFx::FULL;
            return;
        }
        if skip_cursor && is_cursor {
            // Show it at once and never replay it when the cursor moves away.
            self.start[row] = None;
            self.settled[row] = true;
            self.fx[row] = RowFx::FULL;
            return;
        }
        if self.start[row].is_none() {
            if self.settled[row] {
                self.fx[row] = RowFx::FULL;
                return;
            }
            self.start[row] = Some(now);
        }

        let Some(start) = self.start[row] else {
            self.fx[row] = RowFx::FULL;
            return;
        };
        if ms <= 0.0 {
            self.fx[row] = RowFx::FULL;
            self.start[row] = None;
            self.settled[row] = true;
            return;
        }
        let t = now.duration_since(start).as_secs_f32() * 1000.0 / ms;
        if t >= 1.0 {
            self.fx[row] = RowFx::FULL;
            self.start[row] = None;
            self.settled[row] = true;
            return;
        }
        // Ease-out cubic: quick take-off, soft landing.
        let e = 1.0 - (1.0 - t).powi(3);
        self.fx[row] = match style {
            "slide" => RowFx {
                alpha: e,
                dx: -((1.0 - e) * cell_w * 0.75) as i32,
                ..RowFx::FULL
            },
            "wipe" => RowFx {
                alpha: 1.0,
                reveal: e,
                ..RowFx::FULL
            },
            "rise" => RowFx {
                alpha: e,
                dy: ((1.0 - e) * cell_h * 0.55) as i32,
                ..RowFx::FULL
            },
            "drop" => RowFx {
                alpha: e,
                dy: -((1.0 - e) * cell_h * 0.55) as i32,
                ..RowFx::FULL
            },
            // `type` advances in character-sized steps; `wipe` stays smooth.
            "type" => {
                let steps = 24.0;
                let revealed = (e * steps).floor() / steps;
                RowFx {
                    alpha: 1.0,
                    reveal: revealed,
                    ..RowFx::FULL
                }
            }
            "dissolve" => RowFx {
                alpha: 1.0,
                dissolve: e,
                ..RowFx::FULL
            },
            "glow" => RowFx {
                alpha: e,
                flash: (1.0 - e) * 0.9,
                ..RowFx::FULL
            },
            _ => RowFx {
                alpha: e,
                ..RowFx::FULL
            },
        };
    }

    pub fn row_fx(&self) -> &[RowFx] {
        &self.fx
    }

    /// Any row still settling? Keeps animation frames coming from the event loop.
    pub fn active(&self, now: Instant, ms: u64) -> bool {
        let ms = ms.min(2000) as f32;
        if ms <= 0.0 {
            return false;
        }
        self.start.iter().flatten().any(|start| {
            now.saturating_duration_since(*start).as_secs_f32() * 1000.0 < ms
        })
    }

    /// Forget every stamp (used when the font or grid geometry changes so old
    /// rows do not inherit half-finished animations at the new size).
    pub fn reset(&mut self) {
        self.start.iter_mut().for_each(|s| *s = None);
        self.settled.iter_mut().for_each(|s| *s = true);
        self.fx.iter_mut().for_each(|f| *f = RowFx::FULL);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(ms: u64, prompt: bool) -> Config {
        let mut cfg = Config::default();
        cfg.anim.ms = ms;
        cfg.anim.prompt = prompt;
        cfg
    }

    /// The core contract: a blank row gets one stamp when it fills up, and
    /// later frames (typing on the same line) keep that original stamp.
    #[test]
    fn a_new_row_is_stamped_once_and_typing_does_not_restart_it() {
        let mut fx = TextFx::new();
        let t0 = Instant::now();
        fx.advance(3, true, false, false, "fade", 100.0, 8.0, 16.0, t0);
        assert_eq!(fx.start[3], Some(t0));
        let mid = t0 + std::time::Duration::from_millis(30);
        // Still the same row, now with another character on it.
        fx.advance(3, true, false, false, "fade", 100.0, 8.0, 16.0, mid);
        assert_eq!(fx.start[3], Some(t0), "typing restarted the animation");
        assert!(fx.row_fx()[3].alpha > 0.0 && fx.row_fx()[3].alpha < 1.0);

        // Once the duration is over the row settles and stops scheduling.
        let done = t0 + std::time::Duration::from_millis(100);
        let mut settle = TextFx::new();
        settle.advance(3, true, false, false, "fade", 100.0, 8.0, 16.0, t0);
        settle.advance(3, true, false, false, "fade", 100.0, 8.0, 16.0, done);
        assert_eq!(settle.start[3], None);
        assert_eq!(settle.row_fx()[3], RowFx::FULL);
        // A later frame must not treat the settled row as brand new.
        let later = t0 + std::time::Duration::from_millis(250);
        settle.advance(3, true, false, false, "fade", 100.0, 8.0, 16.0, later);
        assert_eq!(settle.start[3], None, "settled row replayed");
        assert_eq!(settle.row_fx()[3], RowFx::FULL);

        // Clearing the row makes the next appearance animate again.
        settle.advance(3, false, false, false, "fade", 100.0, 8.0, 16.0, later);
        settle.advance(3, true, false, false, "fade", 100.0, 8.0, 16.0, later);
        assert_eq!(settle.start[3], Some(later));
    }

    #[test]
    fn wipe_and_slide_have_distinct_shapes() {
        let t0 = Instant::now();
        let mid = t0 + std::time::Duration::from_millis(20);
        let mut wipe = TextFx::new();
        wipe.advance(0, true, false, false, "wipe", 100.0, 8.0, 16.0, t0);
        wipe.advance(0, true, false, false, "wipe", 100.0, 8.0, 16.0, mid);
        assert_eq!(wipe.row_fx()[0].alpha, 1.0);
        assert!(wipe.row_fx()[0].reveal < 1.0);

        let mut slide = TextFx::new();
        slide.advance(0, true, false, false, "slide", 100.0, 8.0, 16.0, t0);
        slide.advance(0, true, false, false, "slide", 100.0, 8.0, 16.0, mid);
        assert!(slide.row_fx()[0].dx < 0);
        assert_eq!(slide.row_fx()[0].reveal, 1.0);
    }

    #[test]
    fn every_style_has_its_own_motion() {
        let t0 = Instant::now();
        let mid = t0 + std::time::Duration::from_millis(20);
        let sample = |style: &str| {
            let mut fx = TextFx::new();
            fx.advance(0, true, false, false, style, 100.0, 8.0, 16.0, t0);
            fx.advance(0, true, false, false, style, 100.0, 8.0, 16.0, mid);
            fx.row_fx()[0]
        };

        // Vertical styles move in opposite directions.
        let rise = sample("rise");
        assert!(rise.dy > 0);
        assert_eq!(rise.dx, 0);
        let drop = sample("drop");
        assert!(drop.dy < 0);

        // `type` reveals in discrete character-sized steps.
        let typey = sample("type");
        assert_eq!(typey.alpha, 1.0);
        assert!(typey.reveal < 1.0);
        let step = 1.0 / 24.0;
        assert!((typey.reveal / step).fract().abs() < 1e-4);

        // `dissolve` uses the per-cell noise path, `glow` the flash path.
        let dissolve = sample("dissolve");
        assert!(dissolve.dissolve > 0.0);
        assert_eq!(dissolve.alpha, 1.0);
        let glow = sample("glow");
        assert!(glow.flash > 0.0);
        assert!(glow.alpha < 1.0);

        // `fade` keeps everything neutral apart from opacity.
        let fade = sample("fade");
        assert!(fade.alpha < 1.0);
        assert_eq!(fade.dx, 0);
        assert_eq!(fade.dy, 0);
        assert_eq!(fade.reveal, 1.0);
        assert_eq!(fade.dissolve, 0.0);
        assert_eq!(fade.flash, 0.0);
    }

    #[test]
    fn the_cursor_row_is_skipped_when_prompt_animation_is_off() {
        let mut fx = TextFx::new();
        let t0 = Instant::now();
        fx.advance(5, true, true, true, "fade", 100.0, 8.0, 16.0, t0);
        assert_eq!(fx.start[5], None);
        assert_eq!(fx.row_fx()[5], RowFx::FULL);
        // The same row animates when the option is on.
        let mut fx = TextFx::new();
        fx.advance(5, true, true, false, "fade", 100.0, 8.0, 16.0, t0);
        assert_eq!(fx.start[5], Some(t0));
    }

    #[test]
    fn active_only_while_a_stamp_is_recent() {
        let mut fx = TextFx::new();
        let t0 = Instant::now();
        fx.start = vec![Some(t0); 2];
        assert!(fx.active(t0, 140));
        let done = t0 + std::time::Duration::from_millis(140);
        assert!(!fx.active(done, 140));
        assert!(!fx.active(t0, 0));
    }

    #[test]
    fn reset_settles_every_row() {
        let mut fx = TextFx::new();
        fx.start = vec![Some(Instant::now()); 2];
        fx.fx = vec![RowFx {
            alpha: 0.2,
            dx: 3,
            reveal: 0.4,
            ..RowFx::FULL
        }; 2];
        fx.reset();
        assert!(fx.start.iter().all(Option::is_none));
        assert!(fx.settled.iter().all(|s| *s));
        assert!(fx.fx.iter().all(|f| *f == RowFx::FULL));
    }

    #[test]
    fn config_sanitizes_animation_values() {
        let mut c = cfg(9_999, true);
        c.anim.style = "orbit".into();
        c.sanitize();
        assert_eq!(c.anim.ms, 2000);
        assert_eq!(c.anim.style, "fade");
        c.anim.style = "WIPE".into();
        c.sanitize();
        assert_eq!(c.anim.style, "wipe");
    }

    #[test]
    fn cursor_row_can_be_kept_out_of_the_animation() {
        let mut c = cfg(140, false);
        c.sanitize();
        assert!(!c.anim.prompt);
    }
}
