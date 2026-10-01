//! The routines Claude Desktop runs on a schedule, and how each one's latest run went.
//!
//! Desktop keeps a folder per account and organization under `claude-code-sessions` (and,
//! for agent mode, `local-agent-mode-sessions`) in its application data, wherever the system
//! and the way it was installed put that: `scheduled-tasks.json` there lists the routines (schedule, folder,
//! prompt file, last run), and every session it opens, a run of a routine among them, is a
//! `local_<id>.json` beside it. A run's file names its routine (`scheduledTaskId`) and keeps
//! Desktop's summary of its last turn (`postTurnSummary`), which says whether it waits for
//! you and what for. Those files grow large, so one is read again only when it changes.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use chrono::DateTime;
use serde_json::Value;

use crate::cron::Cron;
use crate::model::{Routine, Run};
use crate::{fmt, history};

/// Prompts' descriptions and run summaries are kept to this many characters.
const TEXT: usize = 500;

/// The file's modification time and length, which say whether it changed.
type Stamp = (Option<SystemTime>, u64);

pub struct Desktop {
    home: Option<PathBuf>,
    /// The folders to look in, when they are given; else they are looked for every time, so
    /// a Desktop installed or signed in since grove started is found.
    roots: Option<Vec<PathBuf>>,
    /// What each session file said at the time it had its stamp: the routine it ran, and
    /// the run.
    read: HashMap<PathBuf, (Stamp, Option<(String, Run)>)>,
}

impl Desktop {
    pub fn new(home: Option<&Path>) -> Self {
        Self {
            home: home.map(Path::to_path_buf),
            roots: None,
            read: HashMap::new(),
        }
    }

    #[cfg(test)]
    pub fn at(roots: Vec<PathBuf>) -> Self {
        Self {
            home: None,
            roots: Some(roots),
            read: HashMap::new(),
        }
    }

    /// Every routine, with its latest run.
    pub fn load(&mut self) -> Vec<Routine> {
        let mut routines = Vec::new();
        let mut seen = Vec::new();
        let roots = match &self.roots {
            Some(roots) => roots.clone(),
            None => roots(self.home.as_deref()),
        };
        let orgs: Vec<PathBuf> = roots
            .iter()
            .flat_map(|root| children(root))
            .flat_map(|account| children(&account))
            .collect();
        for org in orgs {
            let Some(listed) = read_json(&org.join("scheduled-tasks.json")) else {
                continue;
            };
            let mut runs: HashMap<String, Run> = HashMap::new();
            for file in children(&org) {
                let name = file
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or_default();
                if !(name.starts_with("local_") && name.ends_with(".json")) {
                    continue;
                }
                seen.push(file.clone());
                let Some((routine, run)) = self.run(&file) else {
                    continue;
                };
                let newer = runs
                    .get(&routine)
                    .is_none_or(|kept| kept.started < run.started);
                if newer {
                    runs.insert(routine, run);
                }
            }
            let tasks = listed["scheduledTasks"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            for task in &tasks {
                if let Some(routine) = routine(&org, task, &mut runs) {
                    routines.push(routine);
                }
            }
        }
        self.read.retain(|path, _| seen.contains(path));
        routines
    }

    /// The routine a session file ran and how, reading it again only when it changed.
    fn run(&mut self, file: &Path) -> Option<(String, Run)> {
        let meta = fs::metadata(file).ok()?;
        let stamp = (meta.modified().ok(), meta.len());
        if let Some((kept, run)) = self.read.get(file)
            && *kept == stamp
        {
            return run.clone();
        }
        let run = fs::read_to_string(file)
            .ok()
            // Most sessions are not runs of a routine: no need to parse those.
            .filter(|text| text.contains("\"scheduledTaskId\""))
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
            .and_then(|json| parse_run(&json));
        self.read.insert(file.to_path_buf(), (stamp, run.clone()));
        run
    }
}

/// The command that resumes a routine's last run in a terminal, from the folder it ran in.
pub fn command(routine: &Routine, home: Option<&Path>) -> Option<String> {
    let run = routine.run.as_ref()?;
    let resume = format!(
        "claude --resume {}",
        fmt::shell_word(run.conversation.as_ref()?)
    );
    Some(match run.cwd.as_ref().or(routine.cwd.as_ref()) {
        Some(dir) => format!("cd {} && {resume}", history::shell_path(dir, home)),
        None => resume,
    })
}

/// The link that brings a routine's last run up in Claude Desktop, to answer or approve
/// there: Desktop takes `claude://code/continue?session=local_<id>`, the ids it gives its
/// sessions, and turns down any other.
pub fn desktop_link(routine: &Routine) -> Option<String> {
    let session = &routine.run.as_ref()?.session;
    let id = session.strip_prefix("local_")?;
    let valid =
        (1..=64).contains(&id.len()) && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
    valid.then(|| format!("claude://code/continue?session={session}"))
}

/// The systems whose Desktop folders differ.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum System {
    Mac,
    Windows,
    Other,
}

