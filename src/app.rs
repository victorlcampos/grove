//! The state behind the screen: what was found, what is selected, and what the keys do.

use std::cmp::Ordering as Order;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime};

use ratatui::crossterm::event::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::{Position, Rect};

use crate::agents::{self, Usage as Presence};
use crate::clipboard::Clipboard;
use crate::du::{Usage, Volume};
use crate::fmt;
use crate::git::{Force, Status};
use crate::history;
use crate::i18n::{Lang, Text};
use crate::model::{Activity, Agent, Conversation, Repo, Scan, Session, Worktree, worktree_name};
use crate::theme::Theme;
use crate::worker::{Msg, RemoveJob, SizeJob, Workers};

/// How long a measurement stays fresh: an agent at work changes the files; an idle worktree
/// hardly does, and measuring costs disk reads.
const SIZE_TTL: Duration = Duration::from_secs(60 * 60);
const WORKING_SIZE_TTL: Duration = Duration::from_secs(5 * 60);
const STATUS_TTL: Duration = Duration::from_secs(120);
const SELECTED_STATUS_TTL: Duration = Duration::from_secs(15);
const TOAST_TTL: Duration = Duration::from_secs(6);
/// A removed worktree stays hidden this long, in case a listing made before the removal
/// arrives after it.
const GONE_TTL: Duration = Duration::from_secs(30);
/// A Codex or OpenCode session has the latest conversation of its folder open when that was
/// written since the session started, give or take this much.
const START_SLACK: Duration = Duration::from_secs(2);

/// Which list is on screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum View {
    Worktrees,
    /// The conversations agents keep, to pick one up again.
    Sessions,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sort {
    Activity,
    Size,
    Name,
    Oldest,
}

impl Sort {
    fn next(self) -> Self {
        match self {
            Sort::Activity => Sort::Size,
            Sort::Size => Sort::Name,
            Sort::Name => Sort::Oldest,
            Sort::Oldest => Sort::Activity,
        }
    }

    pub fn index(self) -> usize {
        self as usize
    }
}

/// What a worktree shows at a glance, most pressing first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum State {
    Removing,
    /// An agent session there waits for you.
    Blocked,
    Working,
    /// An agent session is open there, doing nothing right now.
    Idle,
    /// No agent, but some process runs there.
    Busy,
    Free,
    /// The folder is gone; git still lists it.
    Missing,
}

#[derive(Debug, Default)]
pub struct SizeState {
    /// The last complete measurement.
    pub usage: Option<Usage>,
    pub at: Option<Instant>,
    /// Bytes and files counted so far by the measurement in progress.
    pub running: Option<(u64, u64)>,
    pub queued: bool,
    cancel: Option<Arc<AtomicBool>>,
}

impl SizeState {
    pub fn bytes(&self) -> Option<u64> {
        self.usage.as_ref().map(|usage| usage.bytes)
    }

    pub fn busy(&self) -> bool {
        self.queued || self.running.is_some()
    }
}

#[derive(Debug, Default)]
pub struct GitState {
    pub status: Option<Result<Status, String>>,
    pub at: Option<Instant>,
    pub pending: bool,
}

pub enum Mode {
    List,
    Filter,
    Help,
    /// The details of the selected worktree over the whole screen, when the window has no
    /// room for them beside the list.
    Details,
    Confirm(Confirm),
}

/// A removal waiting for a yes: the selected worktree, or every idle one.
pub struct Confirm {
    pub targets: Vec<PathBuf>,
    pub idle: bool,
}

/// Removals started together, reported with one message when the last one ends.
#[derive(Debug, Default)]
pub struct Batch {
    pub pending: HashSet<PathBuf>,
    pub removed: usize,
    pub failed: usize,
    pub freed: u64,
    pub error: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToastKind {
    Ok,
    Error,
    Info,
}

pub struct Toast {
    pub kind: ToastKind,
    pub text: String,
    pub at: Instant,
}

/// A row of the list: a repository heading or one of its worktrees.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Item {
    Repo(usize),
    Tree(usize, usize),
}

/// A row of the sessions list: a project heading or one of its conversations, each by the
/// index of a conversation in `App::history` (the project's first one, for a heading).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Row {
    Project(usize),
    Conversation(usize),
}

/// Counts for the header.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Totals {
    pub worktrees: usize,
    pub working: usize,
    pub blocked: usize,
    pub idle: usize,
    pub bytes: u64,
    pub measuring: usize,
}

pub struct App {
    pub lang: Lang,
    pub text: &'static Text,
    pub theme: Theme,
    pub home: Option<PathBuf>,
    pub roots: Vec<PathBuf>,
    pub repos: Vec<Repo>,
    /// Whether the first search for repositories finished.
    pub found: bool,
    pub scan: Scan,
    pub usage: HashMap<PathBuf, Presence>,
    pub sizes: HashMap<PathBuf, SizeState>,
    pub status: HashMap<PathBuf, GitState>,
    pub volume: Option<Volume>,
    pub sort: Sort,
    pub filter: String,
    pub mode: Mode,
    pub show_details: bool,
    /// The selected worktree, by its resolved path, so it stays selected as the list changes.
    pub selected: Option<PathBuf>,
    /// First item on screen.
    pub offset: usize,
    pub removing: HashSet<PathBuf>,
    pub toasts: Vec<Toast>,
    /// Animation step, from the time.
    pub frame: usize,
    pub quit: bool,
    /// Where each list item was drawn, for the mouse.
    pub hits: Vec<(Rect, usize)>,
    /// Worktrees per page, from the last draw.
    pub page: usize,
    /// Whether the window has room for the details beside or under the list.
    pub details_room: bool,
    /// Clickable key hints and dialog buttons from the last draw, with the key each one
    /// stands for.
    pub buttons: Vec<(Rect, KeyCode)>,
    pub batch: Option<Batch>,
    pub view: View,
    /// The conversations agents keep, the most recent first.
    pub history: Vec<Conversation>,
    /// Whether the first look for conversations finished.
    pub history_found: bool,
    /// The selected conversation, by agent and id, so it stays selected as the list changes.
    pub chosen: Option<(Agent, String)>,
    /// First row of the sessions list on screen.
    pub history_offset: usize,
    pub clipboard: Clipboard,
    started: Instant,
    last_index: usize,
    history_index: usize,
    /// Whether each conversation is the latest of its agent in its folder: the one an agent
    /// that does not say which it has open (Codex, OpenCode) is taken to have when it runs
    /// there.
    newest: Vec<bool>,
    gone: HashMap<PathBuf, Instant>,
    looked_in: Vec<PathBuf>,
    workers: Workers,
}

