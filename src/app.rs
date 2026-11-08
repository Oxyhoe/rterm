//! Window, event loop and the glue tying every other module together.
//!
//! The window holds a [`pane::PaneTree`]: every leaf is a pane, every pane a
//! stack of tabs, and every tab a [`session::Session`] (emulator + PTY + media).
//! Input is routed by pointer position, keys go to the focused pane.

use std::num::NonZeroU32;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use alacritty_terminal::ansi::{Color, CursorShape, NamedColor};
use alacritty_terminal::config::{Config as TermConfig, ConfigCursorStyle};
use alacritty_terminal::event::{Event as TermEvent, EventListener};
use alacritty_terminal::grid::Scroll;
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::{SizeInfo, TermMode};
use anyhow::{Context, Result};

use winit::application::ApplicationHandler;
use winit::dpi::PhysicalSize;
use winit::event::{ElementState, Ime, KeyEvent, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{
    ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy as WinitProxy, OwnedDisplayHandle,
};
use winit::keyboard::{Key, ModifiersState};
use winit::window::{Window, WindowAttributes, WindowId};

use crate::clipboard::{self, Target};
use crate::config::Config;
use crate::font::{CellMetrics, FontSystem};
use crate::input;
use crate::media::MediaItem;
use crate::palette::{self, Palette};
use crate::pane::{Dir, PaneTree, Rect};
use crate::protocol::Pipeline;
use crate::render::{Layout, Renderer};
use crate::session::Session;
use crate::settings::{self, Field, SettingsView};
use crate::theme;

/// How often the config file is stat()ed for changes.
const CONFIG_POLL: Duration = Duration::from_millis(500);
/// How often `theme.mode = "auto"` re-reads the desktop scheme and accent.
const THEME_POLL: Duration = Duration::from_secs(5);
/// Frame interval for animated backgrounds (≈30 fps keeps plasma affordable).
const ANIM_FRAME: Duration = Duration::from_millis(33);
/// Grain twinkle cadence: random noise doesn't need more than ~10 fps, and
/// forcing full redraws at display refresh pinned a CPU core at 100%.
const GRAIN_FRAME: Duration = Duration::from_millis(100);
/// Window within which two clicks count as a double click.
const DOUBLE_CLICK: Duration = Duration::from_millis(400);
/// Padding under the tab labels inside the strip.
const TAB_PAD: i32 = 2;

/// Whether the strip is drawn for a pane with `ntabs` tabs (`ui.tab_bar`).
fn tabbar_shows(mode: &str, ntabs: usize) -> bool {
    match mode {
        "on" => true,
        "off" => false,
        _ => ntabs > 1,
    }
}

/// The next `ui.tab_bar` mode in the Ctrl+Shift+B cycle.
fn next_tabbar_mode(cur: &str) -> &'static str {
    match cur {
        "auto" => "on",
        "on" => "off",
        _ => "auto",
    }
}

/// One drawn tab: its pixel span inside the strip, session and label.
struct TabChip {
    x: i32,
    w: i32,
    /// Where the ✕ button sits inside the chip (0 width when hidden).
    close_x: i32,
    close_w: i32,
    sid: usize,
    label: String,
}

impl TabChip {
    fn has_close(&self) -> bool {
        self.close_w > 0
    }
}

/// Lay tabs out left-to-right from `x0`; the style decides whether the label
/// carries its index and `show_close` appends a ✕ to every chip. Shared by the
/// draw path and the click hit-test so the two can never disagree.
fn tab_chips(
    fonts: &mut FontSystem,
    tabs: &[usize],
    x0: i32,
    style: &str,
    show_close: bool,
    mut title_of: impl FnMut(usize) -> String,
) -> Vec<TabChip> {
    let mut out = Vec::with_capacity(tabs.len());
    let mut x = x0;
    for (i, sid) in tabs.iter().enumerate() {
        let title: String = title_of(*sid).chars().take(18).collect();
        let label = if style == "minimal" {
            format!("{title} ")
        } else {
            format!("{} {title} ", i + 1)
        };
        let label_w = fonts.text_width(&label).ceil() as i32;
        let close_w = if show_close { 14 } else { 0 };
        out.push(TabChip {
            x,
            w: label_w + close_w,
            close_x: x + label_w,
            close_w,
            sid: *sid,
            label,
        });
        x += label_w + close_w + 8;
    }
    out
}

/// `RTERM_TRACE=1` mirrors PTY traffic to stderr while debugging.
pub(crate) fn tracing() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| match std::env::var("RTERM_TRACE") {
        Ok(value) => !value.is_empty() && value != "0",
        Err(_) => false,
    })
}

/// Events delivered from worker threads back to the event loop.
#[derive(Debug)]
pub enum UserEvent {
    /// New PTY output is waiting to be consumed.
    Wake,
    /// A decoded media payload is waiting.
    MediaReady,
    /// The terminal emulator wants the UI to do something.
    Term { sid: usize, event: TermEvent },
    /// A `rterm play` client on the local socket.
    Socket(crate::socket::Msg),
}

pub type EventLoopProxy = WinitProxy<UserEvent>;

/// Forwards emulator events (title, clipboard, OSC replies) to the event loop.
#[derive(Clone)]
pub struct TermProxy(pub EventLoopProxy, pub usize);

impl EventListener for TermProxy {
    fn send_event(&self, event: TermEvent) {
        let _ = self.0.send_event(UserEvent::Term { sid: self.1, event });
    }
}

/// Where a pane sits and how its grid maps to window pixels.
#[derive(Debug, Clone)]
pub struct PaneLayout {
    pub sid: usize,
    pub rect: Rect,
    pub layout: Layout,
    pub tabs: Vec<usize>,
    /// Pixels reserved at the top for the tab strip (0 = no strip).
    pub tabbar: i32,
}

/// One OS window: its surface, pane tree, pointer and focus state.
///
/// Sessions live in the [`App`] but every session belongs to exactly one
/// window's [`PaneTree`], so the two views never fight over input.
struct WindowState {
    surface: softbuffer::Surface<OwnedDisplayHandle, Window>,
    size: (u32, u32),
    tree: PaneTree,
    panes: Vec<PaneLayout>,
    cursor_px: Option<(f64, f64)>,
    held: Option<(MouseButton, bool)>,
    clicks: u8,
    last_click: Instant,
    focused: bool,
}

impl WindowState {
    fn window(&self) -> &Window {
        self.surface.window()
    }
}

/// Commands the key bindings translate into.
#[derive(Debug, Clone, Copy)]
enum Action {
    NewWindow,
    NewTab,
    CloseTab,
    Split(Dir),
    ClosePane,
    SelectTab(usize),
    CycleTab(bool),
    /// Cycle the tab strip through auto → on → off (`Ctrl+Shift+B`).
    CycleTabBar,
    FocusPane(Dir),
    Copy,
    Paste,
    Font(f32),
    Settings,
    /// Scroll the scrollback by one page (`true` = up, into history).
    Page(bool),
    /// Clear screen, scrollback and media overlays.
    Clear,
}

/// What the window shows on startup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// A shell in a single pane.
    Terminal,
    /// The terminal with the settings panel already open (`--settings`).
    Settings,
    /// Only the settings editor (`rterm config --edit`).
    Editor,
}

/// Run the terminal until every pane and window is gone.
pub fn run(cfg: Config, cfg_path: Option<PathBuf>, mode: Mode) -> Result<()> {
    let mut builder = EventLoop::<UserEvent>::with_user_event();
    let event_loop = builder.build().context("event loop")?;
    let proxy = event_loop.create_proxy();
    let display = event_loop.owned_display_handle();
    event_loop.set_control_flow(ControlFlow::WaitUntil(Instant::now() + CONFIG_POLL));

    let mut app = App::new(cfg, cfg_path, display, proxy, mode)?;
    event_loop.run_app(&mut app).context("event loop")?;
    Ok(())
}

pub struct App {
    proxy: EventLoopProxy,
    cfg: Config,
    cfg_path: Option<PathBuf>,
    cfg_mtime: Option<std::time::SystemTime>,

    fonts: FontSystem,
    palette: Palette,
    pipeline: Pipeline,
    socket: Option<crate::socket::Server>,
    renderer: Renderer,

    sessions: Vec<Option<Session>>,
    /// `[colors]` as the file wrote it — the theme only ever overrides a copy.
    base_colors: crate::config::ColorsCfg,

    context: softbuffer::Context<OwnedDisplayHandle>,
    windows: Vec<WindowState>,
    /// Window that owns the keyboard and pointer focus.
    cur: usize,
    /// Open another OS window on the next pass through the event loop.
    pending_window: bool,

    scale: f64,
    mods: ModifiersState,
    blink_on: bool,
    blink_next: Instant,
    started: Instant,
    next_cfg_poll: Instant,
    next_theme_poll: Instant,
    /// Next time an idle animation (fade / grain / animated bg) forces a frame.
    next_anim: Instant,
    dirty: bool,

    /// Open settings (`F1`), or the full editor.
    settings: Option<SettingsView>,
    mode: Mode,
    quit: bool,
}

impl App {
    pub fn new(
        cfg: Config,
        cfg_path: Option<PathBuf>,
        display: OwnedDisplayHandle,
        proxy: EventLoopProxy,
        mode: Mode,
    ) -> Result<Self> {
        let context = softbuffer::Context::new(display)
            .map_err(|err| anyhow::anyhow!("softbuffer context: {err}"))?;
        let base_colors = cfg.colors.clone();
        let mut cfg = cfg;
        cfg.colors = theme::effective(base_colors.clone());
        crate::prompt::sync(&cfg.prompt);
        let fonts = FontSystem::new(&cfg.font, 1.0)?;
        let palette = Palette::from_cfg(&cfg.colors);
        let pipeline = Pipeline::spawn(proxy.clone());
        // `rterm play` streams video over this socket (unless disabled).
        let socket = if cfg.media.allow_socket {
            match crate::socket::Server::spawn(proxy.clone()) {
                Ok(server) => Some(server),
                Err(err) => {
                    eprintln!("rterm: media socket disabled: {err:#}");
                    None
                }
            }
        } else {
            None
        };
        let cfg_mtime = cfg_path.as_deref().and_then(Config::mtime);
        let now = Instant::now();

        let mut app = Self {
            proxy,
            cfg,
            cfg_path,
            cfg_mtime,
            fonts,
            palette,
            pipeline,
            socket,
            renderer: Renderer::new(),
            sessions: Vec::new(),
            base_colors,
            context,
            windows: Vec::new(),
            cur: 0,
            pending_window: false,
            scale: 1.0,
            mods: ModifiersState::empty(),
            blink_on: true,
            blink_next: now + Duration::from_secs(1),
            started: now,
            next_cfg_poll: now + CONFIG_POLL,
            next_theme_poll: now + THEME_POLL,
            next_anim: now,
            dirty: true,
            settings: None,
            mode,
            quit: false,
        };

        if mode == Mode::Editor {
            app.settings = Some(SettingsView::new(true));
        }
        Ok(app)
    }

    // --- windows ------------------------------------------------------------

    /// Create an OS window and track its surface, tree and pointer state.
    fn add_window(&mut self, el: &ActiveEventLoop, attrs: WindowAttributes) -> Option<usize> {
        let window = match el.create_window(attrs) {
            Ok(window) => window,
            Err(err) => {
                eprintln!("rterm: cannot create window: {err}");
                return None;
            }
        };
        window.set_ime_allowed(true);
        if self.windows.is_empty() {
            self.scale = window.scale_factor();
            self.fonts
                .recompute_metrics(&self.cfg.font, self.scale as f32);
        }
        let size = window.inner_size();
        let surface = match softbuffer::Surface::new(&self.context, window) {
            Ok(surface) => surface,
            Err(err) => {
                eprintln!("rterm: cannot create surface: {err}");
                return None;
            }
        };
        self.windows.push(WindowState {
            surface,
            size: (size.width, size.height),
            tree: PaneTree::new(usize::MAX),
            panes: Vec::new(),
            cursor_px: None,
            held: None,
            clicks: 0,
            last_click: Instant::now() - DOUBLE_CLICK,
            focused: true,
        });
        Some(self.windows.len() - 1)
    }

