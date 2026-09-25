//! The list of repositories and their worktrees, in three densities.

use std::time::SystemTime;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::Span;
use ratatui::widgets::{Scrollbar, ScrollbarOrientation, ScrollbarState, StatefulWidget};
use unicode_width::UnicodeWidthStr;

use super::{bar, fit, put, put_right, put_spans, spans_width, state_symbol};
use crate::app::{App, Item, SizeState, State};
use crate::fmt;
use crate::layout::{self, Columns, Density, LEAD, SIZE, TRAIL};
use crate::model::{Activity, Agent, Repo, Worktree, worktree_name};
use crate::theme::{SPINNER, agent_glyph};

pub fn render(buf: &mut Buffer, area: Rect, app: &mut App, items: &[Item]) {
    app.hits.clear();
    if area.width < 6 || area.height == 0 {
        return;
    }
    let longest = items
        .iter()
        .filter_map(|item| match *item {
            Item::Tree(r, w) => {
                let (repo, worktree) = app.tree(r, w);
                Some(spans_width(&name_spans(
                    app,
                    repo,
                    worktree,
                    State::Free,
                    u16::MAX,
                )))
            }
            Item::Repo(_) => None,
        })
        .max()
        .unwrap_or(0);
    let density = layout::density(area, u16::try_from(longest).unwrap_or(u16::MAX));
    let tree_height: u16 = if density == Density::Cards { 2 } else { 1 };
    // A blank line between repositories when the pane is tall enough.
    let spaced = area.height >= 16;
    let heights: Vec<u16> = items
        .iter()
        .enumerate()
        .map(|(i, item)| match item {
            Item::Repo(_) if i > 0 && spaced => 2,
            Item::Repo(_) => 1,
            Item::Tree(..) => tree_height,
        })
        .collect();
    app.page = usize::from((area.height / tree_height).max(1));
    let selected = app.selected_index(items);
    app.offset = scroll(app.offset, selected, &heights, items, area.height);
    let scale = scale(app, items);

    let mut y = area.y;
    let mut hits = Vec::new();
    for (index, item) in items.iter().enumerate().skip(app.offset) {
        let height = shown_height(&heights, items, index, app.offset);
        if y + height > area.bottom() {
            break;
        }
        let row = Rect {
            x: area.x,
            y,
            width: area.width - TRAIL,
            height,
        };
        match *item {
            Item::Repo(r) => {
                let line = Rect {
                    y: row.bottom() - 1,
                    height: 1,
                    ..row
                };
                heading(buf, line, app, items, r, density);
            }
            Item::Tree(r, w) => {
                let is_selected = selected == Some(index);
                if is_selected {
                    buf.set_style(row, app.theme.selected());
                }
                match density {
                    Density::Table(columns) => {
                        table_row(buf, row, app, (r, w), is_selected, columns, scale)
                    }
                    Density::Cards => card_row(buf, row, app, (r, w), is_selected, scale),
                    Density::Minimal => minimal_row(buf, row, app, (r, w), is_selected),
                }
                hits.push((row, index));
            }
        }
        y += height;
    }
    app.hits = hits;

    let total: u16 = heights.iter().sum();
    if total > area.height {
        let position: u16 = heights[..app.offset].iter().sum();
        let mut state = ScrollbarState::new(usize::from(total - area.height) + 1)
            .position(usize::from(position))
            .viewport_content_length(usize::from(area.height));
        Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .begin_symbol(None)
            .end_symbol(None)
            .track_symbol(Some("│"))
            .thumb_symbol("┃")
            .track_style(app.theme.faint())
            .thumb_style(app.theme.muted())
            .render(area, buf, &mut state);
    }
}

/// A heading's blank line is left out at the top of the list.
fn shown_height(heights: &[u16], items: &[Item], index: usize, offset: usize) -> u16 {
    if index == offset && matches!(items[index], Item::Repo(_)) {
        1
    } else {
        heights[index]
    }
}

