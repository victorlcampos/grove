//! The routines Claude Desktop runs on a schedule, the ones waiting for you first, in the same
//! three densities as the other lists, and the details of one: its schedule, how its last run
//! went, what it asks, and the command that resumes that run.

use std::time::SystemTime;

use chrono::{DateTime, Local};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};

use super::details::{ago, column, marked_panel, title};
use super::list;
use super::{centered, fit, put, put_right, put_spans};
use crate::app::{App, Due};
use crate::cron;
use crate::fmt;
use crate::layout::{self, Density, LEAD, TRAIL};
use crate::routines;
use crate::theme::SPINNER;

/// Cells for when it runs next: "qui 09:05".
const NEXT: u16 = 10;
const SCHEDULE: u16 = 20;
const NAME_RANGE: (u16, u16) = (16, 36);

/// The symbol and color of where a routine stands.
pub fn symbol(app: &App, due: Due) -> (&'static str, Color) {
    let theme = &app.theme;
    match due {
        Due::Waiting => ("◆", theme.blocked),
        Due::Running => (SPINNER[app.frame % SPINNER.len()], theme.working),
        Due::Failed => ("✗", theme.error),
        Due::Late => ("!", theme.warn),
        Due::Done => ("✓", theme.ok),
        Due::Never => ("○", theme.free),
        Due::Paused => ("‖", theme.free),
    }
}

pub fn label(app: &App, due: Due) -> &'static str {
    let text = app.text;
    match due {
        Due::Waiting => text.state_blocked,
        Due::Running => text.due_running,
        Due::Failed => text.due_failed,
        Due::Late => text.due_late,
        Due::Done => text.due_done,
        Due::Never => text.due_never,
        Due::Paused => text.due_paused,
    }
}

fn lead(buf: &mut Buffer, x: u16, y: u16, app: &App, due: Due, selected: bool) {
    let (symbol, color) = symbol(app, due);
    if selected {
        put(buf, x, y, 1, "▍", Style::new().fg(app.theme.accent));
    }
    put(buf, x + 1, y, 1, symbol, Style::new().fg(color));
}

pub fn render(buf: &mut Buffer, area: Rect, app: &mut App, rows: &[usize]) {
    app.hits.clear();
    if area.width < 6 || area.height == 0 {
        return;
    }
    let now = Local::now();
    let density = layout::density(area, 0);
    let row_height: u16 = if density == Density::Cards { 2 } else { 1 };
    let headings = vec![false; rows.len()];
    let heights = list::heights(&headings, row_height, area.height);
    app.page = usize::from((area.height / row_height).max(1));
    let selected = app.picked_index(rows);
    app.routines_offset = list::scroll(
        app.routines_offset,
        selected,
        &heights,
        &headings,
        area.height,
    );
    let mut y = area.y;
    let mut hits = Vec::new();
    for (index, &i) in rows.iter().enumerate().skip(app.routines_offset) {
        if y + row_height > area.bottom() {
            break;
        }
        let rect = Rect {
            x: area.x,
            y,
            width: area.width - TRAIL,
            height: row_height,
        };
        let is_selected = selected == Some(index);
        if is_selected {
            buf.set_style(rect, app.theme.selected());
        }
        match density {
            Density::Table(_) => table_row(buf, rect, app, i, is_selected, now),
            Density::Cards => card_row(buf, rect, app, i, is_selected, now),
            Density::Minimal => minimal_row(buf, rect, app, i, is_selected, now),
        }
        hits.push((rect, index));
        y += row_height;
    }
    app.hits = hits;
    list::scrollbar(buf, area, app, &heights, app.routines_offset);
}