    /// Boot the sessions a fresh window starts with.
    fn seed_window(&mut self, wi: usize) {
        if self.mode == Mode::Editor {
            return;
        }
        let Ok(sid) = self.open_session(wi) else {
            self.close_window(wi);
            return;
        };
        self.windows[wi].tree = PaneTree::new(sid);
        if self.mode == Mode::Settings && self.settings.is_none() {
            self.settings = Some(SettingsView::new(false));
        }
        let (w, h) = self.windows[wi].size;
        self.relayout_to(wi, w, h);
        self.sync_title();
    }

    /// `Ctrl+Shift+N`: a brand new window with its own shell.
    fn open_new_window(&mut self, el: &ActiveEventLoop) {
        let attrs = window_attrs(&self.cfg.window.title, &self.cfg);
        let Some(wi) = self.add_window(el, attrs) else {
            return;
        };
        self.seed_window(wi);
        self.cur = wi;
        self.dirty = true;
    }

    /// Drop a window; every session it owns goes with it.
    fn close_window(&mut self, wi: usize) {
        if wi >= self.windows.len() {
            return;
        }
        let win = self.windows.remove(wi);
        for sid in win.tree.sessions() {
            if let Some(session) = self.sessions.get_mut(sid).and_then(Option::as_mut) {
                session.shutdown();
            }
            if let Some(slot) = self.sessions.get_mut(sid) {
                *slot = None;
            }
        }
        drop(win);
        if self.cur >= self.windows.len() {
            self.cur = self.windows.len().saturating_sub(1);
        }
        self.sync_title();
        self.dirty = true;
    }

    /// Which window's tree holds `sid`?
    fn window_of(&self, sid: usize) -> Option<usize> {
        self.windows.iter().position(|w| w.tree.contains(sid))
    }

    // --- sessions ----------------------------------------------------------

    /// Allocate and boot a new terminal inside window `wi`.
    fn open_session(&mut self, wi: usize) -> Result<usize> {
        let sid = self.sessions.len();
        let (w, h, layout) = self
            .windows
            .get(wi)
            .and_then(|w| w.panes.iter().find(|p| p.sid == w.tree.active()))
            .map(|p| (p.rect.w, p.rect.h, p.layout))
            .unwrap_or((
                self.cfg.window.width,
                self.cfg.window.height,
                self.base_layout(),
            ));

        let term_cfg = term_config(&self.cfg);
        let size = term_size(w, h, &layout);
        let mut session = Session::new(
            sid,
            &term_cfg,
            size,
            TermProxy(self.proxy.clone(), sid),
            &self.cfg.media,
        );
        let cwd = std::env::current_dir().ok();
        let mut shell = self.cfg.shell.clone();
        if self.cfg.prompt.enable {
            if let Some(args) = crate::prompt::rcfile_args(shell.program.as_deref(), &shell.args) {
                shell.args = args;
            }
        }
        session.spawn_shell(
            &shell,
            layout.cols.max(1) as u16,
            layout.rows.max(1) as u16,
            cwd,
            self.proxy.clone(),
        )?;
        self.sessions.push(Some(session));
        Ok(sid)
    }

    fn base_layout(&self) -> Layout {
        compute_layout(
            self.cfg.window.width,
            self.cfg.window.height,
            &self.fonts.metrics,
            &self.cfg,
        )
    }

    fn session(&self, sid: usize) -> Option<&Session> {
        self.sessions.get(sid).and_then(Option::as_ref)
    }

    fn session_mut(&mut self, sid: usize) -> Option<&mut Session> {
        self.sessions.get_mut(sid).and_then(Option::as_mut)
    }

    fn active_sid(&self, wi: usize) -> usize {
        self.windows
            .get(wi)
            .map(|w| w.tree.active())
            .unwrap_or(usize::MAX)
    }

    /// The session that owns the keyboard focus right now.
    fn focused_sid(&self) -> usize {
        self.active_sid(self.cur)
    }

    fn active_session_mut(&mut self, wi: usize) -> Option<&mut Session> {
        let sid = self.active_sid(wi);
        self.session_mut(sid)
    }

    /// The pane holding `sid`, whichever window it lives in.
    fn pane(&self, sid: usize) -> Option<&PaneLayout> {
        self.windows
            .iter()
            .find_map(|w| w.panes.iter().find(|p| p.sid == sid))
    }

    fn pane_at(&self, wi: usize, x: i32, y: i32) -> Option<&PaneLayout> {
        self.windows
            .get(wi)?
            .panes
            .iter()
            .find(|p| p.rect.contains(x, y))
    }

    /// Resize window `wi` after a window/font/tree change.
    fn relayout(&mut self, wi: usize) {
        if let Some((w, h)) = self.windows.get(wi).map(|w| w.size) {
            if w > 0 && h > 0 {
                self.relayout_to(wi, w, h);
            }
        }
    }

    /// Font and padding changes touch every window at once.
    fn relayout_all(&mut self) {
        for wi in 0..self.windows.len() {
            self.relayout(wi);
        }
        // New cell metrics make old row stamps meaningless; let the next
        // frame settle everything instead of replaying at the new size.
        for session in self.sessions.iter_mut().flatten() {
            session.fx.reset();
        }
    }

    fn relayout_to(&mut self, wi: usize, w: u32, h: u32) {
        let metrics = self.fonts.metrics.clone();
        let pad_x = self.cfg.ui.padding_x;
        let pad_y = self.cfg.ui.padding_y;
        let show_tabs = self.cfg.ui.tab_bar.clone();
        let strip_bottom = self.cfg.ui.tab_bar_position == "bottom";
        let Some(win) = self.windows.get(wi) else {
            return;
        };
        let mut panes = Vec::new();
        for pane in win.tree.rects(Rect { x: 0, y: 0, w, h }) {
            let tabs = pane.leaf.tabs.clone();
            let active_tab = pane.leaf.active;
            let tabbar = if tabbar_shows(&show_tabs, tabs.len()) {
                metrics.cell_h.round() as i32 + TAB_PAD
            } else {
                0
            };
            // The strip eats a strip of pixels at the chosen edge of the pane.
            let content = Rect {
                x: pane.rect.x,
                y: if strip_bottom {
                    pane.rect.y
                } else {
                    pane.rect.y + tabbar
                },
                w: pane.rect.w,
                h: pane.rect.h.saturating_sub(tabbar as u32),
            };
            let mut layout =
                Layout::compute(content.w.max(1), content.h.max(1), &metrics, pad_x, pad_y);
            layout.cols = layout.cols.max(2);
            layout.rows = layout.rows.max(1);
            layout.origin_x = content.x
                + ((content.w as f32 - layout.cols as f32 * layout.cell_w) / 2.0).round() as i32;
            layout.origin_y = content.y
                + ((content.h as f32 - layout.rows as f32 * layout.cell_h) / 2.0).round() as i32;
            let sid = pane
                .leaf
                .tabs
                .get(active_tab)
                .copied()
                .unwrap_or(usize::MAX);
            panes.push(PaneLayout {
                sid,
                rect: pane.rect,
                layout,
                tabs,
                tabbar,
            });
        }
        self.windows[wi].panes = panes;

        for pane in &self.windows[wi].panes {
            let size = term_size(pane.rect.w, pane.rect.h.max(1), &pane.layout);
            if let Some(slot) = self.sessions.get_mut(pane.sid) {
                if let Some(session) = slot.as_mut() {
                    session.resize(pane.layout.cols as u16, pane.layout.rows as u16, size);
                }
            }
        }
    }

    /// Tear a session down; reports `(window, empty)` so the caller can drop
    /// a window that just lost its last session.
    fn close_session(&mut self, sid: usize) -> Option<(usize, bool)> {
        let wi = self.window_of(sid)?;
        if let Some(session) = self.session_mut(sid) {
            session.shutdown();
        }
        if let Some(slot) = self.sessions.get_mut(sid) {
            *slot = None;
        }
        let empty = self.windows[wi].tree.close(sid);
        if !empty {
            self.relayout(wi);
            self.sync_title();
        }
        self.dirty = true;
        Some((wi, empty))
    }

    fn shutdown_all(&mut self) {
        for session in self.sessions.iter_mut().flatten() {
            session.shutdown();
        }
    }

    /// Push every window's title bar to match its own focused tab.
    fn sync_title(&mut self) {
        let fallback = self.cfg.window.title.clone();
        for wi in 0..self.windows.len() {
            let title = if self.mode == Mode::Editor {
                "rterm settings".to_string()
            } else {
                let sid = self.windows[wi].tree.active();
                self.session(sid)
                    .map(|s| {
                        if s.title.is_empty() {
                            fallback.clone()
                        } else {
                            s.title.clone()
                        }
                    })
                    .unwrap_or_else(|| fallback.clone())
            };
            self.windows[wi].window().set_title(&title);
        }
    }

    // --- window -------------------------------------------------------------

    fn resize(&mut self, wi: usize, w: u32, h: u32) {
        if w == 0 || h == 0 {
            return;
        }
        if let Some(win) = self.windows.get_mut(wi) {
            win.size = (w, h);
        }
        self.relayout_to(wi, w, h);
        self.dirty = true;
    }

    fn adjust_font(&mut self, delta: f32) {
        let size = (self.cfg.font.size + delta).clamp(6.0, 72.0);
        if (size - self.cfg.font.size).abs() < f32::EPSILON {
            return;
        }
        self.cfg.font.size = size;
        self.fonts
            .recompute_metrics(&self.cfg.font, self.scale as f32);
        self.relayout_all();
        self.dirty = true;
    }

    // --- settings -----------------------------------------------------------

    /// Keyboard while the settings panel owns the window.
    fn settings_key(&mut self, event: &KeyEvent) {
        use winit::keyboard::NamedKey as K;

        // A string row being typed into owns every key until Enter or Esc.
        if self.settings.as_ref().is_some_and(|v| v.editing.is_some()) {
            self.settings_edit_key(event);
            return;
        }

        // …and so does the colour picker.
        if self.settings.as_ref().is_some_and(|v| v.picking.is_some()) {
            self.settings_pick_key(event);
            return;
        }

        enum Cmd {
            Move(i32),
            Select(usize),
            Adjust(i32),
            Edit,
            Pick,
            Reset,
            Save,
            SaveAndQuit,
            Revert,
            Close,
        }

        let mods = self.mods;
        let cmd = {
            let Some(view) = self.settings.as_mut() else {
                return;
            };
            let field = view.field();
            let coarse = if mods.shift_key() { 5 } else { 1 };
            match &event.logical_key {
                Key::Named(K::Escape) | Key::Named(K::F1) => Cmd::Close,
                // Shift pages the selection like a long list; plain arrows step one.
                Key::Named(K::ArrowDown) if mods.shift_key() => Cmd::Move(5),
                Key::Named(K::ArrowUp) if mods.shift_key() => Cmd::Move(-5),
                Key::Named(K::ArrowDown) => Cmd::Move(1),
                Key::Named(K::ArrowUp) => Cmd::Move(-1),
                // PgUp/PgDn step by five; typed rows ignore them (use Space/→).
                Key::Named(K::PageDown) => Cmd::Adjust(5),
                Key::Named(K::PageUp) => Cmd::Adjust(-5),
                Key::Named(K::Home) if mods.control_key() => Cmd::Select(0),
                Key::Named(K::Home) => Cmd::Reset,
                Key::Named(K::End) => Cmd::Select(settings::Field::ALL.len() - 1),
                Key::Named(K::ArrowRight) | Key::Named(K::Space) if field.is_editable() => {
                    Cmd::Edit
                }
                Key::Named(K::ArrowLeft) if field.is_editable() => return,
                Key::Named(K::ArrowRight) | Key::Named(K::Space) => Cmd::Adjust(coarse),
                Key::Named(K::ArrowLeft) => Cmd::Adjust(-coarse),
                // Colour rows get the slider picker; everything else saves.
                Key::Named(K::Enter) if field.is_pickable() => Cmd::Pick,
                Key::Named(K::Enter) => {
                    if view.full {
                        Cmd::SaveAndQuit
                    } else {
                        Cmd::Save
                    }
                }
                Key::Character(ch) => match ch.to_string().as_str() {
                    "s" | "S" => Cmd::Save,
                    "r" | "R" => Cmd::Revert,
                    "d" | "D" => Cmd::Reset,
                    _ => return,
                },
                _ => return,
            }
        };

        match cmd {
            Cmd::Move(delta) => {
                if let Some(view) = self.settings.as_mut() {
                    view.move_selection(delta);
                }
            }
            Cmd::Select(index) => {
                if let Some(view) = self.settings.as_mut() {
                    view.select(index);
                }
            }
            Cmd::Adjust(dir) => self.settings_adjust(dir),
            Cmd::Edit => self.settings_begin_edit(),
            Cmd::Pick => self.settings_begin_pick(),
            Cmd::Reset => self.settings_reset(),
            Cmd::Save => self.save_config(),
            Cmd::SaveAndQuit => {
                self.save_config();
                self.quit = true;
            }
            Cmd::Revert => self.revert_config(),
            Cmd::Close => self.close_settings(),
        }
        self.dirty = true;
    }

