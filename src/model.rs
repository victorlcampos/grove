//! What grove knows about repositories, their worktrees and the agent sessions using them.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// A coding agent grove recognizes by its process.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Agent {
    Claude,
    Codex,
    OpenCode,
}

impl Agent {
    pub const ALL: [Agent; 3] = [Agent::Claude, Agent::Codex, Agent::OpenCode];

    pub fn name(self) -> &'static str {
        match self {
            Agent::Claude => "claude",
            Agent::Codex => "codex",
            Agent::OpenCode => "opencode",
        }
    }
}

/// What an agent session is doing, most urgent first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Activity {
    /// Waiting for an answer from you.
    Blocked,
    Working,
    Idle,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Session {
    pub agent: Agent,
    pub pid: u32,
    pub name: Option<String>,
    /// What the session says it is doing, or what it needs from you.
    pub detail: Option<String>,
    pub background: bool,
    pub activity: Activity,
    /// CPU use of the session and its child processes, in percent of a core.
    pub cpu: f32,
    pub started: Option<SystemTime>,
    pub cwd: PathBuf,
}

/// A process that is not an agent session but has its working directory in some folder.
#[derive(Clone, Debug, PartialEq)]
pub struct Proc {
    pub pid: u32,
    pub name: String,
    pub cwd: PathBuf,
    /// The agent session this process descends from.
    pub session: Option<u32>,
}

/// Any process working in some folder, helpers of agent sessions included.
#[derive(Clone, Debug, PartialEq)]
pub struct Running {
    pub pid: u32,
    pub name: String,
    pub cwd: PathBuf,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Scan {
    pub sessions: Vec<Session>,
    /// The processes to show: those an agent session starts on its own (MCP servers) stay
    /// out, since their folder says nothing about where the session works.
    pub procs: Vec<Proc>,
    /// Every process with a folder but grove itself and the processes around it: what a
    /// removal stops.
    pub running: Vec<Running>,
    /// Every running pid, sorted.
    pub alive: Vec<u32>,
    /// grove and the processes it runs under (its terminal's shell, say), which a removal
    /// never stops.
    pub protected: Vec<u32>,
}

impl Scan {
    pub fn is_alive(&self, pid: u32) -> bool {
        self.alive.binary_search(&pid).is_ok()
    }

    /// The agent sessions and processes working inside `folder`, which a removal stops.
    pub fn inside(&self, folder: &Path) -> Vec<u32> {
        let mut pids: Vec<u32> = self
            .sessions
            .iter()
            .map(|session| (session.pid, &session.cwd))
            .chain(
                self.running
                    .iter()
                    .map(|process| (process.pid, &process.cwd)),
            )
            .filter(|(pid, cwd)| cwd.starts_with(folder) && !self.protected.contains(pid))
            .map(|(pid, _)| pid)
            .collect();
        pids.sort_unstable();
        pids.dedup();
        pids
    }
}

/// The pid Claude Code writes in the reason of the lock it puts on a worktree it works in:
/// "claude session x (pid 54795 start …)".
pub fn lock_holder(reason: &str) -> Option<u32> {
    let rest = &reason[reason.find("pid ")? + 4..];
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Worktree {
    /// The path git knows the worktree by.
    pub path: PathBuf,
    /// The same path with symlinks resolved, to compare with process folders.
    pub real: PathBuf,
    pub head: Option<String>,
    pub branch: Option<String>,
    pub detached: bool,
    pub locked: Option<String>,
    /// Set when the folder is gone and git only keeps its record.
    pub prunable: Option<String>,
    pub main: bool,
    /// Last time git touched the worktree's index or HEAD.
    pub touched: Option<SystemTime>,
}

impl Worktree {
    pub fn missing(&self) -> bool {
        self.prunable.is_some()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Repo {
    pub name: String,
    pub common_dir: PathBuf,
    pub bare: bool,
    pub worktrees: Vec<Worktree>,
}

impl Repo {
    /// Where git commands for this repository run: the main worktree when it is there, else
    /// the git directory itself.
    pub fn command_dir(&self) -> &Path {
        self.worktrees
            .iter()
            .find(|w| w.main && !w.missing())
            .map_or(&self.common_dir, |w| &w.path)
    }

    pub fn linked(&self) -> usize {
        self.worktrees.iter().filter(|w| !w.main).count()
    }
}

/// The name a worktree is shown by: its folder, or the repository's name for the main one.
pub fn worktree_name<'a>(repo: &'a Repo, worktree: &'a Worktree) -> &'a str {
    if worktree.main {
        return &repo.name;
    }
    worktree
        .path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_else(|| worktree.path.to_str().unwrap_or("?"))
}