/// What the list says when there is nothing to show.
pub fn empty(buf: &mut Buffer, area: Rect, app: &App) {
    let text = app.text;
    let theme = &app.theme;
    let lines = if !app.routines_found {
        let spin = SPINNER[app.frame % SPINNER.len()];
        vec![Line::from(Span::styled(
            format!("{spin} {}", text.searching_routines),
            theme.muted(),
        ))]
    } else if !app.filter.is_empty() {
        vec![Line::from(vec![
            Span::styled(format!("{} ", text.no_match), theme.muted()),
            Span::styled(format!("“{}”", app.filter), Style::new().fg(theme.accent)),
        ])]
    } else {
        let dirs: Vec<String> = routines::roots(app.home.as_deref())
            .iter()
            .map(|dir| fmt::tilde(dir, app.home.as_deref()))
            .collect();
        let hint = text.no_routines_hint.replace("{dir}", &dirs.join(", "));
        let mut lines = vec![
            Line::from(Span::styled(text.no_routines, Style::new().bold())),
            Line::default(),
        ];
        lines.extend(
            fmt::wrap(&hint, usize::from(area.width.saturating_sub(4)), 4)
                .into_iter()
                .map(|line| Line::from(Span::styled(line, theme.muted()))),
        );
        lines
    };
    centered(buf, area, lines);
}

fn table_row(
    buf: &mut Buffer,
    row: Rect,
    app: &App,
    i: usize,
    selected: bool,
    now: DateTime<Local>,
) {
    let due = app.routine_state(i, now);
    let y = row.y;
    lead(buf, row.x, y, app, due, selected);
    let schedule = if row.width >= 80 { SCHEDULE } else { 0 };
    let flex = row
        .width
        .saturating_sub(LEAD + NEXT + 1 + schedule + u16::from(schedule > 0) * 2);
    let (name, said) = if row.width >= 110 {
        let name = (flex * 2 / 5).clamp(NAME_RANGE.0, NAME_RANGE.1);
        (name, flex.saturating_sub(name + 2))
    } else {
        (flex, 0)
    };
    let mut x = row.x + LEAD;
    put_spans(
        buf,
        x,
        y,
        name,
        fit(name_spans(app, i, due), usize::from(name)),
    );
    x += name;
    if said > 0 {
        x += 2;
        put_spans(
            buf,
            x,
            y,
            said,
            fit(said_spans(app, i, due), usize::from(said)),
        );
        x += said;
    }
    if schedule > 0 {
        x += 2;
        put(
            buf,
            x,
            y,
            schedule,
            &schedule_text(app, i),
            app.theme.muted(),
        );
        x += schedule;
    }
    put_right(buf, x + 1, y, NEXT, next_spans(app, i, now));
}

/// Two lines: the name and when it runs next, then its schedule and what its last run says.
fn card_row(
    buf: &mut Buffer,
    row: Rect,
    app: &App,
    i: usize,
    selected: bool,
    now: DateTime<Local>,
) {
    let due = app.routine_state(i, now);
    lead(buf, row.x, row.y, app, due, selected);
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
    let x = row.x + LEAD;
    let name = row.width.saturating_sub(LEAD + 1 + NEXT);
    put_spans(
        buf,
        x,
        row.y,
        name,
        fit(name_spans(app, i, due), usize::from(name)),
    );
    put_right(buf, x + name + 1, row.y, NEXT, next_spans(app, i, now));
    let mut second = vec![Span::styled(schedule_text(app, i), app.theme.muted())];
    let said = said_spans(app, i, due);
    if !said.is_empty() {
        second.push(Span::raw("  "));
        second.extend(said);
    }
    let width = row.width.saturating_sub(LEAD);
    put_spans(buf, x, row.y + 1, width, fit(second, usize::from(width)));
}

fn minimal_row(
    buf: &mut Buffer,
    row: Rect,
    app: &App,
    i: usize,
    selected: bool,
    now: DateTime<Local>,
) {
    let due = app.routine_state(i, now);
    lead(buf, row.x, row.y, app, due, selected);
    let next = if row.width >= LEAD + 20 { NEXT } else { 0 };
    let name = row.width.saturating_sub(LEAD + next + u16::from(next > 0));
    put_spans(
        buf,
        row.x + LEAD,
        row.y,
        name,
        fit(name_spans(app, i, due), usize::from(name)),
    );
    if next > 0 {
        put_right(
            buf,
            row.x + LEAD + name + 1,
            row.y,
            next,
            next_spans(app, i, now),
        );
    }
}