impl App {
    pub fn new(
        lang: Lang,
        theme: Theme,
        home: Option<PathBuf>,
        roots: Vec<PathBuf>,
        workers: Workers,
    ) -> Self {
        Self {
            lang,
            text: lang.text(),
            theme,
            home,
            roots,
            repos: Vec::new(),
            found: false,
            scan: Scan::default(),
            usage: HashMap::new(),
            sizes: HashMap::new(),
            status: HashMap::new(),
            volume: None,
            sort: Sort::Activity,
            filter: String::new(),
            mode: Mode::List,
            show_details: true,
            selected: None,
            offset: 0,
            removing: HashSet::new(),
            toasts: Vec::new(),
            frame: 0,
            quit: false,
            hits: Vec::new(),
            page: 10,
            details_room: true,
            buttons: Vec::new(),
            batch: None,
            view: View::Worktrees,
            history: Vec::new(),
            history_found: false,
            chosen: None,
            history_offset: 0,
            clipboard: Clipboard::System,
            started: Instant::now(),
            last_index: 0,
            history_index: 0,
            newest: Vec::new(),
            gone: HashMap::new(),
            looked_in: Vec::new(),
            workers,
        }
    }

    pub fn handle(&mut self, msg: Msg) {
        match msg {
            Msg::Input(event) => self.on_event(event),
            Msg::Repos(repos) => self.on_repos(repos),
            Msg::Volume(volume) => self.volume = volume,
            Msg::Scan(scan) => self.on_scan(scan),
            Msg::History(history) => self.on_history(history),
            Msg::Measuring { path, bytes, files } => {
                if let Some(size) = self.sizes.get_mut(&path) {
                    size.queued = false;
                    size.running = Some((bytes, files));
                }
            }
            Msg::Measured { path, usage } => {
                if let Some(size) = self.sizes.get_mut(&path) {
                    size.queued = false;
                    size.running = None;
                    size.at = Some(Instant::now());
                    if usage.is_some() {
                        size.usage = usage;
                    }
                }
            }
            Msg::Status { path, status } => {
                let git = self.status.entry(path).or_default();
                git.pending = false;
                git.at = Some(Instant::now());
                git.status = Some(status);
            }
            Msg::Removed { path, result } => self.on_removed(path, result),
        }
    }

    /// Moves time on: animations, toasts, and the measurements and status checks due.
    pub fn tick(&mut self, now: Instant) {
        self.frame = (now.duration_since(self.started).as_millis() / 100) as usize;
        self.toasts
            .retain(|toast| now.duration_since(toast.at) < TOAST_TTL);
        self.gone.retain(|_, at| now.duration_since(*at) < GONE_TTL);
        self.schedule(now);
    }

    /// Whether something on screen moves, so the screen redraws often.
    pub fn animating(&self) -> bool {
        !self.found
            || (self.view == View::Sessions && !self.history_found)
            || !self.toasts.is_empty()
            || !self.removing.is_empty()
            || self.sizes.values().any(SizeState::busy)
            || self
                .scan
                .sessions
                .iter()
                .any(|s| s.activity == Activity::Working)
    }

    fn on_repos(&mut self, mut repos: Vec<Repo>) {
        for repo in &mut repos {
            repo.worktrees.retain(|w| !self.gone.contains_key(&w.real));
        }
        self.repos = repos;
        self.found = true;
        self.attribute();
        self.keep_selection();
    }

    fn on_scan(&mut self, scan: Scan) {
        self.scan = scan;
        self.attribute();
        let mut folders: Vec<PathBuf> = self.scan.sessions.iter().map(|s| s.cwd.clone()).collect();
        folders.sort();
        folders.dedup();
        if folders != self.looked_in {
            self.looked_in = folders.clone();
            self.workers.look_in(folders);
        }
        self.keep_selection();
    }

    fn on_history(&mut self, history: Vec<Conversation>) {
        let mut seen = HashSet::new();
        self.newest = history
            .iter()
            .map(|conversation| seen.insert((conversation.agent, conversation.cwd.clone())))
            .collect();
        self.history = history;
        self.history_found = true;
        self.keep_selection();
    }

    fn attribute(&mut self) {
        let worktrees: Vec<PathBuf> = self
            .repos
            .iter()
            .flat_map(|repo| repo.worktrees.iter())
            .filter(|w| !w.missing())
            .map(|w| w.real.clone())
            .collect();
        self.usage = agents::attribute(&self.scan, &worktrees);
    }

    pub fn presence(&self, worktree: &Worktree) -> Option<&Presence> {
        self.usage.get(&worktree.real)
    }

    pub fn state(&self, worktree: &Worktree) -> State {
        if self.removing.contains(&worktree.real) {
            return State::Removing;
        }
        if worktree.missing() {
            return State::Missing;
        }
        let Some(presence) = self.presence(worktree) else {
            return State::Free;
        };
        match presence.sessions.iter().map(|p| p.session.activity).min() {
            Some(Activity::Blocked) => State::Blocked,
            Some(Activity::Working) => State::Working,
            Some(Activity::Idle) => State::Idle,
            None if !presence.procs.is_empty() => State::Busy,
            None => State::Free,
        }
    }

    pub fn size(&self, worktree: &Worktree) -> Option<&SizeState> {
        self.sizes.get(&worktree.real)
    }

    pub fn git(&self, worktree: &Worktree) -> Option<&GitState> {
        self.status.get(&worktree.real)
    }

    /// When the worktree was last used: now if an agent is at work there.
    pub fn last_used(&self, worktree: &Worktree) -> Option<SystemTime> {
        match self.state(worktree) {
            State::Working | State::Blocked => Some(SystemTime::now()),
            _ => worktree.touched,
        }
    }

    /// How many worktrees the list has without a filter.
    pub fn listed_worktrees(&self) -> usize {
        self.repos
            .iter()
            .filter(|repo| self.repo_visible(repo))
            .map(|repo| repo.worktrees.len())
            .sum()
    }

    fn repo_visible(&self, repo: &Repo) -> bool {
        repo.linked() > 0
            || repo
                .worktrees
                .iter()
                .any(|w| self.presence(w).is_some_and(|p| !p.sessions.is_empty()))
    }

    fn matches(&self, repo: &Repo, worktree: &Worktree) -> bool {
        if self.filter.is_empty() {
            return true;
        }
        let needle = self.filter.to_lowercase();
        let found = |text: &str| text.to_lowercase().contains(&needle);
        found(&repo.name)
            || found(worktree_name(repo, worktree))
            || worktree.branch.as_deref().is_some_and(found)
            || found(&worktree.path.to_string_lossy())
            || self.presence(worktree).is_some_and(|p| {
                p.sessions
                    .iter()
                    .any(|p| p.session.name.as_deref().is_some_and(found))
            })
    }

    fn compare(&self, repo: &Repo, a: &Worktree, b: &Worktree) -> Order {
        let bytes = |w: &Worktree| self.size(w).and_then(SizeState::bytes).unwrap_or(0);
        let by_sort = match self.sort {
            Sort::Activity => self
                .state(a)
                .cmp(&self.state(b))
                .then_with(|| bytes(b).cmp(&bytes(a))),
            Sort::Size => bytes(b).cmp(&bytes(a)),
            Sort::Name => Order::Equal,
            Sort::Oldest => self.last_used(a).cmp(&self.last_used(b)),
        };
        b.main.cmp(&a.main).then(by_sort).then_with(|| {
            worktree_name(repo, a)
                .to_lowercase()
                .cmp(&worktree_name(repo, b).to_lowercase())
        })
    }