    /// Open the text editor on the row under `index` (clicked with the mouse).
    fn settings_begin_edit_index(&mut self, index: usize) {
        let Some(view) = self.settings.as_mut() else {
            return;
        };
        view.selected = index;
        view.open_group = Field::ALL[index].group();
        view.editing = None;
        view.picking = None;
        self.settings_begin_edit();
    }

    /// Open the HSL slider picker on a colour row; the value previews live
    /// on every step, `Esc` restores the original.
    fn settings_begin_pick(&mut self) {
        let Some(view) = self.settings.as_mut() else {
            return;
        };
        let field = view.field();
        if !field.is_pickable() {
            return;
        }
        let current = field
            .color_hex(&self.cfg)
            .map(str::to_string)
            .unwrap_or_else(|| self.cfg.colors.foreground.clone());
        let fallback = self.cfg.colors.foreground.clone();
        view.editing = None;
        view.status = None;
        view.picking = Some(settings::Pick::open(field, &current, &fallback));
        self.dirty = true;
    }

    /// Keys while the colour picker owns the panel.
    fn settings_pick_key(&mut self, event: &KeyEvent) {
        use winit::keyboard::NamedKey as K;
        if event.state != ElementState::Pressed {
            return;
        }
        enum Cmd {
            Rotate(i32),
            Sat(i32),
            Light(i32),
            Done,
            Cancel,
        }
        let cmd = match &event.logical_key {
            Key::Named(K::ArrowRight) => Cmd::Rotate(1),
            Key::Named(K::ArrowLeft) => Cmd::Rotate(-1),
            Key::Named(K::ArrowDown) => Cmd::Sat(1),
            Key::Named(K::ArrowUp) => Cmd::Sat(-1),
            Key::Named(K::PageDown) => Cmd::Light(5),
            Key::Named(K::PageUp) => Cmd::Light(-5),
            Key::Named(K::Enter) => Cmd::Done,
            Key::Named(K::Escape) | Key::Named(K::F1) => Cmd::Cancel,
            _ => return,
        };

        let step = if self.mods.shift_key() { 10.0 } else { 1.0 };
        let mut changed = false;
        let mut apply_hex = None;
        let mut finish: Option<(String, Option<(settings::Field, String)>)> = None;
        if let Some(view) = self.settings.as_mut() {
            if let Some(pick) = view.picking.as_mut() {
                match cmd {
                    Cmd::Rotate(dir) => {
                        pick.rotate_hue(dir, step);
                        changed = true;
                        apply_hex = Some((pick.field, pick.hex()));
                    }
                    Cmd::Sat(dir) => {
                        pick.adjust_sat(dir, step);
                        changed = true;
                        apply_hex = Some((pick.field, pick.hex()));
                    }
                    Cmd::Light(dir) => {
                        pick.adjust_light(dir, step);
                        changed = true;
                        apply_hex = Some((pick.field, pick.hex()));
                    }
                    Cmd::Done => finish = Some(("colour set".into(), None)),
                    Cmd::Cancel => {
                        finish = Some((
                            "colour reverted".into(),
                            Some((pick.field, pick.orig.clone())),
                        ))
                    }
                }
            }
        }
        if let Some((field, hex)) = apply_hex {
            let _ = self.try_apply(field, &hex);
        }
        if let Some((status, restore)) = finish {
            if self
                .settings
                .as_mut()
                .and_then(|v| v.picking.take())
                .is_some()
            {
                if let Some((field, orig)) = restore {
                    let _ = self.try_apply(field, &orig);
                }
                self.settings_status(status);
            }
        }
        self.dirty = true;
    }

    /// Keystrokes for the in-place editor of a string row.
    fn settings_edit_key(&mut self, event: &KeyEvent) {
        use winit::keyboard::NamedKey as K;

        #[derive(Clone, Copy)]
        enum Op {
            Cancel,
            Commit,
            Insert(char),
            Backspace,
            Delete,
            Left,
            Right,
            Home,
            End,
        }

        let op = match &event.logical_key {
            Key::Named(K::Escape) | Key::Named(K::F1) => Op::Cancel,
            Key::Named(K::Enter) => Op::Commit,
            Key::Named(K::Backspace) => Op::Backspace,
            Key::Named(K::Delete) => Op::Delete,
            Key::Named(K::ArrowLeft) => Op::Left,
            Key::Named(K::ArrowRight) => Op::Right,
            Key::Named(K::Home) => Op::Home,
            Key::Named(K::End) => Op::End,
            Key::Named(K::Space) => Op::Insert(' '),
            Key::Character(ch) => {
                let text = ch.to_string();
                let mut chars = text.chars();
                match (chars.next(), chars.next()) {
                    (Some(c), None) => Op::Insert(c),
                    _ => return,
                }
            }
            _ => return,
        };

        match op {
            Op::Cancel => {
                if let Some(view) = self.settings.as_mut() {
                    view.editing = None;
                    view.status = None;
                }
            }
            Op::Commit => {
                let taken = self.settings.as_mut().and_then(|v| v.editing.take());
                if let Some(edit) = taken {
                    self.settings_commit(edit.field, &edit.buf);
                }
            }
            _ => {
                if let Some(view) = self.settings.as_mut() {
                    if let Some(edit) = view.editing.as_mut() {
                        match op {
                            Op::Insert(c) => edit.insert(c),
                            Op::Backspace => edit.backspace(),
                            Op::Delete => edit.delete(),
                            Op::Left => edit.move_caret(-1),
                            Op::Right => edit.move_caret(1),
                            Op::Home => edit.pos = 0,
                            Op::End => edit.pos = edit.buf.len(),
                            Op::Cancel | Op::Commit => {}
                        }
                    }
                }
            }
        }
        // Live preview: colours and numbers apply on every keystroke, so the
        // terminal recolours while the hex (or value) is still being typed.
        if let Some((field, buf)) = self
            .settings
            .as_ref()
            .and_then(|v| v.editing.as_ref())
            .filter(|e| e.field.is_live())
            .map(|e| (e.field, e.buf.clone()))
        {
            let _ = self.try_apply(field, &buf);
        }
        self.dirty = true;
    }

    /// Open the text editor on the selected row (colours, paths, families…).
    fn settings_begin_edit(&mut self) {
        let Some(view) = self.settings.as_mut() else {
            return;
        };
        let field = view.field();
        // "save as" starts with the profile the picker is on, if any.
        let prefill = if field == settings::Field::ProfileSave {
            view.profile_name().unwrap_or_default().to_string()
        } else {
            match field.edit_prefill(&self.cfg) {
                Some(text) => text,
                None => return,
            }
        };
        view.editing = Some(settings::EditState::new(field, prefill));
        view.status = None;
    }

    /// Apply the typed text to the live config (colour rows preview instantly).
    /// Apply `text` to a cloned config and install it live.
    ///
    /// `Err` explains why the value did not stick (invalid input, or the font
    /// family failed to load — `apply_config` keeps the old font in that case).
    /// Callers that preview while typing simply ignore the error.
    fn try_apply(&mut self, field: settings::Field, text: &str) -> Result<(), String> {
        let mut next = self.cfg.clone();
        field.commit_edit(&mut next, text)?;
        if field.affects_colors() {
            self.base_colors = next.colors.clone();
            if next.theme.mode == "auto" {
                next.theme.mode = "manual".into();
            }
        }
        let want_font = next.font.clone();
        self.apply_config(next);
        if self.cfg.font != want_font {
            return Err(format!("font not loaded: {}", want_font.family));
        }
        Ok(())
    }

    fn settings_commit(&mut self, field: settings::Field, text: &str) {
        if field == settings::Field::ProfileSave {
            self.save_profile(text);
            return;
        }
        if let Err(msg) = self.try_apply(field, text) {
            // Keep the editor open so the value can be corrected.
            if let Some(view) = self.settings.as_mut() {
                view.editing = Some(settings::EditState::new(field, text.to_string()));
                view.set_status(msg);
            }
            return;
        }
        self.settings_status(format!("{} set", field.label()));
    }

    /// Restore the selected row to its default value.
    fn settings_reset(&mut self) {
        let Some(view) = self.settings.as_mut() else {
            return;
        };
        let field = view.field();
        let mut next = self.cfg.clone();
        field.reset(&mut next);
        if field.affects_colors() {
            self.base_colors = next.colors.clone();
            if next.theme.mode == "auto" {
                next.theme.mode = "manual".into();
            }
        }
        self.apply_config(next);
        self.settings_status(format!("{} → default", field.label()));
    }

    /// Step the selected field and re-apply the config for a live preview.
    fn settings_adjust(&mut self, dir: i32) {
        let Some(view) = self.settings.as_mut() else {
            return;
        };
        if view.editing.is_some() || view.selected >= settings::Field::ALL.len() {
            return;
        }
        let field = view.field();
        // The profile rows manage files, not config values: cycle + load,
        // and delete on a rightward step only (a left step hints instead).
        if field == settings::Field::ProfileSelect {
            view.refresh_profiles();
            if view.profiles.is_empty() {
                view.set_status("no profiles yet - type a name in \"save as\"");
                return;
            }
            let n = view.profiles.len() as i32;
            view.profile_idx = ((view.profile_idx as i32 + dir).rem_euclid(n)) as usize;
            let name = view.profiles[view.profile_idx].clone();
            self.load_profile(&name);
            return;
        }
        if field == settings::Field::ProfileDelete {
            let Some(name) = view.profile_name().map(str::to_string) else {
                view.set_status("no profile to delete");
                return;
            };
            if dir < 0 {
                view.set_status(format!("{name}: -> deletes"));
                return;
            }
            match crate::profiles::delete(&name) {
                Ok(()) => {
                    view.refresh_profiles();
                    view.set_status(format!("profile deleted: {name}"));
                }
                Err(err) => view.set_status(format!("delete failed: {err}")),
            }
            return;
        }
        let families = view.families();
        let mut next = self.cfg.clone();
        field.adjust(&mut next, families, dir);
        if field.affects_colors() {
            self.base_colors = next.colors.clone();
            if next.theme.mode == "auto" {
                next.theme.mode = "manual".into();
            }
        }
        self.apply_config(next);
    }

    /// Write `config.toml` and remember it as the config we are editing.
    fn save_config(&mut self) {
        let path = self.cfg_path.clone().unwrap_or_else(Config::config_path);
        let mut to_save = self.cfg.clone();
        to_save.colors = self.base_colors.clone();
        match Config::save(&path, &to_save) {
            Ok(()) => {
                if self.cfg_path.is_none() {
                    self.cfg_path = Some(path.clone());
                }
                self.cfg_mtime = Config::mtime(&path);
                self.settings_status(format!("saved {}", path.display()));
            }
            Err(err) => self.settings_status(format!("save failed: {err}")),
        }
    }

    /// Throw the preview away and reload from disk.
    fn revert_config(&mut self) {
        let Some(path) = self.cfg_path.clone() else {
            self.settings_status("no config file yet".to_string());
            return;
        };
        match Config::load_from(&path) {
            Ok(Some(next)) => {
                self.base_colors = next.colors.clone();
                self.apply_config(next);
                self.settings_status(format!("reverted {}", path.display()));
            }
            Ok(None) => self.settings_status("no config file yet".to_string()),
            Err(err) => self.settings_status(format!("revert failed: {err}")),
        }
    }

