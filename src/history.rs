//! The conversations Claude Code, Codex and OpenCode keep on disk, which `claude --resume`,
//! `codex resume` and `opencode --session` pick up again, and the command that does it.
//!
//! Claude Code writes a transcript per conversation, `~/.claude/projects/<folder>/<id>.jsonl`,
//! and appends its title, name and last prompt again as it goes, so the last 64 kB of a
//! transcript tell all grove needs however long it grew. Codex writes
//! `~/.codex/sessions/<year>/<month>/<day>/rollout-<time>-<id>.jsonl`, which starts with a line
//! about the session, and keeps the names given to them in `~/.codex/session_index.jsonl`.
//! OpenCode keeps a database, which grove asks OpenCode itself to query. A transcript is read
//! again only when it changes.

use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::Value;

use crate::discover;
use crate::fmt;
use crate::model::{Agent, Conversation, Session};

/// How much of the end of a Claude Code transcript is read.
const TAIL: u64 = 64 * 1024;
/// How much of the start of a transcript is read, at most.
const HEAD: u64 = 256 * 1024;
/// Enough of a Codex rollout for its first line, about the session.
const FIRST_LINE: u64 = 64 * 1024;
/// Prompts and titles are kept to this many characters.
const TEXT: usize = 500;
/// OpenCode is asked again at most this often, and only when its database changed.
const OPENCODE_EVERY: Duration = Duration::from_secs(30);
const OPENCODE_TIMEOUT: Duration = Duration::from_secs(15);
const OPENCODE_QUERY: &str = "select id, title, directory, time_created, time_updated \
from session where parent_id is null and time_archived is null";

/// The file's modification time and length, which say whether it changed.
type Stamp = (Option<SystemTime>, u64);

pub struct History {
    claude_dir: Option<PathBuf>,
    codex_dir: Option<PathBuf>,
    opencode_dir: Option<PathBuf>,
    /// What each transcript said at the time it had its stamp.
    read: HashMap<PathBuf, (Stamp, Option<Conversation>)>,
    /// Where each folder conversations worked in belonged, last time it was there.
    places: HashMap<PathBuf, discover::Place>,
    codex_names: (Option<SystemTime>, HashMap<String, String>),
    opencode: OpenCode,
}

#[derive(Default)]
struct OpenCode {
    asked: Option<Instant>,
    stamp: Vec<Option<SystemTime>>,
    found: Vec<Conversation>,
    /// Where the answer to a question still out comes: OpenCode takes a moment to start.
    answer: Option<Receiver<Result<Vec<Conversation>, String>>>,
}

impl History {
    pub fn new(home: Option<&Path>) -> Self {
        let env = |name: &str| {
            std::env::var_os(name)
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
        };
        let under_home = |path: &str| home.map(|home| home.join(path));
        Self::with_dirs(
            env("CLAUDE_CONFIG_DIR").or_else(|| under_home(".claude")),
            env("CODEX_HOME").or_else(|| under_home(".codex")),
            env("XDG_DATA_HOME")
                .map(|dir| dir.join("opencode"))
                .or_else(|| under_home(".local/share/opencode")),
        )
    }

    fn with_dirs(
        claude_dir: Option<PathBuf>,
        codex_dir: Option<PathBuf>,
        opencode_dir: Option<PathBuf>,
    ) -> Self {
        Self {
            claude_dir,
            codex_dir,
            opencode_dir,
            read: HashMap::new(),
            places: HashMap::new(),
            codex_names: (None, HashMap::new()),
            opencode: OpenCode::default(),
        }
    }

    /// Every conversation that can be resumed, the most recent first. `force` asks OpenCode
    /// again even when its database did not change.
    pub fn load(&mut self, force: bool) -> Vec<Conversation> {
        let mut found = Vec::new();
        let mut files = HashSet::new();
        if let Some(dir) = self.claude_dir.clone() {
            for file in claude_transcripts(&dir) {
                found.extend(self.read(&file, read_claude));
                files.insert(file);
            }
        }
        if let Some(dir) = self.codex_dir.clone() {
            let names = self.codex_names(&dir);
            for file in codex_rollouts(&dir) {
                if let Some(mut conversation) = self.read(&file, read_codex) {
                    if let Some(name) = names.get(&conversation.id) {
                        conversation.title = Some(name.clone());
                    }
                    found.push(conversation);
                }
                files.insert(file);
            }
        }
        self.read.retain(|file, _| files.contains(file));
        found.extend(self.opencode(force));

        let mut resolved = HashMap::new();
        for conversation in &mut found {
            let (started, start_there) = self.place(&conversation.start, &mut resolved);
            let (mut here, mut there) = self.place(&conversation.cwd, &mut resolved);
            if here.project != started.project {
                if there {
                    // It went on out of its project (a `cd` elsewhere): it belongs where it
                    // started.
                    conversation.cwd = conversation.start.clone();
                    (here, there) = (started, start_there);
                } else {
                    // Gone before grove saw it, like a worktree removed: it stays with the
                    // project it started in.
                    here = discover::Place {
                        worktree: conversation
                            .cwd
                            .file_name()
                            .map(|name| name.to_string_lossy().into_owned()),
                        ..started
                    };
                }
            }
            conversation.project = here.project;
            conversation.project_name = here.name;
            conversation.worktree = here.worktree;
            conversation.gone = !there;
            conversation.stranded = !start_there;
        }
        found.sort_by(|a, b| {
            b.updated
                .cmp(&a.updated)
                .then_with(|| (a.agent, &a.id).cmp(&(b.agent, &b.id)))
        });
        // Should a conversation be kept in two files, the latest one tells.
        let mut seen = HashSet::new();
        found.retain(|conversation| seen.insert((conversation.agent, conversation.id.clone())));
        found
    }

