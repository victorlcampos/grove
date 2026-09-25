mod agents;
mod app;
#[cfg(test)]
mod demo;
mod discover;
mod du;
mod fmt;
mod git;
mod i18n;
mod layout;
mod model;
mod snapshot;
mod theme;
mod ui;
mod worker;

use std::io::{self, stdout};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

use clap::Parser;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{self, DisableMouseCapture, EnableMouseCapture};
use ratatui::crossterm::execute;
use ratatui::{DefaultTerminal, Terminal};

use crate::app::App;
use crate::discover::Finder;
use crate::i18n::Lang;
use crate::theme::Theme;
use crate::worker::{Msg, Workers};

/// Watches the git worktrees on this computer: which ones a Claude Code, Codex or OpenCode
/// session is using, how much disk each one takes, and removes them with the right git
/// command. Fits anything from a full screen to a small tiling pane.
#[derive(Parser)]
#[command(version, about)]
struct Cli {
    /// Folders to look for repositories in [default: ~/Workspace, ~/Projects, ~/src, ~/code and
    /// the like, the ones that exist]. Repositories where an agent runs are always listed
    #[arg(value_name = "FOLDER")]
    folders: Vec<PathBuf>,

    /// How many folder levels to go down looking for repositories
    #[arg(long, default_value_t = 3, value_name = "LEVELS")]
    depth: usize,

    /// Leave the mouse to the terminal, to select text
    #[arg(long)]
    no_mouse: bool,

    /// Interface language [default: from $LANG]
    #[arg(long, value_enum)]
    lang: Option<Lang>,

    /// Print one screen of this size (like 120x40) after scanning for a while, and exit
    #[arg(long, value_name = "WxH", value_parser = parse_size, hide = true)]
    snapshot: Option<(u16, u16)>,

    /// With --snapshot: seconds to scan before printing
    #[arg(long, default_value_t = 3.0, hide = true)]
    wait: f64,

    /// With --snapshot: also write the screen as an SVG picture
    #[arg(long, value_name = "FILE", hide = true)]
    svg: Option<PathBuf>,
}

fn parse_size(text: &str) -> Result<(u16, u16), String> {
    let (width, height) = text
        .split_once('x')
        .ok_or_else(|| format!("{text:?} is not like 120x40"))?;
    let number = |part: &str| {
        part.parse::<u16>()
            .map_err(|error| format!("{part:?}: {error}"))
    };
    Ok((number(width)?, number(height)?))
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    if !git::available() {
        eprintln!("grove: git was not found; install it and try again");
        return ExitCode::from(2);
    }
    let home = std::env::home_dir();
    let roots = if cli.folders.is_empty() {
        discover::default_roots(home.as_deref())
    } else {
        let mut roots = Vec::new();
        for folder in &cli.folders {
            match discover::canonical(folder) {
                Some(root) if root.is_dir() => roots.push(root),
                _ => {
                    eprintln!("grove: {} is not a folder", folder.display());
                    return ExitCode::from(2);
                }
            }
        }
        roots
    };
    let here = std::env::current_dir()
        .ok()
        .and_then(|dir| discover::canonical(&dir));
    let finder = Finder::new(roots.clone(), cli.depth, home.clone()).also(here);
    let (tx, rx) = mpsc::channel();
    let workers = Workers::start(finder, home.clone(), tx.clone());
    let lang = cli.lang.unwrap_or_else(Lang::from_env);
    let mut app = App::new(lang, Theme::detect(), home, roots, workers);

    if let Some((width, height)) = cli.snapshot {
        return snapshot(&mut app, &rx, (width, height), cli.wait, cli.svg.as_deref());
    }

    // Hand the mouse back to the terminal if grove panics; ratatui restores the rest.
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = execute!(stdout(), DisableMouseCapture);
        previous(info);
    }));
    let result = ratatui::run(|terminal| {
        let mouse = !cli.no_mouse && execute!(stdout(), EnableMouseCapture).is_ok();
        thread::Builder::new()
            .name("grove-input".into())
            .spawn(move || {
                while let Ok(event) = event::read() {
                    if tx.send(Msg::Input(event)).is_err() {
                        return;
                    }
                }
            })
            .expect("failed to start the input thread");
        let result = run(terminal, &mut app, &rx);
        if mouse {
            let _ = execute!(stdout(), DisableMouseCapture);
        }
        result
    });
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("grove: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(terminal: &mut DefaultTerminal, app: &mut App, rx: &Receiver<Msg>) -> io::Result<()> {
    loop {
        app.tick(Instant::now());
        terminal.draw(|frame| ui::render(frame, app))?;
        // Spinners turn about ten times a second; a still screen only needs its clocks.
        let wait = if app.animating() {
            Duration::from_millis(100)
        } else {
            Duration::from_secs(1)
        };
        match rx.recv_timeout(wait) {
            Ok(msg) => {
                app.handle(msg);
                while let Ok(msg) = rx.try_recv() {
                    app.handle(msg);
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return Ok(()),
        }
        if app.quit {
            return Ok(());
        }
    }
}

/// Scans for `wait` seconds and prints one screen, to check what grove sees without a
/// terminal.
fn snapshot(
    app: &mut App,
    rx: &Receiver<Msg>,
    (width, height): (u16, u16),
    wait: f64,
    svg: Option<&std::path::Path>,
) -> ExitCode {
    let deadline = Instant::now() + Duration::from_secs_f64(wait.max(0.0));
    while let Some(left) = deadline.checked_duration_since(Instant::now()) {
        app.tick(Instant::now());
        if let Ok(msg) = rx.recv_timeout(left.min(Duration::from_millis(100))) {
            app.handle(msg);
        }
    }
    app.tick(Instant::now());
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("a test terminal");
    terminal
        .draw(|frame| ui::render(frame, app))
        .expect("drawing on a test terminal");
    let buffer = terminal.backend().buffer();
    print!("{}", snapshot::text(buffer));
    if let Some(path) = svg
        && let Err(error) = std::fs::write(path, snapshot::svg(buffer))
    {
        eprintln!("grove: {}: {error}", path.display());
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}