    fn settings_status(&mut self, text: String) {
        if let Some(view) = self.settings.as_mut() {
            view.set_status(text);
        }
    }

    /// Snapshot the live config as a named profile (`save as` row).
    fn save_profile(&mut self, raw: &str) {
        let reopen = |view: &mut settings::SettingsView, text: String, msg: String| {
            view.editing = Some(settings::EditState::new(settings::Field::ProfileSave, text));
            view.set_status(msg);
        };
        let Some(name) = crate::profiles::sanitize(raw) else {
            if let Some(view) = self.settings.as_mut() {
                reopen(
                    view,
                    raw.to_string(),
                    "invalid name: letters, digits, - _ . only".into(),
                );
            }
            return;
        };
        let mut to_save = self.cfg.clone();
        to_save.colors = self.base_colors.clone();
        match crate::profiles::save(&name, &to_save) {
            Ok(()) => {
                if let Some(view) = self.settings.as_mut() {
                    view.refresh_profiles();
                    if let Some(i) = view.profiles.iter().position(|p| *p == name) {
                        view.profile_idx = i;
                    }
                    view.set_status(format!("profile saved: {name}"));
                }
            }
            Err(err) => {
                if let Some(view) = self.settings.as_mut() {
                    reopen(view, raw.to_string(), format!("save failed: {err}"));
                }
            }
        }
    }

    /// Load a profile and apply it live — the same path as an external edit.
    fn load_profile(&mut self, name: &str) {
        match crate::profiles::load(name) {
            Ok(Some(next)) => {
                self.base_colors = next.colors.clone();
                self.apply_config(next);
                self.settings_status(format!("profile loaded: {name}"));
            }
            Ok(None) => self.settings_status(format!("profile not found: {name}")),
            Err(err) => self.settings_status(format!("profile failed: {err}")),
        }
    }

    fn close_settings(&mut self) {
        if self.mode == Mode::Editor {
            self.quit = true;
        } else {
            self.settings = None;
            self.dirty = true;
        }
    }

    // --- pane commands -------------------------------------------------------

    fn do_action(&mut self, wi: usize, action: Action) {
        match action {
            Action::NewWindow => self.pending_window = true,
            Action::Copy => {
                if let Some(session) = self.active_session_mut(wi) {
                    if let Some(text) = session.term.selection_to_string() {
                        clipboard::store(Target::Clipboard, &text);
                    }
                }
            }
            Action::Paste => {
                let Some(text) = clipboard::load(Target::Clipboard) else {
                    return;
                };
                if let Some(session) = self.active_session_mut(wi) {
                    let mode = *session.term.mode();
                    let bytes = input::bracketed_paste(&text, mode);
                    // A paste rewrites the line wholesale.
                    session.typing.desync();
                    session.pty_write(&bytes);
                }
            }
            Action::Font(delta) => self.adjust_font(delta),
            Action::Page(up) => {
                let sid = self.active_sid(wi);
                if let Some(session) = self.session_mut(sid) {
                    session
                        .term
                        .scroll_display(if up { Scroll::PageUp } else { Scroll::PageDown });
                }
                self.dirty = true;
            }
            Action::Clear => {
                let sid = self.active_sid(wi);
                if let Some(session) = self.session_mut(sid) {
                    session.clear_all();
                }
                self.dirty = true;
            }
            Action::Settings => {
                if self.settings.is_some() {
                    self.settings = None;
                } else {
                    self.settings = Some(SettingsView::new(false));
                }
                self.dirty = true;
            }
            Action::NewTab => {
                let Ok(sid) = self.open_session(wi) else {
                    return;
                };
                self.windows[wi].tree.new_tab(sid);
                self.relayout(wi);
            }
            Action::CloseTab | Action::ClosePane => {
                let sid = self.active_sid(wi);
                if let Some((_, true)) = self.close_session(sid) {
                    self.close_window(wi);
                }
                self.sync_title();
            }
            Action::Split(dir) => {
                let Ok(sid) = self.open_session(wi) else {
                    return;
                };
                self.windows[wi].tree.split(dir, sid);
                self.relayout(wi);
            }
            Action::SelectTab(n) => {
                self.windows[wi].tree.select_tab(n);
                self.relayout(wi);
                self.sync_title();
            }
            Action::CycleTab(forward) => {
                self.windows[wi].tree.cycle_tab(forward);
                self.relayout(wi);
                self.sync_title();
            }
            Action::CycleTabBar => {
                self.cfg.ui.tab_bar = next_tabbar_mode(&self.cfg.ui.tab_bar).to_string();
                self.relayout_all();
                self.dirty = true;
            }
            Action::FocusPane(dir) => self.focus_dir(wi, dir),
        }
    }

    /// Move focus to the nearest pane in `dir`, within one window.
    fn focus_dir(&mut self, wi: usize, dir: Dir) {
        let active = self.active_sid(wi);
        let Some(current) = self.pane(active).map(|p| p.rect) else {
            return;
        };
        let cx = current.x + current.w as i32 / 2;
        let cy = current.y + current.h as i32 / 2;

        let mut best: Option<(i32, usize)> = None;
        for pane in self
            .windows
            .get(wi)
            .map(|w| w.panes.as_slice())
            .unwrap_or_default()
        {
            if pane.sid == active {
                continue;
            }
            let nx = pane.rect.x + pane.rect.w as i32 / 2;
            let ny = pane.rect.y + pane.rect.h as i32 / 2;
            let (dx, dy) = (nx - cx, ny - cy);
            let along = match dir {
                Dir::Left => dx < 0,
                Dir::Right => dx > 0,
                Dir::Up => dy < 0,
                Dir::Down => dy > 0,
            };
            if !along {
                continue;
            }
            let primary = match dir {
                Dir::Left | Dir::Right => dx.abs(),
                Dir::Up | Dir::Down => dy.abs(),
            };
            let cross = match dir {
                Dir::Left | Dir::Right => dy.abs(),
                Dir::Up | Dir::Down => dx.abs(),
            };
            let score = primary + cross * 2;
            if best.map(|(s, _)| score < s).unwrap_or(true) {
                best = Some((score, pane.sid));
            }
        }
        if let Some((_, sid)) = best {
            self.windows[wi].tree.set_active(sid);
            self.sync_title();
            self.dirty = true;
        }
    }

    // --- emulator events ----------------------------------------------------

    /// `rterm play` talking over the local socket.
    fn on_socket(&mut self, msg: crate::socket::Msg) {
        use crate::socket::Msg;
        match msg {
            Msg::Hello { tty, reply } => {
                let _ = reply.send(self.hello_info(&tty));
            }
            Msg::Open {
                sid,
                id,
                col,
                row,
                cols,
                rows,
                w,
                h,
            } => {
                if let Some(session) = self.session_mut(sid) {
                    session.open_video(id, w, h, cols, rows, col, row);
                }
                self.dirty = true;
            }
            Msg::Frame { sid, id, rgba } => {
                if let Some(session) = self.session_mut(sid) {
                    if session.push_video_frame(id, rgba) {
                        self.dirty = true;
                    }
                }
            }
            Msg::Close { sid, id } => {
                if let Some(session) = self.session_mut(sid) {
                    session.close_video(id);
                }
                self.dirty = true;
            }
            Msg::Closed { items } => {
                for (sid, id) in items {
                    if let Some(session) = self.session_mut(sid) {
                        session.close_video(id);
                    }
                }
                self.dirty = true;
            }
        }
    }

    /// Reply for `HELLO`: grid size, cell size, cursor cell and session id.
    fn hello_info(&self, tty: &str) -> String {
        let sid = self.resolve_sid(tty);
        let Some(pane) = self.pane(sid) else {
            return "ERR no pane".into();
        };
        let layout = pane.layout;
        let (col, row) = self
            .session(sid)
            .map(|s| {
                let ctx = s.ctx(&layout);
                (ctx.col, ctx.row)
            })
            .unwrap_or((0, 0));
        format!(
            "SIZE {} {} {:.2} {:.2} {col} {row} {sid}",
            layout.cols, layout.rows, layout.cell_w, layout.cell_h
        )
    }

    /// Match a client's tty (e.g. `/dev/pts/5`) to the session whose shell
    /// owns it; falls back to the focused session.
    fn resolve_sid(&self, tty: &str) -> usize {
        let active = self.active_sid(self.cur);
        if tty.is_empty() {
            return active;
        }
        for slot in &self.sessions {
            let Some(session) = slot.as_ref() else {
                continue;
            };
            let Some(pid) = session.pid else { continue };
            let link = std::fs::read_link(format!("/proc/{pid}/fd/0"));
            if let Ok(link) = link {
                if link.to_string_lossy() == tty {
                    return session.id;
                }
            }
        }
        active
    }

    fn on_term_event(&mut self, sid: usize, event: TermEvent) {
        match event {
            TermEvent::PtyWrite(text) => {
                if let Some(session) = self.session_mut(sid) {
                    session.pty_write(text.as_bytes());
                }
            }
            TermEvent::ClipboardStore(kind, text) => {
                clipboard::store(target(kind), &text);
            }
            TermEvent::ClipboardLoad(kind, format) => {
                if let Some(text) = clipboard::load(target(kind)) {
                    let reply = format(&text);
                    if let Some(session) = self.session_mut(sid) {
                        session.pty_write(reply.as_bytes());
                    }
                }
            }
            TermEvent::ColorRequest(index, formatter) => {
                let named = if index == NamedColor::Background as usize {
                    NamedColor::Background
                } else if index == NamedColor::Cursor as usize {
                    NamedColor::Cursor
                } else {
                    NamedColor::Foreground
                };
                let rgb = {
                    let Some(session) = self.session(sid) else {
                        return;
                    };
                    let content = session.term.renderable_content();
                    palette::resolve(
                        Color::Named(named),
                        content.colors,
                        &self.palette,
                        &self.cfg.colors,
                    )
                };
                let reply = formatter(palette::to_alacritty(rgb));
                if let Some(session) = self.session_mut(sid) {
                    session.pty_write(reply.as_bytes());
                }
            }
            TermEvent::Title(title) => {
                if let Some(session) = self.session_mut(sid) {
                    session.title = title;
                }
                if self.window_of(sid).is_some() {
                    self.sync_title();
                }
            }
            TermEvent::ResetTitle => {
                if let Some(session) = self.session_mut(sid) {
                    session.title.clear();
                }
                if self.window_of(sid).is_some() {
                    self.sync_title();
                }
            }
            TermEvent::Wakeup | TermEvent::MouseCursorDirty | TermEvent::Bell => {
                self.dirty = true;
            }
            TermEvent::CursorBlinkingChange(_) => self.dirty = true,
            TermEvent::Exit => {
                if let Some((wi, true)) = self.close_session(sid) {
                    self.close_window(wi);
                }
                self.sync_title();
            }
        }
    }

    // --- input --------------------------------------------------------------