fn name_spans(app: &App, i: usize, due: Due) -> Vec<Span<'static>> {
    let style = match due {
        Due::Waiting | Due::Running | Due::Failed | Due::Late => Style::new().bold(),
        Due::Paused => app.theme.muted(),
        _ => Style::new(),
    };
    vec![Span::styled(app.routines[i].name.clone(), style)]
}

/// What its last run asks of you, else what Desktop says that run did.
fn said_spans(app: &App, i: usize, due: Due) -> Vec<Span<'static>> {
    let Some(run) = &app.routines[i].run else {
        return Vec::new();
    };
    match (&run.needs, &run.detail, &run.error) {
        (Some(needs), _, _) if due == Due::Waiting => {
            vec![Span::styled(
                needs.clone(),
                Style::new().fg(app.theme.blocked),
            )]
        }
        (_, _, Some(error)) if due == Due::Failed => {
            vec![Span::styled(
                error.clone(),
                Style::new().fg(app.theme.error),
            )]
        }
        (_, Some(detail), _) => vec![Span::styled(detail.clone(), app.theme.muted())],
        _ => Vec::new(),
    }
}

/// Its schedule in words when it is a common one, else as written.
fn schedule_text(app: &App, i: usize) -> String {
    let routine = &app.routines[i];
    routine
        .schedule
        .as_ref()
        .and_then(|cron| cron.describe(app.lang))
        .unwrap_or_else(|| routine.cron.clone())
}

fn next_spans(app: &App, i: usize, now: DateTime<Local>) -> Vec<Span<'static>> {
    match app.next_run(i, now) {
        Some(next) => vec![Span::styled(
            cron::when(next, now, app.lang),
            app.theme.muted(),
        )],
        None => vec![Span::styled("—", app.theme.faint())],
    }
}

fn local(time: SystemTime) -> DateTime<Local> {
    DateTime::<Local>::from(time)
}

/// The details of a routine: its schedule, how its last run went and what it asks, where it
/// runs, and the command that resumes that run.
pub fn details(buf: &mut Buffer, area: Rect, app: &App, i: usize) {
    if area.width < 12 || area.height < 3 {
        return;
    }
    let now = Local::now();
    let due = app.routine_state(i, now);
    let routine = &app.routines[i];
    let inner = marked_panel(
        buf,
        area,
        app,
        &routine.name,
        symbol(app, due),
        label(app, due),
    );
    if inner.is_empty() {
        return;
    }
    let width = usize::from(inner.width);
    let command = command_lines(app, i, width);
    let room = usize::from(inner.height).saturating_sub(command.len() + 1);
    let mut lines = about_lines(app, i, width);
    for section in [
        schedule_lines(app, i, width, now),
        run_lines(app, i, due, width),
    ] {
        let left = room.saturating_sub(lines.len());
        if section.len() <= left {
            lines.extend(section);
        } else {
            if left >= 3 {
                lines.extend(section.into_iter().take(left));
            }
            break;
        }
    }
    column(buf, inner, lines, command);
}

/// Its id, what its prompt says it does, and where it runs.
fn about_lines(app: &App, i: usize, width: usize) -> Vec<Line<'static>> {
    let theme = &app.theme;
    let routine = &app.routines[i];
    let mut lines = vec![Line::styled(
        fmt::truncate(&routine.id, width),
        theme.muted(),
    )];
    if let Some(description) = &routine.description {
        lines.extend(fmt::wrap(description, width, 3).into_iter().map(Line::raw));
    }
    if let Some(cwd) = &routine.cwd {
        let path = fmt::tilde(cwd, app.home.as_deref());
        lines.push(Line::styled(
            fmt::truncate_middle(&path, width),
            theme.muted(),
        ));
    }
    lines
}

