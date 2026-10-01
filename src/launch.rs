//! Picks a conversation up again in Claude Code, beside grove: in a new tab of herdr or a new
//! window of tmux, whichever grove runs in, so a routine's run is answered in the terminal.

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::thread;

use serde_json::Value;

/// How long herdr waits for Claude Code to be ready in the new tab.
const READY_MS: &str = "90000";

/// A conversation to resume, named after what it is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Launch {
    /// The tab's name.
    pub name: String,
    /// The agent's name in herdr: a short word, like the routine's id.
    pub agent: String,
    pub dir: Option<PathBuf>,
    pub conversation: String,
}

/// Where it opened.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Place {
    Herdr,
    Tmux,
}

/// Where launches go: the terminal grove runs in, or a list tests read.
#[derive(Debug)]
pub enum Launcher {
    System,
    #[cfg(test)]
    Memory(Vec<Launch>),
}

impl Launcher {
    /// Opens Claude Code on the conversation; an error when grove runs in neither herdr nor
    /// tmux, or they turned it down.
    pub fn launch(&mut self, job: &Launch) -> Result<Place, String> {
        match self {
            Launcher::System => {
                if set("HERDR_PANE_ID") || set("HERDR_ENV") {
                    herdr(job).map(|()| Place::Herdr)
                } else if set("TMUX") {
                    tmux(job).map(|()| Place::Tmux)
                } else {
                    Err(String::new())
                }
            }
            #[cfg(test)]
            Launcher::Memory(launched) => {
                launched.push(job.clone());
                Ok(Place::Herdr)
            }
        }
    }

    #[cfg(test)]
    pub fn last(&self) -> Option<&Launch> {
        match self {
            Launcher::System => None,
            Launcher::Memory(launched) => launched.last(),
        }
    }
}

fn set(name: &str) -> bool {
    std::env::var_os(name).is_some_and(|value| !value.is_empty())
}

/// A tab named after it, in its folder, where herdr starts Claude Code as an agent, so herdr
/// shows what it does like any other.
fn herdr(job: &Launch) -> Result<(), String> {
    let bin = std::env::var_os("HERDR_BIN_PATH")
        .filter(|bin| !bin.is_empty())
        .unwrap_or_else(|| "herdr".into());
    let mut create = Command::new(&bin);
    create.args(["tab", "create", "--focus", "--label", &job.name]);
    if let Some(workspace) = std::env::var_os("HERDR_WORKSPACE_ID").filter(|w| !w.is_empty()) {
        create.arg("--workspace").arg(workspace);
    }
    if let Some(dir) = &job.dir {
        create.arg("--cwd").arg(dir);
    }
    let out = create
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .map_err(|error| format!("herdr: {error}"))?;
    let json: Value = serde_json::from_slice(&out.stdout).unwrap_or_default();
    let pane = json["result"]["root_pane"]["pane_id"]
        .as_str()
        .ok_or_else(|| first_line(&out.stdout, "herdr tab create"))?
        .to_string();
    let mut start = Command::new(&bin);
    start
        .args([
            "agent", "start", &job.agent, "--kind", "claude", "--pane", &pane,
        ])
        .args(["--timeout", READY_MS, "--", "--resume", &job.conversation])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // herdr answers once Claude Code is ready, seconds later: grove does not wait for it.
    let mut child = start.spawn().map_err(|error| format!("herdr: {error}"))?;
    thread::spawn(move || child.wait());
    Ok(())
}

fn tmux(job: &Launch) -> Result<(), String> {
    let mut window = Command::new("tmux");
    window.args(["new-window", "-n", &job.name]);
    if let Some(dir) = &job.dir {
        window.arg("-c").arg(dir);
    }
    let status = window
        .args(["claude", "--resume", &job.conversation])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|error| format!("tmux: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("tmux {status}"))
    }
}

fn first_line(bytes: &[u8], what: &str) -> String {
    let text = String::from_utf8_lossy(bytes);
    let line = text.lines().next().unwrap_or_default().trim();
    if line.is_empty() {
        format!("{what} failed")
    } else {
        format!("{what}: {line}")
    }
}