    /// The list: each repository with worktrees to show, followed by them in the chosen order.
    pub fn items(&self) -> Vec<Item> {
        let mut items = Vec::new();
        for (r, repo) in self.repos.iter().enumerate() {
            if !self.repo_visible(repo) {
                continue;
            }
            let mut trees: Vec<usize> = (0..repo.worktrees.len())
                .filter(|&w| self.matches(repo, &repo.worktrees[w]))
                .collect();
            if trees.is_empty() {
                continue;
            }
            trees.sort_by(|&a, &b| self.compare(repo, &repo.worktrees[a], &repo.worktrees[b]));
            items.push(Item::Repo(r));
            items.extend(trees.into_iter().map(|w| Item::Tree(r, w)));
        }
        items
    }

    pub fn totals(&self, items: &[Item]) -> Totals {
        let mut totals = Totals::default();
        let mut seen = HashSet::new();
        for item in items {
            let Item::Tree(r, w) = *item else { continue };
            let worktree = &self.repos[r].worktrees[w];
            totals.worktrees += 1;
            if let Some(size) = self.size(worktree) {
                totals.bytes += size.bytes().unwrap_or(0);
                totals.measuring += usize::from(size.busy());
            }
            for presence in self.presence(worktree).map_or(&[][..], |p| &p.sessions[..]) {
                if !seen.insert(presence.session.pid) {
                    continue;
                }
                match presence.session.activity {
                    Activity::Working => totals.working += 1,
                    Activity::Blocked => totals.blocked += 1,
                    Activity::Idle => totals.idle += 1,
                }
            }
        }
        totals
    }

    pub fn tree(&self, r: usize, w: usize) -> (&Repo, &Worktree) {
        let repo = &self.repos[r];
        (repo, &repo.worktrees[w])
    }

    pub fn find(&self, key: &PathBuf) -> Option<(usize, usize)> {
        self.repos.iter().enumerate().find_map(|(r, repo)| {
            repo.worktrees
                .iter()
                .position(|w| w.real == *key)
                .map(|w| (r, w))
        })
    }

    pub fn selected_index(&self, items: &[Item]) -> Option<usize> {
        let key = self.selected.as_ref()?;
        items.iter().position(|item| match *item {
            Item::Tree(r, w) => self.repos[r].worktrees[w].real == *key,
            Item::Repo(_) => false,
        })
    }

    pub fn selected_tree(&self) -> Option<(usize, usize)> {
        self.find(self.selected.as_ref()?)
            .filter(|_| self.selected_index(&self.items()).is_some())
    }

    fn select(&mut self, items: &[Item], index: usize) {
        if let Some(Item::Tree(r, w)) = items.get(index) {
            self.selected = Some(self.repos[*r].worktrees[*w].real.clone());
            self.last_index = index;
        }
    }

    /// The agent session that has the conversation open now. Claude Code says which one each
    /// session has; a Codex or OpenCode session has the latest conversation of its folder,
    /// once it wrote to it.
    pub fn live(&self, i: usize) -> Option<&Session> {
        let conversation = &self.history[i];
        let mut sessions = self
            .scan
            .sessions
            .iter()
            .filter(|session| session.agent == conversation.agent);
        if conversation.agent == Agent::Claude {
            return sessions
                .find(|session| session.conversation.as_ref() == Some(&conversation.id));
        }
        if !self.newest.get(i).copied().unwrap_or(false) {
            return None;
        }
        sessions.find(|session| {
            session.cwd == conversation.cwd
                && session
                    .started
                    .is_none_or(|started| started <= conversation.updated + START_SLACK)
        })
    }

    /// What a conversation shows at a glance: what the session that has it open is doing,
    /// else `Free` (closed), or `Missing` when the folder it resumes from is gone.
    pub fn conversation_state(&self, i: usize) -> State {
        match self.live(i).map(|session| session.activity) {
            Some(Activity::Blocked) => State::Blocked,
            Some(Activity::Working) => State::Working,
            Some(Activity::Idle) => State::Idle,
            None if self.history[i].stranded => State::Missing,
            None => State::Free,
        }
    }

    fn shows(&self, conversation: &Conversation) -> bool {
        if self.filter.is_empty() {
            return true;
        }
        let needle = self.filter.to_lowercase();
        let found = |text: &str| text.to_lowercase().contains(&needle);
        [
            conversation.title.as_deref(),
            conversation.first_prompt.as_deref(),
            conversation.last_prompt.as_deref(),
            conversation.branch.as_deref(),
            conversation.worktree.as_deref(),
            Some(conversation.project_name.as_str()),
            Some(conversation.agent.name()),
            Some(conversation.id.as_str()),
        ]
        .into_iter()
        .flatten()
        .any(found)
            || found(&conversation.cwd.to_string_lossy())
    }

    /// The sessions list: the projects, the one with the latest conversation first, each
    /// followed by its conversations, the open ones first and then the most recent.
    pub fn rows(&self) -> Vec<Row> {
        let mut order: Vec<usize> = (0..self.history.len())
            .filter(|&i| self.shows(&self.history[i]))
            .collect();
        // The history is the most recent first; a stable sort keeps that among equals.
        order.sort_by_cached_key(|&i| self.live(i).is_none());
        let mut groups: Vec<Vec<usize>> = Vec::new();
        let mut group_of: HashMap<&PathBuf, usize> = HashMap::new();
        for i in order {
            let group = *group_of.entry(&self.history[i].project).or_insert_with(|| {
                groups.push(Vec::new());
                groups.len() - 1
            });
            groups[group].push(i);
        }
        let mut rows = Vec::new();
        for group in groups {
            rows.push(Row::Project(group[0]));
            rows.extend(group.into_iter().map(Row::Conversation));
        }
        rows
    }

    pub fn chosen_index(&self, rows: &[Row]) -> Option<usize> {
        let (agent, id) = self.chosen.as_ref()?;
        rows.iter().position(|row| match *row {
            Row::Conversation(i) => self.history[i].agent == *agent && self.history[i].id == *id,
            Row::Project(_) => false,
        })
    }

    /// The selected conversation, when it is on the list.
    pub fn chosen_conversation(&self) -> Option<usize> {
        let rows = self.rows();
        match rows.get(self.chosen_index(&rows)?) {
            Some(Row::Conversation(i)) => Some(*i),
            _ => None,
        }
    }

    fn choose(&mut self, rows: &[Row], index: usize) {
        if let Some(Row::Conversation(i)) = rows.get(index) {
            let conversation = &self.history[*i];
            self.chosen = Some((conversation.agent, conversation.id.clone()));
            self.history_index = index;
        }
    }

