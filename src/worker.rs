//! Background threads: repository discovery, process scans, disk measurement, git status and
//! removal. They report back to the interface through one channel.

use std::collections::{HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use ratatui::crossterm::event::Event;

use crate::agents::{self, Scanner};
use crate::discover::Finder;
use crate::du::{self, Progress, Usage, Volume};
use crate::git::{self, Force};
use crate::model::{Repo, Scan};

const FIND_EVERY: Duration = Duration::from_secs(10);
const SCAN_EVERY: Duration = Duration::from_secs(2);
const PROGRESS_EVERY: Duration = Duration::from_millis(120);
/// Removals and status checks run a few at a time: each waits mostly on the disk.
const REMOVERS: usize = 3;
const STATUS_CHECKERS: usize = 3;

pub enum Msg {
    Input(Event),
    Repos(Vec<Repo>),
    Volume(Option<Volume>),
    Scan(Scan),
    Measuring {
        path: PathBuf,
        bytes: u64,
        files: u64,
    },
    Measured {
        path: PathBuf,
        usage: Option<Usage>,
    },
    Status {
        path: PathBuf,
        status: Result<git::Status, String>,
    },
    Removed {
        path: PathBuf,
        result: Result<(), String>,
    },
}

enum Find {
    Now,
    Folders(Vec<PathBuf>),
}

pub struct SizeJob {
    pub path: PathBuf,
    pub exclude: HashSet<PathBuf>,
    pub urgent: bool,
    pub cancel: Arc<AtomicBool>,
}

/// A worktree to remove: first the processes working in it stop, then git removes it.
pub struct RemoveJob {
    /// Where git runs: the repository's main worktree.
    pub dir: PathBuf,
    /// The worktree as git knows it.
    pub worktree: PathBuf,
    /// The worktree with symlinks resolved, as the interface and the processes know it.
    pub key: PathBuf,
    pub force: Force,
    pub stop: Vec<u32>,
}

pub struct Workers {
    find: Sender<Find>,
    scan: Sender<()>,
    size: Sender<SizeJob>,
    status: Sender<(PathBuf, bool)>,
    remove: Sender<RemoveJob>,
}

impl Workers {
    pub fn start(finder: Finder, home: Option<PathBuf>, tx: Sender<Msg>) -> Self {
        let (find, find_rx) = mpsc::channel();
        let (scan, scan_rx) = mpsc::channel();
        let (size, size_rx) = mpsc::channel();
        let (status, status_rx) = mpsc::channel();
        let (remove, remove_rx) = mpsc::channel();
        let remove_rx = Arc::new(Mutex::new(remove_rx));
        for i in 0..REMOVERS {
            let remove_rx = Arc::clone(&remove_rx);
            let tx = tx.clone();
            spawn(&format!("grove-remove-{i}"), move || {
                remove_loop(&remove_rx, &tx)
            });
        }
        spawn("grove-find", {
            let tx = tx.clone();
            move || find_loop(finder, &find_rx, &tx)
        });
        spawn("grove-scan", {
            let tx = tx.clone();
            move || scan_loop(home.as_deref(), &scan_rx, &tx)
        });
        spawn("grove-du", {
            let tx = tx.clone();
            move || size_loop(&size_rx, &tx)
        });
        spawn("grove-git", move || status_loop(&status_rx, &tx));
        Self {
            find,
            scan,
            size,
            status,
            remove,
        }
    }

    /// Workers that do nothing, for tests.
    #[cfg(test)]
    pub fn idle() -> Self {
        let (find, _) = mpsc::channel();
        let (scan, _) = mpsc::channel();
        let (size, _) = mpsc::channel();
        let (status, _) = mpsc::channel();
        let (remove, _) = mpsc::channel();
        Self {
            find,
            scan,
            size,
            status,
            remove,
        }
    }

    pub fn refresh(&self) {
        let _ = self.find.send(Find::Now);
        let _ = self.scan.send(());
    }

    /// Folders where agents run, so their repositories are listed even without worktrees.
    pub fn look_in(&self, folders: Vec<PathBuf>) {
        let _ = self.find.send(Find::Folders(folders));
    }

    pub fn measure(&self, job: SizeJob) {
        let _ = self.size.send(job);
    }

    pub fn status(&self, path: PathBuf, urgent: bool) {
        let _ = self.status.send((path, urgent));
    }

    pub fn remove(&self, job: RemoveJob) {
        let _ = self.remove.send(job);
    }
}

/// Takes the next removal whenever this thread is free.
fn remove_loop(jobs: &Mutex<Receiver<RemoveJob>>, tx: &Sender<Msg>) {
    loop {
        let job = jobs.lock().unwrap_or_else(|e| e.into_inner()).recv();
        let Ok(job) = job else { return };
        agents::stop(&job.stop, &job.key);
        let result = git::remove(&job.dir, &job.worktree, job.force);
        if tx
            .send(Msg::Removed {
                path: job.key,
                result,
            })
            .is_err()
        {
            return;
        }
    }
}

fn spawn(name: &str, work: impl FnOnce() + Send + 'static) {
    thread::Builder::new()
        .name(name.into())
        .spawn(work)
        .expect("failed to start a thread");
}

fn find_loop(mut finder: Finder, rx: &Receiver<Find>, tx: &Sender<Msg>) {
    let mut folders: Vec<PathBuf> = Vec::new();
    loop {
        let repos = finder.find(&folders);
        let volume = repos
            .first()
            .map(|repo| repo.common_dir.as_path())
            .and_then(du::volume);
        if tx.send(Msg::Repos(repos)).is_err() || tx.send(Msg::Volume(volume)).is_err() {
            return;
        }
        match rx.recv_timeout(FIND_EVERY) {
            Ok(Find::Now) | Err(RecvTimeoutError::Timeout) => {}
            Ok(Find::Folders(new)) => folders = new,
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

fn scan_loop(home: Option<&Path>, rx: &Receiver<()>, tx: &Sender<Msg>) {
    let mut scanner = Scanner::new(home);
    loop {
        if tx.send(Msg::Scan(scanner.scan())).is_err() {
            return;
        }
        match rx.recv_timeout(SCAN_EVERY) {
            Ok(()) | Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

/// Queues a job, once per folder; urgent ones go first.
fn enqueue(queue: &mut VecDeque<SizeJob>, job: SizeJob) {
    queue.retain(|queued| queued.path != job.path);
    if job.urgent {
        queue.push_front(job);
    } else {
        queue.push_back(job);
    }
}

fn size_loop(rx: &Receiver<SizeJob>, tx: &Sender<Msg>) {
    // A few threads: enough to keep the disk busy without taking the machine over.
    let threads = thread::available_parallelism().map_or(2, |n| (n.get() / 2).clamp(2, 4));
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .thread_name(|i| format!("grove-du-{i}"))
        .stack_size(8 << 20)
        .build()
        .expect("failed to start the measuring threads");
    let mut queue: VecDeque<SizeJob> = VecDeque::new();
    loop {
        if queue.is_empty() {
            match rx.recv() {
                Ok(job) => enqueue(&mut queue, job),
                Err(_) => return,
            }
        }
        while let Ok(job) = rx.try_recv() {
            enqueue(&mut queue, job);
        }
        let Some(job) = queue.pop_front() else {
            continue;
        };
        if job.cancel.load(Ordering::Relaxed) {
            continue;
        }
        let progress = Arc::new(Progress::default());
        let (done_tx, done_rx) = mpsc::channel();
        pool.spawn({
            let progress = Arc::clone(&progress);
            let cancel = Arc::clone(&job.cancel);
            let path = job.path.clone();
            let exclude = job.exclude;
            move || {
                let _ = done_tx.send(du::measure(&path, &exclude, &cancel, &progress));
            }
        });
        loop {
            match done_rx.recv_timeout(PROGRESS_EVERY) {
                Ok(usage) => {
                    let usage = usage.filter(|_| !job.cancel.load(Ordering::Relaxed));
                    if tx
                        .send(Msg::Measured {
                            path: job.path,
                            usage,
                        })
                        .is_err()
                    {
                        return;
                    }
                    break;
                }
                Err(RecvTimeoutError::Timeout) => {
                    let update = Msg::Measuring {
                        path: job.path.clone(),
                        bytes: progress.bytes.load(Ordering::Relaxed),
                        files: progress.files.load(Ordering::Relaxed),
                    };
                    if tx.send(update).is_err() {
                        job.cancel.store(true, Ordering::Relaxed);
                        return;
                    }
                    while let Ok(next) = rx.try_recv() {
                        enqueue(&mut queue, next);
                    }
                }
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }
    }
}

/// Hands the status checks, urgent ones first, to a few threads.
fn status_loop(rx: &Receiver<(PathBuf, bool)>, tx: &Sender<Msg>) {
    // No buffer: a check leaves the queue only when a thread is free, so an urgent one can
    // still go ahead of the rest.
    let (work, work_rx) = mpsc::sync_channel::<PathBuf>(0);
    let work_rx = Arc::new(Mutex::new(work_rx));
    for i in 0..STATUS_CHECKERS {
        let work_rx = Arc::clone(&work_rx);
        let tx = tx.clone();
        spawn(&format!("grove-git-{i}"), move || {
            loop {
                let path = work_rx.lock().unwrap_or_else(|e| e.into_inner()).recv();
                let Ok(path) = path else { return };
                let status = git::status(&path);
                if tx.send(Msg::Status { path, status }).is_err() {
                    return;
                }
            }
        });
    }
    let mut queue: VecDeque<PathBuf> = VecDeque::new();
    loop {
        if queue.is_empty() {
            match rx.recv() {
                Ok((path, _)) => queue.push_back(path),
                Err(_) => return,
            }
        }
        while let Ok((path, urgent)) = rx.try_recv() {
            queue.retain(|queued| *queued != path);
            if urgent {
                queue.push_front(path);
            } else {
                queue.push_back(path);
            }
        }
        let Some(path) = queue.pop_front() else {
            continue;
        };
        if work.send(path).is_err() {
            return;
        }
    }
}