/// The first item to show so the selected one is on screen, with its repository heading
/// when there is room, and no empty space left at the bottom.
fn scroll(
    offset: usize,
    selected: Option<usize>,
    heights: &[u16],
    items: &[Item],
    height: u16,
) -> usize {
    if heights.is_empty() {
        return 0;
    }
    let span = |from: usize, to: usize| -> u16 {
        (from..=to)
            .map(|i| shown_height(heights, items, i, from))
            .sum()
    };
    let mut offset = offset.min(heights.len() - 1);
    if let Some(selected) = selected {
        let top = match selected.checked_sub(1) {
            Some(above) if matches!(items[above], Item::Repo(_)) => above,
            _ => selected,
        };
        offset = offset.min(top);
        while offset < selected && span(offset, selected) > height {
            offset += 1;
        }
    }
    while offset > 0 && span(offset - 1, heights.len() - 1) <= height {
        offset -= 1;
    }
    offset
}

/// Bars measure against the largest linked worktree: the main one, which also holds the
/// repository's history, would flatten every other bar.
fn scale(app: &App, items: &[Item]) -> u64 {
    let bytes = |main: bool| {
        items
            .iter()
            .filter_map(|item| match *item {
                Item::Tree(r, w) => Some(app.tree(r, w).1),
                Item::Repo(_) => None,
            })
            .filter(|worktree| worktree.main == main)
            .filter_map(|worktree| app.size(worktree).and_then(SizeState::bytes))
            .max()
    };
    bytes(false).or_else(|| bytes(true)).unwrap_or(0)
}

fn heading(buf: &mut Buffer, row: Rect, app: &App, items: &[Item], r: usize, density: Density) {
    let theme = &app.theme;
    let repo = &app.repos[r];
    let trees: Vec<&Worktree> = items
        .iter()
        .filter_map(|item| match *item {
            Item::Tree(tree_repo, w) if tree_repo == r => Some(&repo.worktrees[w]),
            _ => None,
        })
        .collect();
    let bytes: u64 = trees
        .iter()
        .filter_map(|w| app.size(w).and_then(SizeState::bytes))
        .sum();
    let summary = match density {
        Density::Table(_) => format!("{} worktrees · {}", trees.len(), fmt::size(bytes, app.lang)),
        Density::Cards => format!("{} · {}", trees.len(), fmt::size(bytes, app.lang)),
        Density::Minimal => fmt::size_short(bytes, app.lang),
    };
    let width = row.width;
    let mut x = row.x + 1;
    let name = fmt::truncate(&repo.name, usize::from(width.saturating_sub(2)));
    x += put(
        buf,
        x,
        row.y,
        width.saturating_sub(1),
        &name,
        Style::new().fg(theme.accent).bold(),
    );
    let summary_width = summary.width() as u16;
    let end = row.x + width;
    // The summary goes at the right when a short rule still fits before it.
    let summary_x = end.saturating_sub(summary_width + 1);
    let with_summary = summary_x >= x + 4;
    let rule_end = if with_summary { summary_x - 1 } else { end };
    if matches!(density, Density::Table(_)) {
        let path = fmt::tilde(repo.command_dir(), app.home.as_deref());
        let room = rule_end.saturating_sub(x + 2 + 4);
        if room >= 10 {
            x += 2;
            x += put(
                buf,
                x,
                row.y,
                room,
                &fmt::truncate_middle(&path, usize::from(room)),
                theme.faint(),
            );
        }
    }
    if rule_end > x + 1 {
        let length = rule_end - x - 1;
        put(
            buf,
            x + 1,
            row.y,
            length,
            &"─".repeat(usize::from(length)),
            theme.faint(),
        );
    }
    if with_summary {
        put(
            buf,
            summary_x,
            row.y,
            summary_width,
            &summary,
            theme.muted(),
        );
    }
}

/// The selection mark and the state symbol.
fn lead(buf: &mut Buffer, x: u16, y: u16, app: &App, state: State, selected: bool) {
    let (symbol, color) = state_symbol(app, state);
    if selected {
        put(buf, x, y, 1, "▍", Style::new().fg(app.theme.accent));
    }
    put(buf, x + 1, y, 1, symbol, Style::new().fg(color));
}

