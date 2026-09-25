//! Finds the git repositories on this computer and lists their worktrees.
//!
//! Repositories come from the folders given on the command line (or the usual code folders),
//! from the projects Claude Code and Codex remember, and from the folders where agent sessions
//! run right now. Only the ones with linked worktrees, or with a session in them, are kept.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::git;
use crate::model::{Repo, Worktree};

/// Folders in the home folder where code usually lives.
const USUAL_ROOTS: &[&str] = &[
    "Workspace",
    "workspace",
    "Projects",
    "projects",
    "Developer",
    "dev",
    "Dev",
    "src",
    "code",
    "Code",
    "repos",
    "git",
    "work",
];

/// Folders never worth descending into when looking for repositories.
const SKIP: &[&str] = &["node_modules", "target", "vendor", "venv", "Library"];

/// Home folders that macOS guards with a permission prompt: reading them unasked would pop
/// dialogs, so only folders given explicitly are searched there.
const PROTECTED: &[&str] = &[
    "Desktop",
    "Documents",
    "Downloads",
    "Library",
    "Movies",
    "Music",
    "Pictures",
];

/// The usual code folders that exist in the home folder.
pub fn default_roots(home: Option<&Path>) -> Vec<PathBuf> {
    let Some(home) = home else {
        return Vec::new();
    };
    let mut roots: Vec<PathBuf> = Vec::new();
    for name in USUAL_ROOTS {
        if let Some(real) = canonical(&home.join(name))
            && real.is_dir()
            && !roots.contains(&real)
        {
            roots.push(real);
        }
    }
    roots
}

/// Reads the project folders out of an agent's settings file.
type ProjectReader = fn(&[u8]) -> Vec<PathBuf>;

pub struct Finder {
    roots: Vec<PathBuf>,
    depth: usize,
    home: Option<PathBuf>,
    /// Folders whose repository is always listed, like the one grove started in.
    always: Vec<PathBuf>,
    agent_configs: Vec<(PathBuf, ProjectReader)>,
    cache: HashMap<PathBuf, (Option<SystemTime>, Vec<PathBuf>)>,
}

impl Finder {
    pub fn new(roots: Vec<PathBuf>, depth: usize, home: Option<PathBuf>) -> Self {
        let env = |name: &str| std::env::var_os(name).filter(|value| !value.is_empty());
        let mut agent_configs: Vec<(PathBuf, ProjectReader)> = Vec::new();
        let claude_json = match env("CLAUDE_CONFIG_DIR") {
            Some(dir) => Some(PathBuf::from(dir).join(".claude.json")),
            None => home.as_ref().map(|home| home.join(".claude.json")),
        };
        let codex_config = env("CODEX_HOME")
            .map(PathBuf::from)
            .or_else(|| home.as_ref().map(|home| home.join(".codex")))
            .map(|dir| dir.join("config.toml"));
        agent_configs.extend(claude_json.map(|file| (file, claude_projects as ProjectReader)));
        agent_configs.extend(codex_config.map(|file| (file, codex_projects as ProjectReader)));
        Self {
            roots,
            depth,
            home,
            always: Vec::new(),
            agent_configs,
            cache: HashMap::new(),
        }
    }

    /// Lists the repository holding `folder` too, worktrees or not.
    pub fn also(mut self, folder: Option<PathBuf>) -> Self {
        self.always.extend(folder);
        self
    }

    #[cfg(test)]
    fn without_agent_configs(mut self) -> Self {
        self.agent_configs.clear();
        self
    }