    /// Where `dir` belongs, and whether it is still there. A folder since gone keeps the place
    /// grove last saw it in.
    fn place(
        &mut self,
        dir: &Path,
        resolved: &mut HashMap<PathBuf, (discover::Place, bool)>,
    ) -> (discover::Place, bool) {
        if let Some(found) = resolved.get(dir) {
            return found.clone();
        }
        let there = dir.is_dir();
        let place = if there {
            let place = discover::place(dir);
            self.places.insert(dir.to_path_buf(), place.clone());
            place
        } else {
            self.places
                .get(dir)
                .cloned()
                .unwrap_or_else(|| discover::place(dir))
        };
        resolved.insert(dir.to_path_buf(), (place.clone(), there));
        (place, there)
    }

    /// Reads a transcript with `reader`, or recalls what it said when it has not changed.
    fn read(
        &mut self,
        file: &Path,
        reader: fn(&Path, &fs::Metadata) -> Option<Conversation>,
    ) -> Option<Conversation> {
        let meta = fs::metadata(file).ok()?;
        let stamp = (meta.modified().ok(), meta.len());
        if let Some((seen, conversation)) = self.read.get(file)
            && *seen == stamp
        {
            return conversation.clone();
        }
        let conversation = reader(file, &meta);
        self.read
            .insert(file.to_path_buf(), (stamp, conversation.clone()));
        conversation
    }

    /// The names given to Codex sessions, read again when the index changes.
    fn codex_names(&mut self, dir: &Path) -> HashMap<String, String> {
        let file = dir.join("session_index.jsonl");
        let modified = fs::metadata(&file).and_then(|meta| meta.modified()).ok();
        if self.codex_names.0 != modified || modified.is_none() {
            let names = fs::read(&file)
                .map(|bytes| parse_codex_names(&bytes))
                .unwrap_or_default();
            self.codex_names = (modified, names);
        }
        self.codex_names.1.clone()
    }

    /// Whether an answer from OpenCode is still out, to look for it again soon.
    pub fn waiting(&self) -> bool {
        self.opencode.answer.is_some()
    }

