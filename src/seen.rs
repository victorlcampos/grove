//! The routine runs opened from grove, kept on disk so a run you saw stops waiting for you
//! even after the tab you saw it in closes. A run is kept by its Desktop session and the
//! message Desktop summed up: one that asks something new waits again.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::model::Run;

#[derive(Debug, Default)]
pub struct Seen {
    /// Where they are kept; none in tests.
    path: Option<PathBuf>,
    /// The turn of each run that was opened, by its Desktop session.
    runs: HashMap<String, String>,
}

impl Seen {
    pub fn load(home: Option<&Path>) -> Self {
        let path = file(home, |name| std::env::var_os(name));
        let runs = path
            .as_deref()
            .and_then(|path| fs::read_to_string(path).ok())
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
            .and_then(|json| {
                json.as_object().map(|runs| {
                    runs.iter()
                        .filter_map(|(session, turn)| {
                            Some((session.clone(), turn.as_str()?.to_string()))
                        })
                        .collect()
                })
            })
            .unwrap_or_default();
        Self { path, runs }
    }

    /// Whether this turn of the run was opened.
    pub fn has(&self, run: &Run) -> bool {
        self.runs.get(&run.session) == Some(&turn(run))
    }

    /// Keeps the run as seen at its current turn, with the others still among `sessions`,
    /// and writes them down.
    pub fn mark<'a>(&mut self, run: &Run, sessions: impl IntoIterator<Item = &'a str>) {
        self.runs.insert(run.session.clone(), turn(run));
        let keep: Vec<&str> = sessions.into_iter().collect();
        self.runs
            .retain(|session, _| *session == run.session || keep.contains(&session.as_str()));
        let Some(path) = &self.path else {
            return;
        };
        let json = Value::Object(
            self.runs
                .iter()
                .map(|(session, turn)| (session.clone(), Value::String(turn.clone())))
                .collect(),
        );
        if let Some(dir) = path.parent() {
            let _ = fs::create_dir_all(dir);
        }
        let _ = fs::write(path, json.to_string());
    }
}

/// What tells one turn of a run from the next: the message Desktop summed up, else when it
/// was last active.
fn turn(run: &Run) -> String {
    run.summary_for.clone().unwrap_or_else(|| {
        run.active
            .and_then(|at| at.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or_else(String::new, |at| at.as_millis().to_string())
    })
}

/// Where grove keeps what it remembers, on each system.
fn file(home: Option<&Path>, var: impl Fn(&str) -> Option<std::ffi::OsString>) -> Option<PathBuf> {
    let dir = if cfg!(target_os = "macos") {
        home.map(|home| home.join("Library/Application Support"))
    } else if cfg!(windows) {
        var("LOCALAPPDATA").map(PathBuf::from)
    } else {
        var("XDG_STATE_HOME")
            .filter(|dir| !dir.is_empty())
            .map(PathBuf::from)
            .or_else(|| home.map(|home| home.join(".local/state")))
    }?;
    Some(dir.join("grove").join("seen.json"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::tests::Scratch;

    fn run(session: &str, summary_for: &str) -> Run {
        Run {
            session: session.into(),
            conversation: None,
            title: None,
            cwd: None,
            started: None,
            active: None,
            status: Some("blocked".into()),
            detail: None,
            needs: None,
            error: None,
            archived: false,
            focused: None,
            summary_for: Some(summary_for.into()),
            answered: false,
        }
    }

    #[test]
    fn remembers_the_runs_opened_until_they_ask_again() {
        let dir = Scratch::new("seen");
        let path = dir.0.join("grove/seen.json");
        let mut seen = Seen {
            path: Some(path.clone()),
            runs: HashMap::new(),
        };
        let first = run("local_a", "u1");
        assert!(!seen.has(&first));
        seen.mark(&first, ["local_a", "local_b"]);
        assert!(seen.has(&first));
        // Written down, and read back as it was.
        let text = fs::read_to_string(&path).unwrap();
        let again = Seen {
            path: Some(path.clone()),
            runs: serde_json::from_str::<HashMap<String, String>>(&text).unwrap(),
        };
        assert!(again.has(&first));
        // The same run asking something new waits again.
        assert!(!seen.has(&run("local_a", "u2")));
        // Runs gone from Desktop are let go.
        seen.mark(&run("local_b", "u9"), ["local_b"]);
        assert!(!seen.has(&first));
    }
}