    /// Keeps each list's selection on a row still listed, or the one now where it was.
    pub fn keep_selection(&mut self) {
        let items = self.items();
        let trees = selectable(&items, |item| matches!(item, Item::Tree(..)));
        match settle(self.selected_index(&items), &trees, self.last_index) {
            Some(index) => self.select(&items, index),
            None => self.selected = None,
        }
        let rows = self.rows();
        let conversations = selectable(&rows, |row| matches!(row, Row::Conversation(_)));
        match settle(self.chosen_index(&rows), &conversations, self.history_index) {
            Some(index) => self.choose(&rows, index),
            None => self.chosen = None,
        }
    }

    pub fn move_by(&mut self, delta: isize) {
        match self.view {
            View::Worktrees => {
                let items = self.items();
                let trees = selectable(&items, |item| matches!(item, Item::Tree(..)));
                if let Some(index) = step(self.selected_index(&items), &trees, delta) {
                    self.select(&items, index);
                }
            }
            View::Sessions => {
                let rows = self.rows();
                let conversations = selectable(&rows, |row| matches!(row, Row::Conversation(_)));
                if let Some(index) = step(self.chosen_index(&rows), &conversations, delta) {
                    self.choose(&rows, index);
                }
            }
        }
    }

    /// Copies the command that picks the selected conversation up again, to paste in a
    /// terminal.
    fn copy_command(&mut self) {
        let Some(i) = self.chosen_conversation() else {
            return;
        };
        let live = self.live(i);
        let conversation = &self.history[i];
        let command = history::command(conversation, live, self.home.as_deref());
        let note = match live {
            // Attaching to a background session opens that one, not a copy.
            Some(session) if session.background && session.job.is_some() => None,
            Some(session) => Some(
                self.text
                    .open_elsewhere
                    .replace("{pid}", &session.pid.to_string()),
            ),
            None if conversation.stranded => Some(self.text.folder_gone.to_string()),
            None => None,
        };
        match self.clipboard.copy(&command) {
            Ok(()) => {
                self.toast(
                    ToastKind::Ok,
                    self.text.copied.replace("{command}", &command),
                );
                if let Some(note) = note {
                    self.toast(ToastKind::Info, note);
                }
            }
            Err(error) => {
                let text = format!("{}: {error}", self.text.copy_failed);
                self.toast(ToastKind::Error, text);
            }
        }
    }

    fn on_event(&mut self, event: Event) {
        match event {
            Event::Key(key) if key.kind != KeyEventKind::Release => self.on_key(key),
            Event::Mouse(mouse) => self.on_mouse(mouse),
            _ => {}
        }
    }