    /// The repositories with linked worktrees, plus the ones holding `folders`.
    pub fn find(&mut self, folders: &[PathBuf]) -> Vec<Repo> {
        let home = self.home.clone();
        let home = home.as_deref();
        let mut candidates: Vec<PathBuf> = Vec::new();
        for root in &self.roots {
            scan(root, self.depth, &mut candidates);
        }
        let known = self.known_projects();
        candidates.extend(known.into_iter().filter(|dir| !protected(dir, home)));
        let mut wanted = HashSet::new();
        let always = self.always.clone();
        for folder in always.iter().chain(folders) {
            if !protected(folder, home)
                && let Some(common) = common_dir(folder)
            {
                wanted.insert(common);
                candidates.push(folder.clone());
            }
        }

        let mut seen = HashSet::new();
        let mut repos = Vec::new();
        for dir in candidates {
            let Some(common) = common_dir(&dir) else {
                continue;
            };
            if !seen.insert(common.clone()) {
                continue;
            }
            if !wanted.contains(&common) && !has_linked_worktrees(&common) {
                continue;
            }
            let Ok(repo) = list(&common) else {
                continue;
            };
            // A repository at the home folder (dotfiles) would mean measuring the whole home.
            let holds_home = repo
                .worktrees
                .iter()
                .any(|w| w.main && home.is_some_and(|home| home.starts_with(&w.real)));
            if !holds_home {
                repos.push(repo);
            }
        }
        repos.sort_by_cached_key(|repo| (repo.name.to_lowercase(), repo.common_dir.clone()));
        repos
    }

    /// The project folders Claude Code and Codex remember, reread only when their files change.
    fn known_projects(&mut self) -> Vec<PathBuf> {
        let mut dirs = Vec::new();
        for (file, read) in &self.agent_configs {
            let modified = fs::metadata(file).and_then(|meta| meta.modified()).ok();
            match self.cache.get(file) {
                Some((seen, cached)) if *seen == modified => dirs.extend(cached.iter().cloned()),
                _ => {
                    let found = fs::read(file).map(|bytes| read(&bytes)).unwrap_or_default();
                    dirs.extend(found.iter().cloned());
                    self.cache.insert(file.clone(), (modified, found));
                }
            }
        }
        dirs
    }
}

/// `~/.claude.json` keeps a `projects` object keyed by folder.
fn claude_projects(bytes: &[u8]) -> Vec<PathBuf> {
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(bytes) else {
        return Vec::new();
    };
    value
        .get("projects")
        .and_then(|projects| projects.as_object())
        .map(|projects| projects.keys().map(PathBuf::from).collect())
        .unwrap_or_default()
}

/// `~/.codex/config.toml` has a `[projects."<folder>"]` table per trusted folder.
fn codex_projects(bytes: &[u8]) -> Vec<PathBuf> {
    String::from_utf8_lossy(bytes)
        .lines()
        .filter_map(|line| {
            let rest = line.trim().strip_prefix("[projects.\"")?;
            rest.strip_suffix("\"]").map(PathBuf::from)
        })
        .collect()
}