impl System {
    fn current() -> Self {
        if cfg!(target_os = "macos") {
            System::Mac
        } else if cfg!(windows) {
            System::Windows
        } else {
            System::Other
        }
    }
}

/// The folders under Desktop's data that keep routines: those of Claude Code sessions and
/// those of agent mode, in the same format.
const KINDS: [&str; 2] = ["claude-code-sessions", "local-agent-mode-sessions"];

/// Where Desktop keeps its routines on this computer: `$GROVE_DESKTOP_DIR` when set, else
/// where each system puts Desktop's data, the ones that exist.
pub fn roots(home: Option<&Path>) -> Vec<PathBuf> {
    let found: Vec<PathBuf> = data_dirs(System::current(), home, |name| std::env::var_os(name))
        .into_iter()
        .flat_map(|dir| KINDS.map(|kind| dir.join(kind)))
        .collect();
    let existing: Vec<PathBuf> = found.iter().filter(|dir| dir.is_dir()).cloned().collect();
    // With none, all of them, to say where grove looked.
    if existing.is_empty() { found } else { existing }
}

/// The folders Desktop may keep its data in. On Windows it is in the roaming application
/// data, or, installed from the Microsoft Store, in its package's own copy of it, under a
/// folder named after the package (`Claude_<publisher>`).
fn data_dirs(
    system: System,
    home: Option<&Path>,
    var: impl Fn(&str) -> Option<std::ffi::OsString>,
) -> Vec<PathBuf> {
    if let Some(dir) = var("GROVE_DESKTOP_DIR").filter(|dir| !dir.is_empty()) {
        return vec![PathBuf::from(dir)];
    }
    let mut dirs = Vec::new();
    match system {
        System::Mac => {
            dirs.extend(home.map(|home| home.join("Library/Application Support/Claude")))
        }
        System::Windows => {
            dirs.extend(var("APPDATA").map(|dir| PathBuf::from(dir).join("Claude")));
            if let Some(local) = var("LOCALAPPDATA").map(PathBuf::from) {
                dirs.push(local.join("Claude"));
                let packages = children(&local.join("Packages"));
                dirs.extend(
                    packages
                        .into_iter()
                        .filter(|package| {
                            package
                                .file_name()
                                .and_then(|name| name.to_str())
                                .is_some_and(|name| name.starts_with("Claude_"))
                        })
                        .map(|package| package.join("LocalCache/Roaming/Claude")),
                );
            }
        }
        System::Other => {
            let config = var("XDG_CONFIG_HOME")
                .map(PathBuf::from)
                .or_else(|| home.map(|home| home.join(".config")));
            dirs.extend(config.map(|config| config.join("Claude")));
        }
    }
    dirs
}

fn children(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut paths: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).collect();
    paths.sort();
    paths
}

fn read_json(path: &Path) -> Option<Value> {
    serde_json::from_str(&fs::read_to_string(path).ok()?).ok()
}

fn text(value: &Value) -> Option<String> {
    let text = value.as_str()?.trim();
    (!text.is_empty()).then(|| text.chars().take(TEXT).collect())
}

/// Milliseconds since 1970, as Desktop writes its times.
fn millis(value: &Value) -> Option<SystemTime> {
    let ms = value.as_f64().filter(|ms| *ms > 0.0)?;
    Some(UNIX_EPOCH + Duration::from_millis(ms as u64))
}

/// A time written like `2026-09-30T20:30:00.000Z`.
fn iso(value: &Value) -> Option<SystemTime> {
    let time = DateTime::parse_from_rfc3339(value.as_str()?).ok()?;
    Some(SystemTime::from(time))
}

fn parse_run(json: &Value) -> Option<(String, Run)> {
    let routine = text(&json["scheduledTaskId"])?;
    let summary = &json["postTurnSummary"];
    // A summary of an earlier turn says nothing about where the run is now.
    let current = json["postTurnSummaryFor"].is_null()
        || json["postTurnSummaryFor"] == json["lastAssistantUuid"];
    let (status, detail, needs) = if summary.is_object() && current {
        (
            text(&summary["status_category"]),
            text(&summary["status_detail"]),
            text(&summary["needs_action"]),
        )
    } else {
        (None, None, None)
    };
    let run = Run {
        session: text(&json["sessionId"])?,
        conversation: text(&json["cliSessionId"]),
        title: text(&json["title"]),
        cwd: text(&json["cwd"]).map(PathBuf::from),
        started: millis(&json["createdAt"]),
        active: millis(&json["lastActivityAt"]),
        status,
        detail,
        needs,
        error: text(&json["error"]),
        archived: json["isArchived"].as_bool().unwrap_or(false),
    };
    Some((routine, run))
}

