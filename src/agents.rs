//! Finds the Claude Code, Codex and OpenCode sessions running on this computer, and the other
//! processes working inside some folder.
//!
//! Every agent is found by its process and working folder. Claude Code also describes its
//! sessions in `~/.claude/sessions/<pid>.json` (name, folder, busy or idle) and its background
//! jobs in `~/.claude/jobs/<id>/state.json` (working, blocked or done, plus a line on what it
//! is doing), which grove reads for the details.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::Value;
use sysinfo::{
    Pid, ProcessRefreshKind, ProcessStatus, ProcessesToUpdate, Signal, System, UpdateKind,
};

use crate::discover::canonical;
use crate::model::{Activity, Agent, Proc, Running, Scan, Session};

/// CPU use, in percent of a core, above which a session without a reported status counts as
/// working.
const BUSY_CPU: f32 = 3.0;

/// How long a process asked to quit gets before it is killed.
const STOP_GRACE: Duration = Duration::from_secs(3);

/// `claude` subcommands that are tools around sessions, not sessions.
const CLAUDE_TOOLS: &[&str] = &[
    "agents",
    "auth",
    "config",
    "daemon",
    "doctor",
    "install",
    "mcp",
    "migrate-installer",
    "plugin",
    "setup-token",
    "update",
];

/// Agents run their tool commands through a shell. Anything else a session starts (an MCP
/// server, say) keeps the folder the session started in, which says nothing about where it
/// works.
fn is_shell(name: &str) -> bool {
    matches!(
        file_stem(name).trim_start_matches('-'),
        "sh" | "bash" | "zsh" | "fish" | "dash" | "ksh" | "tcsh" | "csh" | "nu" | "pwsh" | "cmd"
    )
}

/// A process as a scan sees it.
#[derive(Clone, Debug, Default)]
pub struct Info {
    pub pid: u32,
    pub parent: Option<u32>,
    pub name: String,
    pub exe: String,
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub cpu: f32,
    /// Seconds since the Unix epoch.
    pub start: u64,
}

/// A session Claude Code describes in its sessions folder.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ClaudeSession {
    pub pid: u32,
    pub cwd: Option<PathBuf>,
    pub name: Option<String>,
    pub background: bool,
    pub status: Option<String>,
    pub started: Option<SystemTime>,
    pub job: Option<Job>,
}

/// A Claude Code background job's own account of what it is doing.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Job {
    pub state: Option<String>,
    pub tempo: Option<String>,
    pub detail: Option<String>,
    pub needs: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Agent(Agent),
    /// Claude Code's daemon and terminal hosts: machinery, not sessions.
    Helper,
}

pub struct Scanner {
    system: System,
    claude_dir: Option<PathBuf>,
    me: u32,
}

impl Scanner {
    pub fn new(home: Option<&Path>) -> Self {
        let claude_dir = std::env::var_os("CLAUDE_CONFIG_DIR")
            .filter(|dir| !dir.is_empty())
            .map(PathBuf::from)
            .or_else(|| home.map(|home| home.join(".claude")));
        Self {
            system: System::new(),
            claude_dir,
            me: std::process::id(),
        }
    }