    /// OpenCode's conversations, as it last told them. It is asked again, in the background,
    /// when its database changes, and not too often.
    fn opencode(&mut self, force: bool) -> Vec<Conversation> {
        let Some(dir) = &self.opencode_dir else {
            return Vec::new();
        };
        if let Some(answer) = &self.opencode.answer {
            match answer.try_recv() {
                Ok(result) => {
                    if let Ok(found) = result {
                        self.opencode.found = found;
                    }
                    self.opencode.answer = None;
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => self.opencode.answer = None,
            }
        }
        let stamp: Vec<Option<SystemTime>> = ["opencode.db", "opencode.db-wal"]
            .iter()
            .map(|name| {
                fs::metadata(dir.join(name))
                    .and_then(|meta| meta.modified())
                    .ok()
            })
            .collect();
        if stamp.iter().all(Option::is_none) {
            return Vec::new();
        }
        let due = self.opencode.asked.is_none_or(|asked| {
            let wait = if force {
                Duration::from_secs(2)
            } else {
                OPENCODE_EVERY
            };
            (force || stamp != self.opencode.stamp) && asked.elapsed() >= wait
        });
        if due && self.opencode.answer.is_none() {
            self.opencode.asked = Some(Instant::now());
            self.opencode.stamp = stamp;
            let (tx, rx) = mpsc::channel();
            let asking = thread::Builder::new()
                .name("grove-opencode".into())
                .spawn(move || {
                    let _ = tx.send(ask_opencode());
                });
            if asking.is_ok() {
                self.opencode.answer = Some(rx);
            }
        }
        self.opencode.found.clone()
    }
}

/// The command that opens a conversation again, to paste in a terminal: `cd` to where it
/// started, then the agent's own resume. A Claude Code background session still running is
/// attached to instead.
pub fn command(conversation: &Conversation, live: Option<&Session>, home: Option<&Path>) -> String {
    if let Some(job) = live
        .filter(|session| session.background)
        .and_then(|session| session.job.as_deref())
    {
        return format!("claude attach {}", fmt::shell_word(job));
    }
    let id = fmt::shell_word(&conversation.id);
    let resume = match conversation.agent {
        Agent::Claude => format!("claude --resume {id}"),
        Agent::Codex => format!("codex resume {id}"),
        Agent::OpenCode => format!("opencode --session {id}"),
    };
    format!("cd {} && {resume}", shell_path(&conversation.start, home))
}

/// A folder as a shell word, with `~` for the home folder where shells expand it (bash, zsh,
/// fish, PowerShell), outside the quotes. Windows paths take `/`, which its shells take too.
pub(crate) fn shell_path(path: &Path, home: Option<&Path>) -> String {
    let word = |path: &Path| {
        let text = path.to_string_lossy();
        if cfg!(windows) {
            fmt::shell_word(&text.replace('\\', "/"))
        } else {
            fmt::shell_word(&text)
        }
    };
    if let Some(home) = home
        && let Ok(rest) = path.strip_prefix(home)
    {
        if rest.as_os_str().is_empty() {
            return "~".into();
        }
        return format!("~/{}", word(rest));
    }
    word(path)
}

/// Every conversation file Claude Code keeps, `projects/<folder>/<id>.jsonl`; its subagents
/// keep theirs a level deeper, and are left out.
fn claude_transcripts(dir: &Path) -> Vec<PathBuf> {
    let Ok(folders) = fs::read_dir(dir.join("projects")) else {
        return Vec::new();
    };
    let mut files = Vec::new();
    for folder in folders.flatten() {
        let Ok(entries) = fs::read_dir(folder.path()) else {
            continue;
        };
        files.extend(
            entries
                .flatten()
                .map(|entry| entry.path())
                .filter(|path| path.extension().is_some_and(|ext| ext == "jsonl")),
        );
    }
    files
}

fn read_claude(file: &Path, meta: &fs::Metadata) -> Option<Conversation> {
    let id = file.file_stem()?.to_str()?.to_string();
    let tail = read_tail(file, meta.len(), TAIL)?;
    let facts = claude_tail(&tail);
    // Conversations run by a program (`claude -p`, the SDK) are not for picking up by hand.
    if facts
        .entrypoint
        .as_deref()
        .is_some_and(|entry| entry.starts_with("sdk"))
    {
        return None;
    }
    let title = facts
        .custom_title
        .or(facts.agent_name)
        .or(facts.ai_title)
        .or(facts.summary);
    if title.is_none() && facts.last_prompt.is_none() {
        return None;
    }
    let cwd = facts.cwd?;
    let folder = file.parent()?.file_name()?.to_string_lossy().into_owned();
    let start = started_in(&cwd, &folder)
        .or_else(|| first_cwd(file))
        .unwrap_or_else(|| cwd.clone());
    Some(Conversation {
        agent: Agent::Claude,
        id,
        title,
        first_prompt: None,
        last_prompt: facts.last_prompt,
        cwd,
        start,
        branch: facts.branch,
        created: meta.created().ok(),
        updated: meta.modified().unwrap_or(UNIX_EPOCH),
        pr: facts.pr,
        project: PathBuf::new(),
        project_name: String::new(),
        worktree: None,
        gone: false,
        stranded: false,
    })
}

/// What the end of a Claude Code transcript says.
#[derive(Debug, Default, PartialEq)]
struct Tail {
    custom_title: Option<String>,
    agent_name: Option<String>,
    ai_title: Option<String>,
    summary: Option<String>,
    last_prompt: Option<String>,
    pr: Option<(u64, String)>,
    cwd: Option<PathBuf>,
    branch: Option<String>,
    entrypoint: Option<String>,
}

/// Reads the end of a transcript, which may start in the middle of a line: the latest title,
/// name and last prompt, and the folder and branch of the latest entry.
fn claude_tail(tail: &[u8]) -> Tail {
    let mut facts = Tail::default();
    for line in tail.split(|&byte| byte == b'\n') {
        let Some(kind) = line
            .strip_prefix(br#"{"type":""#)
            .and_then(|rest| rest.split(|&byte| byte == b'"').next())
        else {
            continue;
        };
        let field = |key: &str| {
            let value: Value = serde_json::from_slice(line).ok()?;
            value.get(key)?.as_str().and_then(one_line)
        };
        match kind {
            b"custom-title" => facts.custom_title = field("customTitle"),
            b"agent-name" => facts.agent_name = field("agentName"),
            b"ai-title" => facts.ai_title = field("aiTitle"),
            b"summary" => facts.summary = field("summary"),
            b"last-prompt" => {
                if let Some(prompt) = field("lastPrompt") {
                    facts.last_prompt = Some(prompt);
                }
            }
            b"pr-link" => {
                let value: Option<Value> = serde_json::from_slice(line).ok();
                facts.pr = value.and_then(|value| {
                    let number = value.get("prNumber")?.as_u64()?;
                    let url = value.get("prUrl")?.as_str()?.to_string();
                    Some((number, url))
                });
            }
            _ => {}
        }
    }
    facts.cwd = last_string(tail, "cwd").map(PathBuf::from);
    facts.branch = last_string(tail, "gitBranch").filter(|branch| !branch.is_empty());
    facts.entrypoint = last_string(tail, "entrypoint");
    facts
}

/// The folder a conversation started in, among the ones around where it is now: Claude Code
/// keeps its transcripts in a folder named after it.
fn started_in(cwd: &Path, folder: &str) -> Option<PathBuf> {
    cwd.ancestors()
        .find(|dir| claude_folder(dir) == folder)
        .map(Path::to_path_buf)
}

/// The name of the folder Claude Code keeps a folder's transcripts in: every character but
/// ASCII letters and digits turned into `-`.
fn claude_folder(dir: &Path) -> String {
    dir.to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

/// The folder of the first entry of a transcript, for when its name says nothing.
fn first_cwd(file: &Path) -> Option<PathBuf> {
    let head = read_head(file, HEAD)?;
    let key = br#""cwd":""#;
    let line = head
        .split(|&byte| byte == b'\n')
        .find(|line| find(line, key).is_some())?;
    last_string(line, "cwd").map(PathBuf::from)
}

/// Every session file Codex keeps, `sessions/<year>/<month>/<day>/rollout-….jsonl`.
fn codex_rollouts(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut folders = vec![(dir.join("sessions"), 0)];
    while let Some((folder, depth)) = folders.pop() {
        let Ok(entries) = fs::read_dir(&folder) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                if depth < 3 {
                    folders.push((path, depth + 1));
                }
            } else if path.extension().is_some_and(|ext| ext == "jsonl")
                && path
                    .file_name()
                    .is_some_and(|name| name.to_string_lossy().starts_with("rollout-"))
            {
                files.push(path);
            }
        }
    }
    files
}

fn read_codex(file: &Path, meta: &fs::Metadata) -> Option<Conversation> {
    // The first line tells whether the rollout is one to resume; most are not.
    let mut head = read_head(file, FIRST_LINE)?;
    if !head.contains(&b'\n') && (head.len() as u64) < meta.len() {
        head = read_head(file, HEAD)?;
    }
    let first: Value = serde_json::from_slice(head.split(|&byte| byte == b'\n').next()?).ok()?;
    if first.get("type")?.as_str()? != "session_meta" {
        return None;
    }
    let payload = first.get("payload")?;
    // Codex resumes the sessions people had in its terminal interface or an editor, not the
    // ones `codex exec` ran nor its subagents.
    let interactive = match payload.get("source") {
        None => true,
        Some(Value::String(source)) => matches!(source.as_str(), "cli" | "vscode"),
        Some(_) => false,
    };
    if !interactive {
        return None;
    }
    let id = payload.get("id")?.as_str()?.to_string();
    let cwd = PathBuf::from(payload.get("cwd")?.as_str()?);
    if (head.len() as u64) < meta.len().min(HEAD) {
        head = read_head(file, HEAD)?;
    }
    Some(Conversation {
        agent: Agent::Codex,
        id,
        title: None,
        first_prompt: head
            .split(|&byte| byte == b'\n')
            .skip(1)
            .find_map(codex_prompt),
        last_prompt: None,
        start: cwd.clone(),
        cwd,
        branch: payload
            .pointer("/git/branch")
            .and_then(Value::as_str)
            .map(str::to_string),
        created: meta.created().ok(),
        updated: meta.modified().unwrap_or(UNIX_EPOCH),
        pr: None,
        project: PathBuf::new(),
        project_name: String::new(),
        worktree: None,
        gone: false,
        stranded: false,
    })
}

/// What the person asked, from a rollout line: a `user_message` event, or a user message
/// that is not one of those Codex opens a session with (the environment, the AGENTS.md
/// files).
fn codex_prompt(line: &[u8]) -> Option<String> {
    let event = find(line, br#""type":"user_message""#).is_some();
    if !event && find(line, br#""role":"user""#).is_none() {
        return None;
    }
    let value: Value = serde_json::from_slice(line).ok()?;
    let payload = value.get("payload")?;
    let text = match payload.get("type")?.as_str()? {
        "user_message" => payload.get("message")?.as_str()?.to_string(),
        "message" if payload.get("role")?.as_str()? == "user" => payload
            .get("content")?
            .as_array()?
            .iter()
            .filter_map(|part| part.get("text")?.as_str())
            .collect::<Vec<_>>()
            .join(" "),
        _ => return None,
    };
    let text = one_line(&text)?;
    if text.starts_with('<') || text.starts_with("# AGENTS.md") {
        return None;
    }
    Some(text)
}

/// Reads `session_index.jsonl`: a line per name given, the last one for a session winning.
fn parse_codex_names(bytes: &[u8]) -> HashMap<String, String> {
    let mut names = HashMap::new();
    for line in bytes.split(|&byte| byte == b'\n') {
        let Ok(value) = serde_json::from_slice::<Value>(line) else {
            continue;
        };
        let (Some(id), Some(name)) = (
            value.get("id").and_then(Value::as_str),
            value
                .get("thread_name")
                .and_then(Value::as_str)
                .and_then(one_line),
        ) else {
            continue;
        };
        names.insert(id.to_string(), name);
    }
    names
}

/// Asks OpenCode for its conversations through its own database command.
fn ask_opencode() -> Result<Vec<Conversation>, String> {
    let mut child = Command::new("opencode")
        .args(["db", OPENCODE_QUERY, "--format", "json", "--pure"])
        .current_dir(std::env::temp_dir())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("opencode: {error}"))?;
    let mut stdout = child.stdout.take().ok_or("opencode: no output")?;
    let reader = thread::spawn(move || {
        let mut out = Vec::new();
        let _ = stdout.read_to_end(&mut out);
        out
    });
    let deadline = Instant::now() + OPENCODE_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(50)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("opencode: no answer".into());
            }
        }
    };
    let out = reader.join().map_err(|_| "opencode: output lost")?;
    if !status.success() {
        return Err(format!("opencode: {status}"));
    }
    Ok(parse_opencode(&out))
}

