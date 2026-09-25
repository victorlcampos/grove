//! Runs git: worktree status and removal.

use std::ffi::OsStr;
use std::path::Path;
use std::process::{Command, Stdio};

/// A git command run in `dir` that never waits for a password and never takes the index lock
/// from an agent committing in the same worktree.
pub fn command(dir: &Path) -> Command {
    let mut command = Command::new("git");
    command
        .arg("-C")
        .arg(dir)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null());
    command
}

/// Runs git and returns its output, or what it printed on failure.
pub fn run<I, S>(dir: &Path, args: I) -> Result<Vec<u8>, String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let output = command(dir)
        .args(args)
        .output()
        .map_err(|error| format!("git: {error}"))?;
    if output.status.success() {
        return Ok(output.stdout);
    }
    let message = String::from_utf8_lossy(&output.stderr).trim().to_string();
    Err(if message.is_empty() {
        format!("git: {}", output.status)
    } else {
        message
    })
}

pub fn available() -> bool {
    Command::new("git")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Status {
    /// Files changed in the index or the working tree, renames included.
    pub changed: u32,
    pub untracked: u32,
    pub conflicts: u32,
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    pub commit: Option<Commit>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Commit {
    /// Seconds since the Unix epoch.
    pub time: i64,
    pub hash: String,
    pub subject: String,
}

impl Status {
    /// Files `git worktree remove` would refuse to delete without `--force`.
    pub fn at_risk(&self) -> u32 {
        self.changed + self.untracked + self.conflicts
    }
}

pub fn status(dir: &Path) -> Result<Status, String> {
    let output = run(
        dir,
        [
            "status",
            "--porcelain=v2",
            "--branch",
            "-z",
            "--untracked-files=normal",
        ],
    )?;
    let mut status = parse_status(&output);
    // Fails on a branch without commits, which is fine.
    status.commit = run(dir, ["log", "-1", "--format=%ct%x00%h%x00%s"])
        .ok()
        .and_then(|output| parse_commit(&output));
    Ok(status)
}

/// Reads `git status --porcelain=v2 --branch -z`.
pub fn parse_status(output: &[u8]) -> Status {
    let text = String::from_utf8_lossy(output);
    let mut status = Status::default();
    let mut fields = text.split('\0');
    while let Some(field) = fields.next() {
        if let Some(upstream) = field.strip_prefix("# branch.upstream ") {
            status.upstream = Some(upstream.to_string());
        } else if let Some(counts) = field.strip_prefix("# branch.ab ") {
            for count in counts.split(' ') {
                if let Some(n) = count.strip_prefix('+') {
                    status.ahead = n.parse().unwrap_or(0);
                } else if let Some(n) = count.strip_prefix('-') {
                    status.behind = n.parse().unwrap_or(0);
                }
            }
        } else if field.starts_with("1 ") {
            status.changed += 1;
        } else if field.starts_with("2 ") {
            status.changed += 1;
            // A rename carries the original path as its own field.
            fields.next();
        } else if field.starts_with("u ") {
            status.conflicts += 1;
        } else if field.starts_with("? ") {
            status.untracked += 1;
        }
    }
    status
}

fn parse_commit(output: &[u8]) -> Option<Commit> {
    let text = String::from_utf8_lossy(output);
    let mut parts = text.trim_end_matches('\n').splitn(3, '\0');
    let time = parts.next()?.trim().parse().ok()?;
    let hash = parts.next()?.to_string();
    let subject = parts.next().unwrap_or_default().to_string();
    Some(Commit {
        time,
        hash,
        subject,
    })
}

/// How hard `git worktree remove` pushes. A removal always forces: removing is the decision,
/// so changed and untracked files go too, and git asks for the force twice to remove a
/// locked worktree.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Force {
    /// `--force`: deletes changed and untracked files too.
    Once,
    /// `--force --force`: also removes a locked worktree.
    Twice,
}

impl Force {
    pub fn needed(locked: bool) -> Self {
        if locked { Force::Twice } else { Force::Once }
    }

    fn flags(self) -> &'static [&'static str] {
        match self {
            Force::Once => &["--force"],
            Force::Twice => &["--force", "--force"],
        }
    }
}

/// The removal command as the user would type it, with `~` for the home folder.
pub fn remove_command(dir: &str, worktree: &str, force: Force) -> String {
    let mut words = vec![
        "git".to_string(),
        "-C".into(),
        quote(dir),
        "worktree".into(),
        "remove".into(),
    ];
    words.extend(force.flags().iter().map(|flag| flag.to_string()));
    words.push(quote(worktree));
    words.join(" ")
}

fn quote(word: &str) -> String {
    let plain = |c: char| c.is_ascii_alphanumeric() || "~/._-+=:,@%".contains(c);
    if !word.is_empty() && word.chars().all(plain) {
        word.to_string()
    } else {
        format!("'{}'", word.replace('\'', r"'\''"))
    }
}

/// Removes the worktree with `git worktree remove`, run from the repository's main worktree.
pub fn remove(dir: &Path, worktree: &Path, force: Force) -> Result<(), String> {
    let mut args: Vec<&OsStr> = vec![OsStr::new("worktree"), OsStr::new("remove")];
    args.extend(force.flags().iter().map(OsStr::new));
    args.push(worktree.as_os_str());
    run(dir, args).map(|_| ())
}