    pub fn scan(&mut self) -> Scan {
        let refresh = ProcessRefreshKind::nothing()
            .with_cpu()
            .with_cwd(UpdateKind::Always)
            .with_cmd(UpdateKind::OnlyIfNotSet)
            .with_exe(UpdateKind::OnlyIfNotSet)
            .without_tasks();
        self.system
            .refresh_processes_specifics(ProcessesToUpdate::All, true, refresh);
        let infos: Vec<Info> = self
            .system
            .processes()
            .values()
            .map(|process| Info {
                pid: process.pid().as_u32(),
                parent: process.parent().map(|pid| pid.as_u32()),
                name: process.name().to_string_lossy().into_owned(),
                exe: process
                    .exe()
                    .map(|exe| exe.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                args: process
                    .cmd()
                    .iter()
                    .map(|arg| arg.to_string_lossy().into_owned())
                    .collect(),
                cwd: process.cwd().map(Path::to_path_buf),
                cpu: process.cpu_usage(),
                start: process.start_time(),
            })
            .collect();
        let claude = self
            .claude_dir
            .as_deref()
            .map(read_claude_sessions)
            .unwrap_or_default();
        build(&infos, &claude, self.me)
    }
}

/// What a process is, from its name, executable and arguments.
pub fn role(info: &Info) -> Option<Role> {
    let tokens: Vec<&str> = info
        .args
        .iter()
        .flat_map(|arg| arg.split_whitespace())
        .collect();
    let first = tokens.first().map_or("", |token| file_stem(token));
    let named = |name: &str| file_stem(&info.exe) == name || info.name == name || first == name;
    if named("claude") || info.exe.contains("/claude/versions/") || run_by_script(&tokens, "claude")
    {
        let helper = tokens
            .iter()
            .any(|token| matches!(*token, "bg-pty-host" | "--bg-pty-host"))
            || tokens
                .get(1)
                .is_some_and(|command| CLAUDE_TOOLS.contains(command));
        return Some(if helper {
            Role::Helper
        } else {
            Role::Agent(Agent::Claude)
        });
    }
    for agent in [Agent::Codex, Agent::OpenCode] {
        if named(agent.name()) || run_by_script(&tokens, agent.name()) {
            return Some(Role::Agent(agent));
        }
    }
    None
}

/// `node /usr/local/bin/claude`: an agent installed as a script.
fn run_by_script(tokens: &[&str], name: &str) -> bool {
    let runtime = tokens.first().map(|token| file_stem(token));
    matches!(runtime, Some("node" | "bun" | "deno"))
        && tokens.get(1).is_some_and(|script| {
            file_stem(script) == name || (name == "claude" && script.contains("claude-code/cli"))
        })
}

fn file_stem(path: &str) -> &str {
    let base = path.rsplit(['/', '\\']).next().unwrap_or(path);
    base.strip_suffix(".exe").unwrap_or(base)
}

/// Claude Code keeps a few spare processes warm to start background sessions fast; one not
/// yet claimed by a session is machinery too.
fn unclaimed_spare(info: &Info) -> bool {
    info.args
        .iter()
        .flat_map(|arg| arg.split_whitespace())
        .any(|token| matches!(token, "bg-spare" | "--bg-spare"))
}

/// What a Claude Code session is doing, from its status and, for a background job, the job's
/// state and tempo.
pub fn claude_activity(status: Option<&str>, job: Option<&Job>) -> Activity {
    let state = job.and_then(|job| job.state.as_deref());
    let tempo = job.and_then(|job| job.tempo.as_deref());
    if status == Some("busy") {
        return Activity::Working;
    }
    let asks = |text: Option<&str>| {
        text.is_some_and(|text| {
            ["block", "wait", "permission", "input"]
                .iter()
                .any(|word| text.contains(word))
        })
    };
    if asks(tempo) || asks(state) || asks(status) {
        return Activity::Blocked;
    }
    if state == Some("working") && tempo == Some("active") {
        // Subagents and background tasks keep a job working while its main loop is idle.
        return Activity::Working;
    }
    Activity::Idle
}

/// Turns the process list into agent sessions and the processes working in some folder.
pub fn build(infos: &[Info], claude: &[ClaudeSession], me: u32) -> Scan {
    let by_pid: HashMap<u32, &Info> = infos.iter().map(|info| (info.pid, info)).collect();
    let roles: HashMap<u32, Role> = infos
        .iter()
        .filter_map(|info| role(info).map(|role| (info.pid, role)))
        .collect();
    let described: HashMap<u32, &ClaudeSession> = claude
        .iter()
        .filter(|session| {
            // A stale file may name a pid some other program has now.
            by_pid.get(&session.pid).is_some_and(|info| {
                roles.get(&info.pid) == Some(&Role::Agent(Agent::Claude))
                    || info.exe.to_lowercase().contains("claude")
            })
        })
        .map(|session| (session.pid, session))
        .collect();

    let mut session_pids: Vec<u32> = Vec::new();
    let mut machinery: HashSet<u32> = HashSet::new();
    for info in infos {
        if described.contains_key(&info.pid) {
            session_pids.push(info.pid);
            continue;
        }
        let agent = match roles.get(&info.pid) {
            Some(Role::Agent(agent)) => *agent,
            Some(Role::Helper) => {
                machinery.insert(info.pid);
                continue;
            }
            None => continue,
        };
        if agent == Agent::Claude && unclaimed_spare(info) {
            machinery.insert(info.pid);
            continue;
        }
        let nested =
            ancestors(info.pid, &by_pid).any(|pid| roles.get(&pid) == Some(&Role::Agent(agent)));
        if !nested {
            session_pids.push(info.pid);
        }
    }
    let session_set: HashSet<u32> = session_pids.iter().copied().collect();

    let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
    for info in infos {
        if let Some(parent) = info.parent {
            children.entry(parent).or_default().push(info.pid);
        }
    }

    let mut sessions: Vec<Session> = Vec::new();
    for pid in &session_pids {
        let info = by_pid[pid];
        let cpu = tree_cpu(*pid, &children, &by_pid, &session_set);
        let started = Some(UNIX_EPOCH + Duration::from_secs(info.start)).filter(|_| info.start > 0);
        let session = match described.get(pid) {
            Some(file) => {
                let activity = claude_activity(file.status.as_deref(), file.job.as_ref());
                let job = file.job.as_ref();
                let detail = match activity {
                    Activity::Blocked => {
                        job.and_then(|job| job.needs.clone().or(job.detail.clone()))
                    }
                    _ => job.and_then(|job| job.detail.clone()),
                };
                let cwd = file
                    .cwd
                    .as_deref()
                    .map(|cwd| canonical(cwd).unwrap_or_else(|| cwd.to_path_buf()))
                    .or_else(|| info.cwd.clone());
                let Some(cwd) = cwd else { continue };
                Session {
                    agent: Agent::Claude,
                    pid: *pid,
                    name: file.name.clone(),
                    detail,
                    background: file.background,
                    activity,
                    cpu,
                    started: file.started.or(started),
                    cwd,
                }
            }
            None => {
                let Some(Role::Agent(agent)) = roles.get(pid) else {
                    continue;
                };
                let Some(cwd) = info.cwd.clone() else {
                    continue;
                };
                Session {
                    agent: *agent,
                    pid: *pid,
                    name: None,
                    detail: None,
                    background: false,
                    activity: if cpu >= BUSY_CPU {
                        Activity::Working
                    } else {
                        Activity::Idle
                    },
                    cpu,
                    started,
                    cwd,
                }
            }
        };
        sessions.push(session);
    }
    sessions.sort_by_key(|session| (session.activity, session.agent, session.pid));

    let protected: Vec<u32> = std::iter::once(me).chain(ancestors(me, &by_pid)).collect();
    let mut procs = Vec::new();
    let mut running = Vec::new();
    for info in infos {
        let Some(cwd) = &info.cwd else { continue };
        if cwd.parent().is_none()
            || protected.contains(&info.pid)
            || ancestors(info.pid, &by_pid).any(|pid| pid == me)
        {
            continue;
        }
        running.push(Running {
            pid: info.pid,
            name: info.name.clone(),
            cwd: cwd.clone(),
        });
        if session_set.contains(&info.pid) || machinery.contains(&info.pid) {
            continue;
        }
        let chain: Vec<u32> = ancestors(info.pid, &by_pid).collect();
        let session = chain.iter().copied().find(|pid| session_set.contains(pid));
        if let Some(at) = chain.iter().position(|pid| Some(*pid) == session) {
            // The first process under the session that is not the agent itself decides.
            let below = chain[..at].iter().rev().copied().chain([info.pid]);
            let top = below
                .filter(|pid| !matches!(roles.get(pid), Some(Role::Agent(_))))
                .find_map(|pid| by_pid.get(&pid));
            if !top.is_some_and(|top| is_shell(&top.name)) {
                continue;
            }
        }
        procs.push(Proc {
            pid: info.pid,
            name: info.name.clone(),
            cwd: cwd.clone(),
            session,
        });
    }
    procs.sort_by_key(|proc| proc.pid);
    let mut alive: Vec<u32> = infos.iter().map(|info| info.pid).collect();
    alive.sort_unstable();
    running.sort_by_key(|process| process.pid);
    Scan {
        sessions,
        procs,
        running,
        alive,
        protected,
    }
}

/// Stops the processes still working inside `folder`: asks them to quit, and kills the ones
/// still there after a few seconds. A pid is checked again first, since the process seen in
/// the last scan may be gone and its number taken by another program. Returns how many it
/// stopped.
pub fn stop(pids: &[u32], folder: &Path) -> usize {
    if pids.is_empty() {
        return 0;
    }
    let pids: Vec<Pid> = pids.iter().map(|&pid| Pid::from_u32(pid)).collect();
    let mut system = System::new();
    let with_cwd = ProcessRefreshKind::nothing().with_cwd(UpdateKind::Always);
    system.refresh_processes_specifics(ProcessesToUpdate::Some(&pids), true, with_cwd);
    let targets: Vec<Pid> = pids
        .into_iter()
        .filter(|pid| {
            system
                .process(*pid)
                .and_then(|process| process.cwd())
                .is_some_and(|cwd| cwd.starts_with(folder))
        })
        .collect();
    for pid in &targets {
        if let Some(process) = system.process(*pid)
            && process.kill_with(Signal::Term).is_none()
        {
            process.kill();
        }
    }
    let deadline = Instant::now() + STOP_GRACE;
    loop {
        system.refresh_processes_specifics(
            ProcessesToUpdate::Some(&targets),
            true,
            ProcessRefreshKind::nothing(),
        );
        let running: Vec<&sysinfo::Process> = targets
            .iter()
            .filter_map(|pid| system.process(*pid))
            .filter(|process| process.status() != ProcessStatus::Zombie)
            .collect();
        if running.is_empty() {
            break;
        }
        if Instant::now() >= deadline {
            for process in running {
                process.kill();
            }
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    targets.len()
}

/// The parents of a process, nearest first.
fn ancestors<'a>(pid: u32, by_pid: &'a HashMap<u32, &'a Info>) -> impl Iterator<Item = u32> + 'a {
    let mut current = by_pid.get(&pid).and_then(|info| info.parent);
    let mut steps = 0;
    std::iter::from_fn(move || {
        let pid = current?;
        steps += 1;
        if steps > 64 || pid == 0 {
            return None;
        }
        current = by_pid.get(&pid).and_then(|info| info.parent);
        Some(pid)
    })
}

/// CPU of a session and its children, leaving out other sessions it started.
fn tree_cpu(
    pid: u32,
    children: &HashMap<u32, Vec<u32>>,
    by_pid: &HashMap<u32, &Info>,
    sessions: &HashSet<u32>,
) -> f32 {
    let mut total = 0.0;
    let mut stack = vec![pid];
    let mut seen = HashSet::new();
    while let Some(pid) = stack.pop() {
        if !seen.insert(pid) {
            continue;
        }
        total += by_pid.get(&pid).map_or(0.0, |info| info.cpu);
        for child in children.get(&pid).into_iter().flatten() {
            if !sessions.contains(child) {
                stack.push(*child);
            }
        }
    }
    total
}

fn read_claude_sessions(claude_dir: &Path) -> Vec<ClaudeSession> {
    let Ok(entries) = fs::read_dir(claude_dir.join("sessions")) else {
        return Vec::new();
    };
    let mut sessions = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|ext| ext != "json") {
            continue;
        }
        let Some((mut session, job_id)) =
            fs::read(&path).ok().and_then(|bytes| parse_session(&bytes))
        else {
            continue;
        };
        if let Some(job_id) = job_id {
            let state = claude_dir.join("jobs").join(job_id).join("state.json");
            session.job = fs::read(state).ok().and_then(|bytes| parse_job(&bytes));
        }
        sessions.push(session);
    }
    sessions
}

/// Reads `~/.claude/sessions/<pid>.json`, returning the session and its background job id.
pub fn parse_session(bytes: &[u8]) -> Option<(ClaudeSession, Option<String>)> {
    let value: Value = serde_json::from_slice(bytes).ok()?;
    let text = |key: &str| value.get(key).and_then(Value::as_str).map(str::to_string);
    let pid = u32::try_from(value.get("pid")?.as_u64()?).ok()?;
    let job_id = text("jobId").filter(|id| {
        !id.is_empty()
            && id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    });
    let session = ClaudeSession {
        pid,
        cwd: text("cwd").map(PathBuf::from),
        name: text("name").filter(|name| !name.is_empty()),
        background: text("kind").as_deref() == Some("bg"),
        status: text("status"),
        started: value
            .get("startedAt")
            .and_then(Value::as_u64)
            .map(|ms| UNIX_EPOCH + Duration::from_millis(ms)),
        job: None,
    };
    Some((session, job_id))
}

/// Reads `~/.claude/jobs/<id>/state.json`.
pub fn parse_job(bytes: &[u8]) -> Option<Job> {
    let value: Value = serde_json::from_slice(bytes).ok()?;
    let text = |key: &str| {
        value
            .get(key)
            .and_then(Value::as_str)
            .map(|text| text.split_whitespace().collect::<Vec<_>>().join(" "))
            .filter(|text| !text.is_empty())
    };
    Some(Job {
        state: text("state"),
        tempo: text("tempo"),
        detail: text("detail"),
        needs: text("needs"),
    })
}

/// A session or process seen in a worktree.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Usage {
    pub sessions: Vec<Presence>,
    pub procs: Vec<Proc>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Presence {
    pub session: Session,
    /// The child process through which a session works here, when the session itself runs
    /// in another folder.
    pub via: Option<(u32, String)>,
}

/// Places every session and process in the deepest worktree holding its folder.
pub fn attribute(scan: &Scan, worktrees: &[PathBuf]) -> HashMap<PathBuf, Usage> {
    let owner = |path: &Path| {
        worktrees
            .iter()
            .filter(|worktree| path.starts_with(worktree))
            .max_by_key(|worktree| worktree.as_os_str().len())
    };
    let mut usage: HashMap<PathBuf, Usage> = HashMap::new();
    let mut session_home: HashMap<u32, Option<&PathBuf>> = HashMap::new();
    for session in &scan.sessions {
        let home = owner(&session.cwd);
        session_home.insert(session.pid, home);
        if let Some(worktree) = home {
            usage
                .entry(worktree.clone())
                .or_default()
                .sessions
                .push(Presence {
                    session: session.clone(),
                    via: None,
                });
        }
    }
    for proc in &scan.procs {
        let Some(worktree) = owner(&proc.cwd) else {
            continue;
        };
        let Some(pid) = proc.session else {
            usage
                .entry(worktree.clone())
                .or_default()
                .procs
                .push(proc.clone());
            continue;
        };
        if session_home.get(&pid).copied().flatten() == Some(worktree) {
            continue;
        }
        let Some(session) = scan.sessions.iter().find(|session| session.pid == pid) else {
            continue;
        };
        let entry = usage.entry(worktree.clone()).or_default();
        if !entry
            .sessions
            .iter()
            .any(|presence| presence.session.pid == pid)
        {
            entry.sessions.push(Presence {
                session: session.clone(),
                via: Some((proc.pid, proc.name.clone())),
            });
        }
    }
    for entry in usage.values_mut() {
        entry
            .sessions
            .sort_by_key(|p| (p.session.activity, p.session.agent, p.session.pid));
    }
    usage
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(pid: u32, parent: u32, name: &str, args: &[&str], cwd: &str) -> Info {
        Info {
            pid,
            parent: Some(parent),
            name: name.into(),
            exe: String::new(),
            args: args.iter().map(|arg| arg.to_string()).collect(),
            cwd: (!cwd.is_empty()).then(|| PathBuf::from(cwd)),
            cpu: 0.0,
            start: 1_790_000_000,
        }
    }

    #[test]
    fn tells_agents_from_their_machinery() {
        let role_of = |name: &str, exe: &str, args: &[&str]| {
            role(&Info {
                name: name.into(),
                exe: exe.into(),
                args: args.iter().map(|arg| arg.to_string()).collect(),
                ..Info::default()
            })
        };
        let claude = Some(Role::Agent(Agent::Claude));
        assert_eq!(
            role_of(
                "claude",
                "/Users/me/.local/bin/claude",
                &["claude", "--resume", "x"]
            ),
            claude
        );
        assert_eq!(
            role_of(
                "2.1.282",
                "/Users/me/.local/share/claude/versions/2.1.282",
                &[]
            ),
            claude
        );
        assert_eq!(
            role_of("claude", "", &["claude bg-spare --bg-spare /tmp/s.sock"]),
            claude
        );
        assert_eq!(
            role_of(
                "claude",
                "",
                &["claude bg-pty-host --bg-pty-host /tmp/p.sock"]
            ),
            Some(Role::Helper)
        );
        assert_eq!(
            role_of("claude", "", &["claude", "daemon", "run"]),
            Some(Role::Helper)
        );
        assert_eq!(
            role_of("claude", "", &["claude", "agents", "--cwd", "."]),
            Some(Role::Helper)
        );
        assert_eq!(role_of("claude", "", &["claude", "-p", "agents"]), claude);
        assert_eq!(
            role_of(
                "node",
                "/usr/bin/node",
                &[
                    "node",
                    "/usr/lib/node_modules/@anthropic-ai/claude-code/cli.js"
                ]
            ),
            claude
        );
        assert_eq!(
            role_of("codex", "/Users/me/.local/bin/codex", &["codex"]),
            Some(Role::Agent(Agent::Codex))
        );
        assert_eq!(
            role_of("node", "/usr/bin/node", &["node", "/usr/local/bin/codex"]),
            Some(Role::Agent(Agent::Codex))
        );
        assert_eq!(
            role_of("opencode", "/Users/me/.opencode/bin/opencode", &[]),
            Some(Role::Agent(Agent::OpenCode))
        );
        assert_eq!(
            role_of(
                "Claude",
                "/Applications/Claude.app/Contents/MacOS/Claude",
                &[]
            ),
            None
        );
        assert_eq!(
            role_of(
                "chrome-native-host",
                "/Applications/Claude.app/Contents/Helpers/chrome-native-host",
                &[]
            ),
            None
        );
        assert_eq!(
            role_of("codex-linux-sandbox", "/x/codex-linux-sandbox", &[]),
            None
        );
        assert_eq!(role_of("zsh", "/bin/zsh", &["-zsh"]), None);
    }

    #[test]
    fn claude_jobs_tell_working_from_waiting() {
        let job = |state: &str, tempo: &str| Job {
            state: Some(state.into()),
            tempo: Some(tempo.into()),
            ..Job::default()
        };
        assert_eq!(claude_activity(Some("busy"), None), Activity::Working);
        assert_eq!(claude_activity(Some("idle"), None), Activity::Idle);
        assert_eq!(
            claude_activity(Some("idle"), Some(&job("blocked", "blocked"))),
            Activity::Blocked
        );
        assert_eq!(
            claude_activity(Some("idle"), Some(&job("working", "blocked"))),
            Activity::Blocked
        );
        assert_eq!(
            claude_activity(Some("idle"), Some(&job("working", "active"))),
            Activity::Working
        );
        assert_eq!(
            claude_activity(Some("idle"), Some(&job("working", "idle"))),
            Activity::Idle
        );
        assert_eq!(
            claude_activity(Some("idle"), Some(&job("done", "idle"))),
            Activity::Idle
        );
        assert_eq!(
            claude_activity(Some("busy"), Some(&job("blocked", "blocked"))),
            Activity::Working
        );
    }

    #[test]
    fn reads_claude_session_and_job_files() {
        let (session, job) = parse_session(
            br#"{"pid":4242,"sessionId":"0f3a9c21","cwd":"/code/app/.claude/worktrees/x","kind":"bg","name":"coupon rules","status":"busy","jobId":"0f3a9c21","startedAt":1790349305885}"#,
        )
        .unwrap();
        assert_eq!(session.pid, 4242);
        assert_eq!(
            session.cwd.as_deref(),
            Some(Path::new("/code/app/.claude/worktrees/x"))
        );
        assert!(session.background);
        assert_eq!(session.name.as_deref(), Some("coupon rules"));
        assert_eq!(job.as_deref(), Some("0f3a9c21"));
        let (_, job) = parse_session(br#"{"pid":1,"jobId":"../../etc"}"#).unwrap();
        assert_eq!(job, None, "a job id never walks out of the jobs folder");
        assert!(parse_session(br#"{"cwd":"/x"}"#).is_none());

        let job = parse_job(br#"{"state":"blocked","tempo":"blocked","detail":"design decision\n needed","needs":"reply A or B","fan":[]}"#).unwrap();
        assert_eq!(job.state.as_deref(), Some("blocked"));
        assert_eq!(job.detail.as_deref(), Some("design decision needed"));
        assert_eq!(job.needs.as_deref(), Some("reply A or B"));
    }

    /// Claude Code's background machinery, an interactive Claude in a terminal, a Codex
    /// started through node, and the processes around them.
    fn system() -> (Vec<Info>, Vec<ClaudeSession>) {
        let mut infos = vec![
            info(1, 0, "launchd", &[], "/"),
            info(100, 1, "claude", &["claude", "daemon", "run"], "/Users/me"),
            info(
                110,
                100,
                "claude",
                &["claude bg-pty-host --bg-pty-host /tmp/a.sock"],
                "/code/app",
            ),
            info(
                111,
                110,
                "claude",
                &["claude bg-spare --bg-spare /tmp/a.claim.sock"],
                "/code/app/.claude/worktrees/x",
            ),
            info(
                112,
                111,
                "zsh",
                &["/bin/zsh"],
                "/code/app/.claude/worktrees/x",
            ),
            info(
                113,
                112,
                "ruby",
                &["ruby", "bin/rails", "test"],
                "/code/app/.claude/worktrees/y",
            ),
            // MCP servers keep the folder the session started in.
            info(114, 111, "mcp-server", &["/x/mcp-server"], "/code/app"),
            info(
                115,
                111,
                "node",
                &["node", "chrome-devtools-mcp"],
                "/code/app",
            ),
            info(116, 115, "Google Chrome", &[], "/code/app"),
            info(
                130,
                200,
                "claude",
                &["claude", "agents", "--cwd", "."],
                "/code/app",
            ),
            info(
                120,
                100,
                "claude",
                &["claude bg-pty-host --bg-pty-host /tmp/b.sock"],
                "/code/app",
            ),
            info(
                121,
                120,
                "claude",
                &["claude bg-spare --bg-spare /tmp/b.claim.sock"],
                "/tmp/spare",
            ),
            info(200, 1, "fish", &["fish"], "/code/app"),
            info(201, 200, "claude", &["claude"], "/code/app"),
            info(
                300,
                1,
                "node",
                &["node", "/usr/local/bin/codex"],
                "/code/app/.claude/worktrees/y",
            ),
            info(
                301,
                300,
                "codex",
                &["/x/codex"],
                "/code/app/.claude/worktrees/y",
            ),
            info(
                302,
                301,
                "cargo",
                &["cargo", "build"],
                "/code/app/.claude/worktrees/y",
            ),
            info(400, 1, "grove", &["grove"], "/code/app/.claude/worktrees/x"),
            info(
                401,
                400,
                "git",
                &["git", "status"],
                "/code/app/.claude/worktrees/z",
            ),
            info(
                500,
                1,
                "nvim",
                &["nvim"],
                "/code/app/.claude/worktrees/z/src",
            ),
        ];
        infos.iter_mut().find(|i| i.pid == 302).unwrap().cpu = 95.0;
        let claude = vec![
            ClaudeSession {
                pid: 111,
                cwd: Some("/code/app/.claude/worktrees/x".into()),
                name: Some("bug hunt".into()),
                background: true,
                status: Some("idle".into()),
                job: Some(Job {
                    state: Some("blocked".into()),
                    tempo: Some("blocked".into()),
                    detail: Some("hunting".into()),
                    needs: Some("reply A or B".into()),
                }),
                ..ClaudeSession::default()
            },
            ClaudeSession {
                pid: 201,
                cwd: Some("/code/app".into()),
                name: Some("terminal".into()),
                status: Some("busy".into()),
                ..ClaudeSession::default()
            },
            // Left behind by a session that ended; the pid is some shell now.
            ClaudeSession {
                pid: 200,
                cwd: Some("/code/app".into()),
                ..ClaudeSession::default()
            },
        ];
        (infos, claude)
    }

    #[test]
    fn finds_sessions_and_the_processes_around_them() {
        let (infos, claude) = system();
        let scan = build(&infos, &claude, 400);
        let sessions: Vec<(u32, Agent, Activity)> = scan
            .sessions
            .iter()
            .map(|s| (s.pid, s.agent, s.activity))
            .collect();
        // Codex is its node wrapper, with the native binary nested in it.
        assert_eq!(
            sessions,
            [
                (111, Agent::Claude, Activity::Blocked),
                (201, Agent::Claude, Activity::Working),
                (300, Agent::Codex, Activity::Working),
            ]
        );
        let blocked = &scan.sessions[0];
        assert_eq!(blocked.detail.as_deref(), Some("reply A or B"));
        assert!(blocked.background);
        assert!(
            scan.sessions[2].cpu >= 95.0,
            "codex counts its cargo grandchild"
        );

        let procs: Vec<(u32, Option<u32>)> =
            scan.procs.iter().map(|p| (p.pid, p.session)).collect();
        // Left out: the daemon, pty hosts, idle spare, `claude agents`, the sessions, what
        // sessions start outside a shell (MCP servers and the Chrome one opened), and
        // grove with its own git.
        assert_eq!(
            procs,
            [(112, Some(111)), (113, Some(111)), (200, None), (500, None)]
        );
        assert!(scan.alive.binary_search(&116).is_ok());
        assert!(scan.alive.binary_search(&999).is_err());
        assert_eq!(scan.protected, [400, 1], "grove and what it runs under");
    }

    #[cfg(unix)]
    #[test]
    fn stops_only_what_still_works_in_the_folder() {
        use crate::git::tests::Scratch;
        let inside = Scratch::new("stop-inside");
        let outside = Scratch::new("stop-outside");
        let sleep = |dir: &Path| {
            std::process::Command::new("sleep")
                .arg("30")
                .current_dir(dir)
                .spawn()
                .unwrap()
        };
        let mut target = sleep(&inside.0);
        let mut bystander = sleep(&outside.0);
        // The bystander's pid comes too, as from a stale scan: its folder spares it.
        assert_eq!(stop(&[target.id(), bystander.id()], &inside.0), 1);
        assert!(!target.wait().unwrap().success(), "stopped by a signal");
        assert!(bystander.try_wait().unwrap().is_none(), "still running");
        bystander.kill().unwrap();
        bystander.wait().unwrap();
        assert_eq!(stop(&[], &inside.0), 0);
    }

    #[test]
    fn places_each_session_in_the_deepest_worktree() {
        let (infos, claude) = system();
        let scan = build(&infos, &claude, 400);
        let worktrees: Vec<PathBuf> = [
            "/code/app",
            "/code/app/.claude/worktrees/x",
            "/code/app/.claude/worktrees/y",
            "/code/app/.claude/worktrees/z",
        ]
        .iter()
        .map(PathBuf::from)
        .collect();
        let usage = attribute(&scan, &worktrees);
        let sessions = |worktree: &str| -> Vec<(u32, Option<u32>)> {
            usage
                .get(Path::new(worktree))
                .map(|u| {
                    u.sessions
                        .iter()
                        .map(|p| (p.session.pid, p.via.as_ref().map(|v| v.0)))
                        .collect()
                })
                .unwrap_or_default()
        };
        let procs = |worktree: &str| -> Vec<u32> {
            usage
                .get(Path::new(worktree))
                .map(|u| u.procs.iter().map(|p| p.pid).collect())
                .unwrap_or_default()
        };
        assert_eq!(sessions("/code/app"), [(201, None)]);
        assert_eq!(procs("/code/app"), [200]);
        assert_eq!(sessions("/code/app/.claude/worktrees/x"), [(111, None)]);
        assert_eq!(procs("/code/app/.claude/worktrees/x"), Vec::<u32>::new());
        // The x session runs tests in y through its ruby child.
        assert_eq!(
            sessions("/code/app/.claude/worktrees/y"),
            [(111, Some(113)), (300, None)]
        );
        assert_eq!(procs("/code/app/.claude/worktrees/y"), Vec::<u32>::new());
        assert_eq!(
            sessions("/code/app/.claude/worktrees/z"),
            Vec::<(u32, Option<u32>)>::new()
        );
        assert_eq!(procs("/code/app/.claude/worktrees/z"), [500]);

        // A removal stops everything working inside, the agents' own helpers included, but
        // never grove, its git, or the terminal it runs in.
        let inside = |worktree: &str| scan.inside(Path::new(worktree));
        assert_eq!(inside("/code/app/.claude/worktrees/x"), [111, 112]);
        assert_eq!(
            inside("/code/app/.claude/worktrees/y"),
            [113, 300, 301, 302]
        );
        assert_eq!(inside("/code/app/.claude/worktrees/z"), [500]);
    }
}