/// Reads the rows `opencode db --format json` prints.
fn parse_opencode(bytes: &[u8]) -> Vec<Conversation> {
    let Ok(Value::Array(rows)) = serde_json::from_slice(bytes) else {
        return Vec::new();
    };
    rows.iter()
        .filter_map(|row| {
            let time = |key: &str| {
                row.get(key)
                    .and_then(Value::as_u64)
                    .map(|ms| UNIX_EPOCH + Duration::from_millis(ms))
            };
            let cwd = PathBuf::from(row.get("directory")?.as_str()?);
            Some(Conversation {
                agent: Agent::OpenCode,
                id: row.get("id")?.as_str()?.to_string(),
                title: row.get("title").and_then(Value::as_str).and_then(one_line),
                first_prompt: None,
                last_prompt: None,
                start: cwd.clone(),
                cwd,
                branch: None,
                created: time("time_created"),
                updated: time("time_updated").unwrap_or(UNIX_EPOCH),
                pr: None,
                project: PathBuf::new(),
                project_name: String::new(),
                worktree: None,
                gone: false,
                stranded: false,
            })
        })
        .collect()
}

/// The text on one line, cut to a few hundred characters; `None` when there is none.
fn one_line(text: &str) -> Option<String> {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.is_empty() {
        return None;
    }
    Some(match text.char_indices().nth(TEXT) {
        Some((at, _)) => format!("{}…", &text[..at]),
        None => text,
    })
}