    fn shortcut(&self, event: &KeyEvent) -> Option<Action> {
        let mods = self.mods;
        let ctrl_shift = mods.control_key() && mods.shift_key();
        if !ctrl_shift && matches!(event.logical_key, Key::Named(winit::keyboard::NamedKey::F1)) {
            return Some(Action::Settings);
        }
        // Classic X11 clipboard keys: Ctrl+Insert copies, Shift+Insert pastes.
        if matches!(
            event.logical_key,
            Key::Named(winit::keyboard::NamedKey::Insert)
        ) {
            if mods.control_key() && !mods.shift_key() && !mods.alt_key() {
                return Some(Action::Copy);
            }
            if mods.shift_key() && !mods.control_key() && !mods.alt_key() {
                return Some(Action::Paste);
            }
        }
        // Shift+PageUp/PageDown: scroll the scrollback without a mouse.
        if mods.shift_key() && !mods.control_key() && !mods.alt_key() {
            if let Key::Named(
                key @ (winit::keyboard::NamedKey::PageUp | winit::keyboard::NamedKey::PageDown),
            ) = &event.logical_key
            {
                return Some(Action::Page(*key == winit::keyboard::NamedKey::PageUp));
            }
        }
        if ctrl_shift {
            if let Key::Character(text) = &event.logical_key {
                return match text.to_lowercase().as_str() {
                    "c" => Some(Action::Copy),
                    "v" => Some(Action::Paste),
                    "k" => Some(Action::Clear),
                    "b" => Some(Action::CycleTabBar),
                    "+" | "=" => Some(Action::Font(1.0)),
                    "-" | "_" => Some(Action::Font(-1.0)),
                    "n" => Some(Action::NewWindow),
                    "t" => Some(Action::NewTab),
                    "w" => Some(Action::CloseTab),
                    "e" => Some(Action::Split(Dir::Right)),
                    "o" => Some(Action::Split(Dir::Down)),
                    "x" => Some(Action::ClosePane),
                    "1" => Some(Action::SelectTab(0)),
                    "2" => Some(Action::SelectTab(1)),
                    "3" => Some(Action::SelectTab(2)),
                    "4" => Some(Action::SelectTab(3)),
                    "5" => Some(Action::SelectTab(4)),
                    "6" => Some(Action::SelectTab(5)),
                    "7" => Some(Action::SelectTab(6)),
                    "8" => Some(Action::SelectTab(7)),
                    "9" => Some(Action::SelectTab(8)),
                    _ => None,
                };
            }
            if let Key::Named(winit::keyboard::NamedKey::Tab) = event.logical_key {
                return Some(Action::CycleTab(true));
            }
            if let Key::Named(
                key @ (winit::keyboard::NamedKey::ArrowLeft
                | winit::keyboard::NamedKey::ArrowRight
                | winit::keyboard::NamedKey::ArrowUp
                | winit::keyboard::NamedKey::ArrowDown),
            ) = event.logical_key
            {
                let dir = match key {
                    winit::keyboard::NamedKey::ArrowLeft => Dir::Left,
                    winit::keyboard::NamedKey::ArrowRight => Dir::Right,
                    winit::keyboard::NamedKey::ArrowUp => Dir::Up,
                    _ => Dir::Down,
                };
                return Some(Action::FocusPane(dir));
            }
        }
        if mods.control_key()
            && !mods.shift_key()
            && matches!(
                event.logical_key,
                Key::Named(winit::keyboard::NamedKey::Tab)
            )
        {
            return Some(Action::CycleTab(true));
        }
        None
    }

    fn on_key(&mut self, wi: usize, event: KeyEvent) {
        use winit::keyboard::NamedKey as K;
        if event.state != ElementState::Pressed {
            return;
        }
        if self.settings.is_some() {
            self.settings_key(&event);
            return;
        }
        if let Some(action) = self.shortcut(&event) {
            self.do_action(wi, action);
            return;
        }

        // `End` / `→` type in the grey suggestion. Both do nothing else while
        // the cursor already sits at the end of the line, so nothing is lost.
        if self.cfg.ui.suggest
            && !self.mods.control_key()
            && !self.mods.alt_key()
            && matches!(event.logical_key, Key::Named(K::End | K::ArrowRight))
            && self.accept_suggestion(wi).is_some()
        {
            if self.cursor_blinks() {
                self.blink_on = true;
                self.blink_next = Instant::now()
                    + Duration::from_millis(self.cfg.cursor.blink_interval_ms);
            }
            self.dirty = true;
            return;
        }

        let mods = self.mods;
        let mut cleared = false;
        if let Some(session) = self.active_session_mut(wi) {
            if session.term.selection.is_some() && !mods.shift_key() {
                session.term.selection = None;
                cleared = true;
            }

            // Feed the suggestion tracker with the same keystrokes the shell
            // gets. Anything that could rewrite the line behind our back
            // (completion, cursor motion, shortcuts) stops the tracking.
            match &event.logical_key {
                Key::Named(K::Enter) if !mods.control_key() && !mods.alt_key() => {
                    session.commit_typed();
                }
                Key::Named(K::Backspace) if !mods.control_key() => session.typing.backspace(),
                Key::Character(_) | Key::Named(K::Space)
                    if !mods.control_key() && !mods.alt_key() =>
                {
                    if let Some(text) = event.text.as_deref() {
                        for c in text.chars().filter(|c| !c.is_control()) {
                            session.typing.push(c);
                        }
                    }
                }
                _ => session.typing.desync(),
            }

            let mode = *session.term.mode();
            if let Some(bytes) =
                input::encode_key(&event.logical_key, event.text.as_deref(), mods, mode)
            {
                session.pty_write(&bytes);
            }
        }
        if cleared {
            self.dirty = true;
        }

        // Typing keeps the cursor solid for a while.
        if self.cursor_blinks() {
            self.blink_on = true;
            self.blink_next =
                Instant::now() + Duration::from_millis(self.cfg.cursor.blink_interval_ms);
        }
    }

    /// Type the grey tail in as if the user had typed it themselves;
    /// `None` when there is no suggestion under the cursor.
    fn accept_suggestion(&mut self, wi: usize) -> Option<String> {
        let sid = self.active_sid(wi);
        let layout = self.pane(sid)?.layout;
        let rest = self
            .session(sid)?
            .suggestion(&layout, self.cfg.ui.suggest)?
            .0;
        if let Some(session) = self.active_session_mut(wi) {
            session.pty_write(rest.as_bytes());
            session.typing.accept(&rest);
        }
        Some(rest)
    }

    fn on_text(&mut self, wi: usize, text: &str) {
        if text.is_empty() || self.settings.is_some() {
            return;
        }
        if let Some(session) = self.active_session_mut(wi) {
            session.pty_write(text.as_bytes());
        }
    }

    fn on_cursor_moved(&mut self, wi: usize, x: f64, y: f64) {
        self.windows[wi].cursor_px = Some((x, y));
        let Some(pane) = self.pane_at(wi, x as i32, y as i32) else {
            return;
        };
        let sid = pane.sid;
        let layout = pane.layout;
        let Some((col, row)) = layout.cell_at(x as i32, y as i32) else {
            return;
        };
        let Some(session) = self.session(sid) else {
            return;
        };
        let mode = *session.term.mode();

        if let Some((button, reported)) = self.windows[wi].held {
            if reported {
                if let Some(seq) = input::mouse_drag(button, col, row, self.mods, mode) {
                    if let Some(session) = self.session_mut(sid) {
                        session.pty_write(&seq);
                    }
                }
            } else if let Some(session) = self.session_mut(sid) {
                let point = session.point_at(col, row, &layout);
                let side = session.side_at(col, x, &layout);
                if let Some(selection) = session.term.selection.as_mut() {
                    selection.update(point, side);
                    self.dirty = true;
                }
            }
            return;
        }

        if let Some(seq) = input::mouse_motion(col, row, self.mods, mode) {
            if let Some(session) = self.session_mut(sid) {
                session.pty_write(&seq);
            }
        }
    }

    fn on_mouse_down(&mut self, wi: usize, button: MouseButton) {
        let Some((px, py)) = self.windows[wi].cursor_px else {
            return;
        };
        if let Some(view) = self.settings.as_ref() {
            let geo = settings::geometry(
                &mut self.fonts,
                &self.cfg,
                view,
                self.windows[wi].size.0 as i32,
                self.windows[wi].size.1 as i32,
            );
            // Clicks inside the colour-picker card drag its handles.
            if view.picking.is_some() {
                let g = settings::picker_geometry(&geo, geo.row_of(view.selected));
                let (mx, my) = (px as i32, py as i32);
                if g.contains(mx, my) {
                    let mut apply: Option<(settings::Field, String)> = None;
                    if let Some(view) = self.settings.as_mut() {
                        if let Some(pick) = view.picking.as_mut() {
                            let dx = mx as f32 + 0.5 - g.cx;
                            let dy = my as f32 + 0.5 - g.cy;
                            let d = (dx * dx + dy * dy).sqrt();
                            if g.in_slider(mx, my) {
                                let t = (mx - g.slider[0]) as f32 / g.slider[2].max(1) as f32;
                                pick.light = t.clamp(0.0, 1.0) * 100.0;
                            } else if d <= g.disc + 6.0 {
                                // Inside the disc: angle picks the hue, the
                                // distance from the centre picks the saturation.
                                pick.hue = dy.atan2(dx).to_degrees().rem_euclid(360.0);
                                pick.sat = (d / g.disc).clamp(0.0, 1.0) * 100.0;
                            } else if d <= g.ring + 3.0 {
                                pick.hue = dy.atan2(dx).to_degrees().rem_euclid(360.0);
                            }
                            apply = Some((pick.field, pick.hex()));
                        }
                    }
                    if let Some((field, hex)) = apply {
                        let _ = self.try_apply(field, &hex);
                    }
                    self.dirty = true;
                    return;
                }
            }
            if let Some(index) = geo.hit(px as i32, py as i32) {
                let Some(view) = self.settings.as_mut() else {
                    return;
                };
                let field = Field::ALL[index];
                // Clicking a collapsed section header opens it.
                if view.open_group == field.group() {
                    // Already open → act on the row itself below.
                } else {
                    view.open_group = field.group();
                    view.selected = index;
                    self.dirty = true;
                    return;
                }
                match field {
                    Field::ProfileSelect => {
                        let name = view.profile_name().map(str::to_string);
                        if let Some(name) = name {
                            self.load_profile(&name);
                        }
                    }
                    Field::ProfileDelete => {
                        let name = view.profile_name().map(str::to_string);
                        if let Some(name) = name {
                            let _ = crate::profiles::delete(&name);
                            view.refresh_profiles();
                            view.set_status(format!("deleted {name}"));
                        }
                    }
                    Field::Preset => self.settings_adjust(1),
                    f if f.is_pickable() => {
                        view.editing = None;
                        view.selected = index;
                        let current = f
                            .color_hex(&self.cfg)
                            .map(str::to_string)
                            .unwrap_or_else(|| self.cfg.colors.foreground.clone());
                        view.picking = Some(settings::Pick::open(
                            f,
                            &current,
                            &self.cfg.colors.foreground,
                        ));
                        self.dirty = true;
                        return;
                    }
                    f if f.is_editable() => {
                        self.settings_begin_edit_index(index);
                    }
                    _ => {
                        view.select(index);
                    }
                }
            }
            self.dirty = true;
            return;
        }
        let Some(pane) = self.pane_at(wi, px as i32, py as i32) else {
            return;
        };
        let sid = pane.sid;
        let layout = pane.layout;
        // A click inside the strip picks the tab under the cursor.
        let strip = pane.tabbar;
        let strip_tabs = pane.tabs.clone();
        let prect = pane.rect;
        if strip > 0 {
            let (mx, my) = (px as i32, py as i32);
            let strip_y = if self.cfg.ui.tab_bar_position == "bottom" {
                prect.y + prect.h as i32 - strip
            } else {
                prect.y
            };
            if my >= strip_y && my < strip_y + strip && mx >= prect.x {
                let chips = tab_chips(
                    &mut self.fonts,
                    &strip_tabs,
                    prect.x + 6,
                    &self.cfg.ui.tab_style,
                    self.cfg.ui.tab_close,
                    |s| {
                        self.sessions
                            .get(s)
                            .and_then(Option::as_ref)
                            .map(|t| t.title.clone())
                            .filter(|t| !t.is_empty())
                            .unwrap_or_else(|| "shell".into())
                    },
                );
                // The ✕ wins over selecting the tab it belongs to.
                let close = chips.iter().find(|c| {
                    c.has_close() && mx >= c.close_x && mx < c.close_x + c.close_w
                });
                if let Some(chip) = close {
                    let victim = chip.sid;
                    if let Some((_, empty)) = self.close_session(victim) {
                        if empty {
                            self.close_window(wi);
                        }
                    }
                    self.sync_title();
                    self.dirty = true;
                    return;
                }
                let hit = chips
                    .iter()
                    .position(|c| mx >= c.x - 4 && mx < c.x + c.w + 2)
                    .and_then(|i| strip_tabs.get(i).copied());
                if let Some(tab_sid) = hit {
                    if tab_sid != sid {
                        self.windows[wi].tree.set_active(tab_sid);
                        self.relayout(wi);
                        self.sync_title();
                    }
                    self.dirty = true;
                }
                return;
            }
        }
        let Some((col, row)) = layout.cell_at(px as i32, py as i32) else {
            return;
        };
        if sid != self.active_sid(wi) {
            self.windows[wi].tree.set_active(sid);
            self.sync_title();
            self.dirty = true;
        }
        let Some(session) = self.session(sid) else {
            return;
        };
        let mode = *session.term.mode();
        let shift = self.mods.shift_key();

        // Apps asked for mouse reporting and the user is not forcing selection.
        if !shift {
            if let Some(seq) = input::mouse_button(button, true, col, row, self.mods, mode) {
                if let Some(session) = self.session_mut(sid) {
                    session.pty_write(&seq);
                }
                self.windows[wi].held = Some((button, true));
                return;
            }
        }

        match button {
            MouseButton::Left => {
                let now = Instant::now();
                let double =
                    now.saturating_duration_since(self.windows[wi].last_click) < DOUBLE_CLICK;
                self.windows[wi].last_click = now;
                let ty = if self.mods.alt_key() {
                    SelectionType::Block
                } else {
                    self.windows[wi].clicks = if double {
                        (self.windows[wi].clicks + 1) % 3
                    } else {
                        0
                    };
                    match self.windows[wi].clicks {
                        1 => SelectionType::Semantic,
                        2 => SelectionType::Lines,
                        _ => SelectionType::Simple,
                    }
                };
                if let Some(session) = self.session_mut(sid) {
                    let point = session.point_at(col, row, &layout);
                    let side = session.side_at(col, px, &layout);
                    session.term.selection = Some(Selection::new(ty, point, side));
                }
                self.windows[wi].held = Some((button, false));
                self.dirty = true;
            }
            MouseButton::Middle if self.cfg.ui.middle_click_paste => {
                self.paste_sid(sid, Target::Selection);
            }
            _ => {}
        }
    }