fn table_row(
    buf: &mut Buffer,
    row: Rect,
    app: &App,
    (r, w): (usize, usize),
    selected: bool,
    columns: Columns,
    scale: u64,
) {
    let (repo, worktree) = app.tree(r, w);
    let state = app.state(worktree);
    let y = row.y;
    lead(buf, row.x, y, app, state, selected);
    let mut x = row.x + LEAD;
    put_spans(
        buf,
        x,
        y,
        columns.name,
        name_spans(app, repo, worktree, state, columns.name),
    );
    x += columns.name;
    if columns.branch > 0 {
        x += 1;
        put_spans(
            buf,
            x,
            y,
            columns.branch,
            branch_spans(app, worktree, columns.branch),
        );
        x += columns.branch;
    }
    if columns.git > 0 {
        x += 1;
        put_spans(
            buf,
            x,
            y,
            columns.git,
            fit(git_spans(app, worktree), usize::from(columns.git)),
        );
        x += columns.git;
    }
    x += 2;
    let agents = fit(
        agents_spans(app, worktree, columns.agents),
        usize::from(columns.agents),
    );
    put_spans(buf, x, y, columns.agents, agents);
    x += columns.agents + 1;
    put_right(buf, x, y, columns.size, size_spans(app, worktree, false));
    x += columns.size + 1;
    put_spans(
        buf,
        x,
        y,
        columns.bar,
        bar_spans(app, worktree, scale, columns.bar),
    );
    x += columns.bar;
    if columns.age > 0 {
        put_right(buf, x + 1, y, columns.age, age_spans(app, worktree, state));
    }
}

/// Two lines: name and size, then what is going on there and a size bar.
fn card_row(
    buf: &mut Buffer,
    row: Rect,
    app: &App,
    (r, w): (usize, usize),
    selected: bool,
    scale: u64,
) {
    let (repo, worktree) = app.tree(r, w);
    let state = app.state(worktree);
    lead(buf, row.x, row.y, app, state, selected);
    if selected {
        put(
            buf,
            row.x,
            row.y + 1,
            1,
            "▍",
            Style::new().fg(app.theme.accent),
        );
    }
    let text_width = row.width.saturating_sub(LEAD + 1 + SIZE);
    let x = row.x + LEAD;
    put_spans(
        buf,
        x,
        row.y,
        text_width,
        name_spans(app, repo, worktree, state, text_width),
    );
    put_right(
        buf,
        x + text_width + 1,
        row.y,
        SIZE,
        size_spans(app, worktree, false),
    );
    let second = fit(
        second_line(app, worktree, text_width),
        usize::from(text_width),
    );
    put_spans(buf, x, row.y + 1, text_width, second);
    put_spans(
        buf,
        x + text_width + 1,
        row.y + 1,
        SIZE,
        bar_spans(app, worktree, scale, SIZE),
    );
}

fn minimal_row(buf: &mut Buffer, row: Rect, app: &App, (r, w): (usize, usize), selected: bool) {
    let (repo, worktree) = app.tree(r, w);
    let state = app.state(worktree);
    lead(buf, row.x, row.y, app, state, selected);
    let size_width = if row.width >= LEAD + 10 { 5 } else { 0 };
    let name_width = row.width.saturating_sub(LEAD + size_width);
    put_spans(
        buf,
        row.x + LEAD,
        row.y,
        name_width,
        name_spans(app, repo, worktree, state, name_width),
    );
    if size_width > 0 {
        put_right(
            buf,
            row.x + LEAD + name_width + 1,
            row.y,
            4,
            size_spans(app, worktree, true),
        );
    }
}

fn name_spans(
    app: &App,
    repo: &Repo,
    worktree: &Worktree,
    state: State,
    width: u16,
) -> Vec<Span<'static>> {
    let theme = &app.theme;
    let name = worktree_name(repo, worktree).to_string();
    let style = match state {
        State::Missing => theme.muted().add_modifier(Modifier::CROSSED_OUT),
        State::Removing => theme.muted(),
        State::Blocked | State::Working | State::Idle => Style::new().bold(),
        State::Busy | State::Free => Style::new(),
    };
    let mut tags = Vec::new();
    if worktree.main {
        tags.push(Span::styled(
            format!(" {}", app.text.main_tag),
            theme.muted(),
        ));
    }
    if worktree.locked.is_some() {
        tags.push(Span::styled(
            format!(" {}", app.text.locked_tag),
            Style::new().fg(theme.locked),
        ));
    }
    let width = usize::from(width);
    let mut spans = vec![Span::styled(fmt::truncate(&name, width), style)];
    // Tags only when the whole name fits with them.
    if name.width() + spans_width(&tags) <= width {
        spans.extend(tags);
    }
    spans
}