#[cfg(test)]
pub(crate) mod tests {
    use std::fs;
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn reads_the_porcelain_status() {
        let output = b"# branch.oid 903109d6\0# branch.head feat/x\0\
# branch.upstream origin/feat/x\0# branch.ab +3 -1\0\
1 .M N... 100644 100644 100644 aaa bbb Gemfile\0\
2 R. N... 100644 100644 100644 aaa bbb R100 new.rb\0old.rb\0\
u UU N... 100644 100644 100644 100644 aaa bbb ccc conflict.rb\0\
? notes.txt\0? tmp/\0";
        let status = parse_status(output);
        assert_eq!(
            status,
            Status {
                changed: 2,
                untracked: 2,
                conflicts: 1,
                upstream: Some("origin/feat/x".into()),
                ahead: 3,
                behind: 1,
                commit: None,
            }
        );
        assert_eq!(status.at_risk(), 5);
    }

    #[test]
    fn a_new_branch_has_no_upstream_nor_changes() {
        let status = parse_status(b"# branch.oid (initial)\0# branch.head probe\0");
        assert_eq!(status, Status::default());
    }

    #[test]
    fn reads_the_last_commit() {
        let commit = parse_commit(b"1790350000\0903109d\0feat: add grove\n").unwrap();
        assert_eq!(commit.time, 1_790_350_000);
        assert_eq!(commit.hash, "903109d");
        assert_eq!(commit.subject, "feat: add grove");
        assert!(parse_commit(b"").is_none());
    }

    #[test]
    fn builds_the_command_the_user_would_type() {
        assert_eq!(
            remove_command("~/code/app", "~/code/app/.claude/worktrees/x", Force::Once),
            "git -C ~/code/app worktree remove --force ~/code/app/.claude/worktrees/x"
        );
        assert_eq!(
            remove_command("~/code/app", "~/wt/it's here", Force::Twice),
            r"git -C ~/code/app worktree remove --force --force '~/wt/it'\''s here'"
        );
        assert_eq!(Force::needed(false), Force::Once);
        assert_eq!(Force::needed(true), Force::Twice);
    }

    /// A scratch folder removed when dropped.
    pub(crate) struct Scratch(pub PathBuf);

    impl Scratch {
        pub(crate) fn new(name: &str) -> Self {
            static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
            let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let dir =
                std::env::temp_dir().join(format!("grove-test-{name}-{}-{n}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            Self(dir.canonicalize().unwrap())
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// A repository with no commits and one worktree per name, each on its own orphan branch
    /// (so no commit is needed).
    pub(crate) fn repo_with_worktrees(scratch: &Scratch, names: &[&str]) -> PathBuf {
        let repo = scratch.0.join("repo");
        fs::create_dir_all(&repo).unwrap();
        run(&repo, ["init", "-q"]).unwrap();
        for name in names {
            let path = scratch.0.join(name);
            run(
                &repo,
                [
                    "worktree",
                    "add",
                    "-q",
                    "--orphan",
                    "-b",
                    name,
                    path.to_str().unwrap(),
                ],
            )
            .unwrap();
        }
        repo
    }

    #[test]
    fn removes_worktrees_with_the_force_each_case_needs() {
        let scratch = Scratch::new("remove");
        let repo = repo_with_worktrees(&scratch, &["clean", "dirty", "locked", "gone"]);
        let wt = |name: &str| scratch.0.join(name);

        remove(&repo, &wt("clean"), Force::needed(false)).unwrap();
        assert!(!wt("clean").exists());

        // Without --force git refuses a worktree with untracked files; grove always forces.
        let dirty = wt("dirty");
        fs::write(dirty.join("notes.txt"), "draft").unwrap();
        let plain = ["worktree", "remove"].map(OsStr::new);
        let refused = run(&repo, plain.iter().copied().chain([dirty.as_os_str()])).unwrap_err();
        assert!(refused.contains("--force"), "{refused}");
        assert_eq!(status(&dirty).unwrap().untracked, 1);
        remove(&repo, &dirty, Force::needed(false)).unwrap();
        assert!(!dirty.exists());

        let locked = wt("locked");
        let lock = ["worktree", "lock", "--reason", "agent"].map(OsStr::new);
        run(&repo, lock.iter().copied().chain([locked.as_os_str()])).unwrap();
        remove(&repo, &locked, Force::Once).unwrap_err();
        remove(&repo, &locked, Force::needed(true)).unwrap();
        assert!(!locked.exists());

        // A folder deleted by hand: the removal only clears git's record.
        fs::remove_dir_all(wt("gone")).unwrap();
        remove(&repo, &wt("gone"), Force::needed(false)).unwrap();

        // Git never removes the main worktree, whatever the force.
        remove(&repo, &repo, Force::Twice).unwrap_err();
        assert!(repo.join(".git").exists());
        let list = String::from_utf8(run(&repo, ["worktree", "list"]).unwrap()).unwrap();
        assert_eq!(list.lines().count(), 1, "{list}");
    }
}