    fn on_mouse_up(&mut self, wi: usize, button: MouseButton) {
        let Some((held, reported)) = self.windows[wi].held.take() else {
            return;
        };
        if held != button {
            self.windows[wi].held = Some((held, reported));
            return;
        }
        let Some((px, py)) = self.windows[wi].cursor_px else {
            return;
        };
        let Some(pane) = self.pane_at(wi, px as i32, py as i32) else {
            return;
        };
        let sid = pane.sid;
        let layout = pane.layout;
        let Some((col, row)) = layout.cell_at(px as i32, py as i32) else {
            return;
        };

        if reported {
            let mode = self.session(sid).map(|s| *s.term.mode());
            if let Some(mode) = mode {
                if let Some(seq) = input::mouse_button(button, false, col, row, self.mods, mode) {
                    if let Some(session) = self.session_mut(sid) {
                        session.pty_write(&seq);
                    }
                }
            }
            return;
        }
        if button == MouseButton::Left {
            if let Some(session) = self.session_mut(sid) {
                let point = session.point_at(col, row, &layout);
                let side = session.side_at(col, px, &layout);
                if let Some(selection) = session.term.selection.as_mut() {
                    selection.update(point, side);
                }
            }
            if self.cfg.ui.copy_on_select {
                self.copy_sid(sid);
            }
            self.dirty = true;
        }
    }

    fn on_wheel(&mut self, wi: usize, delta: MouseScrollDelta) {
        if self.settings.is_some() {
            // The wheel steps values, but never while text is being typed.
            if self.settings.as_ref().is_some_and(|v| v.editing.is_some()) {
                self.dirty = true;
                return;
            }
            let steps = match delta {
                MouseScrollDelta::LineDelta(_, y) => f64::from(y),
                MouseScrollDelta::PixelDelta(pos) => f64::from(pos.y as f32) / 24.0,
            };
            if steps > 0.0 {
                self.settings_adjust(1);
            } else if steps < 0.0 {
                self.settings_adjust(-1);
            }
            self.dirty = true;
            return;
        }
        let Some((px, py)) = self.windows[wi].cursor_px else {
            return;
        };
        let Some(pane) = self.pane_at(wi, px as i32, py as i32) else {
            return;
        };
        let sid = pane.sid;
        let layout = pane.layout;
        let notches = match delta {
            MouseScrollDelta::LineDelta(_, y) => f64::from(y),
            MouseScrollDelta::PixelDelta(pos) => f64::from(pos.y as f32) / f64::from(layout.cell_h),
        };
        let steps = if notches.abs() >= 1.0 {
            notches.round() as i32
        } else if notches.abs() > f64::EPSILON {
            notches.signum() as i32
        } else {
            0
        };
        if steps == 0 {
            return;
        }
        let up = steps > 0;
        let force_scroll = self.mods.shift_key();
        let mode = self
            .session(sid)
            .map(|s| *s.term.mode())
            .unwrap_or(TermMode::empty());

        if !force_scroll {
            if let Some((col, row)) = layout.cell_at(px as i32, py as i32) {
                if let Some(seq) = input::mouse_wheel(up, col, row, self.mods, mode) {
                    if let Some(session) = self.session_mut(sid) {
                        for _ in 0..steps.abs().min(16) {
                            session.pty_write(&seq);
                        }
                    }
                    return;
                }
            }

            // Alternate screen: translate the wheel into arrow keys.
            let alt_scroll = self
                .session(sid)
                .map(|s| {
                    s.term.selection.is_none()
                        && mode.contains(TermMode::ALT_SCREEN)
                        && mode.contains(TermMode::ALTERNATE_SCROLL)
                })
                .unwrap_or(false);
            if alt_scroll {
                let key = if up { b'A' } else { b'B' };
                let seq: &[u8] = if mode.contains(TermMode::APP_CURSOR) {
                    &[0x1b, b'O', key]
                } else {
                    &[0x1b, b'[', key]
                };
                if let Some(session) = self.session_mut(sid) {
                    for _ in 0..steps.abs().min(16) {
                        session.pty_write(seq);
                    }
                }
                return;
            }
        }

        let amount = steps.saturating_mul(self.cfg.ui.scroll_lines as i32);
        if let Some(session) = self.session_mut(sid) {
            session.term.scroll_display(Scroll::Delta(amount));
        }
        self.dirty = true;
    }

    fn copy_sid(&mut self, sid: usize) {
        let text = self.session(sid).and_then(|s| s.term.selection_to_string());
        if let Some(text) = text {
            clipboard::store(Target::Clipboard, &text);
        }
    }

    fn paste_sid(&mut self, sid: usize, target: Target) {
        let Some(text) = clipboard::load(target) else {
            return;
        };
        if let Some(session) = self.session_mut(sid) {
            let mode = *session.term.mode();
            let bytes = input::bracketed_paste(&text, mode);
            session.pty_write(&bytes);
        }
    }

    fn on_focus(&mut self, wi: usize, focused: bool) {
        self.windows[wi].focused = focused;
        let sid = self.active_sid(wi);
        if let Some(session) = self.session_mut(sid) {
            session.term.is_focused = focused;
            if session.term.mode().contains(TermMode::FOCUS_IN_OUT) {
                let seq: &[u8] = if focused { b"\x1b[I" } else { b"\x1b[O" };
                session.pty_write(seq);
            }
        }
        self.blink_on = true;
        self.dirty = true;
    }

    // --- scheduling ---------------------------------------------------------

    fn cursor_blinks(&self) -> bool {
        self.cfg.cursor.blink
            || self
                .session(self.focused_sid())
                .map(|s| s.term.cursor_style().blinking)
                .unwrap_or(false)
    }

    fn animated_background(&self) -> bool {
        match self.cfg.effects.background.as_str() {
            "plasma" | "aurora" => true,
            "gradient" => self.cfg.effects.gradient_animated,
            _ => false,
        }
    }

    /// Still fading in, text animating, or grain/animated bg should keep twinkling.
    fn needs_frames(&self) -> bool {
        let now = Instant::now();
        self.fade_active()
            || self.cfg.effects.grain
            || self.animated_background()
            || self.sessions.iter().flatten().any(|s| s.fx.active(now, self.cfg.anim.ms))
    }

    fn fade_active(&self) -> bool {
        const FADE: Duration = Duration::from_millis(700);
        // One extra frame so the last multiply lands on k == 1.0 exactly.
        self.cfg.effects.fade_in && self.started.elapsed() < FADE + ANIM_FRAME
    }

    /// Cadence for the next forced animation frame.
    fn anim_period(&self) -> Duration {
        // Fade, text entrance and animated backgrounds want smooth motion;
        // grain is noise.
        if self.fade_active() || self.animated_background() || self.text_animating() {
            ANIM_FRAME
        } else {
            GRAIN_FRAME
        }
    }

    fn text_animating(&self) -> bool {
        let now = Instant::now();
        self.sessions
            .iter()
            .flatten()
            .any(|s| s.fx.active(now, self.cfg.anim.ms))
    }

    fn update_blink(&mut self, now: Instant) {
        if !self.cursor_blinks() {
            self.blink_on = true;
            return;
        }
        if now >= self.blink_next {
            self.blink_on = !self.blink_on;
            self.blink_next = now + Duration::from_millis(self.cfg.cursor.blink_interval_ms);
            self.dirty = true;
        }
    }

    fn schedule(&self, el: &ActiveEventLoop, now: Instant) {
        let mut next = self.next_cfg_poll;
        for session in self.sessions.iter().flatten() {
            if let Some(deadline) = session.media.next_deadline(now) {
                next = next.min(deadline);
            }
        }
        if self.needs_frames() {
            next = next.min(self.next_anim.max(now));
        }
        if self.cursor_blinks() {
            next = next.min(self.blink_next);
        }
        el.set_control_flow(ControlFlow::WaitUntil(next.max(now)));
    }

    /// Re-read the desktop scheme and the niri accent now and then, so
    /// switching themes outside rterm is picked up live.
    fn poll_theme(&mut self, now: Instant) {
        if self.cfg.theme.mode != "auto" || now < self.next_theme_poll {
            return;
        }
        self.next_theme_poll = now + THEME_POLL;
        let colors = theme::effective(self.base_colors.clone());
        if colors != self.cfg.colors {
            self.cfg.colors = colors;
            self.palette = Palette::from_cfg(&self.cfg.colors);
            self.renderer.invalidate_bg();
            self.dirty = true;
        }
    }

    fn poll_config(&mut self, now: Instant) {
        self.next_cfg_poll = now + CONFIG_POLL;
        if self.settings.is_some() {
            // The editor owns the config; external edits resume once it closes.
            return;
        }
        let Some(path) = self.cfg_path.clone() else {
            return;
        };
        let mtime = Config::mtime(&path);
        if mtime == self.cfg_mtime {
            return;
        }
        self.cfg_mtime = mtime;
        let loaded = match Config::load_from(&path) {
            Ok(Some(cfg)) => cfg,
            Ok(None) => Config::default(),
            Err(err) => {
                eprintln!("rterm: config error: {err}");
                return;
            }
        };
        self.base_colors = loaded.colors.clone();
        self.apply_config(loaded);
    }

