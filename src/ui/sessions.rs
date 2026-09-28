//! The sessions list: the conversations agents keep, by project, in the same three densities
//! as the worktrees, and the details of one with the command that picks it up again.

use std::time::SystemTime;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use super::details::{ago, column, panel, title};
use super::list::{self, heading_line, lead};
use super::{centered, fit, put, put_right, put_spans, spans_width};
use crate::app::{App, Row, State};
use crate::fmt;
use crate::history;
use crate::layout::{self, Density, LEAD, SessionColumns, TRAIL};
use crate::model::{Activity, Agent, Conversation};
use crate::theme::{SPINNER, agent_glyph};

/// Cells for the time since a conversation was last active.
const AGE: u16 = 5;

pub fn render(buf: &mut Buffer, area: Rect, app: &mut App, rows: &[Row]) {
    app.hits.clear();
    if area.width < 6 || area.height == 0 {
        return;
    }
    let density = layout::density(area, 0);
    let row_height: u16 = if density == Density::Cards { 2 } else { 1 };
    let headings: Vec<bool> = rows
        .iter()
        .map(|row| matches!(row, Row::Project(_)))
        .collect();
    let heights = list::heights(&headings, row_height, area.height);
    app.page = usize::from((area.height / row_height).max(1));
    let selected = app.chosen_index(rows);
    app.history_offset = list::scroll(
        app.history_offset,
        selected,
        &heights,
        &headings,
        area.height,
    );
    let columns = layout::session_columns(area.width);

    let mut y = area.y;
    let mut hits = Vec::new();
    for (index, row) in rows.iter().enumerate().skip(app.history_offset) {
        let height = list::shown_height(&heights, &headings, index, app.history_offset);
        if y + height > area.bottom() {
            break;
        }
        let rect = Rect {
            x: area.x,
            y,
            width: area.width - TRAIL,
            height,
        };
        match *row {
            Row::Project(first) => {
                let line = Rect {
                    y: rect.bottom() - 1,
                    height: 1,
                    ..rect
                };
                heading(buf, line, app, rows, first, density);
            }
            Row::Conversation(i) => {
                let is_selected = selected == Some(index);
                if is_selected {
                    buf.set_style(rect, app.theme.selected());
                }
                match density {
                    Density::Table(_) => table_row(buf, rect, app, i, is_selected, columns),
                    Density::Cards => card_row(buf, rect, app, i, is_selected),
                    Density::Minimal => minimal_row(buf, rect, app, i, is_selected),
                }
                hits.push((rect, index));
            }
        }
        y += height;
    }
    app.hits = hits;
    list::scrollbar(buf, area, app, &heights, app.history_offset);
}

/// What the list says when there is nothing to show.
pub fn empty(buf: &mut Buffer, area: Rect, app: &App) {
    let text = app.text;
    let theme = &app.theme;
    let lines = if !app.history_found {
        let spin = SPINNER[app.frame % SPINNER.len()];
        vec![Line::from(Span::styled(
            format!("{spin} {}", text.searching_sessions),
            theme.muted(),
        ))]
    } else if !app.filter.is_empty() {
        vec![Line::from(vec![
            Span::styled(format!("{} ", text.no_match), theme.muted()),
            Span::styled(format!("“{}”", app.filter), Style::new().fg(theme.accent)),
        ])]
    } else {
        let mut lines = vec![
            Line::from(Span::styled(text.no_conversations, Style::new().bold())),
            Line::default(),
        ];
        lines.extend(
            fmt::wrap(
                text.no_conversations_hint,
                usize::from(area.width.saturating_sub(4)),
                4,
            )
            .into_iter()
            .map(|line| Line::from(Span::styled(line, theme.muted()))),
        );
        lines
    };
    centered(buf, area, lines);
}

fn heading(buf: &mut Buffer, row: Rect, app: &App, rows: &[Row], first: usize, density: Density) {
    let project = &app.history[first].project;
    let count = rows
        .iter()
        .filter(|row| matches!(row, Row::Conversation(i) if app.history[*i].project == *project))
        .count();
    let (summary, path) = match density {
        Density::Table(_) => (app.text.sessions_tab.of(count), Some(project.as_path())),
        _ => (count.to_string(), None),
    };
    heading_line(
        buf,
        row,
        app,
        &app.history[first].project_name,
        path,
        &summary,
    );
}