/// Collects the folders under `dir` that hold a repository, without descending into them.
fn scan(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    if dir.join(".git").exists() || is_bare(dir) {
        out.push(dir.to_path_buf());
        return;
    }
    if depth == 0 {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut subdirs: Vec<PathBuf> = entries
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .filter(|entry| {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            !name.starts_with('.') && !SKIP.contains(&name.as_ref())
        })
        .map(|entry| entry.path())
        .collect();
    subdirs.sort();
    for subdir in subdirs {
        scan(&subdir, depth - 1, out);
    }
}

/// The git directory shared by every worktree of the repository holding `dir`.
pub fn common_dir(dir: &Path) -> Option<PathBuf> {
    let mut current = Some(dir);
    while let Some(dir) = current {
        let dot_git = dir.join(".git");
        if let Ok(meta) = fs::metadata(&dot_git) {
            if meta.is_dir() {
                return canonical(&dot_git);
            }
            if meta.is_file() {
                return common_dir_of_gitfile(&dot_git);
            }
        }
        if is_bare(dir) {
            return canonical(dir);
        }
        current = dir.parent();
    }
    None
}

/// A linked worktree's `.git` file points at its folder inside the main git directory, whose
/// `commondir` leads back to the main one. A submodule's has no `commondir`.
fn common_dir_of_gitfile(file: &Path) -> Option<PathBuf> {
    let text = fs::read_to_string(file).ok()?;
    let target = text.lines().find_map(|line| line.strip_prefix("gitdir:"))?;
    let git_dir = file.parent()?.join(target.trim());
    match fs::read_to_string(git_dir.join("commondir")) {
        Ok(common) => canonical(&git_dir.join(common.trim())),
        Err(_) => canonical(&git_dir),
    }
}

fn is_bare(dir: &Path) -> bool {
    dir.join("HEAD").is_file() && dir.join("objects").is_dir() && dir.join("refs").is_dir()
}

fn has_linked_worktrees(common: &Path) -> bool {
    fs::read_dir(common.join("worktrees")).is_ok_and(|entries| {
        entries
            .flatten()
            .any(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
    })
}

/// The repository whose git directory is `common`, with its worktrees as git lists them.
pub fn list(common: &Path) -> Result<Repo, String> {
    let output = git::run(common, ["worktree", "list", "--porcelain", "-z"])?;
    let entries = parse_porcelain(&output);
    let bare = entries.first().is_some_and(|entry| entry.bare);
    let admin = admin_dirs(common);
    let mut worktrees = Vec::new();
    for (i, entry) in entries.into_iter().enumerate() {
        if entry.bare {
            continue;
        }
        let main = i == 0;
        let real = canonical(&entry.path).unwrap_or_else(|| entry.path.clone());
        let touched = if main {
            touched(common)
        } else {
            admin
                .get(&real)
                .or_else(|| admin.get(&entry.path))
                .and_then(|dir| touched(dir))
        };
        worktrees.push(Worktree {
            path: entry.path,
            real,
            head: entry.head,
            branch: entry.branch,
            detached: entry.detached,
            locked: entry.locked,
            prunable: entry.prunable,
            main,
            touched,
        });
    }
    Ok(Repo {
        name: repo_name(common, &worktrees),
        common_dir: common.to_path_buf(),
        bare,
        worktrees,
    })
}

fn repo_name(common: &Path, worktrees: &[Worktree]) -> String {
    let dir = worktrees
        .iter()
        .find(|w| w.main)
        .map_or(common, |w| w.path.as_path());
    let name = dir.file_name().map_or_else(
        || dir.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    );
    match name.strip_suffix(".git") {
        Some(stem) if !stem.is_empty() => stem.to_string(),
        _ => name,
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Entry {
    pub path: PathBuf,
    pub head: Option<String>,
    pub branch: Option<String>,
    pub detached: bool,
    pub bare: bool,
    pub locked: Option<String>,
    pub prunable: Option<String>,
}

/// Reads `git worktree list --porcelain -z`: attributes separated by NUL, worktrees by an
/// empty attribute.
pub fn parse_porcelain(output: &[u8]) -> Vec<Entry> {
    let text = String::from_utf8_lossy(output);
    let mut entries = Vec::new();
    let mut current: Option<Entry> = None;
    for field in text.split('\0') {
        if field.is_empty() {
            entries.extend(current.take());
            continue;
        }
        let (key, value) = field.split_once(' ').unwrap_or((field, ""));
        if key == "worktree" {
            entries.extend(current.take());
            current = Some(Entry {
                path: PathBuf::from(value),
                ..Entry::default()
            });
            continue;
        }
        let Some(entry) = current.as_mut() else {
            continue;
        };
        match key {
            "HEAD" if value.bytes().any(|b| b != b'0') => entry.head = Some(value.to_string()),
            "branch" => {
                entry.branch = Some(
                    value
                        .strip_prefix("refs/heads/")
                        .unwrap_or(value)
                        .to_string(),
                );
            }
            "detached" => entry.detached = true,
            "bare" => entry.bare = true,
            "locked" => entry.locked = Some(value.to_string()),
            "prunable" => entry.prunable = Some(value.to_string()),
            _ => {}
        }
    }
    entries.extend(current);
    entries
}

/// Maps each linked worktree folder to its own folder inside the git directory.
fn admin_dirs(common: &Path) -> HashMap<PathBuf, PathBuf> {
    let mut dirs = HashMap::new();
    let Ok(entries) = fs::read_dir(common.join("worktrees")) else {
        return dirs;
    };
    for entry in entries.flatten() {
        let admin = entry.path();
        let Ok(gitdir) = fs::read_to_string(admin.join("gitdir")) else {
            continue;
        };
        let gitfile = admin.join(gitdir.trim());
        let Some(worktree) = gitfile.parent() else {
            continue;
        };
        let worktree = canonical(worktree).unwrap_or_else(|| worktree.to_path_buf());
        dirs.insert(worktree, admin);
    }
    dirs
}

/// When git last wrote the index or HEAD of a worktree.
fn touched(git_dir: &Path) -> Option<SystemTime> {
    ["index", "HEAD", "logs/HEAD"]
        .iter()
        .filter_map(|file| {
            fs::metadata(git_dir.join(file))
                .and_then(|m| m.modified())
                .ok()
        })
        .max()
}

/// The path with symlinks resolved, in the form processes report their folders in.
pub fn canonical(path: &Path) -> Option<PathBuf> {
    let real = fs::canonicalize(path).ok()?;
    #[cfg(windows)]
    if let Some(plain) = real.to_str().and_then(|s| s.strip_prefix(r"\\?\")) {
        return Some(PathBuf::from(plain));
    }
    Some(real)
}

/// Whether reading `path` would make macOS ask for permission.
pub fn protected(path: &Path, home: Option<&Path>) -> bool {
    if !cfg!(target_os = "macos") {
        return false;
    }
    if path.starts_with("/Volumes") {
        return true;
    }
    let Some(rest) = home.and_then(|home| path.strip_prefix(home).ok()) else {
        return false;
    };
    rest.components()
        .next()
        .is_some_and(|first| PROTECTED.iter().any(|name| first.as_os_str() == *name))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::tests::{Scratch, repo_with_worktrees};

    #[test]
    fn reads_the_porcelain_worktree_list() {
        let output = b"worktree /code/app\0HEAD 461bb2ff\0branch refs/heads/main\0\0\
worktree /code/app/.claude/worktrees/x\0HEAD 903109d6\0branch refs/heads/feat/x\0locked agent running\0\0\
worktree /code/wt-detached\0HEAD e445a0a6\0detached\0locked\0\0\
worktree /code/wt-gone\0HEAD 0000000000000000000000000000000000000000\0branch refs/heads/gone\0prunable gitdir file points to non-existent location\0\0";
        let entries = parse_porcelain(output);
        assert_eq!(entries.len(), 4);
        assert_eq!(entries[0].path, PathBuf::from("/code/app"));
        assert_eq!(entries[0].branch.as_deref(), Some("main"));
        assert_eq!(entries[1].branch.as_deref(), Some("feat/x"));
        assert_eq!(entries[1].locked.as_deref(), Some("agent running"));
        assert!(entries[2].detached && entries[2].branch.is_none());
        assert_eq!(entries[2].locked.as_deref(), Some(""));
        assert_eq!(entries[3].head, None, "an unborn branch has no HEAD");
        assert!(entries[3].prunable.is_some());
    }

    #[test]
    fn a_bare_repository_lists_itself_first() {
        let entries = parse_porcelain(
            b"worktree /srv/app.git\0bare\0\0worktree /srv/wt\0HEAD 1234\0branch refs/heads/x\0\0",
        );
        assert!(entries[0].bare);
        assert!(!entries[1].bare);
    }

    #[test]
    fn finds_the_common_dir_from_any_worktree_folder() {
        let scratch = Scratch::new("common");
        let repo = repo_with_worktrees(&scratch, &["one"]);
        let common = repo.join(".git").canonicalize().unwrap();
        assert_eq!(common_dir(&repo).as_deref(), Some(common.as_path()));
        let deep = scratch.0.join("one/some/deep/folder");
        fs::create_dir_all(&deep).unwrap();
        assert_eq!(common_dir(&deep).as_deref(), Some(common.as_path()));
        assert_eq!(common_dir(&scratch.0), None);
    }

    #[test]
    fn lists_worktrees_with_their_state() {
        let scratch = Scratch::new("list");
        let repo = repo_with_worktrees(&scratch, &["alive", "gone"]);
        fs::remove_dir_all(scratch.0.join("gone")).unwrap();
        let listed = list(&repo.join(".git")).unwrap();
        assert_eq!(listed.name, "repo");
        assert!(!listed.bare);
        let names: Vec<_> = listed.worktrees.iter().map(|w| w.path.clone()).collect();
        assert_eq!(
            names,
            [
                repo.clone(),
                scratch.0.join("alive"),
                scratch.0.join("gone")
            ]
        );
        assert!(listed.worktrees[0].main && !listed.worktrees[1].main);
        assert_eq!(listed.worktrees[1].branch.as_deref(), Some("alive"));
        assert!(listed.worktrees[1].touched.is_some());
        assert!(listed.worktrees[2].missing());
        assert_eq!(listed.command_dir(), repo.as_path());
    }

    #[test]
    fn keeps_repositories_with_worktrees_or_agents() {
        let scratch = Scratch::new("find");
        repo_with_worktrees(&scratch, &["wt"]);
        for name in ["plain", "agent"] {
            let dir = scratch.0.join(name);
            fs::create_dir_all(&dir).unwrap();
            git::run(&dir, ["init", "-q"]).unwrap();
        }
        let deep = scratch.0.join("a/b/c/d/deep");
        fs::create_dir_all(&deep).unwrap();
        git::run(&deep, ["init", "-q"]).unwrap();
        git::run(
            &deep,
            ["worktree", "add", "-q", "--orphan", "-b", "x", "../x"],
        )
        .unwrap();

        let mut finder = Finder::new(vec![scratch.0.clone()], 3, None).without_agent_configs();
        let names = |repos: Vec<Repo>| repos.into_iter().map(|r| r.name).collect::<Vec<_>>();
        // `plain` has no linked worktree and `deep` is below the search depth.
        assert_eq!(names(finder.find(&[])), ["repo"]);
        let agent = scratch.0.join("agent");
        assert_eq!(names(finder.find(&[agent])), ["agent", "repo"]);
    }

    #[test]
    fn reads_the_projects_agents_remember() {
        let claude = br#"{"numStartups": 3, "projects": {"/code/a": {"x": 1}, "/code/b": {}}}"#;
        assert_eq!(
            claude_projects(claude),
            [PathBuf::from("/code/a"), PathBuf::from("/code/b")]
        );
        assert!(claude_projects(b"not json").is_empty());
        let codex =
            b"model = \"gpt\"\n[projects.\"/code/a\"]\ntrust_level = \"trusted\"\n[mcp.x]\n";
        assert_eq!(codex_projects(codex), [PathBuf::from("/code/a")]);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn stays_out_of_folders_macos_guards() {
        let home = Path::new("/Users/me");
        assert!(protected(Path::new("/Users/me/Documents/app"), Some(home)));
        assert!(protected(
            Path::new("/Users/me/Library/Mobile Documents/x"),
            Some(home)
        ));
        assert!(protected(Path::new("/Volumes/usb/app"), Some(home)));
        assert!(!protected(Path::new("/Users/me/Workspace/app"), Some(home)));
        assert!(!protected(Path::new("/Users/me/Documentsx"), Some(home)));
    }
}