fn branch_spans(app: &App, worktree: &Worktree, width: u16) -> Vec<Span<'static>> {
    let theme = &app.theme;
    if worktree.missing() {
        return vec![Span::styled(
            app.text.state_missing,
            Style::new().fg(theme.missing),
        )];
    }
    let (label, style) = match (&worktree.branch, &worktree.head) {
        (Some(branch), _) => (branch.clone(), theme.muted()),
        (None, Some(head)) => (head.chars().take(7).collect(), theme.muted().italic()),
        (None, None) => (app.text.detached.to_string(), theme.muted().italic()),
    };
    // Branches often share a prefix ("worktree-…"): the end tells them apart.
    let label = fmt::truncate_middle(&label, usize::from(width.saturating_sub(2)));
    vec![
        Span::styled("⎇ ", theme.faint()),
        Span::styled(label, style),
    ]
}

fn git_spans(app: &App, worktree: &Worktree) -> Vec<Span<'static>> {
    let theme = &app.theme;
    if worktree.missing() {
        return Vec::new();
    }
    match app.git(worktree).and_then(|git| git.status.as_ref()) {
        None => vec![Span::styled("…", theme.faint())],
        Some(Err(_)) => vec![Span::styled("!", Style::new().fg(theme.error))],
        Some(Ok(status)) => {
            let mut spans = Vec::new();
            match status.at_risk() {
                0 => spans.push(Span::styled("✓", theme.muted())),
                n => spans.push(Span::styled(format!("±{n}"), Style::new().fg(theme.warn))),
            }
            if status.ahead > 0 {
                spans.push(Span::styled(format!(" ↑{}", status.ahead), theme.muted()));
            }
            if status.behind > 0 {
                spans.push(Span::styled(format!(" ↓{}", status.behind), theme.muted()));
            }
            spans
        }
    }
}

/// Who is in the worktree: one session by name, several by agent and state, or the
/// processes when no agent is there.
fn agents_spans(app: &App, worktree: &Worktree, width: u16) -> Vec<Span<'static>> {
    let theme = &app.theme;
    if app.removing.contains(&worktree.real) {
        return vec![Span::styled(
            app.text.state_removing,
            Style::new().fg(theme.error),
        )];
    }
    if worktree.missing() {
        return vec![Span::styled(
            format!("✗ {}", app.text.state_missing),
            Style::new().fg(theme.missing),
        )];
    }
    let Some(presence) = app.presence(worktree) else {
        return Vec::new();
    };
    if presence.sessions.is_empty() {
        let Some(first) = presence.procs.first() else {
            return Vec::new();
        };
        return vec![
            Span::styled(
                format!("◎ {} ", presence.procs.len()),
                Style::new().fg(theme.busy),
            ),
            Span::styled(first.name.clone(), theme.muted()),
        ];
    }
    if let [only] = presence.sessions.as_slice() {
        let session = &only.session;
        let mut spans = vec![Span::styled(
            format!("{} {}", agent_glyph(session.agent), session.agent.name()),
            Style::new().fg(theme.agent(session.agent)).bold(),
        )];
        if let Some((_, via)) = &only.via {
            spans.push(Span::styled(format!(" via {via}"), theme.muted()));
        } else if let Some(name) = &session.name {
            spans.push(Span::raw(format!("  {name}")));
            if let Some(detail) = &session.detail
                && spans_width(&spans) + 16 <= usize::from(width)
            {
                spans.push(Span::styled(format!(" · {detail}"), theme.muted()));
            }
        } else {
            spans.push(Span::styled(
                format!("  pid {} · {:.0}% cpu", session.pid, session.cpu),
                theme.muted(),
            ));
        }
        return spans;
    }
    let mut spans = Vec::new();
    for agent in Agent::ALL {
        let n = presence
            .sessions
            .iter()
            .filter(|p| p.session.agent == agent)
            .count();
        if n == 0 {
            continue;
        }
        if !spans.is_empty() {
            spans.push(Span::raw(" "));
        }
        let count = if n > 1 {
            format!(" ×{n}")
        } else {
            String::new()
        };
        spans.push(Span::styled(
            format!("{} {}{count}", agent_glyph(agent), agent.name()),
            Style::new().fg(theme.agent(agent)).bold(),
        ));
    }
    let spin = SPINNER[app.frame % SPINNER.len()];
    for (activity, symbol, color) in [
        (Activity::Blocked, "◆", theme.blocked),
        (Activity::Working, spin, theme.working),
        (Activity::Idle, "●", theme.idle),
    ] {
        let n = presence
            .sessions
            .iter()
            .filter(|p| p.session.activity == activity)
            .count();
        if n > 0 {
            spans.push(Span::styled(
                format!("  {symbol}{n}"),
                Style::new().fg(color),
            ));
        }
    }
    spans
}