    fn apply_config(&mut self, mut next: Config) {
        next.colors = theme::effective(self.base_colors.clone());
        let font_changed = next.font != self.cfg.font;
        let family_changed = next.font.family != self.cfg.font.family
            || next.font.bold_family != self.cfg.font.bold_family
            || next.font.italic_family != self.cfg.font.italic_family;
        let prev_font = self.cfg.font.clone();
        let colors_changed = next.colors != self.cfg.colors;
        let effects_changed = next.effects != self.cfg.effects;
        let media_changed = next.media != self.cfg.media;
        let prompt_changed = next.prompt != self.cfg.prompt;
        let term_changed = next.scrollback != self.cfg.scrollback || next.cursor != self.cfg.cursor;
        let relayout = font_changed
            || next.ui.padding_x != self.cfg.ui.padding_x
            || next.ui.padding_y != self.cfg.ui.padding_y
            || next.ui.tab_bar != self.cfg.ui.tab_bar
            || next.ui.tab_bar_position != self.cfg.ui.tab_bar_position
            || next.shell != self.cfg.shell;

        self.cfg = next;
        if prompt_changed {
            crate::prompt::sync(&self.cfg.prompt);
        }
        if colors_changed {
            self.palette = Palette::from_cfg(&self.cfg.colors);
            self.renderer.invalidate_bg();
        }
        if family_changed {
            match FontSystem::new(&self.cfg.font, self.scale as f32) {
                Ok(fonts) => self.fonts = fonts,
                Err(err) => {
                    eprintln!("rterm: font error: {err}");
                    self.cfg.font = prev_font;
                }
            }
        } else if font_changed {
            self.fonts
                .recompute_metrics(&self.cfg.font, self.scale as f32);
        }
        if effects_changed {
            self.renderer.invalidate_bg();
        }
        if media_changed {
            for session in self.sessions.iter_mut().flatten() {
                session.media.set_budget(self.cfg.media.max_cache_mb);
                session.media.loop_gifs = self.cfg.media.loop_gifs;
            }
        }
        if term_changed {
            let cfg = term_config(&self.cfg);
            for session in self.sessions.iter_mut().flatten() {
                session.term.update_config(&cfg);
            }
        }
        if relayout {
            self.relayout_all();
        }
        self.dirty = true;
    }

    // --- presentation -------------------------------------------------------

    /// Render window `wi` and present its buffer.
    fn draw(&mut self, wi: usize) {
        let cur = self.cur;
        let Some(physical) = self
            .windows
            .get(wi)
            .map(WindowState::window)
            .map(|w| w.inner_size())
        else {
            return;
        };
        if physical.width == 0 || physical.height == 0 {
            // Minimized window: stop asking for frames, `Resized` re-arms us.
            return;
        }
        if physical != PhysicalSize::new(self.windows[wi].size.0, self.windows[wi].size.1) {
            self.resize(wi, physical.width, physical.height);
        }
        let (w, h) = (physical.width as usize, physical.height as usize);
        let (Some(nw), Some(nh)) = (
            NonZeroU32::new(physical.width),
            NonZeroU32::new(physical.height),
        ) else {
            return;
        };
        if self.windows[wi].surface.resize(nw, nh).is_err() {
            return;
        }

        {
            let win = &self.windows[wi];
            let panes = &win.panes;
            let active = win.tree.active();
            let focused = win.focused;
            let Self {
                renderer,
                fonts,
                sessions,
                palette,
                cfg,
                blink_on,
                started,
                settings: overlay,
                ..
            } = self;

            renderer.ensure_size(w, h);
            renderer.draw_background(cfg, started.elapsed());

            for pane in panes.iter() {
                let Some(session) = sessions.get_mut(pane.sid).and_then(Option::as_mut) else {
                    continue;
                };
                session.refresh_fx(&pane.layout, cfg, Instant::now());
                let mut content = session.term.renderable_content();
                if !cfg.media.above_text {
                    renderer.draw_media(&session.media, &pane.layout);
                }
                renderer.draw_cells(
                    &mut content,
                    fonts,
                    palette,
                    cfg,
                    &pane.layout,
                    session.fx.row_fx(),
                );
                if cfg.media.above_text {
                    renderer.draw_media(&session.media, &pane.layout);
                }

                let cursor = content.cursor;
                let row = cursor.point.line.0 + content.display_offset as i32;
                let col = cursor.point.column.0;
                let mut ch = session.term.grid()[cursor.point].c;
                // Grey tail of an older command. It is only drawn when the
                // screen agrees with what was typed, so it can never lie.
                let ghost = if pane.sid == active
                    && row >= 0
                    && row < pane.layout.rows as i32
                    && col < pane.layout.cols
                {
                    session.suggestion(&pane.layout, cfg.ui.suggest)
                } else {
                    None
                };
                if let Some((rest, _)) = &ghost {
                    let color = palette::parse_hex_or(&cfg.ui.suggest_color, [0x7d, 0x85, 0x90]);
                    let baseline =
                        pane.layout.cell_y(row as usize) as f32 + fonts.metrics.baseline;
                    renderer.label(fonts, pane.layout.cell_x(col), baseline, rest, color);
                    if let Some(first) = rest.chars().next() {
                        ch = first;
                    }
                }
                let visible = pane.sid == active
                    && cursor.shape != CursorShape::Hidden
                    && row >= 0
                    && row < pane.layout.rows as i32
                    && col < pane.layout.cols
                    && *blink_on;
                renderer.draw_cursor(
                    &pane.layout,
                    cfg,
                    palette,
                    content.colors,
                    fonts,
                    cursor.shape,
                    col,
                    row as usize,
                    ch,
                    visible,
                    focused,
                );
            }

            // Hairlines between split panes.
            if panes.len() > 1 {
                let bg = palette::parse_hex_or(&cfg.colors.background, [0x16, 0x16, 0x1e]);
                let accent = palette::parse_hex_or(
                    cfg.colors
                        .selection_background
                        .as_deref()
                        .unwrap_or("#7aa2f7"),
                    [0x7a, 0xa2, 0xf7],
                );
                let sep = palette::mix(bg, accent, 0.45);
                for pane in panes.iter() {
                    if pane.rect.x > 0 {
                        renderer.fill_box(pane.rect.x, pane.rect.y, 1, pane.rect.h as i32, sep);
                    }
                    if pane.rect.y > 0 {
                        renderer.fill_box(pane.rect.x, pane.rect.y, pane.rect.w as i32, 1, sep);
                    }
                }
            }

            // Tab strips and the focus border, on top of the pane contents.
            let accent = palette::parse_hex_or(
                cfg.colors
                    .selection_background
                    .as_deref()
                    .unwrap_or("#7aa2f7"),
                [0x7a, 0xa2, 0xf7],
            );
            for pane in panes.iter() {
                if pane.tabbar > 0 {
                    let bottom = cfg.ui.tab_bar_position == "bottom";
                    let strip_y = if bottom {
                        pane.rect.y + pane.rect.h as i32 - pane.tabbar
                    } else {
                        pane.rect.y
                    };
                    let bg = palette::parse_hex_or(
                        &cfg.ui.tab_bar_bg,
                        palette::parse_hex_or(&cfg.colors.background, [0x16, 0x16, 0x1e]),
                    );
                    let fg = palette::parse_hex_or(
                        &cfg.ui.tab_inactive_fg,
                        palette::parse_hex_or(&cfg.colors.foreground, [0xcc, 0xcc, 0xcc]),
                    );
                    let active_bg = palette::parse_hex_or(&cfg.ui.tab_active_bg, accent);
                    let active_fg = palette::parse_hex_or(
                        &cfg.ui.tab_active_fg,
                        palette::parse_hex_or(&cfg.colors.background, [0x16, 0x16, 0x1e]),
                    );
                    let a = (cfg.ui.tab_bar_alpha.clamp(0.0, 1.0) * 255.0).round() as u8;
                    renderer.fill_box_alpha(
                        pane.rect.x,
                        strip_y,
                        pane.rect.w as i32,
                        pane.tabbar,
                        bg,
                        a,
                    );
                    let baseline = strip_y as f32 + fonts.metrics.baseline + TAB_PAD as f32;
                    let chips = tab_chips(
                        fonts,
                        &pane.tabs,
                        pane.rect.x + 6,
                        &cfg.ui.tab_style,
                        cfg.ui.tab_close,
                        |sid| {
                            sessions
                                .get(sid)
                                .and_then(Option::as_ref)
                                .map(|s| s.title.clone())
                                .filter(|t| !t.is_empty())
                                .unwrap_or_else(|| "shell".into())
                        },
                    );
                    for (i, chip) in chips.iter().enumerate() {
                        let selected = chip.sid == pane.sid;
                        match cfg.ui.tab_style.as_str() {
                            "underline" => {
                                if selected {
                                    renderer.fill_box(
                                        chip.x - 4,
                                        strip_y + pane.tabbar - 2,
                                        chip.w + 6,
                                        2,
                                        active_bg,
                                    );
                                } else if i > 0 {
                                    renderer.fill_box(
                                        chip.x - 6,
                                        strip_y + 4,
                                        1,
                                        (pane.tabbar - 8).max(1),
                                        palette::mix(bg, fg, 0.5),
                                    );
                                }
                                let color = if selected { active_bg } else { fg };
                                renderer.label(fonts, chip.x, baseline, &chip.label, color);
                            }
                            "minimal" => {
                                let color = if selected { active_bg } else { palette::mix(fg, bg, 0.55) };
                                renderer.label(fonts, chip.x, baseline, &chip.label, color);
                            }
                            _ => {
                                if selected {
                                    renderer.fill_box(
                                        chip.x - 4,
                                        strip_y,
                                        chip.w + 6,
                                        pane.tabbar,
                                        active_bg,
                                    );
                                }
                                let color = if selected { active_fg } else { fg };
                                renderer.label(fonts, chip.x, baseline, &chip.label, color);
                            }
                        }
                        if chip.has_close() {
                            let color = if selected {
                                active_fg
                            } else {
                                palette::mix(fg, bg, 0.7)
                            };
                            renderer.label(fonts, chip.close_x + 2, baseline, "×", color);
                        }
                    }
                }
                if pane.sid == active {
                    renderer.outline(
                        pane.rect.x,
                        pane.rect.y,
                        pane.rect.w as i32,
                        pane.rect.h as i32,
                        accent,
                    );
                    if cfg.effects.glow && pane.rect.w > 4 && pane.rect.h > 4 {
                        renderer.outline(
                            pane.rect.x + 1,
                            pane.rect.y + 1,
                            pane.rect.w as i32 - 2,
                            pane.rect.h as i32 - 2,
                            palette::mix(
                                palette::parse_hex_or(
                                    cfg.colors
                                        .selection_background
                                        .as_deref()
                                        .unwrap_or("#7aa2f7"),
                                    [0x7a, 0xa2, 0xf7],
                                ),
                                palette::parse_hex_or(&cfg.colors.background, [0x16, 0x16, 0x1e]),
                                0.55,
                            ),
                        );
                    }
                }
            }

            renderer.post_effects(cfg, started.elapsed());
            // The settings overlay belongs to the window holding the focus.
            if wi == cur {
                if let Some(view) = overlay.as_ref() {
                    settings::draw(renderer, fonts, cfg, view, w as i32, h as i32);
                }
            }
        }

        let surface = &mut self.windows[wi].surface;
        if let Ok(mut buffer) = surface.buffer_mut() {
            let bw = buffer.width().get() as usize;
            let bh = buffer.height().get() as usize;
            if bw == w && bh == h && self.renderer.fb.len() == w * h {
                // Premultiplied alpha: colours are already scaled by opacity
                // in draw_background, so the top byte just carries it. The fb
                // packs 0x00RRGGBB, so the byte must be stamped every frame -
                // skipping it when a == 255 would leave alpha 0 (invisible).
                let a = (self.cfg.window.opacity.clamp(0.2, 1.0) * 255.0 + 0.5) as u32;
                for px in &mut self.renderer.fb {
                    *px = (*px & 0x00ff_ffff) | (a << 24);
                }
                buffer.copy_from_slice(&self.renderer.fb);
                let _ = buffer.present();
            }
        }
    }
}