    pub fn on_key(&mut self, key: KeyEvent) {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.quit = true;
            return;
        }
        match self.mode {
            Mode::Help => self.mode = Mode::List,
            Mode::Confirm(_) => self.on_confirm_key(key),
            Mode::Filter => self.on_filter_key(key),
            Mode::Details => self.on_details_key(key),
            Mode::List => self.on_list_key(key),
        }
    }

    fn on_list_key(&mut self, key: KeyEvent) {
        let page = self.page.max(1) as isize;
        let worktrees = self.view == View::Worktrees;
        match key.code {
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Esc => self.filter.clear(),
            KeyCode::Up | KeyCode::Char('k') => self.move_by(-1),
            KeyCode::Down | KeyCode::Char('j') => self.move_by(1),
            KeyCode::PageUp => self.move_by(-page),
            KeyCode::PageDown => self.move_by(page),
            KeyCode::Home | KeyCode::Char('g') => self.move_by(isize::MIN),
            KeyCode::End | KeyCode::Char('G') => self.move_by(isize::MAX),
            // The lists sit side by side, like their tabs on top.
            KeyCode::Left | KeyCode::Char('h') => self.view = View::Worktrees,
            KeyCode::Right | KeyCode::Char('l') => self.view = View::Sessions,
            KeyCode::Enter if !worktrees => self.copy_command(),
            KeyCode::Enter | KeyCode::Char('i') => self.toggle_details(),
            KeyCode::Char('d') | KeyCode::Delete if worktrees => self.ask_removal(),
            KeyCode::Char('D') if worktrees => self.ask_idle_removal(),
            KeyCode::Char('/') => {
                // A new search, like `/` in less and vim.
                self.filter.clear();
                self.mode = Mode::Filter;
            }
            KeyCode::Char('s') if worktrees => self.sort = self.sort.next(),
            KeyCode::Char('r') => self.refresh(false),
            KeyCode::Char('R') => self.refresh(true),
            KeyCode::Char('?') => self.mode = Mode::Help,
            _ => {}
        }
        self.keep_selection();
    }

    fn on_filter_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.filter.clear();
                self.mode = Mode::List;
            }
            KeyCode::Enter => self.mode = Mode::List,
            KeyCode::Backspace => {
                self.filter.pop();
            }
            KeyCode::Up => self.move_by(-1),
            KeyCode::Down => self.move_by(1),
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.filter.push(c);
            }
            _ => {}
        }
        self.keep_selection();
    }

    fn on_details_key(&mut self, key: KeyEvent) {
        let worktrees = self.view == View::Worktrees;
        match key.code {
            KeyCode::Enter if !worktrees => self.copy_command(),
            KeyCode::Esc | KeyCode::Enter | KeyCode::Left | KeyCode::Char('h' | 'i' | 'q') => {
                self.mode = Mode::List;
            }
            KeyCode::Up | KeyCode::Char('k') => self.move_by(-1),
            KeyCode::Down | KeyCode::Char('j') => self.move_by(1),
            KeyCode::Char('d') | KeyCode::Delete if worktrees => self.ask_removal(),
            _ => {}
        }
    }

    fn on_confirm_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc | KeyCode::Char('n' | 'q') => self.mode = Mode::List,
            KeyCode::Enter | KeyCode::Char('y') => self.remove(),
            _ => {}
        }
    }

    fn on_mouse(&mut self, mouse: MouseEvent) {
        let at = Position::new(mouse.column, mouse.row);
        if mouse.kind == MouseEventKind::Down(MouseButton::Left)
            && let Some(&(_, code)) = self.buttons.iter().find(|(rect, _)| rect.contains(at))
        {
            self.on_key(KeyEvent::from(code));
            return;
        }
        if !matches!(self.mode, Mode::List) {
            return;
        }
        match mouse.kind {
            MouseEventKind::ScrollDown => self.move_by(1),
            MouseEventKind::ScrollUp => self.move_by(-1),
            MouseEventKind::Down(MouseButton::Left) => {
                let Some(&(_, index)) = self.hits.iter().find(|(rect, _)| rect.contains(at)) else {
                    return;
                };
                match self.view {
                    View::Worktrees => {
                        let items = self.items();
                        self.select(&items, index);
                    }
                    // A click selects a session; another on it copies its command.
                    View::Sessions => {
                        let rows = self.rows();
                        if self.chosen_index(&rows) == Some(index) {
                            self.copy_command();
                        } else {
                            self.choose(&rows, index);
                        }
                    }
                }
            }
            _ => {}
        }
    }

    fn toggle_details(&mut self) {
        let chosen = match self.view {
            View::Worktrees => self.selected.is_some(),
            View::Sessions => self.chosen.is_some(),
        };
        if self.details_room {
            self.show_details = !self.show_details;
        } else if chosen {
            self.mode = Mode::Details;
        }
    }

    fn refresh(&mut self, every_size: bool) {
        self.workers.refresh();
        for git in self.status.values_mut() {
            git.at = None;
        }
        if every_size {
            for size in self.sizes.values_mut() {
                if !size.busy() {
                    size.at = None;
                }
            }
        } else if let Some(key) = self.selected.clone() {
            self.measure(key, true);
        }
    }

    /// Queues a measurement of the worktree at `key`, leaving out the worktrees inside it.
    fn measure(&mut self, key: PathBuf, urgent: bool) {
        let exclude: HashSet<PathBuf> = self
            .repos
            .iter()
            .flat_map(|repo| repo.worktrees.iter())
            .filter(|w| w.real != key && w.real.starts_with(&key))
            .map(|w| w.real.clone())
            .collect();
        let size = self.sizes.entry(key.clone()).or_default();
        if size.busy() {
            return;
        }
        let cancel = Arc::new(AtomicBool::new(false));
        size.queued = true;
        size.cancel = Some(Arc::clone(&cancel));
        self.workers.measure(SizeJob {
            path: key,
            exclude,
            urgent,
            cancel,
        });
    }

    fn schedule(&mut self, now: Instant) {
        let visible: Vec<(PathBuf, bool, bool)> = self
            .items()
            .into_iter()
            .filter_map(|item| match item {
                Item::Tree(r, w) => Some(&self.repos[r].worktrees[w]),
                Item::Repo(_) => None,
            })
            .filter(|w| !w.missing() && !self.removing.contains(&w.real))
            .map(|w| {
                let working = self.state(w) == State::Working;
                (
                    w.real.clone(),
                    self.selected.as_ref() == Some(&w.real),
                    working,
                )
            })
            .collect();
        for (key, selected, working) in visible {
            let stale = |at: Option<Instant>, ttl: Duration| {
                at.is_none_or(|at| now.duration_since(at) > ttl)
            };
            let size = self.sizes.entry(key.clone()).or_default();
            let size_ttl = if working { WORKING_SIZE_TTL } else { SIZE_TTL };
            if !size.busy() && stale(size.at, size_ttl) {
                self.measure(key.clone(), false);
            }
            let ttl = if selected {
                SELECTED_STATUS_TTL
            } else {
                STATUS_TTL
            };
            let git = self.status.entry(key.clone()).or_default();
            if !git.pending && stale(git.at, ttl) {
                git.pending = true;
                self.workers.status(key, selected);
            }
        }
    }

    fn ask_removal(&mut self) {
        let Some((r, w)) = self.selected_tree() else {
            return;
        };
        let worktree = &self.repos[r].worktrees[w];
        if worktree.main {
            self.toast(ToastKind::Error, self.text.main_refused.to_string());
            return;
        }
        if self.removing.contains(&worktree.real) {
            return;
        }
        let key = worktree.real.clone();
        // Check for uncommitted files right away, to tell what will be lost.
        let git = self.status.entry(key.clone()).or_default();
        if !git.pending {
            git.pending = true;
            self.workers.status(key.clone(), true);
        }
        self.mode = Mode::Confirm(Confirm {
            targets: vec![key],
            idle: false,
        });
    }

    /// The worktrees on the list nobody uses: no agent session, no process, and not a main
    /// worktree.
    pub fn idle_worktrees(&self) -> Vec<PathBuf> {
        self.items()
            .into_iter()
            .filter_map(|item| match item {
                Item::Tree(r, w) => Some(&self.repos[r].worktrees[w]),
                Item::Repo(_) => None,
            })
            .filter(|w| !w.main && matches!(self.state(w), State::Free | State::Missing))
            .map(|w| w.real.clone())
            .collect()
    }

    fn ask_idle_removal(&mut self) {
        let targets = self.idle_worktrees();
        if targets.is_empty() {
            self.toast(ToastKind::Info, self.text.idle_none.to_string());
            return;
        }
        self.mode = Mode::Confirm(Confirm {
            targets,
            idle: true,
        });
    }

    /// Starts the removals the dialog asked about and closes it: they run in the background,
    /// each worktree turning into a spinner until git is done with it.
    fn remove(&mut self) {
        let Mode::Confirm(confirm) = std::mem::replace(&mut self.mode, Mode::List) else {
            return;
        };
        let mut started = Vec::new();
        for key in confirm.targets {
            let Some((r, w)) = self.find(&key) else {
                continue;
            };
            let repo = &self.repos[r];
            let worktree = &repo.worktrees[w];
            if worktree.main || self.removing.contains(&key) {
                continue;
            }
            self.workers.remove(RemoveJob {
                dir: repo.command_dir().to_path_buf(),
                worktree: worktree.path.clone(),
                key: key.clone(),
                force: Force::needed(worktree.locked.is_some()),
                stop: self.scan.inside(&worktree.real),
            });
            if let Some(cancel) = self.sizes.get(&key).and_then(|size| size.cancel.as_ref()) {
                cancel.store(true, Ordering::Relaxed);
            }
            self.removing.insert(key.clone());
            started.push(key);
        }
        if confirm.idle && !started.is_empty() {
            self.batch
                .get_or_insert_with(Batch::default)
                .pending
                .extend(started);
        }
    }

    fn on_removed(&mut self, key: PathBuf, result: Result<(), String>) {
        self.removing.remove(&key);
        let name = match self.find(&key) {
            Some((r, w)) => {
                let (repo, worktree) = self.tree(r, w);
                worktree_name(repo, worktree).to_string()
            }
            None => key.file_name().map_or_else(
                || key.display().to_string(),
                |name| name.to_string_lossy().into_owned(),
            ),
        };
        let batch = self
            .batch
            .as_mut()
            .and_then(|batch| batch.pending.remove(&key).then_some(batch));
        let in_batch = batch.is_some();
        match result {
            Ok(()) => {
                let freed = self
                    .sizes
                    .remove(&key)
                    .and_then(|size| size.usage)
                    .map_or(0, |usage| usage.bytes);
                if let Some(batch) = batch {
                    batch.removed += 1;
                    batch.freed += freed;
                }
                self.status.remove(&key);
                for repo in &mut self.repos {
                    repo.worktrees.retain(|w| w.real != key);
                }
                self.gone.insert(key, Instant::now());
                self.attribute();
                if !in_batch {
                    let text = if freed > 0 {
                        self.text
                            .removed_freed
                            .replace("{name}", &name)
                            .replace("{size}", &fmt::size(freed, self.lang))
                    } else {
                        self.text.removed.replace("{name}", &name)
                    };
                    self.toast(ToastKind::Ok, text);
                }
            }
            Err(error) => {
                let first = error.lines().next().unwrap_or_default().to_string();
                if let Some(batch) = batch {
                    batch.failed += 1;
                    batch.error.get_or_insert(format!("{name}: {first}"));
                } else {
                    let text = self.text.remove_failed.replace("{name}", &name);
                    self.toast(ToastKind::Error, format!("{text}: {first}"));
                }
                self.status.entry(key.clone()).or_default().pending = true;
                self.workers.status(key, true);
            }
        }
        if self
            .batch
            .as_ref()
            .is_some_and(|batch| batch.pending.is_empty())
        {
            let batch = self.batch.take().unwrap_or_default();
            let size = fmt::size(batch.freed, self.lang);
            if batch.failed == 0 {
                let text = self
                    .text
                    .idle_done
                    .replace("{n}", &batch.removed.to_string())
                    .replace("{size}", &size);
                self.toast(ToastKind::Ok, text);
            } else {
                let text = self
                    .text
                    .idle_failed
                    .replace("{done}", &batch.removed.to_string())
                    .replace("{failed}", &batch.failed.to_string())
                    .replace("{error}", batch.error.as_deref().unwrap_or_default());
                self.toast(ToastKind::Error, text);
            }
        }
        self.keep_selection();
        if self.batch.is_none() {
            self.workers.refresh();
        }
    }

    pub fn toast(&mut self, kind: ToastKind, text: String) {
        self.toasts.push(Toast {
            kind,
            text,
            at: Instant::now(),
        });
        if self.toasts.len() > 3 {
            self.toasts.remove(0);
        }
    }
}

