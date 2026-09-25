//! Boxes over the list: the removal confirmation and the help.

use std::collections::BTreeMap;

use ratatui::buffer::Buffer;
use ratatui::crossterm::event::KeyCode;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Paragraph, Widget};
use unicode_width::UnicodeWidthStr;

use super::{fit, spans_width, state_symbol};
use crate::app::{App, Confirm, State};
use crate::fmt;
use crate::git::{self, Force};
use crate::layout;
use crate::model::{Agent, lock_holder, worktree_name};
use crate::theme::{SPINNER, agent_glyph};

/// A cleared box with a rounded border and a title, centered in `area`; returns the space
/// inside it, one cell in from the sides.
fn frame(buf: &mut Buffer, area: Rect, width: u16, height: u16, title: &str, color: Color) -> Rect {
    let rect = layout::centered(area, width, height);
    Clear.render(rect, buf);
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(color))
        .title(Line::from(Span::styled(
            format!(" {title} "),
            Style::new().fg(color).bold(),
        )));
    let inner = block.inner(rect);
    block.render(rect, buf);
    Rect {
        x: inner.x + 1,
        width: inner.width.saturating_sub(2),
        ..inner
    }
}

fn key(app: &App, key: &str) -> Span<'static> {
    if app.theme.colored() {
        Span::styled(
            format!(" {key} "),
            Style::new().fg(Color::Black).bg(app.theme.accent).bold(),
        )
    } else {
        Span::styled(format!("[{key}]"), Style::new().bold())
    }
}

/// Lines that wrap a message under a symbol.
fn warning(
    lines: &mut Vec<Line<'static>>,
    width: usize,
    symbol: &str,
    message: &str,
    style: Style,
) {
    for (i, part) in fmt::wrap(message, width.saturating_sub(2), 3)
        .into_iter()
        .enumerate()
    {
        let lead = if i == 0 {
            format!("{symbol} ")
        } else {
            "  ".into()
        };
        lines.push(Line::styled(format!("{lead}{part}"), style));
    }
}

/// Draws the confirmation and returns where its buttons are, with the key each one presses.
pub fn confirm(buf: &mut Buffer, area: Rect, app: &App, confirm: &Confirm) -> Vec<(Rect, KeyCode)> {
    let width = area.width.min(90);
    let inner_width = usize::from(width.saturating_sub(4));
    let (title, mut lines) = if confirm.idle {
        (app.text.idle_title, idle_lines(app, confirm, inner_width))
    } else {
        (
            app.text.remove_title,
            single_lines(app, confirm, inner_width),
        )
    };
    lines.push(Line::default());
    let remove_label = if confirm.idle {
        app.text.key_remove_all
    } else {
        app.text.key_remove
    };
    let enter = key(app, "⏎");
    let esc = key(app, "Esc");
    let remove_width = (enter.width() + 1 + remove_label.width()) as u16;
    let cancel_width = (esc.width() + 1 + app.text.key_cancel.width()) as u16;
    lines.push(Line::from(vec![
        enter,
        Span::raw(format!(" {remove_label}")),
        Span::raw("    "),
        esc,
        Span::raw(format!(" {}", app.text.key_cancel)),
    ]));

    let height = lines.len() as u16 + 2;
    let inner = frame(buf, area, width, height, title, app.theme.error);
    let buttons_y = inner.y + lines.len() as u16 - 1;
    let lines: Vec<Line> = lines
        .into_iter()
        .map(|line| Line::from(fit(line.spans, inner_width)).style(line.style))
        .collect();
    Paragraph::new(lines).render(inner, buf);
    if buttons_y >= inner.bottom() {
        return Vec::new();
    }
    let button =
        |x: u16, width: u16| Rect::new(x, buttons_y, width.min(inner.right().saturating_sub(x)), 1);
    vec![
        (button(inner.x, remove_width), KeyCode::Enter),
        (
            button(inner.x + remove_width + 4, cancel_width),
            KeyCode::Esc,
        ),
    ]
}