impl ApplicationHandler<UserEvent> for App {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if !self.windows.is_empty() {
            return;
        }
        let attrs = window_attrs(&self.cfg.window.title, &self.cfg);
        let Some(wi) = self.add_window(el, attrs) else {
            el.exit();
            return;
        };
        self.seed_window(wi);
        self.sync_title();
        self.dirty = true;
        self.schedule(el, Instant::now());
    }

    fn user_event(&mut self, _el: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::Wake | UserEvent::MediaReady => self.dirty = true,
            UserEvent::Term { sid, event } => self.on_term_event(sid, event),
            UserEvent::Socket(msg) => self.on_socket(msg),
        }
    }

    fn window_event(&mut self, el: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        let Some(wi) = self.windows.iter().position(|win| win.window().id() == id) else {
            return;
        };
        match event {
            WindowEvent::CloseRequested => {
                self.close_window(wi);
                if self.windows.is_empty() {
                    self.shutdown_all();
                    el.exit();
                }
            }
            WindowEvent::Resized(size) => self.resize(wi, size.width, size.height),
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                self.scale = scale_factor;
                self.fonts
                    .recompute_metrics(&self.cfg.font, scale_factor as f32);
                self.relayout_all();
                self.dirty = true;
            }
            WindowEvent::RedrawRequested => self.draw(wi),
            WindowEvent::Focused(focused) => {
                if focused {
                    self.cur = wi;
                }
                self.on_focus(wi, focused);
            }
            WindowEvent::ModifiersChanged(modifiers) => {
                self.cur = wi;
                self.mods = modifiers.state();
            }
            WindowEvent::KeyboardInput { event, .. } => {
                self.cur = wi;
                self.on_key(wi, event);
            }
            WindowEvent::Ime(Ime::Commit(text)) => {
                self.cur = wi;
                self.on_text(wi, &text);
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.cur = wi;
                self.on_cursor_moved(wi, position.x, position.y);
            }
            WindowEvent::MouseInput { state, button, .. } => {
                self.cur = wi;
                match state {
                    ElementState::Pressed => self.on_mouse_down(wi, button),
                    ElementState::Released => self.on_mouse_up(wi, button),
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                self.cur = wi;
                self.on_wheel(wi, delta);
            }
            _ => {}
        }
        self.schedule(el, Instant::now());
    }

    fn about_to_wait(&mut self, el: &ActiveEventLoop) {
        let now = Instant::now();
        self.poll_config(now);
        self.poll_theme(now);

        if self.pending_window {
            self.pending_window = false;
            self.open_new_window(el);
        }

        let mut closed = Vec::new();
        let mut fed = false;
        let layouts: Vec<Layout> = (0..self.sessions.len())
            .map(|sid| {
                self.pane(sid)
                    .map(|p| p.layout)
                    .unwrap_or_else(|| self.base_layout())
            })
            .collect();
        for (sid, (slot, layout)) in self.sessions.iter_mut().zip(layouts.iter()).enumerate() {
            let Some(session) = slot.as_mut() else {
                continue;
            };
            let drain = session.drain_pty(layout, &mut self.pipeline);
            fed |= drain.fed;
            if drain.eof {
                closed.push(sid);
            } else {
                session.flush_stale();
            }
        }
        if fed {
            self.dirty = true;
        }
        for sid in closed {
            if let Some((wi, true)) = self.close_session(sid) {
                self.close_window(wi);
            }
            self.sync_title();
        }

        let mut changed = false;
        for done in self.pipeline.drain() {
            let sid = done.meta.sid;
            if let Some(session) = self.session_mut(sid) {
                session.on_media_done(done);
            }
            changed = true;
        }
        if changed {
            self.dirty = true;
        }

        for session in self.sessions.iter_mut().flatten() {
            if session.media.advance(now) {
                self.dirty = true;
            }
        }
        self.update_blink(now);

        if self.quit || self.windows.is_empty() {
            self.shutdown_all();
            el.exit();
            return;
        }
        // Fade-in / grain / animated backgrounds must keep frames coming even
        // when the shell is idle — but at their own cadence, never by
        // re-requesting a redraw on every pass of the event loop (that busy
        // loop is what pegged a core at 99% with grain enabled).
        if self.needs_frames() && now >= self.next_anim {
            self.next_anim = now + self.anim_period();
            self.dirty = true;
        }
        if self.dirty {
            for window in &self.windows {
                window.window().request_redraw();
            }
            self.dirty = false;
        }
        self.schedule(el, now);
    }

    fn exiting(&mut self, _el: &ActiveEventLoop) {
        self.shutdown_all();
    }
}

/// Attributes for a new rterm window, tagged with a WM class that matches
/// `rterm.desktop` so launchers and the compositor can associate them.
fn window_attrs(title: &str, cfg: &Config) -> WindowAttributes {
    let attrs = WindowAttributes::default()
        .with_title(title)
        .with_transparent(true)
        .with_inner_size(winit::dpi::LogicalSize::new(
            cfg.window.width,
            cfg.window.height,
        ));
    wm_class(attrs)
}

#[cfg(all(unix, not(any(target_os = "macos", target_os = "ios"))))]
fn wm_class(attrs: WindowAttributes) -> WindowAttributes {
    use winit::platform::wayland::WindowAttributesExtWayland;
    use winit::platform::x11::WindowAttributesExtX11;
    if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        WindowAttributesExtWayland::with_name(attrs, "rterm", "rterm")
    } else {
        WindowAttributesExtX11::with_name(attrs, "rterm", "rterm")
    }
}

#[cfg(not(all(unix, not(any(target_os = "macos", target_os = "ios")))))]
fn wm_class(attrs: WindowAttributes) -> WindowAttributes {
    attrs
}

/// Grid geometry that matches what the emulator computes from the same window.
fn compute_layout(w: u32, h: u32, metrics: &CellMetrics, cfg: &Config) -> Layout {
    let mut layout = Layout::compute(w, h, metrics, cfg.ui.padding_x, cfg.ui.padding_y);
    // `SizeInfo` enforces these minimums; mirror them so both agree.
    layout.cols = layout.cols.max(2);
    layout.rows = layout.rows.max(1);
    layout.origin_x = ((w as f32 - layout.cols as f32 * layout.cell_w) / 2.0).round() as i32;
    layout.origin_y = ((h as f32 - layout.rows as f32 * layout.cell_h) / 2.0).round() as i32;
    layout
}

/// `SizeInfo` whose columns/rows are exactly `layout`'s.
fn term_size(w: u32, h: u32, layout: &Layout) -> SizeInfo {
    // Nudge the padding below center: `SizeInfo` truncates the column count,
    // so a slightly smaller padding can never lose a column (the epsilon stays
    // under half a cell, so it can never gain one either).
    let eps_x = (layout.cell_w * 0.25).min(0.5);
    let eps_y = (layout.cell_h * 0.25).min(0.5);
    let pad_x = (w as f32 - layout.cols as f32 * layout.cell_w) / 2.0 - eps_x;
    let pad_y = (h as f32 - layout.rows as f32 * layout.cell_h) / 2.0 - eps_y;
    SizeInfo::new(
        w as f32,
        h as f32,
        layout.cell_w,
        layout.cell_h,
        pad_x,
        pad_y,
        false,
    )
}

/// Terminal-side configuration derived from the rterm config.
fn term_config(cfg: &Config) -> TermConfig<()> {
    let mut term = TermConfig::<()>::default();
    term.scrolling
        .set_history(cfg.scrollback.lines.min(u32::MAX as usize) as u32);
    term.cursor.style = ConfigCursorStyle::Shape(match cfg.cursor.style.as_str() {
        "beam" => CursorShape::Beam,
        "underline" => CursorShape::Underline,
        _ => CursorShape::Block,
    });
    term.cursor.unfocused_hollow = cfg.cursor.unfocused_hollow;
    term
}

fn target(kind: alacritty_terminal::term::ClipboardType) -> Target {
    match kind {
        alacritty_terminal::term::ClipboardType::Clipboard => Target::Clipboard,
        alacritty_terminal::term::ClipboardType::Selection => Target::Selection,
    }
}

/// Does the item's cell rectangle (derived size included) cover this cell?
#[allow(dead_code)]
fn cell_rect_contains(item: &MediaItem, col: usize, row: usize, layout: &Layout) -> bool {
    let cols = if item.cols > 0 {
        item.cols
    } else {
        ((f64::from(item.px_w) / f64::from(layout.cell_w.max(1.0))).ceil() as usize).max(1)
    };
    let rows = if item.rows > 0 {
        item.rows
    } else {
        ((f64::from(item.px_h) / f64::from(layout.cell_h.max(1.0))).ceil() as usize).max(1)
    };
    col >= item.col && col < item.col + cols && row >= item.row && row < item.row + rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use alacritty_terminal::grid::Dimensions;

    /// Visibility is the only thing `ui.tab_bar` really controls; the cycle
    /// must visit every mode and fall back to the default on garbage.
    #[test]
    fn tabbar_visibility_follows_mode() {
        assert!(!tabbar_shows("auto", 1), "a lone tab stays hidden");
        assert!(tabbar_shows("auto", 2));
        assert!(tabbar_shows("on", 1), "explicit on shows even one tab");
        assert!(!tabbar_shows("off", 9));
        assert_eq!(next_tabbar_mode("auto"), "on");
        assert_eq!(next_tabbar_mode("on"), "off");
        assert_eq!(next_tabbar_mode("off"), "auto");
        assert_eq!(next_tabbar_mode("garbage"), "auto");
    }

    /// Drawn positions and click hit-tests share this layout, so chips must
    /// run left-to-right without overlapping and honour the label style.
    #[test]
    fn tab_chips_lay_out_left_to_right() {
        let cfg = Config::default();
        let mut fonts = FontSystem::new(&cfg.font, 1.0).expect("font system");
        let titles = |sid: usize| format!("term{sid}");
        let chips = tab_chips(&mut fonts, &[0, 1, 2], 10, "chip", false, titles);
        assert_eq!(chips.len(), 3);
        assert_eq!(chips[0].x, 10);
        assert!(chips[0].label.starts_with("1 "), "chip keeps the index");
        assert!(!chips[0].has_close(), "no ✕ when the button is off");
        for pair in chips.windows(2) {
            assert!(
                pair[1].x >= pair[0].x + pair[0].w,
                "chips must not overlap"
            );
        }
        let minimal = tab_chips(&mut fonts, &[0], 10, "minimal", false, titles);
        assert!(
            minimal[0].label.len() < chips[0].label.len(),
            "minimal drops the index"
        );

        // With the button on, the ✕ lives inside its own chip and the chips
        // still do not run into each other.
        let with_close = tab_chips(&mut fonts, &[0, 1], 10, "chip", true, titles);
        for chip in &with_close {
            assert!(chip.has_close());
            assert!(chip.close_x >= chip.x, "the ✕ starts after the label");
            assert!(
                chip.close_x + chip.close_w <= chip.x + chip.w,
                "the ✕ stays inside the chip"
            );
        }
        assert!(
            with_close[1].x >= with_close[0].x + with_close[0].w,
            "chips must not overlap"
        );
    }

    /// The emulator derives its grid from the `SizeInfo` we hand it, while the
    /// renderer works off `Layout`. They must agree exactly, otherwise clicks
    /// and highlights land on the wrong cells.
    #[test]
    fn term_size_matches_layout_grid() {
        let mut cfg = Config::default();
        let mut fonts = FontSystem::new(&cfg.font, 1.0).expect("font system");
        let sizes = [
            (320u32, 240u32),
            (640, 480),
            (800, 600),
            (1024, 768),
            (1920, 1080),
            (133, 77),
        ];

        for font_size in [12.0f32, 15.5, 28.0] {
            cfg.font.size = font_size;
            fonts.recompute_metrics(&cfg.font, 1.0);
            for (pad_x, pad_y) in [(0u32, 0u32), (8, 12), (40, 40)] {
                cfg.ui.padding_x = pad_x;
                cfg.ui.padding_y = pad_y;
                for (w, h) in sizes {
                    let layout = compute_layout(w, h, &fonts.metrics, &cfg);
                    let size = term_size(w, h, &layout);
                    assert_eq!(
                        size.columns(),
                        layout.cols,
                        "font={font_size} pad=({pad_x},{pad_y}) window={w}x{h}"
                    );
                    assert_eq!(
                        size.screen_lines(),
                        layout.rows,
                        "font={font_size} pad=({pad_x},{pad_y}) window={w}x{h}"
                    );
                    assert!(
                        layout.cols as f32 * layout.cell_w <= w as f32 + layout.cell_w,
                        "grid wider than window"
                    );
                }
            }
        }
    }

    #[test]
    fn grid_never_smaller_than_the_emulator_minimum() {
        let cfg = Config::default();
        let fonts = FontSystem::new(&cfg.font, 1.0).expect("font system");
        let layout = compute_layout(1, 1, &fonts.metrics, &cfg);
        assert!(layout.cols >= 2);
        assert!(layout.rows >= 1);
    }
}