/// The indexes of the rows `pick` takes, which the selection moves over.
fn selectable<T>(rows: &[T], pick: impl Fn(&T) -> bool) -> Vec<usize> {
    (0..rows.len()).filter(|&i| pick(&rows[i])).collect()
}

/// Where the selection is once the list changed: on the same row when it is still listed,
/// else on the first selectable row from where it was, else on the last one.
fn settle(current: Option<usize>, selectable: &[usize], last: usize) -> Option<usize> {
    current.or_else(|| {
        selectable
            .iter()
            .find(|&&i| i >= last)
            .or(selectable.last())
            .copied()
    })
}

/// The selectable row `delta` of them away from the selected one.
fn step(current: Option<usize>, selectable: &[usize], delta: isize) -> Option<usize> {
    let last = selectable.len().checked_sub(1)?;
    let at = current
        .and_then(|index| selectable.iter().position(|&i| i == index))
        .unwrap_or(0);
    let next = (at as isize).saturating_add(delta).clamp(0, last as isize);
    selectable.get(next as usize).copied()
}

#[cfg(test)]
impl App {
    pub fn with_data(repos: Vec<Repo>, scan: Scan) -> Self {
        let mut app = App::new(Lang::En, Theme::rgb(), None, Vec::new(), Workers::idle());
        app.clipboard = Clipboard::Memory(Vec::new());
        app.handle(Msg::Repos(repos));
        app.handle(Msg::Scan(scan));
        app
    }

