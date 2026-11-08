//! Shell process management via portable-pty.

use std::io::{Read, Write};
use std::path::PathBuf;

use anyhow::{Context, Result};
use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};

use crate::config::ShellCfg;

pub struct Pty {
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
}

pub struct Spawned {
    pub pty: Pty,
    pub reader: Box<dyn Read + Send>,
    pub killer: Box<dyn portable_pty::ChildKiller + Send + Sync>,
    /// PID of the shell, used to match `rterm play` clients to their pane.
    pub pid: Option<u32>,
}

pub fn spawn(
    shell: &ShellCfg,
    cols: u16,
    rows: u16,
    env: &[(String, String)],
    cwd: Option<PathBuf>,
) -> Result<Spawned> {
    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .context("openpty")?;

    // portable-pty panics on args() for new_default_prog(); resolve the
    // shell ourselves whenever extra argv is requested (e.g. --rcfile).
    let mut cmd = match (&shell.program, shell.args.is_empty()) {
        (Some(prog), _) => CommandBuilder::new(prog),
        (None, true) => CommandBuilder::new_default_prog(),
        (None, false) => {
            CommandBuilder::new(std::env::var("SHELL").unwrap_or_else(|_| "bash".to_string()))
        }
    };
    if !shell.args.is_empty() {
        cmd.args(&shell.args);
    }
    for (k, v) in env {
        cmd.env(k, v);
    }
    if let Some(dir) = cwd {
        cmd.cwd(dir);
    }

    let child = pair.slave.spawn_command(cmd).context("spawn shell")?;
    let reader = pair.master.try_clone_reader().context("pty reader")?;
    let writer = pair.master.take_writer().context("pty writer")?;
    let killer = child.clone_killer();
    let pid = child.process_id();
    drop(pair.slave);
    drop(child);

    Ok(Spawned {
        pty: Pty {
            master: pair.master,
            writer,
        },
        reader,
        killer,
        pid,
    })
}

impl Pty {
    pub fn resize(&mut self, cols: u16, rows: u16) -> Result<()> {
        self.master
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .context("pty resize")
    }

    pub fn write(&mut self, data: &[u8]) -> Result<()> {
        self.writer.write_all(data).context("pty write")?;
        self.writer.flush().ok();
        Ok(())
    }
}
