//! A made-up computer for tests and screenshots: three repositories with worktrees in every
//! state, and Claude Code, Codex and OpenCode sessions in them.

use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use crate::app::App;
use crate::du::Volume;
use crate::git::{Commit, Status};
use crate::model::{Activity, Agent, Proc, Repo, Running, Scan, Session, Worktree};
use crate::worker::Msg;

const DAY: u64 = 86_400;

fn ago(seconds: u64) -> SystemTime {
    SystemTime::now() - Duration::from_secs(seconds)
}

fn worktree(path: &str, branch: Option<&str>, main: bool, days: u64) -> Worktree {
    Worktree {
        path: PathBuf::from(path),
        real: PathBuf::from(path),
        head: Some("903109d6b7b9e13e6968c0bc5efdab65b3908537".into()),
        branch: branch.map(str::to_string),
        detached: branch.is_none(),
        locked: None,
        prunable: None,
        main,
        touched: Some(ago(days * DAY + 3600)),
    }
}

fn repo(name: &str, worktrees: Vec<Worktree>) -> Repo {
    Repo {
        name: name.into(),
        common_dir: PathBuf::from(format!("/Users/me/Workspace/{name}/.git")),
        bare: false,
        worktrees,
    }
}

#[allow(clippy::too_many_arguments)]
fn session(
    agent: Agent,
    pid: u32,
    name: Option<&str>,
    detail: Option<&str>,
    activity: Activity,
    cwd: &str,
    background: bool,
    minutes: u64,
) -> Session {
    Session {
        agent,
        pid,
        name: name.map(str::to_string),
        detail: detail.map(str::to_string),
        background,
        activity,
        cpu: if activity == Activity::Working {
            38.5
        } else {
            0.4
        },
        started: Some(ago(minutes * 60)),
        cwd: PathBuf::from(cwd),
    }
}

fn proc(pid: u32, name: &str, cwd: &str) -> Proc {
    Proc {
        pid,
        name: name.into(),
        cwd: PathBuf::from(cwd),
        session: None,
    }
}

const BOARD: &str = "/Users/me/Workspace/board";
const SHOP: &str = "/Users/me/Workspace/shop";
const TOOLS: &str = "/Users/me/Workspace/tools";
const TOOLS_DNS: &str = "/Users/me/Workspace/tools-dns";
const TOOLS_OLD: &str = "/Users/me/Workspace/tools-old-experiment";

/// Where Claude Code puts the worktrees it creates.
fn wt(repo: &str, name: &str) -> String {
    format!("{repo}/.claude/worktrees/{name}")
}

pub fn repos() -> Vec<Repo> {
    let mut archive = worktree(
        &wt(BOARD, "archive-cards"),
        Some("worktree-archive-cards"),
        false,
        0,
    );
    archive.locked =
        Some("claude session archive-cards (pid 54795 start Fri Sep 25 17:13:46 2026)".into());
    let mut promote = worktree(
        &wt(BOARD, "promote-subboard"),
        Some("promote-subboard"),
        false,
        6,
    );
    promote.locked =
        Some("claude session promote-subboard (pid 11111 start Sat Sep 19 09:02:11 2026)".into());
    let mut old = worktree(TOOLS_OLD, Some("spike/old-experiment"), false, 41);
    old.prunable = Some("gitdir file points to non-existent location".into());
    vec![
        repo(
            "board",
            vec![
                worktree(BOARD, Some("perf/board-rendering"), true, 0),
                archive,
                promote,
                worktree(&wt(BOARD, "migrate-cards"), Some("migrate-cards"), false, 2),
            ],
        ),
        repo(
            "shop",
            vec![
                worktree(SHOP, Some("feat/design-system"), true, 0),
                worktree(&wt(SHOP, "bench-cli"), Some("bench-cli"), false, 12),
                worktree(
                    &wt(SHOP, "api-error-messages"),
                    Some("worktree-api-error-messages"),
                    false,
                    9,
                ),
                worktree(
                    &wt(SHOP, "checkout-coupons"),
                    Some("worktree-checkout-coupons"),
                    false,
                    0,
                ),
                worktree(
                    &wt(SHOP, "search-index"),
                    Some("worktree-search-index"),
                    false,
                    1,
                ),
                worktree(
                    &wt(SHOP, "order-pipeline"),
                    Some("worktree-order-pipeline"),
                    false,
                    0,
                ),
                worktree(&wt(SHOP, "fix-pr-512"), None, false, 4),
                worktree(
                    &wt(SHOP, "cli-release"),
                    Some("worktree-cli-release"),
                    false,
                    3,
                ),
            ],
        ),
        repo(
            "tools",
            vec![
                worktree(TOOLS, Some("chore/terraform-upgrade"), true, 1),
                worktree(TOOLS_DNS, Some("feat/dns-records"), false, 0),
                old,
            ],
        ),
    ]
}