fn table_row(
    buf: &mut Buffer,
    row: Rect,
    app: &App,
    i: usize,
    selected: bool,
    columns: SessionColumns,
) {
    let conversation = &app.history[i];
    let state = app.conversation_state(i);
    let y = row.y;
    lead(buf, row.x, y, app, state, selected);
    let mut x = row.x + LEAD;
    put_spans(
        buf,
        x,
        y,
        columns.agent,
        agent_spans(app, conversation.agent, columns.agent > 1),
    );
    x += columns.agent + 1;
    put_spans(
        buf,
        x,
        y,
        columns.title,
        fit(
            title_spans(app, i, state, columns.title),
            usize::from(columns.title),
        ),
    );
    x += columns.title;
    if columns.place > 0 {
        x += 2;
        put_spans(
            buf,
            x,
            y,
            columns.place,
            fit(place_spans(app, conversation), usize::from(columns.place)),
        );
        x += columns.place;
    }
    put_right(buf, x + 1, y, columns.age, age_spans(app, i, state));
}

/// Two lines: the title and when it was last active, then the agent and where it worked.
fn card_row(buf: &mut Buffer, row: Rect, app: &App, i: usize, selected: bool) {
    let conversation = &app.history[i];
    let state = app.conversation_state(i);
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
    let text_width = row.width.saturating_sub(LEAD + 1 + AGE);
    let x = row.x + LEAD;
    put_spans(
        buf,
        x,
        row.y,
        text_width,
        fit(
            title_spans(app, i, state, text_width),
            usize::from(text_width),
        ),
    );
    put_right(
        buf,
        x + text_width + 1,
        row.y,
        AGE,
        age_spans(app, i, state),
    );
    let mut second = agent_spans(app, conversation.agent, true);
    let place = place_spans(app, conversation);
    if !place.is_empty() {
        second.push(Span::raw("  "));
        second.extend(place);
    }
    let width = row.width.saturating_sub(LEAD);
    put_spans(buf, x, row.y + 1, width, fit(second, usize::from(width)));
}

fn minimal_row(buf: &mut Buffer, row: Rect, app: &App, i: usize, selected: bool) {
    let state = app.conversation_state(i);
    lead(buf, row.x, row.y, app, state, selected);
    let age_width = if row.width >= LEAD + 14 { AGE - 1 } else { 0 };
    let title_width = row
        .width
        .saturating_sub(LEAD + age_width + u16::from(age_width > 0));
    put_spans(
        buf,
        row.x + LEAD,
        row.y,
        title_width,
        fit(
            title_spans(app, i, state, title_width),
            usize::from(title_width),
        ),
    );
    if age_width > 0 {
        put_right(
            buf,
            row.x + LEAD + title_width + 1,
            row.y,
            age_width,
            age_spans(app, i, state),
        );
    }
}

fn agent_spans(app: &App, agent: Agent, named: bool) -> Vec<Span<'static>> {
    let text = if named {
        format!("{} {}", agent_glyph(agent), agent.name())
    } else {
        agent_glyph(agent).to_string()
    };
    vec![Span::styled(
        text,
        Style::new().fg(app.theme.agent(agent)).bold(),
    )]
}

/// Its title, else what was asked in it; for one open now, what the session says it does.
fn title_spans(app: &App, i: usize, state: State, width: u16) -> Vec<Span<'static>> {
    let theme = &app.theme;
    let conversation = &app.history[i];
    let style = match state {
        State::Blocked | State::Working | State::Idle => Style::new().bold(),
        State::Missing => theme.muted(),
        _ => Style::new(),
    };
    let mut spans = match (&conversation.title, conversation.label()) {
        (Some(title), _) => vec![Span::styled(title.clone(), style)],
        (None, Some(prompt)) => vec![Span::styled(prompt.to_string(), style.italic())],
        (None, None) => vec![Span::styled(app.text.untitled, theme.muted().italic())],
    };
    if let Some(detail) = app.live(i).and_then(|session| session.detail.as_deref())
        && spans_width(&spans) + 16 <= usize::from(width)
    {
        spans.push(Span::styled(format!(" · {detail}"), theme.muted()));
    }
    spans
}

