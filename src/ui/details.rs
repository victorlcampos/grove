//! The details of one worktree: where it is, its git state, the sessions and processes in it,
//! what takes its space, and the command that removes it.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Paragraph, Widget};
use unicode_width::UnicodeWidthStr;

use super::{bar, fit, state_label, state_symbol};
use crate::app::{App, State};
use crate::fmt;
use crate::git::{self, Force};
use crate::model::{Activity, Repo, Worktree, lock_holder, worktree_name};
use crate::theme::{SPINNER, agent_glyph};

pub fn render(buf: &mut Buffer, area: Rect, app: &App, r: usize, w: usize) {
    if area.width < 12 || area.height < 3 {
        return;
    }
    let theme = &app.theme;
    let (repo, worktree) = app.tree(r, w);
    let state = app.state(worktree);
    let (symbol, color) = state_symbol(app, state);
    let label = format!(" {symbol} {} ", state_label(app, state));
    let room = usize::from(area.width).saturating_sub(label.width() + 6);
    let name = fmt::truncate(worktree_name(repo, worktree), room);
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(theme.faint())
        .title(Line::from(vec![
            Span::raw(" "),
            // Its own color: the title would take the faint one of the border.
            Span::styled(
                name,
                Style::new()
                    .fg(Color::Reset)
                    .bold()
                    .remove_modifier(Modifier::DIM),
            ),
            Span::raw(" "),
        ]))
        .title_top(Line::from(Span::styled(label, Style::new().fg(color))).right_aligned());
    let inner = block.inner(area);
    block.render(area, buf);
    let inner = Rect {
        x: inner.x + 1,
        width: inner.width.saturating_sub(2),
        ..inner
    };
    if inner.is_empty() {
        return;
    }

    if inner.width >= 96 {
        let left_width = inner.width * 11 / 20;
        let right = Rect {
            x: inner.x + left_width + 3,
            width: inner.width - left_width - 3,
            ..inner
        };
        let left = Rect {
            width: left_width,
            ..inner
        };
        Paragraph::new(info_lines(app, worktree, usize::from(left.width))).render(left, buf);
        let width = usize::from(right.width);
        let command = command_lines(app, repo, worktree, width);
        let entries = usize::from(right.height).saturating_sub(command.len() + 3);
        column(
            buf,
            right,
            space_lines(app, worktree, width, entries),
            command,
        );
    } else {
        let width = usize::from(inner.width);
        let command = command_lines(app, repo, worktree, width);
        let mut lines = info_lines(app, worktree, width);
        lines.push(Line::default());
        let entries = usize::from(inner.height).saturating_sub(lines.len() + command.len() + 3);
        lines.extend(space_lines(app, worktree, width, entries));
        column(buf, inner, lines, command);
    }
}

/// Draws `lines` from the top and `bottom` against the bottom edge, when both fit.
fn column(buf: &mut Buffer, area: Rect, lines: Vec<Line<'static>>, bottom: Vec<Line<'static>>) {
    let needed = lines.len() + 1 + bottom.len();
    let height = usize::from(area.height);
    if !bottom.is_empty() && needed <= height {
        let bottom_height = bottom.len() as u16;
        let bottom_area = Rect {
            y: area.bottom() - bottom_height,
            height: bottom_height,
            ..area
        };
        Paragraph::new(bottom).render(bottom_area, buf);
    }
    Paragraph::new(lines).render(area, buf);
}

fn title(app: &App, text: &str, extra: Option<String>) -> Line<'static> {
    let mut spans = vec![Span::styled(text.to_string(), app.theme.muted().bold())];
    if let Some(extra) = extra {
        spans.push(Span::styled(format!("  {extra}"), app.theme.muted()));
    }
    Line::from(spans)
}

fn ago(app: &App, when: SystemTime) -> String {
    let seconds = SystemTime::now()
        .duration_since(when)
        .map_or(0, |elapsed| elapsed.as_secs());
    fmt::ago(seconds, app.lang)
}