/// The second line of a card: who is there, or the branch and its pending changes.
fn second_line(app: &App, worktree: &Worktree, width: u16) -> Vec<Span<'static>> {
    let agents = agents_spans(app, worktree, width);
    if !agents.is_empty() {
        return agents;
    }
    let mut spans = branch_spans(app, worktree, width);
    if let Some(Ok(status)) = app.git(worktree).and_then(|git| git.status.as_ref())
        && status.at_risk() > 0
    {
        spans.push(Span::styled(
            format!("  ±{}", status.at_risk()),
            Style::new().fg(app.theme.warn),
        ));
    }
    spans
}

fn size_spans(app: &App, worktree: &Worktree, short: bool) -> Vec<Span<'static>> {
    let theme = &app.theme;
    let format = |bytes| {
        if short {
            fmt::size_short(bytes, app.lang)
        } else {
            fmt::size(bytes, app.lang)
        }
    };
    if worktree.missing() {
        return vec![Span::styled("—", theme.faint())];
    }
    let Some(size) = app.size(worktree) else {
        return vec![Span::styled("…", theme.faint())];
    };
    match (size.bytes(), size.running) {
        (Some(bytes), _) => vec![Span::raw(format(bytes))],
        (None, Some((bytes, _))) => {
            let spin = SPINNER[app.frame % SPINNER.len()];
            let text = if short {
                spin.to_string()
            } else {
                format!("{spin} {}", format(bytes))
            };
            vec![Span::styled(text, theme.muted())]
        }
        (None, None) => vec![Span::styled("…", theme.faint())],
    }
}

fn bar_spans(app: &App, worktree: &Worktree, scale: u64, width: u16) -> Vec<Span<'static>> {
    let theme = &app.theme;
    let bytes = app
        .size(worktree)
        .and_then(|size| size.bytes().or(size.running.map(|(bytes, _)| bytes)));
    let Some(bytes) = bytes.filter(|_| scale > 0) else {
        return vec![Span::styled("·".repeat(usize::from(width)), theme.faint())];
    };
    let fraction = bytes as f64 / scale as f64;
    let color = if worktree.main {
        theme.muted
    } else {
        theme.size_color(fraction)
    };
    bar(theme, fraction, width, color)
}

fn age_spans(app: &App, worktree: &Worktree, state: State) -> Vec<Span<'static>> {
    let Some(when) = app.last_used(worktree) else {
        return Vec::new();
    };
    let seconds = SystemTime::now()
        .duration_since(when)
        .map_or(0, |elapsed| elapsed.as_secs());
    let style = if matches!(state, State::Working | State::Blocked) {
        Style::new().fg(app.theme.working)
    } else {
        app.theme.muted()
    };
    vec![Span::styled(fmt::ago_short(seconds, app.lang), style)]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scrolls_to_the_selection_with_its_heading() {
        let items = [
            Item::Repo(0),
            Item::Tree(0, 0),
            Item::Tree(0, 1),
            Item::Repo(1),
            Item::Tree(1, 0),
            Item::Tree(1, 1),
        ];
        let heights = [1, 1, 1, 2, 1, 1];
        // Everything fits: no scrolling.
        assert_eq!(scroll(3, Some(5), &heights, &items, 10), 0);
        // Going down shows the heading of the second repository with its first worktree.
        assert_eq!(scroll(0, Some(4), &heights, &items, 3), 3);
        // Going back up to the first worktree brings its heading back.
        assert_eq!(scroll(3, Some(1), &heights, &items, 3), 0);
        // A window too short for heading and worktree keeps the worktree.
        assert_eq!(scroll(0, Some(4), &heights, &items, 1), 4);
    }
}