/// Where it worked: its linked worktree, else its branch.
fn place_spans(app: &App, conversation: &Conversation) -> Vec<Span<'static>> {
    let theme = &app.theme;
    match (&conversation.worktree, &conversation.branch) {
        (Some(worktree), _) => {
            let style = if conversation.gone {
                theme.muted().add_modifier(Modifier::CROSSED_OUT)
            } else {
                Style::new()
            };
            vec![Span::styled(worktree.clone(), style)]
        }
        (None, Some(branch)) => vec![
            Span::styled("⎇ ", theme.faint()),
            Span::styled(branch.clone(), theme.muted()),
        ],
        (None, None) => Vec::new(),
    }
}

fn age_spans(app: &App, i: usize, state: State) -> Vec<Span<'static>> {
    let busy = matches!(state, State::Working | State::Blocked);
    let seconds = if busy {
        0
    } else {
        SystemTime::now()
            .duration_since(app.history[i].updated)
            .map_or(0, |elapsed| elapsed.as_secs())
    };
    let style = if busy {
        Style::new().fg(app.theme.working)
    } else {
        app.theme.muted()
    };
    vec![Span::styled(fmt::ago_short(seconds, app.lang), style)]
}

/// A conversation's state in words.
fn state_label(app: &App, state: State) -> &'static str {
    let text = app.text;
    match state {
        State::Blocked => text.state_blocked,
        State::Working => text.state_working,
        State::Idle => text.state_open,
        State::Missing => text.state_missing,
        _ => text.state_closed,
    }
}

/// The details of a conversation: where and when it worked, what was asked in it, the session
/// that has it open, and the command that picks it up again.
pub fn details(buf: &mut Buffer, area: Rect, app: &App, i: usize) {
    if area.width < 12 || area.height < 3 {
        return;
    }
    let conversation = &app.history[i];
    let state = app.conversation_state(i);
    let name = conversation.label().unwrap_or(app.text.untitled);
    let inner = panel(buf, area, app, name, state, state_label(app, state));
    if inner.is_empty() {
        return;
    }
    let width = usize::from(inner.width);
    let command = command_lines(app, i, width);
    // The command stays in sight: the rest takes the room it leaves.
    let room = usize::from(inner.height).saturating_sub(command.len() + 1);
    column(buf, inner, info_lines(app, i, width, room), command);
}

/// Where and when it worked, then what was asked in it and the session that has it open, as
/// much of them as `room` lines take.
fn info_lines(app: &App, i: usize, width: usize, room: usize) -> Vec<Line<'static>> {
    let mut lines = place_lines(app, i, width);
    let mut sections = prompt_lines(app, i, width)
        .into_iter()
        .chain(open_lines(app, i, width));
    for section in sections.by_ref() {
        let left = room.saturating_sub(lines.len());
        if section.len() <= left {
            lines.extend(section);
        } else {
            // A blank line and a heading, and at least one line under them.
            if left >= 3 {
                lines.extend(section.into_iter().take(left));
            }
            break;
        }
    }
    lines
}