fn info_lines(app: &App, worktree: &Worktree, width: usize) -> Vec<Line<'static>> {
    let theme = &app.theme;
    let text = app.text;
    let mut lines = Vec::new();
    let path = fmt::tilde(&worktree.path, app.home.as_deref());
    lines.push(Line::styled(
        fmt::truncate_middle(&path, width),
        theme.muted(),
    ));

    let git = app.git(worktree).and_then(|git| git.status.as_ref());
    let mut branch = match (&worktree.branch, &worktree.head) {
        (Some(branch), _) => vec![Span::styled("⎇ ", theme.faint()), Span::raw(branch.clone())],
        (None, Some(head)) => vec![
            Span::styled("⎇ ", theme.faint()),
            Span::styled(
                format!("{} ({})", &head[..head.len().min(7)], text.detached),
                theme.muted().italic(),
            ),
        ],
        (None, None) => vec![Span::styled(
            format!("⎇ {}", text.detached),
            theme.muted().italic(),
        )],
    };
    if worktree.main {
        branch.push(Span::styled(format!("  {}", text.main_tag), theme.muted()));
    }
    if let Some(Ok(status)) = git
        && (status.ahead > 0 || status.behind > 0)
    {
        branch.push(Span::styled(
            format!("  ↑{} ↓{}", status.ahead, status.behind),
            theme.muted(),
        ));
        if let Some(upstream) = &status.upstream {
            branch.push(Span::styled(format!(" {upstream}"), theme.faint()));
        }
    }
    lines.push(Line::from(fit(branch, width)));

    if !worktree.missing() {
        let line = match git {
            None => vec![Span::styled(text.checking, theme.muted())],
            Some(Err(error)) => vec![Span::styled(
                format!(
                    "{}: {}",
                    text.git_failed,
                    error.lines().next().unwrap_or_default()
                ),
                ratatui::style::Style::new().fg(theme.error),
            )],
            Some(Ok(status)) => match &status.commit {
                Some(commit) => {
                    let when =
                        UNIX_EPOCH + Duration::from_secs(u64::try_from(commit.time).unwrap_or(0));
                    vec![
                        Span::styled(format!("{} ", commit.hash), Style::new().fg(theme.accent)),
                        Span::raw(commit.subject.clone()),
                        Span::styled(format!(" · {}", ago(app, when)), theme.muted()),
                    ]
                }
                None => vec![Span::styled(text.no_commits, theme.muted())],
            },
        };
        lines.push(Line::from(fit(line, width)));
        if let Some(Ok(status)) = git {
            let line = if status.at_risk() == 0 {
                vec![Span::styled(
                    format!("✓ {}", text.clean),
                    Style::new().fg(theme.ok),
                )]
            } else {
                let mut parts = Vec::new();
                if status.changed > 0 {
                    parts.push(text.changed.of(status.changed as usize));
                }
                if status.untracked > 0 {
                    parts.push(format!("{} {}", status.untracked, text.untracked));
                }
                if status.conflicts > 0 {
                    parts.push(format!("{} {}", status.conflicts, text.conflicts));
                }
                vec![Span::styled(
                    format!("± {}", parts.join(" · ")),
                    Style::new().fg(theme.warn),
                )]
            };
            lines.push(Line::from(fit(line, width)));
        }
    }
    if let Some(reason) = &worktree.locked {
        let reason = if reason.is_empty() {
            text.locked_tag
        } else {
            reason.as_str()
        };
        let mut line = format!("⊘ {}: {reason}", text.locked_tag);
        if let Some(pid) = lock_holder(reason)
            && !app.scan.is_alive(pid)
        {
            line.push_str(" · ");
            line.push_str(text.lock_stale);
        }
        for part in fmt::wrap(&line, width, 2) {
            lines.push(Line::styled(part, Style::new().fg(theme.locked)));
        }
    }
    if !matches!(app.state(worktree), State::Working | State::Blocked)
        && let Some(touched) = worktree.touched
    {
        lines.push(Line::styled(
            fmt::truncate(&format!("{} {}", text.last_used, ago(app, touched)), width),
            theme.muted(),
        ));
    }

    lines.push(Line::default());
    let presence = app.presence(worktree);
    let sessions = presence.map_or(&[][..], |p| &p.sessions[..]);
    lines.push(title(
        app,
        text.sessions_title,
        (!sessions.is_empty()).then(|| sessions.len().to_string()),
    ));
    if sessions.is_empty() {
        lines.push(Line::styled(text.no_sessions, theme.faint()));
    }
    let spin = SPINNER[app.frame % SPINNER.len()];
    for presence in sessions {
        let session = &presence.session;
        let (symbol, color, label) = match session.activity {
            Activity::Blocked => ("◆", theme.blocked, text.state_blocked),
            Activity::Working => (spin, theme.working, text.state_working),
            Activity::Idle => ("●", theme.idle, text.state_idle),
        };
        let mut head = vec![
            Span::styled(format!("{symbol} "), Style::new().fg(color)),
            Span::styled(
                format!("{} {}", agent_glyph(session.agent), session.agent.name()),
                Style::new().fg(theme.agent(session.agent)).bold(),
            ),
        ];
        if let Some(name) = &session.name {
            head.push(Span::styled(format!("  {name}"), Style::new().bold()));
        }
        head.push(Span::styled(format!(" · {label}"), Style::new().fg(color)));
        lines.push(Line::from(fit(head, width)));
        if let Some(detail) = &session.detail {
            for part in fmt::wrap(detail, width.saturating_sub(2), 2) {
                lines.push(Line::styled(format!("  {part}"), theme.muted()));
            }
        }
        let mut facts = vec![
            if session.background {
                text.background
            } else {
                text.terminal
            }
            .to_string(),
            format!("pid {}", session.pid),
            format!("{:.0}% cpu", session.cpu),
        ];
        if let Some(started) = session.started {
            facts.push(ago(app, started));
        }
        if let Some((pid, via)) = &presence.via {
            facts.push(format!("via {via} {pid}"));
        }
        lines.push(Line::styled(
            fmt::truncate(&format!("  {}", facts.join(" · ")), width),
            theme.faint(),
        ));
    }

    let procs = presence.map_or(&[][..], |p| &p.procs[..]);
    if !procs.is_empty() {
        lines.push(Line::default());
        lines.push(title(app, text.procs_title, Some(procs.len().to_string())));
        let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
        for proc in procs {
            *counts.entry(proc.name.as_str()).or_default() += 1;
        }
        let names: Vec<String> = counts
            .into_iter()
            .map(|(name, n)| {
                if n > 1 {
                    format!("{name} ×{n}")
                } else {
                    name.to_string()
                }
            })
            .collect();
        for part in fmt::wrap(&names.join(" · "), width, 3) {
            lines.push(Line::styled(part, theme.muted()));
        }
    }
    lines
}