pub fn scan() -> Scan {
    let sessions = vec![
        session(
            Agent::Claude,
            44861,
            Some("board-ui"),
            None,
            Activity::Idle,
            BOARD,
            false,
            190,
        ),
        session(
            Agent::Claude,
            54795,
            Some("archive column cards"),
            Some("moving the column archive to the cards; models and tests"),
            Activity::Working,
            &wt(BOARD, "archive-cards"),
            true,
            23,
        ),
        session(
            Agent::OpenCode,
            70001,
            None,
            None,
            Activity::Idle,
            &wt(BOARD, "migrate-cards"),
            false,
            75,
        ),
        session(
            Agent::Claude,
            5947,
            Some("api docs review"),
            Some("send a prompt to start"),
            Activity::Blocked,
            SHOP,
            true,
            167,
        ),
        session(
            Agent::Claude,
            59223,
            Some("tui clock themes"),
            Some("50 themes and a live picker; saving done"),
            Activity::Working,
            SHOP,
            true,
            112,
        ),
        session(
            Agent::Claude,
            6126,
            Some("dependency audit"),
            Some("3 outdated gems; report ready"),
            Activity::Idle,
            SHOP,
            true,
            167,
        ),
        session(
            Agent::Claude,
            58380,
            Some("coupon rules"),
            Some(
                "reply A or B: (A) apply the coupon to each item; (B) to the whole order, as today",
            ),
            Activity::Blocked,
            &wt(SHOP, "checkout-coupons"),
            true,
            114,
        ),
        session(
            Agent::Claude,
            75833,
            Some("pipeline optimization"),
            Some("bug hunter scanning PR #123 (3 lenses); CI in progress"),
            Activity::Working,
            &wt(SHOP, "order-pipeline"),
            true,
            105,
        ),
        session(
            Agent::Codex,
            81234,
            None,
            None,
            Activity::Working,
            TOOLS_DNS,
            false,
            12,
        ),
    ];
    let browser = wt(SHOP, "search-index");
    let procs = vec![
        proc(991, "fish", BOARD),
        proc(1211, "fish", SHOP),
        proc(36037, "htop", SHOP),
        proc(11098, "Google Chrome", &browser),
        proc(11110, "Google Chrome Helper", &browser),
        proc(11118, "Google Chrome Helper", &browser),
    ];
    let mut alive: Vec<u32> = sessions
        .iter()
        .map(|s| s.pid)
        .chain(procs.iter().map(|p| p.pid))
        .collect();
    alive.sort_unstable();
    let running = sessions
        .iter()
        .map(|s| Running {
            pid: s.pid,
            name: s.agent.name().into(),
            cwd: s.cwd.clone(),
        })
        .chain(procs.iter().map(|p| Running {
            pid: p.pid,
            name: p.name.clone(),
            cwd: p.cwd.clone(),
        }))
        .collect();
    Scan {
        sessions,
        procs,
        running,
        alive,
        protected: Vec::new(),
    }
}

fn status(changed: u32, untracked: u32, ahead: u32, subject: &str, hours: u64) -> Status {
    Status {
        changed,
        untracked,
        conflicts: 0,
        upstream: (ahead > 0).then(|| "origin/feat".to_string()),
        ahead,
        behind: 0,
        commit: Some(Commit {
            time: (SystemTime::now() - Duration::from_secs(hours * 3600))
                .duration_since(SystemTime::UNIX_EPOCH)
                .map_or(0, |d| d.as_secs() as i64),
            hash: "903109d".into(),
            subject: subject.into(),
        }),
    }
}

/// The made-up computer with every worktree measured and checked.
pub fn app() -> App {
    let mut app = App::with_data(repos(), scan());
    app.home = Some(PathBuf::from("/Users/me"));
    app.volume = Some(Volume {
        free: 312_400_000_000,
        total: 994_660_000_000,
    });
    let sizes: [(String, u64); 14] = [
        (BOARD.into(), 1_450_000_000),
        (wt(BOARD, "archive-cards"), 612_000_000),
        (wt(BOARD, "promote-subboard"), 588_000_000),
        (wt(BOARD, "migrate-cards"), 590_000_000),
        (SHOP.into(), 12_300_000_000),
        (wt(SHOP, "bench-cli"), 1_800_000_000),
        (wt(SHOP, "api-error-messages"), 1_210_000_000),
        (wt(SHOP, "checkout-coupons"), 1_930_000_000),
        (wt(SHOP, "search-index"), 3_120_000_000),
        (wt(SHOP, "order-pipeline"), 2_140_000_000),
        (wt(SHOP, "fix-pr-512"), 905_000_000),
        (wt(SHOP, "cli-release"), 402_000_000),
        (TOOLS.into(), 96_000_000),
        (TOOLS_DNS.into(), 41_000_000),
    ];
    for (path, bytes) in &sizes {
        app.set_size(path, *bytes);
    }
    let statuses = [
        (
            wt(SHOP, "checkout-coupons"),
            status(12, 3, 3, "feat(checkout): coupon rules per item", 2),
        ),
        (
            wt(SHOP, "order-pipeline"),
            status(4, 0, 1, "fix: order pipeline steps", 1),
        ),
        (
            wt(SHOP, "cli-release"),
            status(3, 2, 0, "chore: release script", 70),
        ),
        (
            SHOP.into(),
            status(0, 0, 0, "Merge branch 'feat/design-system'", 3),
        ),
    ];
    for (path, status) in statuses {
        app.handle(Msg::Status {
            path: PathBuf::from(path),
            status: Ok(status),
        });
    }
    for repo in repos() {
        for worktree in repo.worktrees {
            if !app.status.contains_key(&worktree.real) && !worktree.missing() {
                app.handle(Msg::Status {
                    path: worktree.real,
                    status: Ok(status(0, 0, 0, "chore: sync", 30)),
                });
            }
        }
    }
    app
}
