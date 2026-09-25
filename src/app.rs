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
use crate::du::{Usage, Volume};
use crate::fmt;
use crate::git::{Force, Status};
use crate::i18n::{Lang, Text};
use crate::model::{Activity, Repo, Scan, Worktree, worktree_name};
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
    started: Instant,
    last_index: usize,
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
            started: Instant::now(),
            last_index: 0,
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

    /// Keeps the selection on a worktree still listed, or the one now where it was.
    pub fn keep_selection(&mut self) {
        let items = self.items();
        if let Some(index) = self.selected_index(&items) {
            self.last_index = index;
            return;
        }
        let trees: Vec<usize> = (0..items.len())
            .filter(|&i| matches!(items[i], Item::Tree(..)))
            .collect();
        match trees
            .iter()
            .find(|&&i| i >= self.last_index)
            .or(trees.last())
        {
            Some(&index) => self.select(&items, index),
            None => self.selected = None,
        }
    }

    pub fn move_by(&mut self, delta: isize) {
        let items = self.items();
        let trees: Vec<usize> = (0..items.len())
            .filter(|&i| matches!(items[i], Item::Tree(..)))
            .collect();
        if trees.is_empty() {
            return;
        }
        let current = self
            .selected_index(&items)
            .and_then(|index| trees.iter().position(|&t| t == index))
            .unwrap_or(0);
        let next = (current as isize)
            .saturating_add(delta)
            .clamp(0, trees.len() as isize - 1);
        self.select(&items, trees[next as usize]);
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
        match key.code {
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Esc => self.filter.clear(),
            KeyCode::Up | KeyCode::Char('k') => self.move_by(-1),
            KeyCode::Down | KeyCode::Char('j') => self.move_by(1),
            KeyCode::PageUp => self.move_by(-page),
            KeyCode::PageDown => self.move_by(page),
            KeyCode::Home | KeyCode::Char('g') => self.move_by(isize::MIN),
            KeyCode::End | KeyCode::Char('G') => self.move_by(isize::MAX),
            KeyCode::Enter | KeyCode::Right | KeyCode::Char('l') => self.toggle_details(),
            KeyCode::Char('d') | KeyCode::Delete => self.ask_removal(),
            KeyCode::Char('D') => self.ask_idle_removal(),
            KeyCode::Char('/') => {
                // A new search, like `/` in less and vim.
                self.filter.clear();
                self.mode = Mode::Filter;
            }
            KeyCode::Char('s') => self.sort = self.sort.next(),
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
        match key.code {
            KeyCode::Esc | KeyCode::Enter | KeyCode::Left | KeyCode::Char('h' | 'q') => {
                self.mode = Mode::List;
            }
            KeyCode::Up | KeyCode::Char('k') => self.move_by(-1),
            KeyCode::Down | KeyCode::Char('j') => self.move_by(1),
            KeyCode::Char('d') | KeyCode::Delete => self.ask_removal(),
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
                if let Some(&(_, index)) = self.hits.iter().find(|(rect, _)| rect.contains(at)) {
                    let items = self.items();
                    self.select(&items, index);
                }
            }
            _ => {}
        }
    }

    fn toggle_details(&mut self) {
        if self.details_room {
            self.show_details = !self.show_details;
        } else if self.selected.is_some() {
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

#[cfg(test)]
impl App {
    pub fn with_data(repos: Vec<Repo>, scan: Scan) -> Self {
        let mut app = App::new(Lang::En, Theme::rgb(), None, Vec::new(), Workers::idle());
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