/// What takes the space: the largest entries at the top of the worktree, as bars, as many as
/// `room` lines allow (at least a few), the rest summed up.
fn space_lines(app: &App, worktree: &Worktree, width: usize, room: usize) -> Vec<Line<'static>> {
    let theme = &app.theme;
    let text = app.text;
    let lang = app.lang;
    if worktree.missing() {
        return Vec::new();
    }
    let size = app.size(worktree);
    let Some(usage) = size.and_then(|size| size.usage.as_ref()) else {
        let line = match size.and_then(|size| size.running) {
            Some((bytes, files)) => format!(
                "{} {}… {} · {} {}",
                SPINNER[app.frame % SPINNER.len()],
                text.measuring,
                fmt::size(bytes, lang),
                fmt::count(files, lang),
                text.files
            ),
            None => text.not_measured.to_string(),
        };
        return vec![
            title(app, text.space_title, None),
            Line::styled(fmt::truncate(&line, width), theme.muted()),
        ];
    };
    let mut lines = vec![title(
        app,
        text.space_title,
        Some(format!(
            "{} · {} {}",
            fmt::size(usage.bytes, lang),
            fmt::count(usage.files, lang),
            text.files
        )),
    )];
    let shown = if usage.top.len() <= room.max(4) {
        usage.top.len()
    } else {
        room.clamp(4, 14) - 1
    };
    let mut entries: Vec<(String, u64)> = usage.top.iter().take(shown).cloned().collect();
    let rest: u64 = usage.top.iter().skip(shown).map(|(_, bytes)| bytes).sum();
    if rest > 0 {
        entries.push((text.others.to_string(), rest));
    }
    let name_width = entries
        .iter()
        .map(|(name, _)| name.width())
        .max()
        .unwrap_or(0)
        .clamp(4, 22)
        .min(width / 3);
    let bar_width = width.saturating_sub(name_width + 2 + 1 + 8) as u16;
    for (name, bytes) in entries {
        let fraction = if usage.bytes > 0 {
            bytes as f64 / usage.bytes as f64
        } else {
            0.0
        };
        let mut spans = vec![Span::raw(format!(
            "{:<name_width$}  ",
            fmt::truncate(&name, name_width)
        ))];
        if bar_width >= 4 {
            spans.extend(bar(theme, fraction, bar_width, theme.size_color(fraction)));
        }
        spans.push(Span::styled(
            format!(" {:>8}", fmt::size(bytes, lang)),
            theme.muted(),
        ));
        lines.push(Line::from(fit(spans, width)));
    }
    if let Some((bytes, _)) = size.and_then(|size| size.running) {
        let line = format!(
            "{} {}… {}",
            SPINNER[app.frame % SPINNER.len()],
            text.measuring,
            fmt::size(bytes, lang)
        );
        lines.push(Line::styled(fmt::truncate(&line, width), theme.faint()));
    }
    lines
}

/// The command `d` runs, for the record.
fn command_lines(app: &App, repo: &Repo, worktree: &Worktree, width: usize) -> Vec<Line<'static>> {
    if worktree.main || width < 20 {
        return Vec::new();
    }
    let home = app.home.as_deref();
    let command = git::remove_command(
        &fmt::tilde(repo.command_dir(), home),
        &fmt::tilde(&worktree.path, home),
        Force::needed(worktree.locked.is_some()),
    );
    fmt::hard_wrap(&format!("$ {command}"), width, 3)
        .into_iter()
        .map(|line| Line::styled(line, app.theme.faint()))
        .collect()
}
