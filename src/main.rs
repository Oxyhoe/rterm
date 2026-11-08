mod anim;
mod app;
mod clipboard;
mod config;
mod font;
mod image_decode;
mod input;
mod intercept;
mod iterm;
mod kitty;
mod media;
mod palette;
mod pane;
mod play;
mod presets;
mod profiles;
mod prompt;
mod protocol;
mod pty;
mod render;
mod session;
mod settings;
mod socket;
mod suggest;
mod theme;

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{bail, Context, Result};

use crate::config::Config;

const HELP: &str = "\
rterm — a media-friendly terminal emulator

Usage:
  rterm [options]                 start the terminal
  rterm config [--init] [--edit] [--path <file>]  manage the config file
  rterm play [options] <file>     show images, GIFs and video inside the terminal

Options:
  -e, --command <cmd...>    run <cmd> instead of $SHELL (rest of the args)
  -d, --working-dir <dir>   start in <dir>
  -W, --cols <n>            video width in pane cells (default: fit the pane)
  -H, --rows <n>            video height in pane cells
  --fps <n>                 video frame rate cap, default 30 (socket) / 15 (PTY)
  --size <WxH>              explicit pixel size (PTY video fallback)
  -l, --loop                loop the video
  -h, --help                print this help
  -V, --version             print the version

Config:
  --init                    write the default config (unless it exists) and
                            print the config path
  --edit                    open the settings editor in a window (Enter saves
                            and quits, Esc quits without saving)
  --path <file>             use <file> instead of the default config path
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("rterm: {err:#}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: &[String]) -> Result<()> {
    if args.iter().any(|arg| arg == "-h" || arg == "--help") {
        print!("{HELP}");
        return Ok(());
    }
    if args.iter().any(|arg| arg == "-V" || arg == "--version") {
        println!("rterm {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }

    let mut command: Option<Vec<String>> = None;
    let mut dir: Option<PathBuf> = None;
    let mut settings = false;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "config" => return config_cmd(&args[i + 1..]),
            "play" => return play::run(play::Options::parse(&args[i + 1..])?),
            "--settings" => {
                settings = true;
                i += 1;
            }
            "-e" | "--command" => {
                i += 1;
                if i >= args.len() {
                    bail!("-e requires a command to run");
                }
                command = Some(args[i..].to_vec());
                break;
            }
            "-d" | "--working-dir" => {
                i += 1;
                let value = args
                    .get(i)
                    .ok_or_else(|| anyhow::anyhow!("-d requires a directory"))?;
                dir = Some(PathBuf::from(value));
                i += 1;
            }
            other => bail!("unknown argument: {other} (try --help)"),
        }
    }

    let (mut cfg, cfg_path) = Config::load();
    if let Some(dir) = dir {
        std::env::set_current_dir(&dir)
            .with_context(|| format!("cannot enter {}", dir.display()))?;
    }
    if let Some(cmd) = command {
        cfg.shell.program = Some(cmd[0].clone());
        cfg.shell.args = cmd[1..].to_vec();
    }

    let mode = if settings {
        app::Mode::Settings
    } else {
        app::Mode::Terminal
    };
    app::run(cfg, cfg_path, mode)
}

fn config_cmd(args: &[String]) -> Result<()> {
    let mut init = false;
    let mut edit = false;
    let mut path: Option<PathBuf> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--init" => {
                init = true;
                i += 1;
            }
            "--edit" => {
                edit = true;
                i += 1;
            }
            "--path" => {
                i += 1;
                let value = args
                    .get(i)
                    .ok_or_else(|| anyhow::anyhow!("--path requires a file"))?;
                path = Some(PathBuf::from(value));
                i += 1;
            }
            other => bail!("unknown config argument: {other} (try --help)"),
        }
    }

    let path = path.unwrap_or_else(Config::config_path);
    if edit {
        let cfg = Config::load_from(&path)
            .with_context(|| format!("reading {}", path.display()))?
            .unwrap_or_default();
        return app::run(cfg, Some(path), app::Mode::Editor);
    }
    if init {
        if path.exists() {
            eprintln!("rterm: {} already exists", path.display());
            return Ok(());
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("cannot create {}", parent.display()))?;
        }
        Config::write_default(&path)?;
    }
    println!("{}", path.display());
    Ok(())
}