fn routine(org: &Path, task: &Value, runs: &mut HashMap<String, Run>) -> Option<Routine> {
    let id = text(&task["id"])?;
    let cron = text(&task["cronExpression"]).unwrap_or_default();
    let prompt = text(&task["filePath"]).map(PathBuf::from);
    Some(Routine {
        key: format!("{}/{id}", org.display()),
        name: text(&task["displayName"]).unwrap_or_else(|| id.clone()),
        description: prompt.as_deref().and_then(description),
        schedule: Cron::parse(&cron),
        cron,
        enabled: task["enabled"].as_bool().unwrap_or(true),
        cwd: text(&task["cwd"]).map(PathBuf::from),
        prompt,
        created: millis(&task["createdAt"]),
        last_run: iso(&task["lastRunAt"]),
        last_due: iso(&task["lastScheduledFor"]),
        run: runs.remove(&id),
        id,
    })
}

/// The `description:` of a prompt file's front matter.
fn description(path: &Path) -> Option<String> {
    let text = fs::read_to_string(path).ok()?;
    let mut lines = text.lines();
    if lines.next()?.trim() != "---" {
        return None;
    }
    lines
        .take_while(|line| line.trim() != "---")
        .find_map(|line| line.strip_prefix("description:"))
        .map(|value| value.trim().trim_matches('"').chars().take(TEXT).collect())
        .filter(|value: &String| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::tests::Scratch;

    /// A Desktop folder with two routines: one whose last run waits for an answer, after an
    /// earlier run that finished, and one paused that never ran.
    fn desktop() -> (Scratch, Desktop) {
        let dir = Scratch::new("routines");
        let org = dir.0.join("account/org");
        fs::create_dir_all(&org).unwrap();
        let prompt = dir.0.join("SKILL.md");
        fs::write(
            &prompt,
            "---\nname: status\ndescription: Weekdays 17:30 — posts the daily status\n---\nDo it.\n",
        )
        .unwrap();
        let tasks = serde_json::json!({ "scheduledTasks": [
            {
                "id": "status", "cronExpression": "30 17 * * 1-5", "enabled": true,
                "filePath": prompt, "cwd": "/Users/me/code",
                "createdAt": 1_786_744_006_610_u64,
                "lastRunAt": "2026-09-30T20:36:46.559Z",
                "lastScheduledFor": "2026-09-30T20:30:00.000Z"
            },
            {
                "id": "notices", "displayName": "Slack notices", "cronExpression": "5 9 * * 1-5",
                "enabled": false
            }
        ]});
        fs::write(org.join("scheduled-tasks.json"), tasks.to_string()).unwrap();
        let session = |id: &str, created: u64, summary: Value| {
            serde_json::json!({
                "sessionId": id, "cliSessionId": format!("cli-{id}"), "cwd": "/Users/me/code",
                "createdAt": created, "lastActivityAt": created + 60_000,
                "title": "Daily status", "scheduledTaskId": "status", "isArchived": false,
                "postTurnSummary": summary, "postTurnSummaryFor": "u1", "lastAssistantUuid": "u1"
            })
        };
        let old = session(
            "local_old",
            1_790_000_000_000,
            serde_json::json!({ "status_category": "completed", "status_detail": "posted", "needs_action": "" }),
        );
        let new = session(
            "local_new",
            1_790_800_606_558,
            serde_json::json!({
                "status_category": "blocked",
                "status_detail": "Run it again with the cards in validation?",
                "needs_action": "Run it again with the cards in validation?"
            }),
        );
        fs::write(org.join("local_old.json"), old.to_string()).unwrap();
        fs::write(org.join("local_new.json"), new.to_string()).unwrap();
        fs::write(org.join("local_chat.json"), r#"{"sessionId":"local_chat"}"#).unwrap();
        let desktop = Desktop::at(vec![dir.0.clone()]);
        (dir, desktop)
    }

    #[test]
    fn reads_the_routines_and_their_latest_run() {
        let (_dir, mut desktop) = desktop();
        let routines = desktop.load();
        assert_eq!(routines.len(), 2);
        let status = &routines[0];
        assert_eq!(status.name, "status");
        assert_eq!(
            status.description.as_deref(),
            Some("Weekdays 17:30 — posts the daily status")
        );
        assert!(status.schedule.is_some() && status.enabled);
        assert_eq!(status.cwd.as_deref(), Some(Path::new("/Users/me/code")));
        assert_eq!(
            status.last_due,
            Some(UNIX_EPOCH + Duration::from_secs(1_790_800_200))
        );
        let run = status.run.as_ref().unwrap();
        assert_eq!(run.session, "local_new", "the latest run");
        assert_eq!(run.conversation.as_deref(), Some("cli-local_new"));
        assert_eq!(run.status.as_deref(), Some("blocked"));
        assert_eq!(
            run.needs.as_deref(),
            Some("Run it again with the cards in validation?")
        );
        let notices = &routines[1];
        assert_eq!(notices.name, "Slack notices");
        assert!(!notices.enabled && notices.run.is_none());
    }

    #[test]
    fn a_summary_of_an_earlier_turn_is_left_out() {
        let (dir, mut desktop) = desktop();
        let file = dir.0.join("account/org/local_new.json");
        let mut json: Value = serde_json::from_str(&fs::read_to_string(&file).unwrap()).unwrap();
        json["lastAssistantUuid"] = "u2".into();
        fs::write(&file, json.to_string()).unwrap();
        let routines = desktop.load();
        let run = routines[0].run.as_ref().unwrap();
        assert_eq!((run.status.as_deref(), run.needs.as_deref()), (None, None));
    }

    #[test]
    fn finds_desktop_on_each_system() {
        let none = |_: &str| None;
        let home = Path::new("/Users/me");
        assert_eq!(
            data_dirs(System::Mac, Some(home), none),
            [PathBuf::from(
                "/Users/me/Library/Application Support/Claude"
            )]
        );
        assert_eq!(
            data_dirs(System::Other, Some(home), none),
            [PathBuf::from("/Users/me/.config/Claude")]
        );
        // Installed from the Microsoft Store, its data is in its package, whatever the
        // publisher part of the package's name.
        let local = Scratch::new("localappdata");
        for package in [
            "Claude_pzs8sxrjxfjjc",
            "Microsoft.WindowsTerminal_8wekyb3d8bbwe",
        ] {
            fs::create_dir_all(local.0.join("Packages").join(package)).unwrap();
        }
        let windows = |name: &str| match name {
            "APPDATA" => Some(r"C:\Users\me\AppData\Roaming".into()),
            "LOCALAPPDATA" => Some(local.0.clone().into_os_string()),
            _ => None,
        };
        assert_eq!(
            data_dirs(System::Windows, None, windows),
            [
                PathBuf::from(r"C:\Users\me\AppData\Roaming").join("Claude"),
                local.0.join("Claude"),
                local
                    .0
                    .join("Packages/Claude_pzs8sxrjxfjjc")
                    .join("LocalCache/Roaming/Claude"),
            ]
        );
        // A folder of your own wins on any system.
        let mine = |name: &str| (name == "GROVE_DESKTOP_DIR").then(|| "/elsewhere/Claude".into());
        assert_eq!(
            data_dirs(System::Windows, Some(home), mine),
            [PathBuf::from("/elsewhere/Claude")]
        );
    }

    #[test]
    fn reads_agent_mode_routines_too() {
        let (dir, _) = desktop();
        let agent = dir.0.join("agent/account/org");
        fs::create_dir_all(&agent).unwrap();
        let tasks = serde_json::json!({ "scheduledTasks": [
            { "id": "weekly-review", "cronExpression": "0 9 * * 1", "enabled": true }
        ]});
        fs::write(agent.join("scheduled-tasks.json"), tasks.to_string()).unwrap();
        let mut desktop = Desktop::at(vec![dir.0.clone(), dir.0.join("agent")]);
        let ids: Vec<String> = desktop.load().into_iter().map(|r| r.id).collect();
        assert_eq!(ids, ["status", "notices", "weekly-review"]);
    }

    #[test]
    fn links_to_the_run_the_way_desktop_takes_it() {
        let (_dir, mut desktop) = desktop();
        let routines = desktop.load();
        assert_eq!(
            desktop_link(&routines[0]).as_deref(),
            Some("claude://code/continue?session=local_new")
        );
        assert_eq!(desktop_link(&routines[1]), None, "never ran");
        // Desktop turns down any other id, so grove does not hand one over.
        let mut odd = routines[0].clone();
        for session in [
            "chat_1",
            "local_",
            "local_a&b=c",
            &format!("local_{}", "a".repeat(65)),
        ] {
            odd.run.as_mut().unwrap().session = session.to_string();
            assert_eq!(desktop_link(&odd), None, "{session}");
        }
    }

    #[test]
    fn nothing_without_desktop() {
        let mut desktop = Desktop::at(vec![PathBuf::from("/no/such/folder")]);
        assert!(desktop.load().is_empty());
    }
}