/// The value of the last `"key":"…"` in `bytes`. In a transcript line that is the entry's own
/// field: the fields every entry has come after what it carries.
fn last_string(bytes: &[u8], key: &str) -> Option<String> {
    let needle = format!("\"{key}\":\"");
    let mut end = bytes.len();
    while let Some(at) = rfind(&bytes[..end], needle.as_bytes()) {
        // From the value's opening quote on.
        let value = &bytes[at + needle.len() - 1..];
        if let Some(Ok(text)) = serde_json::Deserializer::from_slice(value)
            .into_iter::<String>()
            .next()
        {
            return Some(text);
        }
        end = at;
    }
    None
}

fn find(bytes: &[u8], needle: &[u8]) -> Option<usize> {
    bytes
        .windows(needle.len())
        .position(|window| window == needle)
}

fn rfind(bytes: &[u8], needle: &[u8]) -> Option<usize> {
    bytes
        .windows(needle.len())
        .rposition(|window| window == needle)
}

fn read_head(file: &Path, max: u64) -> Option<Vec<u8>> {
    let mut bytes = Vec::new();
    File::open(file)
        .ok()?
        .take(max)
        .read_to_end(&mut bytes)
        .ok()?;
    Some(bytes)
}

fn read_tail(file: &Path, len: u64, max: u64) -> Option<Vec<u8>> {
    let mut handle = File::open(file).ok()?;
    handle.seek(SeekFrom::Start(len.saturating_sub(max))).ok()?;
    let mut bytes = Vec::new();
    handle.take(max).read_to_end(&mut bytes).ok()?;
    Some(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::tests::{Scratch, repo_with_worktrees};
    use crate::model::Activity;

    #[test]
    fn reads_the_end_of_a_claude_transcript() {
        // The tail starts in the middle of a line; a tool's result carries its own "cwd".
        let tail = br#"cut in the middle","cwd":"/older/entry"}
{"parentUuid":"a","type":"user","message":{"role":"user","content":"hi"},"toolUseResult":{"cwd":"/where/the/tool/ran"},"userType":"external","entrypoint":"cli","cwd":"/code/app/.claude/worktrees/x","sessionId":"s","version":"2.1.283","gitBranch":"worktree-x"}
{"type":"custom-title","customTitle":"coupon\n rules","sessionId":"s"}
{"type":"agent-name","agentName":"coupons","sessionId":"s"}
{"type":"ai-title","aiTitle":"Coupon rules per item","sessionId":"s"}
{"type":"last-prompt","lastPrompt":"apply it per item","leafUuid":"l","sessionId":"s"}
{"type":"last-prompt","leafUuid":"m","sessionId":"s"}
{"type":"pr-link","sessionId":"s","prNumber":512,"prUrl":"https://github.com/acme/shop/pull/512"}
"#;
        let facts = claude_tail(tail);
        assert_eq!(facts.custom_title.as_deref(), Some("coupon rules"));
        assert_eq!(facts.agent_name.as_deref(), Some("coupons"));
        assert_eq!(facts.ai_title.as_deref(), Some("Coupon rules per item"));
        assert_eq!(
            facts.last_prompt.as_deref(),
            Some("apply it per item"),
            "a later line without a prompt keeps it"
        );
        assert_eq!(
            facts.pr,
            Some((512, "https://github.com/acme/shop/pull/512".into()))
        );
        assert_eq!(
            facts.cwd.as_deref(),
            Some(Path::new("/code/app/.claude/worktrees/x")),
            "the entry's own folder, not the tool's"
        );
        assert_eq!(facts.branch.as_deref(), Some("worktree-x"));
        assert_eq!(facts.entrypoint.as_deref(), Some("cli"));
        assert_eq!(claude_tail(b"").cwd, None);
    }

    #[test]
    fn finds_the_folder_a_claude_conversation_started_in() {
        assert_eq!(
            claude_folder(Path::new("/Users/me/code/app/.claude/worktrees/x")),
            "-Users-me-code-app--claude-worktrees-x"
        );
        let started = |cwd: &str| started_in(Path::new(cwd), "-code-app");
        assert_eq!(
            started("/code/app/.claude/worktrees/x"),
            Some(PathBuf::from("/code/app"))
        );
        assert_eq!(started("/code/app"), Some(PathBuf::from("/code/app")));
        assert_eq!(started("/elsewhere"), None);
    }

    #[test]
    fn tells_what_a_person_asked_codex_from_what_codex_adds() {
        let message = |text: &str| {
            format!(
                r#"{{"type":"response_item","payload":{{"type":"message","role":"user","content":[{{"type":"input_text","text":{text:?}}}]}}}}"#
            )
        };
        let prompt = |line: String| codex_prompt(line.as_bytes());
        assert_eq!(
            prompt(message("<environment_context>\n  <cwd>/x</cwd>")),
            None
        );
        assert_eq!(prompt(message("# AGENTS.md instructions for /x")), None);
        assert_eq!(
            prompt(message("fix the\nflaky spec")).as_deref(),
            Some("fix the flaky spec")
        );
        assert_eq!(
            prompt(
                r#"{"type":"event_msg","payload":{"type":"user_message","message":"hi"}}"#.into()
            )
            .as_deref(),
            Some("hi")
        );
        assert_eq!(
            prompt(
                r#"{"type":"event_msg","payload":{"type":"agent_message","message":"hi"}}"#.into()
            ),
            None
        );
        let names = parse_codex_names(
            br#"{"id":"a","thread_name":"first","updated_at":"x"}
{"id":"b","thread_name":"other"}
{"id":"a","thread_name":"renamed"}
"#,
        );
        assert_eq!(names.get("a").map(String::as_str), Some("renamed"));
        assert_eq!(names.len(), 2);
    }

    #[test]
    fn reads_opencode_rows() {
        let rows = parse_opencode(
            br#"[{"id":"ses_1","title":"Pricing page","directory":"/code/site","time_created":1790462508246,"time_updated":1790529426177}]"#,
        );
        assert_eq!(rows.len(), 1);
        let row = &rows[0];
        assert_eq!((row.agent, row.id.as_str()), (Agent::OpenCode, "ses_1"));
        assert_eq!(row.title.as_deref(), Some("Pricing page"));
        assert_eq!(row.start, PathBuf::from("/code/site"));
        assert_eq!(
            row.updated,
            UNIX_EPOCH + Duration::from_millis(1_790_529_426_177)
        );
        assert!(parse_opencode(b"Error: no such table").is_empty());
    }

    fn write(path: &Path, text: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    #[test]
    fn loads_what_can_be_resumed_and_reads_again_what_changed() {
        let scratch = Scratch::new("history");
        let repo = repo_with_worktrees(&scratch, &["wt"]);
        let worktree = scratch.0.join("wt");
        let at = |dir: &Path| dir.to_str().unwrap().replace('\\', "\\\\");
        let entry = |cwd: &Path, entrypoint: &str| {
            format!(
                r#"{{"type":"user","message":{{"role":"user","content":"hi"}},"entrypoint":"{entrypoint}","cwd":"{}","gitBranch":"main"}}"#,
                at(cwd)
            )
        };
        let claude = scratch.0.join("claude");
        let project = claude.join("projects").join(claude_folder(&repo));
        let transcript = project.join("0f3a9c21-4b1e-4c2d-9e7f-8c1d2a3b4c5d.jsonl");
        // It started in the repository and went on in a linked worktree.
        write(
            &transcript,
            &format!(
                "{}\n{}\n{{\"type\":\"ai-title\",\"aiTitle\":\"coupon rules\"}}\n",
                entry(&repo, "cli"),
                entry(&worktree, "cli")
            ),
        );
        // `claude -p` runs are left out, and so are conversations where nothing was asked.
        write(
            &project.join("sdk.jsonl"),
            &format!(
                "{}\n{{\"type\":\"last-prompt\",\"lastPrompt\":\"x\"}}\n",
                entry(&repo, "sdk-cli")
            ),
        );
        write(
            &project.join("empty.jsonl"),
            &format!("{}\n", entry(&repo, "cli")),
        );
        // A subagent's transcript, a level deeper.
        write(
            &project.join("0f3a9c21/subagents/agent-1.jsonl"),
            &format!(
                "{}\n{{\"type\":\"ai-title\",\"aiTitle\":\"sub\"}}\n",
                entry(&repo, "cli")
            ),
        );

        let codex = scratch.0.join("codex");
        let meta = |id: &str, source: &str| {
            format!(
                r#"{{"type":"session_meta","payload":{{"id":"{id}","cwd":"{}","source":{source},"git":{{"branch":"feat/x"}}}}}}"#,
                at(&worktree)
            )
        };
        let day = codex.join("sessions/2026/09/25");
        write(
            &day.join("rollout-2026-09-25T12-23-21-019e.jsonl"),
            &format!(
                "{}\n{}\n",
                meta("019e", r#""cli""#),
                r#"{"type":"event_msg","payload":{"type":"user_message","message":"add the dns records"}}"#
            ),
        );
        write(
            &day.join("rollout-2026-09-25T12-24-00-01a0.jsonl"),
            &format!("{}\n", meta("01a0", r#"{"subagent":{"other":"guardian"}}"#)),
        );
        write(
            &codex.join("session_index.jsonl"),
            "{\"id\":\"019e\",\"thread_name\":\"DNS records\"}\n",
        );

        let mut history = History::with_dirs(Some(claude), Some(codex), None);
        let found = history.load(false);
        let summary: Vec<(Agent, &str, Option<&str>)> = found
            .iter()
            .map(|c| (c.agent, c.id.as_str(), c.title.as_deref()))
            .collect();
        assert_eq!(summary.len(), 2, "{summary:?}");
        let claude_one = found.iter().find(|c| c.agent == Agent::Claude).unwrap();
        assert_eq!(claude_one.id, "0f3a9c21-4b1e-4c2d-9e7f-8c1d2a3b4c5d");
        assert_eq!(claude_one.title.as_deref(), Some("coupon rules"));
        assert_eq!(claude_one.start, repo, "resumed from where it started");
        assert_eq!(claude_one.cwd, worktree, "shown where it went on");
        assert_eq!(claude_one.project, repo);
        assert_eq!(claude_one.project_name, "repo");
        assert_eq!(claude_one.worktree.as_deref(), Some("wt"));
        assert!(!claude_one.gone);
        let codex_one = found.iter().find(|c| c.agent == Agent::Codex).unwrap();
        assert_eq!(codex_one.title.as_deref(), Some("DNS records"));
        assert_eq!(
            codex_one.first_prompt.as_deref(),
            Some("add the dns records")
        );
        assert_eq!(codex_one.branch.as_deref(), Some("feat/x"));
        assert_eq!(
            (codex_one.start.as_path(), codex_one.project.as_path()),
            (worktree.as_path(), repo.as_path())
        );

        // A transcript is read again once it changes; the rest is remembered.
        std::thread::sleep(Duration::from_millis(20));
        let mut text = fs::read_to_string(&transcript).unwrap();
        text.push_str("{\"type\":\"custom-title\",\"customTitle\":\"coupons per item\"}\n");
        fs::write(&transcript, text).unwrap();
        let found = history.load(false);
        assert_eq!(found.len(), 2);
        assert!(
            found
                .iter()
                .any(|c| c.title.as_deref() == Some("coupons per item"))
        );

        // Its worktree removed, it stays where it was, gone.
        crate::git::run(
            &repo,
            ["worktree", "remove", "--force", worktree.to_str().unwrap()],
        )
        .unwrap();
        let claude_one = history
            .load(false)
            .into_iter()
            .find(|c| c.agent == Agent::Claude)
            .unwrap();
        assert!(
            claude_one.gone && !claude_one.stranded,
            "resumed from the repository"
        );
        assert_eq!(claude_one.project, repo);
        assert_eq!(claude_one.worktree.as_deref(), Some("wt"));
        let codex_one = history
            .load(false)
            .into_iter()
            .find(|c| c.agent == Agent::Codex)
            .unwrap();
        assert!(codex_one.stranded, "Codex resumes from the worktree itself");
        assert_eq!(codex_one.project, repo, "where grove saw it last");
    }

    fn conversation(agent: Agent, id: &str, start: &str) -> Conversation {
        Conversation {
            agent,
            id: id.into(),
            title: None,
            first_prompt: None,
            last_prompt: None,
            cwd: PathBuf::from(start),
            start: PathBuf::from(start),
            branch: None,
            created: None,
            updated: UNIX_EPOCH,
            pr: None,
            project: PathBuf::from(start),
            project_name: String::new(),
            worktree: None,
            gone: false,
            stranded: false,
        }
    }

    fn session(background: bool, job: Option<&str>) -> Session {
        Session {
            agent: Agent::Claude,
            pid: 4242,
            name: None,
            detail: None,
            background,
            activity: Activity::Idle,
            cpu: 0.0,
            started: None,
            cwd: PathBuf::from("/x"),
            conversation: Some("0f3a9c21-4b1e".into()),
            job: job.map(str::to_string),
        }
    }

    #[test]
    fn builds_the_command_that_resumes_each_agent() {
        let home = Some(Path::new("/Users/me"));
        let resume = |agent, id, start| command(&conversation(agent, id, start), None, home);
        assert_eq!(
            resume(Agent::Claude, "0f3a9c21-4b1e", "/Users/me/Workspace/shop"),
            "cd ~/Workspace/shop && claude --resume 0f3a9c21-4b1e"
        );
        assert_eq!(
            resume(Agent::Codex, "019ebca5-89ff", "/Users/me/Workspace/tools"),
            "cd ~/Workspace/tools && codex resume 019ebca5-89ff"
        );
        assert_eq!(
            resume(Agent::OpenCode, "ses_f2208140bffe", "/Users/me/site"),
            "cd ~/site && opencode --session ses_f2208140bffe"
        );
        assert_eq!(
            resume(Agent::Claude, "a", "/Users/me/My Code/it's"),
            r"cd ~/'My Code/it'\''s' && claude --resume a",
            "the ~ stays out of the quotes, where shells expand it"
        );
        assert_eq!(
            resume(Agent::Claude, "a", "/Users/me"),
            "cd ~ && claude --resume a"
        );
        assert_eq!(
            resume(Agent::Claude, "a", "/srv/app"),
            "cd /srv/app && claude --resume a"
        );
        let open = conversation(Agent::Claude, "0f3a9c21-4b1e", "/Users/me/shop");
        assert_eq!(
            command(&open, Some(&session(true, Some("0f3a9c21"))), home),
            "claude attach 0f3a9c21",
            "a background session still running is attached to"
        );
        assert_eq!(
            command(&open, Some(&session(false, None)), home),
            "cd ~/shop && claude --resume 0f3a9c21-4b1e"
        );
    }
}
