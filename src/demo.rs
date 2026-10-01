//! A made-up computer for tests and screenshots: three repositories with worktrees in every
//! state, Claude Code, Codex and OpenCode sessions in them, the conversations those agents
//! keep, and Claude Desktop routines in every state.

use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use chrono::{Local, Timelike};

use crate::app::App;
use crate::cron::Cron;
use crate::du::Volume;
use crate::git::{Commit, Status};
use crate::model::{
    Activity, Agent, Conversation, Proc, Repo, Routine, Run, Running, Scan, Session, Worktree,
};
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
        conversation: (agent == Agent::Claude).then(|| conversation_id(pid)),
        job: (agent == Agent::Claude && background).then(|| conversation_id(pid)[..8].to_string()),
    }
}

/// The id of the conversation the made-up Claude Code session `pid` has open.
fn conversation_id(pid: u32) -> String {
    format!("{pid:08x}-4b1e-4c2d-9e7f-8c1d2a3b4c5d")
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

/// A made-up conversation last active `minutes` ago in `cwd`, which it started in.
fn conversation(agent: Agent, id: &str, title: &str, cwd: &str, minutes: u64) -> Conversation {
    let tools_worktree = [TOOLS_DNS, TOOLS_OLD].contains(&cwd);
    let (project, worktree) = [BOARD, SHOP, TOOLS]
        .iter()
        .find_map(|repo| {
            if cwd == *repo {
                return Some((*repo, None));
            }
            let name = cwd.strip_prefix(&format!("{repo}/.claude/worktrees/"))?;
            Some((*repo, Some(name.to_string())))
        })
        .or_else(|| tools_worktree.then(|| (TOOLS, cwd.rsplit('/').next().map(str::to_string))))
        .unwrap_or((cwd, None));
    Conversation {
        agent,
        id: id.into(),
        title: Some(title.into()),
        first_prompt: None,
        last_prompt: None,
        cwd: PathBuf::from(cwd),
        start: PathBuf::from(cwd),
        branch: worktree.as_ref().map(|name| format!("worktree-{name}")),
        created: Some(ago(minutes * 60 + 3 * 3600)),
        updated: ago(minutes * 60),
        pr: None,
        project: PathBuf::from(project),
        project_name: project.rsplit('/').next().unwrap_or(project).to_string(),
        worktree,
        gone: false,
        stranded: false,
    }
}

/// The conversations the made-up agents keep: the ones the sessions above have open, and
/// older ones, the most recent first.
pub fn history() -> Vec<Conversation> {
    let claude = |pid: u32, title: &str, cwd: &str, minutes: u64| {
        conversation(Agent::Claude, &conversation_id(pid), title, cwd, minutes)
    };
    let mut coupons = claude(58380, "coupon rules", &wt(SHOP, "checkout-coupons"), 1);
    coupons.start = PathBuf::from(SHOP);
    coupons.last_prompt = Some("should a coupon apply to each item or to the whole order?".into());
    let mut pipeline = claude(
        75833,
        "pipeline optimization",
        &wt(SHOP, "order-pipeline"),
        0,
    );
    pipeline.pr = Some((123, "https://github.com/acme/shop/pull/123".into()));
    pipeline.last_prompt = Some("hunt the bugs in PR #123 before it merges".into());
    let mut archive = claude(
        54795,
        "archive column cards",
        &wt(BOARD, "archive-cards"),
        0,
    );
    archive.start = PathBuf::from(BOARD);
    let mut clock = claude(59223, "tui clock themes", SHOP, 2);
    clock.branch = Some("feat/design-system".into());
    let mut docs = claude(5947, "api docs review", SHOP, 3);
    docs.branch = Some("feat/design-system".into());
    let mut dns = conversation(
        Agent::Codex,
        "019ebca5-89ff-7983-a0fd-051925b36676",
        "DNS records for the staging zone",
        TOOLS_DNS,
        1,
    );
    dns.title = None;
    dns.first_prompt = Some("add the DNS records for the staging zone to the terraform".into());
    let mut audit = claude(6126, "dependency audit", SHOP, 40);
    audit.branch = Some("feat/design-system".into());
    let mut board = claude(44861, "board-ui", BOARD, 50);
    board.branch = Some("perf/board-rendering".into());
    let migrate = conversation(
        Agent::OpenCode,
        "ses_f201e4329ffe3PkqigSf6iemRe",
        "migrate cards to column records",
        &wt(BOARD, "migrate-cards"),
        70,
    );
    let mut flaky = conversation(
        Agent::Claude,
        "a8c1f0d2-5b7e-4f3a-9d6c-2e1b0a9f8e7d",
        "flaky checkout spec",
        SHOP,
        26 * 60,
    );
    flaky.branch = Some("feat/design-system".into());
    flaky.pr = Some((498, "https://github.com/acme/shop/pull/498".into()));
    flaky.last_prompt = Some("open a PR with the fix and the new spec".into());
    let mut plan = conversation(
        Agent::Codex,
        "019e8e11-2c4d-7aa0-b3f1-6d2c9e4a7b10",
        "terraform plan for the dns module",
        TOOLS_DNS,
        3 * 24 * 60,
    );
    plan.title = None;
    plan.first_prompt = Some("run terraform plan for the dns module and explain the diff".into());
    let pricing = conversation(
        Agent::OpenCode,
        "ses_f2208140bffeF1sLYrI6y9WrlV",
        "sketch the pricing page",
        "/Users/me/Workspace/landing",
        4 * 24 * 60,
    );
    let promote = claude(
        11111,
        "promote subboards to boards",
        &wt(BOARD, "promote-subboard"),
        6 * 24 * 60,
    );
    let bench = conversation(
        Agent::Codex,
        "019e5a02-77b3-7c21-8e4f-3a9d1c6b2e58",
        "profile the CLI startup",
        &wt(SHOP, "bench-cli"),
        12 * 24 * 60,
    );
    let mut old = conversation(
        Agent::Claude,
        "3f9e2d1c-0b8a-4765-9c3d-2e1f0a9b8c7d",
        "try the new DNS provider",
        TOOLS_OLD,
        41 * 24 * 60,
    );
    old.gone = true;
    old.stranded = true;
    vec![
        pipeline, archive, coupons, dns, clock, docs, audit, board, migrate, flaky, plan, pricing,
        promote, bench, old,
    ]
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

fn routine(id: &str, name: Option<&str>, cron: &str, cwd: &str) -> Routine {
    Routine {
        key: format!("/Users/me/Library/Application Support/Claude/org/{id}"),
        id: id.into(),
        name: name.unwrap_or(id).into(),
        description: None,
        cron: cron.into(),
        schedule: Cron::parse(cron),
        enabled: true,
        cwd: Some(PathBuf::from(cwd)),
        prompt: Some(PathBuf::from(format!(
            "/Users/me/.claude/scheduled-tasks/{id}/SKILL.md"
        ))),
        created: Some(ago(30 * DAY)),
        last_run: None,
        last_due: None,
        run: None,
    }
}

fn run(id: &str, title: &str, cwd: &str, minutes: u64) -> Run {
    Run {
        session: format!("local_{id}"),
        conversation: Some(format!("{id}-7d1c-4e2a-9b3f-5a6c7d8e9f01")),
        title: Some(title.into()),
        cwd: Some(PathBuf::from(cwd)),
        started: Some(ago(minutes * 60)),
        active: Some(ago(minutes * 60 - 30)),
        status: Some("completed".into()),
        detail: None,
        needs: None,
        error: None,
        archived: false,
        summary_for: None,
        answered: false,
    }
}

/// Routines in every state: waiting for an answer, running now, failed, late, done, never
/// run and paused. Their times follow the clock, so each one is in its state whenever the
/// tests run.
pub fn routines() -> Vec<Routine> {
    let now = Local::now();
    // Runs when it was last due, so it is not late.
    let ran_when_due = |routine: &mut Routine, run: Run| {
        let due = routine
            .schedule
            .as_ref()
            .and_then(|cron| cron.last_before(now));
        routine.last_due = due.map(SystemTime::from);
        routine.last_run = routine.last_due.map(|due| due + Duration::from_secs(40));
        routine.run = Some(run);
    };

    let mut notes = routine("release-notes", Some("Release notes"), "0 9 * * 1-5", SHOP);
    let mut waiting = run("a1b2c3d4", "Release notes for v2.4", SHOP, 50);
    waiting.status = Some("blocked".into());
    waiting.needs =
        Some("Publish the notes for v2.4 now, or wait for the last PR to merge?".into());
    waiting.detail = waiting.needs.clone();
    notes.description =
        Some("Weekdays 09:00 — drafts the release notes from the merged PRs".into());
    ran_when_due(&mut notes, waiting);

    let mut bench = routine(
        "nightly-bench",
        None,
        "30 2 * * *",
        &wt(BOARD, "archive-cards"),
    );
    let mut running = run("b2c3d4e5", "Nightly bench", BOARD, 23);
    // The Claude Code session working in archive-cards is this run.
    running.conversation = Some(conversation_id(54795));
    running.status = None;
    ran_when_due(&mut bench, running);

    let mut audit = routine("dependency-audit", None, "0 8 * * 1", SHOP);
    let mut failed = run("c3d4e5f6", "Dependency audit", SHOP, 3 * 60);
    failed.status = None;
    failed.error = Some("The github MCP server did not start".into());
    ran_when_due(&mut audit, failed);

    // Due every hour, half an hour from the current minute: it was due half an hour ago and
    // last ran three days ago.
    let minute = (now.minute() + 30) % 60;
    let mut digest = routine(
        "weekly-digest",
        Some("Weekly digest"),
        &format!("{minute} * * * *"),
        TOOLS,
    );
    digest.last_due = Some(ago(3 * DAY));
    digest.last_run = Some(ago(3 * DAY - 50));
    digest.run = Some(run("d4e5f6a7", "Weekly digest", TOOLS, 3 * 24 * 60));

    let mut standup = routine(
        "standup-summary",
        Some("Standup summary"),
        "45 9 * * 1-5",
        BOARD,
    );
    let mut done = run("e5f6a7b8", "Standup summary", BOARD, 5 * 60);
    done.detail = Some("posted the summary in #team".into());
    ran_when_due(&mut standup, done);

    let mut inbox = routine("inbox-triage", None, "15 7 * * *", TOOLS);
    inbox.created = Some(SystemTime::now());

    let mut cleanup = routine("cleanup-branches", None, "0 18 * * 5", SHOP);
    cleanup.enabled = false;

    vec![cleanup, standup, inbox, digest, audit, bench, notes]
}

/// The made-up computer with every worktree measured and checked.
pub fn app() -> App {
    let mut app = App::with_data(repos(), scan());
    app.handle(Msg::History(history()));
    app.handle(Msg::Routines(routines()));
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