    pub fn set_size(&mut self, path: &str, bytes: u64) {
        let size = self.sizes.entry(PathBuf::from(path)).or_default();
        size.at = Some(Instant::now());
        size.usage = Some(Usage {
            bytes,
            files: bytes / 9_000,
            top: vec![
                ("node_modules".into(), bytes * 46 / 100),
                (".git".into(), bytes * 21 / 100),
                ("tmp".into(), bytes * 14 / 100),
                ("log".into(), bytes * 9 / 100),
                ("app".into(), bytes * 5 / 100),
                ("vendor".into(), bytes * 3 / 100),
                ("spec".into(), bytes / 100),
                ("Gemfile.lock".into(), bytes / 100),
            ],
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::demo;

    fn names(app: &App) -> Vec<String> {
        app.items()
            .iter()
            .map(|item| match *item {
                Item::Repo(r) => format!("# {}", app.repos[r].name),
                Item::Tree(r, w) => {
                    let (repo, worktree) = app.tree(r, w);
                    worktree_name(repo, worktree).to_string()
                }
            })
            .collect()
    }

    #[test]
    fn lists_repositories_with_their_worktrees_busiest_first() {
        let app = demo::app();
        let names = names(&app);
        assert_eq!(names[0], "# board");
        let shop = names.iter().position(|n| n == "# shop").unwrap();
        // The main worktree first, then waiting, working, idle, busy, free, missing.
        assert_eq!(
            &names[shop + 1..shop + 5],
            ["shop", "checkout-coupons", "order-pipeline", "search-index"]
        );
        assert_eq!(names.last().unwrap(), "tools-old-experiment");
    }

    #[test]
    fn sorts_by_size_name_and_age() {
        let mut app = demo::app();
        app.sort = Sort::Size;
        let by_size = names(&app);
        let shop = by_size.iter().position(|n| n == "# shop").unwrap();
        assert_eq!(by_size[shop + 1], "shop", "the main worktree stays first");
        assert_eq!(by_size[shop + 2], "search-index");
        app.sort = Sort::Name;
        assert_eq!(names(&app)[shop + 2], "api-error-messages");
        app.sort = Sort::Oldest;
        assert_eq!(names(&app)[shop + 2], "bench-cli", "untouched for 12 days");
    }

    #[test]
    fn filters_by_name_branch_or_session() {
        let mut app = demo::app();
        app.filter = "optimization".into();
        assert_eq!(names(&app), ["# shop", "order-pipeline"]);
        app.filter = "PROMOTE".into();
        assert_eq!(names(&app), ["# board", "promote-subboard"]);
        app.filter = "nothing like this".into();
        assert!(app.items().is_empty());
    }

    #[test]
    fn a_slash_starts_a_new_search() {
        let mut app = demo::app();
        app.on_key(KeyEvent::from(KeyCode::Char('/')));
        for c in "coupon".chars() {
            app.on_key(KeyEvent::from(KeyCode::Char(c)));
        }
        app.on_key(KeyEvent::from(KeyCode::Enter));
        assert_eq!(names(&app), ["# shop", "checkout-coupons"]);
        let (r, w) = app.selected_tree().unwrap();
        assert_eq!(
            worktree_name(&app.repos[r], &app.repos[r].worktrees[w]),
            "checkout-coupons"
        );
        app.on_key(KeyEvent::from(KeyCode::Char('/')));
        assert!(app.filter.is_empty());
        app.on_key(KeyEvent::from(KeyCode::Esc));
        assert!(matches!(app.mode, Mode::List));
        assert_eq!(app.totals(&app.items()).worktrees, 15);
    }

    #[test]
    fn counts_each_session_once() {
        let app = demo::app();
        let totals = app.totals(&app.items());
        assert_eq!(totals.worktrees, 15);
        assert_eq!((totals.working, totals.blocked, totals.idle), (4, 2, 3));
    }

    #[test]
    fn keeps_the_selection_on_the_same_worktree() {
        let mut app = demo::app();
        app.move_by(3);
        let chosen = app.selected.clone();
        app.sort = Sort::Name;
        app.keep_selection();
        assert_eq!(app.selected, chosen);
        app.move_by(isize::MAX);
        let last = app.items().len() - 1;
        assert_eq!(app.selected_index(&app.items()), Some(last));
        app.move_by(isize::MIN);
        assert_eq!(app.selected_index(&app.items()), Some(1));
    }

    #[test]
    fn never_removes_the_main_worktree() {
        let mut app = demo::app();
        app.move_by(isize::MIN);
        let (r, w) = app.selected_tree().unwrap();
        assert!(app.repos[r].worktrees[w].main);
        app.on_key(KeyEvent::from(KeyCode::Char('d')));
        assert!(matches!(app.mode, Mode::List));
        assert_eq!(app.toasts.len(), 1);
    }

    fn key(name: &str) -> PathBuf {
        let repo = if name.starts_with("tools") {
            return PathBuf::from(format!("/Users/me/Workspace/{name}"));
        } else if ["archive-cards", "promote-subboard", "migrate-cards"].contains(&name) {
            "board"
        } else {
            "shop"
        };
        PathBuf::from(format!(
            "/Users/me/Workspace/{repo}/.claude/worktrees/{name}"
        ))
    }

    #[test]
    fn a_removal_asks_once_and_closes_the_dialog() {
        let mut app = demo::app();
        app.move_by(1);
        let archive = key("archive-cards");
        assert_eq!(app.selected.as_ref(), Some(&archive));
        app.on_key(KeyEvent::from(KeyCode::Char('d')));
        let Mode::Confirm(confirm) = &app.mode else {
            panic!("no dialog")
        };
        assert_eq!(confirm.targets, std::slice::from_ref(&archive));
        assert!(!confirm.idle);
        // What runs in it is stopped first: the Claude session working there.
        assert_eq!(app.scan.inside(&archive), [54795]);

        app.on_key(KeyEvent::from(KeyCode::Enter));
        assert!(matches!(app.mode, Mode::List), "no waiting on git");
        let (r, w) = app.find(&archive).unwrap();
        assert_eq!(app.state(&app.repos[r].worktrees[w]), State::Removing);

        app.handle(Msg::Removed {
            path: archive.clone(),
            result: Err("fatal: permission denied".into()),
        });
        assert_eq!(app.toasts.last().unwrap().kind, ToastKind::Error);
        assert_ne!(app.state(&app.repos[r].worktrees[w]), State::Removing);

        app.on_key(KeyEvent::from(KeyCode::Char('d')));
        app.on_key(KeyEvent::from(KeyCode::Char('y')));
        app.handle(Msg::Removed {
            path: archive.clone(),
            result: Ok(()),
        });
        assert!(app.find(&archive).is_none());
        let toast = &app.toasts.last().unwrap().text;
        assert!(
            toast.contains("archive-cards removed · 612 MB freed"),
            "{toast}"
        );
        assert!(app.selected.is_some() && app.selected != Some(archive.clone()));

        // A listing made before the removal does not bring it back.
        let mut repos = app.repos.clone();
        repos[r].worktrees.push(Worktree {
            path: archive.clone(),
            real: archive.clone(),
            head: None,
            branch: None,
            detached: false,
            locked: None,
            prunable: None,
            main: false,
            touched: None,
        });
        app.handle(Msg::Repos(repos));
        assert!(app.find(&archive).is_none());
    }

    #[test]
    fn removes_every_idle_worktree_at_once() {
        let mut app = demo::app();
        app.on_key(KeyEvent::from(KeyCode::Char('D')));
        let Mode::Confirm(confirm) = &app.mode else {
            panic!("no dialog")
        };
        assert!(confirm.idle);
        // Not the main ones, nor where a session or a process is.
        let expected: Vec<PathBuf> = [
            "promote-subboard",
            "bench-cli",
            "api-error-messages",
            "fix-pr-512",
            "cli-release",
            "tools-old-experiment",
        ]
        .into_iter()
        .map(key)
        .collect();
        let mut targets = confirm.targets.clone();
        targets.sort();
        let mut sorted = expected.clone();
        sorted.sort();
        assert_eq!(targets, sorted);

        app.on_key(KeyEvent::from(KeyCode::Enter));
        assert!(matches!(app.mode, Mode::List));
        assert_eq!(app.removing.len(), 6);
        for (i, path) in expected.iter().enumerate() {
            let result = if i == 2 {
                Err("fatal: could not remove".to_string())
            } else {
                Ok(())
            };
            app.handle(Msg::Removed {
                path: path.clone(),
                result,
            });
            assert_eq!(
                app.toasts.len(),
                usize::from(i == 5),
                "one message at the end"
            );
        }
        let toast = &app.toasts[0];
        assert_eq!(toast.kind, ToastKind::Error);
        assert!(
            toast
                .text
                .starts_with("5 removed, 1 failed: api-error-messages"),
            "{}",
            toast.text
        );
        assert!(app.batch.is_none());
        assert!(app.find(&key("bench-cli")).is_none());
        assert!(app.find(&key("api-error-messages")).is_some());

        // With nothing idle left but the failed one, a second round takes just it.
        app.on_key(KeyEvent::from(KeyCode::Char('D')));
        assert!(matches!(&app.mode, Mode::Confirm(c) if c.targets == [key("api-error-messages")]));
    }

    #[test]
    fn says_so_when_nothing_is_idle() {
        let mut app = demo::app();
        app.filter = "coupon".into();
        app.on_key(KeyEvent::from(KeyCode::Char('D')));
        assert!(matches!(app.mode, Mode::List));
        assert_eq!(app.toasts[0].kind, ToastKind::Info);
    }

    fn session_names(app: &App) -> Vec<String> {
        app.rows()
            .iter()
            .map(|row| match *row {
                Row::Project(i) => format!("# {}", app.history[i].project_name),
                Row::Conversation(i) => app.history[i].label().unwrap_or("-").to_string(),
            })
            .collect()
    }

    fn conversation(app: &App, label: &str) -> usize {
        app.history
            .iter()
            .position(|c| c.label() == Some(label))
            .unwrap_or_else(|| panic!("no conversation {label:?}"))
    }

    const DNS: &str = "add the DNS records for the staging zone to the terraform";
    const PLAN: &str = "run terraform plan for the dns module and explain the diff";

    #[test]
    fn arrows_go_between_worktrees_and_sessions() {
        let mut app = demo::app();
        let key = |app: &mut App, code| app.on_key(KeyEvent::from(code));
        assert_eq!(app.view, View::Worktrees);
        key(&mut app, KeyCode::Left);
        assert_eq!(app.view, View::Worktrees, "nothing further left");
        key(&mut app, KeyCode::Right);
        assert_eq!(app.view, View::Sessions);
        key(&mut app, KeyCode::Right);
        assert_eq!(app.view, View::Sessions, "nor further right");
        // What removes worktrees does nothing among the sessions.
        key(&mut app, KeyCode::Char('D'));
        key(&mut app, KeyCode::Char('d'));
        assert!(matches!(app.mode, Mode::List));
        key(&mut app, KeyCode::Char('h'));
        assert_eq!(app.view, View::Worktrees);
        key(&mut app, KeyCode::Char('l'));
        assert_eq!(app.view, View::Sessions);
        key(&mut app, KeyCode::Left);
        assert_eq!(app.view, View::Worktrees);
        // Enter still shows the details of a worktree, and `i` too.
        let shown = app.show_details;
        key(&mut app, KeyCode::Enter);
        key(&mut app, KeyCode::Char('i'));
        assert_eq!(app.show_details, shown);
        key(&mut app, KeyCode::Char('i'));
        assert_eq!(app.show_details, !shown);
    }

    #[test]
    fn lists_sessions_by_project_the_open_ones_first() {
        let app = demo::app();
        assert_eq!(
            session_names(&app),
            [
                "# shop",
                "pipeline optimization",
                "coupon rules",
                "tui clock themes",
                "api docs review",
                "dependency audit",
                "flaky checkout spec",
                "profile the CLI startup",
                "# board",
                "archive column cards",
                "board-ui",
                "migrate cards to column records",
                "promote subboards to boards",
                "# tools",
                DNS,
                PLAN,
                "try the new DNS provider",
                "# landing",
                "sketch the pricing page",
            ]
        );
    }

    #[test]
    fn tells_which_session_has_a_conversation_open() {
        let app = demo::app();
        let state = |label: &str| {
            let i = conversation(&app, label);
            (app.conversation_state(i), app.live(i).map(|s| s.pid))
        };
        assert_eq!(state("coupon rules"), (State::Blocked, Some(58380)));
        assert_eq!(state("board-ui"), (State::Idle, Some(44861)));
        assert_eq!(state("flaky checkout spec"), (State::Free, None));
        // Codex and OpenCode do not say: they have the latest conversation of their folder.
        assert_eq!(state(DNS), (State::Working, Some(81234)));
        assert_eq!(state(PLAN), (State::Free, None));
        assert_eq!(
            state("migrate cards to column records"),
            (State::Idle, Some(70001))
        );
        assert_eq!(state("try the new DNS provider"), (State::Missing, None));
    }

    /// Selects the conversation and presses Enter; returns what was copied.
    fn copy(app: &mut App, label: &str) -> String {
        let i = conversation(app, label);
        app.chosen = Some((app.history[i].agent, app.history[i].id.clone()));
        app.toasts.clear();
        app.on_key(KeyEvent::from(KeyCode::Enter));
        app.clipboard.last().unwrap_or_default().to_string()
    }

    #[test]
    fn enter_copies_the_command_that_resumes_a_session() {
        let mut app = demo::app();
        app.view = View::Sessions;
        assert_eq!(
            copy(&mut app, "flaky checkout spec"),
            "cd ~/Workspace/shop && claude --resume a8c1f0d2-5b7e-4f3a-9d6c-2e1b0a9f8e7d"
        );
        assert_eq!(app.toasts.len(), 1);
        assert!(
            app.toasts[0]
                .text
                .starts_with("Copied: cd ~/Workspace/shop"),
            "{}",
            app.toasts[0].text
        );
        assert_eq!(
            copy(&mut app, "coupon rules"),
            format!("claude attach {:08x}", 58380),
            "a background session still running is attached to"
        );
        assert_eq!(app.toasts.len(), 1);
        // One open in a terminal: resuming opens a second copy, and grove says so.
        assert_eq!(
            copy(&mut app, "board-ui"),
            format!(
                "cd ~/Workspace/board && claude --resume {:08x}-4b1e-4c2d-9e7f-8c1d2a3b4c5d",
                44861
            )
        );
        assert!(
            app.toasts[1].text.contains("pid 44861"),
            "{}",
            app.toasts[1].text
        );
        assert_eq!(
            copy(&mut app, DNS),
            "cd ~/Workspace/tools-dns && codex resume 019ebca5-89ff-7983-a0fd-051925b36676"
        );
        assert_eq!(
            copy(&mut app, "sketch the pricing page"),
            "cd ~/Workspace/landing && opencode --session ses_f2208140bffeF1sLYrI6y9WrlV"
        );
        copy(&mut app, "try the new DNS provider");
        assert_eq!(app.toasts[1].text, "Its folder is gone");
    }

    #[test]
    fn a_click_selects_a_session_and_another_copies_it() {
        let mut app = demo::app();
        app.on_key(KeyEvent::from(KeyCode::Right));
        crate::ui::tests::draw(&mut app, 160, 40);
        let click = |rect: Rect| {
            Msg::Input(Event::Mouse(MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: rect.x + 4,
                row: rect.y,
                modifiers: KeyModifiers::NONE,
            }))
        };
        let (rect, index) = app.hits[2];
        app.handle(click(rect));
        assert_eq!(app.chosen_index(&app.rows()), Some(index));
        assert_eq!(app.clipboard.last(), None);
        app.handle(click(rect));
        let i = app.chosen_conversation().unwrap();
        let expected = history::command(&app.history[i], app.live(i), app.home.as_deref());
        assert_eq!(app.clipboard.last(), Some(expected.as_str()));
    }

    #[test]
    fn the_filter_finds_sessions_by_prompt_or_agent() {
        let mut app = demo::app();
        app.view = View::Sessions;
        app.filter = "terraform".into();
        assert_eq!(session_names(&app), ["# tools", DNS, PLAN]);
        app.filter = "OPENCODE".into();
        assert_eq!(
            session_names(&app),
            [
                "# board",
                "migrate cards to column records",
                "# landing",
                "sketch the pricing page"
            ]
        );
        app.keep_selection();
        let i = app.chosen_conversation().unwrap();
        assert_eq!(
            app.history[i].agent,
            Agent::OpenCode,
            "the selection follows"
        );
    }

    #[test]
    fn buttons_answer_clicks() {
        let mut app = demo::app();
        app.buttons = vec![(Rect::new(10, 20, 8, 1), KeyCode::Char('D'))];
        let click = |column, row| {
            Event::Mouse(MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column,
                row,
                modifiers: KeyModifiers::NONE,
            })
        };
        app.handle(Msg::Input(click(3, 20)));
        assert!(matches!(app.mode, Mode::List));
        app.handle(Msg::Input(click(12, 20)));
        assert!(matches!(&app.mode, Mode::Confirm(c) if c.idle));
    }
}