/// When it runs, when it runs next, and when it last was due and ran.
fn schedule_lines(app: &App, i: usize, width: usize, now: DateTime<Local>) -> Vec<Line<'static>> {
    let theme = &app.theme;
    let text = app.text;
    let routine = &app.routines[i];
    let mut lines = vec![
        Line::default(),
        title(app, text.schedule_title, Some(routine.cron.clone())),
    ];
    let words = schedule_text(app, i);
    let mut first = vec![Span::raw(words)];
    if let Some(next) = app.next_run(i, now) {
        let seconds = next.signed_duration_since(now).num_seconds().max(0) as u64;
        first.push(Span::styled(
            format!(
                "  · {} {} · {}",
                text.next_run,
                cron::when(next, now, app.lang),
                text.in_time
                    .replace("{t}", &fmt::ago_short(seconds, app.lang)),
            ),
            theme.muted(),
        ));
    }
    lines.push(Line::from(fit(first, width)));
    let mut last = Vec::new();
    if let Some(due) = routine.last_due {
        last.push(format!(
            "{} {}",
            text.last_due,
            cron::when(local(due), now, app.lang)
        ));
    }
    if let Some(ran) = routine.last_run {
        last.push(format!(
            "{} {}",
            text.ran_at,
            cron::when(local(ran), now, app.lang)
        ));
    }
    if !last.is_empty() {
        lines.push(Line::styled(
            fmt::truncate(&last.join(" · "), width),
            theme.muted(),
        ));
    }
    if let Some(missed) = app.missed(i, now) {
        let note = text
            .late_note
            .replace("{time}", &cron::when(missed, now, app.lang));
        lines.extend(
            fmt::wrap(&note, width, 2)
                .into_iter()
                .map(|line| Line::styled(line, Style::new().fg(theme.warn))),
        );
    }
    lines
}

/// How its last run went, and what it asks of you.
fn run_lines(app: &App, i: usize, due: Due, width: usize) -> Vec<Line<'static>> {
    let theme = &app.theme;
    let text = app.text;
    let Some(run) = &app.routines[i].run else {
        return vec![
            Line::default(),
            title(app, text.last_run_title, None),
            Line::styled(text.never_ran, theme.muted()),
        ];
    };
    let mut lines = vec![
        Line::default(),
        title(
            app,
            text.last_run_title,
            run.started.map(|started| ago(app, started)),
        ),
    ];
    if let Some(title) = &run.title {
        lines.push(Line::raw(fmt::truncate(title, width)));
    }
    if run.answered {
        lines.push(Line::styled(
            fmt::truncate(app.text.answered_note, width),
            theme.muted(),
        ));
    }
    if let Some(error) = &run.error {
        for part in fmt::wrap(error, width, 3) {
            lines.push(Line::styled(part, Style::new().fg(theme.error)));
        }
    }
    match &run.needs {
        Some(needs) if due == Due::Waiting => {
            lines.push(Line::default());
            lines.push(title(app, text.asks_title, None));
            for part in fmt::wrap(needs, width, 6) {
                lines.push(Line::styled(part, Style::new().fg(theme.blocked)));
            }
        }
        _ => {
            if let Some(detail) = &run.detail {
                for part in fmt::wrap(detail, width, 4) {
                    lines.push(Line::styled(part, theme.muted()));
                }
            }
        }
    }
    lines
}

/// The command `⏎` copies, and how to use it.
fn command_lines(app: &App, i: usize, width: usize) -> Vec<Line<'static>> {
    if width < 20 {
        return Vec::new();
    }
    let Some(command) = routines::command(&app.routines[i], app.home.as_deref()) else {
        return Vec::new();
    };
    let mut lines: Vec<Line> = fmt::hard_wrap(&format!("$ {command}"), width, 3)
        .into_iter()
        .map(|line| Line::styled(line, Style::new().fg(app.theme.accent)))
        .collect();
    lines.push(Line::styled(
        fmt::truncate(app.text.routine_copy_hint, width),
        app.theme.faint(),
    ));
    lines
}