fn place_lines(app: &App, i: usize, width: usize) -> Vec<Line<'static>> {
    let theme = &app.theme;
    let text = app.text;
    let conversation = &app.history[i];
    let mut lines = Vec::new();
    lines.push(Line::from(fit(
        vec![
            Span::styled(
                format!(
                    "{} {}",
                    agent_glyph(conversation.agent),
                    conversation.agent.name()
                ),
                Style::new().fg(theme.agent(conversation.agent)).bold(),
            ),
            Span::styled(format!("  {}", conversation.id), theme.muted()),
        ],
        width,
    )));
    let path = fmt::tilde(&conversation.cwd, app.home.as_deref());
    let path_style = if conversation.gone {
        Style::new().fg(theme.missing)
    } else {
        theme.muted()
    };
    lines.push(Line::styled(fmt::truncate_middle(&path, width), path_style));
    let mut place = Vec::new();
    if let Some(worktree) = &conversation.worktree {
        place.push(Span::raw(worktree.clone()));
    }
    if let Some(branch) = &conversation.branch {
        if !place.is_empty() {
            place.push(Span::raw("  "));
        }
        place.push(Span::styled("⎇ ", theme.faint()));
        place.push(Span::raw(branch.clone()));
    }
    if conversation.gone {
        place.push(Span::styled(
            format!("  ✗ {}", text.state_missing),
            Style::new().fg(theme.missing),
        ));
    }
    if !place.is_empty() {
        lines.push(Line::from(fit(place, width)));
    }
    let mut times = Vec::new();
    if let Some(created) = conversation.created {
        times.push(format!("{} {}", text.started, ago(app, created)));
    }
    times.push(format!(
        "{} {}",
        text.last_active,
        ago(app, conversation.updated)
    ));
    lines.push(Line::styled(
        fmt::truncate(&times.join(" · "), width),
        theme.muted(),
    ));
    if let Some((number, url)) = &conversation.pr {
        lines.push(Line::from(fit(
            vec![
                Span::styled(format!("PR #{number}"), Style::new().fg(theme.accent)),
                Span::styled(
                    format!("  {}", url.trim_start_matches("https://")),
                    theme.muted(),
                ),
            ],
            width,
        )));
    }
    lines
}

/// What was asked in it: the last prompt, or the first when that is all the agent keeps.
fn prompt_lines(app: &App, i: usize, width: usize) -> Option<Vec<Line<'static>>> {
    let text = app.text;
    let conversation = &app.history[i];
    let (heading, prompt) = match (&conversation.last_prompt, &conversation.first_prompt) {
        (Some(prompt), _) => (text.last_prompt, prompt),
        (None, Some(prompt)) => (text.first_prompt, prompt),
        (None, None) => return None,
    };
    let mut lines = vec![Line::default(), title(app, heading, None)];
    lines.extend(fmt::wrap(prompt, width, 4).into_iter().map(Line::raw));
    Some(lines)
}

/// The session that has it open, and what it says it is doing.
fn open_lines(app: &App, i: usize, width: usize) -> Option<Vec<Line<'static>>> {
    let theme = &app.theme;
    let text = app.text;
    let session = app.live(i)?;
    let spin = SPINNER[app.frame % SPINNER.len()];
    let (symbol, color, label) = match session.activity {
        Activity::Blocked => ("◆", theme.blocked, text.state_blocked),
        Activity::Working => (spin, theme.working, text.state_working),
        Activity::Idle => ("●", theme.idle, text.state_idle),
    };
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
    let mut lines = vec![
        Line::default(),
        title(app, text.open_now, None),
        Line::from(fit(
            vec![
                Span::styled(format!("{symbol} {label}"), Style::new().fg(color)),
                Span::styled(format!(" · {}", facts.join(" · ")), theme.muted()),
            ],
            width,
        )),
    ];
    if let Some(detail) = &session.detail {
        for part in fmt::wrap(detail, width.saturating_sub(2), 2) {
            lines.push(Line::styled(format!("  {part}"), theme.muted()));
        }
    }
    Some(lines)
}

/// The command `⏎` copies, and how to use it.
fn command_lines(app: &App, i: usize, width: usize) -> Vec<Line<'static>> {
    if width < 20 {
        return Vec::new();
    }
    let command = history::command(&app.history[i], app.live(i), app.home.as_deref());
    let mut lines: Vec<Line> = fmt::hard_wrap(&format!("$ {command}"), width, 3)
        .into_iter()
        .map(|line| Line::styled(line, Style::new().fg(app.theme.accent)))
        .collect();
    lines.push(Line::styled(
        fmt::truncate(app.text.copy_hint, width),
        app.theme.faint(),
    ));
    lines
}