/// Removing one worktree: what it stops, what is lost, and the git command.
fn single_lines(app: &App, confirm: &Confirm, width: usize) -> Vec<Line<'static>> {
    let theme = &app.theme;
    let text = app.text;
    let home = app.home.as_deref();
    let mut lines: Vec<Line> = Vec::new();
    let Some((r, w)) = confirm.targets.first().and_then(|key| app.find(key)) else {
        return lines;
    };
    let (repo, worktree) = app.tree(r, w);
    let mut head = vec![Span::styled(
        worktree_name(repo, worktree).to_string(),
        Style::new().bold(),
    )];
    if let Some(bytes) = app.size(worktree).and_then(|size| size.bytes()) {
        head.push(Span::styled(
            format!("  {}", fmt::size(bytes, app.lang)),
            theme.muted(),
        ));
    }
    lines.push(Line::from(head));
    lines.push(Line::styled(
        fmt::truncate_middle(&fmt::tilde(&worktree.path, home), width),
        theme.muted(),
    ));
    lines.push(Line::default());

    let stops = app.scan.inside(&worktree.real);
    if !stops.is_empty() {
        let style = Style::new().fg(theme.error).bold();
        warning(&mut lines, width, "■", &text.stops.of(stops.len()), style);
        let mut others: BTreeMap<&str, Vec<u32>> = BTreeMap::new();
        for pid in &stops {
            if let Some(session) = app.scan.sessions.iter().find(|s| s.pid == *pid) {
                let mut spans = vec![
                    Span::raw("    "),
                    Span::styled(
                        format!("{} {}", agent_glyph(session.agent), session.agent.name()),
                        Style::new().fg(theme.agent(session.agent)).bold(),
                    ),
                ];
                if let Some(name) = &session.name {
                    spans.push(Span::raw(format!("  {name}")));
                }
                spans.push(Span::styled(format!(" · pid {pid}"), theme.muted()));
                lines.push(Line::from(spans));
            } else if let Some(process) = app.scan.running.iter().find(|p| p.pid == *pid) {
                others.entry(process.name.as_str()).or_default().push(*pid);
            }
        }
        for (name, pids) in others {
            let pids: Vec<String> = pids.iter().map(u32::to_string).collect();
            let label = if pids.len() > 1 { "pids" } else { "pid" };
            lines.push(Line::from(vec![
                Span::raw(format!("    {name}")),
                Span::styled(format!(" · {label} {}", pids.join(" ")), theme.muted()),
            ]));
        }
    }
    if !worktree.missing()
        && let Some(Ok(status)) = app.git(worktree).and_then(|git| git.status.as_ref())
        && status.at_risk() > 0
    {
        let message = text.will_be_lost.of(status.at_risk() as usize);
        warning(
            &mut lines,
            width,
            "±",
            &message,
            Style::new().fg(theme.warn),
        );
    }
    if let Some(reason) = &worktree.locked {
        let mut message = text.locked.replace("{reason}", reason.trim());
        if let Some(pid) = lock_holder(reason)
            && !app.scan.is_alive(pid)
        {
            message = format!("{message} · {}", text.lock_stale);
        }
        warning(
            &mut lines,
            width,
            "⊘",
            &message,
            Style::new().fg(theme.locked),
        );
    }
    if worktree.missing() {
        warning(&mut lines, width, "✗", text.warn_missing, theme.muted());
    }
    if let Some(branch) = &worktree.branch {
        let message = text.note_branch.replace("{branch}", branch);
        warning(&mut lines, width, "·", &message, theme.faint());
    }
    lines.push(Line::default());
    let command = git::remove_command(
        &fmt::tilde(repo.command_dir(), home),
        &fmt::tilde(&worktree.path, home),
        Force::needed(worktree.locked.is_some()),
    );
    for part in fmt::hard_wrap(&format!("$ {command}"), width, 4) {
        lines.push(Line::styled(part, Style::new().fg(theme.accent)));
    }
    lines
}

/// Removing every idle worktree: how many, how much space, and which.
fn idle_lines(app: &App, confirm: &Confirm, width: usize) -> Vec<Line<'static>> {
    let theme = &app.theme;
    let text = app.text;
    let mut lines: Vec<Line> = Vec::new();
    let mut trees: Vec<(String, u64)> = Vec::new();
    let mut dirty = 0;
    let repos: std::collections::HashSet<usize> = confirm
        .targets
        .iter()
        .filter_map(|key| app.find(key).map(|(r, _)| r))
        .collect();
    for key in &confirm.targets {
        let Some((r, w)) = app.find(key) else {
            continue;
        };
        let (repo, worktree) = app.tree(r, w);
        let bytes = app
            .size(worktree)
            .and_then(|size| size.bytes())
            .unwrap_or(0);
        let name = if repos.len() > 1 {
            format!("{} / {}", repo.name, worktree_name(repo, worktree))
        } else {
            worktree_name(repo, worktree).to_string()
        };
        trees.push((name, bytes));
        if let Some(Ok(status)) = app.git(worktree).and_then(|git| git.status.as_ref()) {
            dirty += usize::from(status.at_risk() > 0);
        }
    }
    trees.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    let total: u64 = trees.iter().map(|(_, bytes)| bytes).sum();
    lines.push(Line::styled(
        text.idle_summary
            .replace("{n}", &trees.len().to_string())
            .replace("{size}", &fmt::size(total, app.lang)),
        Style::new().bold(),
    ));
    lines.push(Line::default());
    const SHOWN: usize = 10;
    let shown = if trees.len() <= SHOWN + 1 {
        trees.len()
    } else {
        SHOWN
    };
    let (symbol, color) = state_symbol(app, State::Free);
    for (name, bytes) in trees.iter().take(shown) {
        let size = if *bytes > 0 {
            fmt::size(*bytes, app.lang)
        } else {
            "—".into()
        };
        let room = width.saturating_sub(size.width() + 3);
        let name = fmt::truncate(name, room);
        let gap = width.saturating_sub(name.width() + size.width() + 2);
        lines.push(Line::from(vec![
            Span::styled(format!("{symbol} "), Style::new().fg(color)),
            Span::raw(name),
            Span::raw(" ".repeat(gap)),
            Span::styled(size, theme.muted()),
        ]));
    }
    if trees.len() > shown {
        lines.push(Line::styled(
            text.idle_more
                .replace("{n}", &(trees.len() - shown).to_string()),
            theme.muted(),
        ));
    }
    lines.push(Line::default());
    if dirty > 0 {
        warning(
            &mut lines,
            width,
            "±",
            &text.idle_dirty.of(dirty),
            Style::new().fg(theme.warn),
        );
    }
    warning(
        &mut lines,
        width,
        "$",
        text.idle_command,
        Style::new().fg(theme.accent),
    );
    lines
}

pub fn help(buf: &mut Buffer, area: Rect, app: &App) {
    let theme = &app.theme;
    let text = app.text;
    let key_width = text
        .help_keys
        .iter()
        .map(|(key, _)| key.width())
        .max()
        .unwrap_or(0)
        + 3;
    let mut lines: Vec<Line> = text
        .help_keys
        .iter()
        .map(|(key, action)| {
            Line::from(vec![
                Span::styled(
                    format!("{key:<key_width$}"),
                    Style::new().fg(theme.accent).bold(),
                ),
                Span::raw(action.to_string()),
            ])
        })
        .collect();
    lines.push(Line::default());
    lines.push(Line::styled(text.legend_title, theme.muted().bold()));
    let spin = SPINNER[app.frame % SPINNER.len()];
    let legend = [
        vec![
            (spin, theme.working, text.state_working),
            ("◆", theme.blocked, text.state_blocked),
            ("●", theme.idle, text.state_idle),
        ],
        vec![
            ("◎", theme.busy, text.state_busy),
            ("○", theme.free, text.state_free),
            ("✗", theme.missing, text.state_missing),
        ],
        Agent::ALL
            .iter()
            .map(|agent| (agent_glyph(*agent), theme.agent(*agent), agent.name()))
            .collect(),
    ];
    for row in legend {
        let mut spans = Vec::new();
        for (symbol, color, label) in row {
            spans.push(Span::styled(
                format!("{symbol} "),
                Style::new().fg(color).bold(),
            ));
            spans.push(Span::raw(format!("{label}   ")));
        }
        lines.push(Line::from(spans));
    }
    let content = lines
        .iter()
        .map(|line| spans_width(&line.spans))
        .max()
        .unwrap_or(0);
    let width = (content as u16 + 4).min(area.width);
    let height = lines.len() as u16 + 2;
    let inner = frame(buf, area, width, height, text.help_title, theme.accent);
    let width = usize::from(inner.width);
    let lines: Vec<Line> = lines
        .into_iter()
        .map(|line| Line::from(fit(line.spans, width)).style(line.style))
        .collect();
    Paragraph::new(lines).render(inner, buf);
}
