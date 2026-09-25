//! Draws the screen.

mod details;
mod dialog;
mod list;

use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::KeyCode;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Widget};
use unicode_width::UnicodeWidthStr;

use crate::app::{App, Item, Mode, State, ToastKind};
use crate::fmt;
use crate::i18n::Keys;
use crate::layout;
use crate::theme::{SPINNER, Theme};

pub fn render(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    let items = app.items();
    let overlay = matches!(app.mode, Mode::Details);
    let screen = layout::split(area, app.show_details && !overlay);
    app.details_room = layout::split(area, true).details.is_some();
    let buf = frame.buffer_mut();

    if let Some(header_area) = screen.header {
        header(buf, header_area, app, &items);
    }
    if items.is_empty() {
        app.hits.clear();
        empty(buf, screen.list, app);
    } else {
        list::render(buf, screen.list, app, &items);
    }
    let selected = app.selected_tree();
    if let (Some(panel), Some((r, w))) = (screen.details, selected) {
        details::render(buf, panel, app, r, w);
    }
    if overlay && let Some((r, w)) = selected {
        let body = Rect {
            y: screen.list.y,
            height: screen.list.height,
            ..area
        };
        Clear.render(body, buf);
        details::render(buf, body, app, r, w);
    }
    let mut buttons = match screen.footer {
        Some(footer_area) => footer(buf, footer_area, app, &items),
        None => Vec::new(),
    };
    match &app.mode {
        Mode::Help => dialog::help(buf, area, app),
        // Over a dialog, only its own buttons answer.
        Mode::Confirm(confirm) => buttons = dialog::confirm(buf, area, app, confirm),
        _ => {}
    }
    app.buttons = buttons;
    toasts(buf, area, screen.footer, app);
}

/// Writes `text` at `(x, y)`, cut to `width` cells, and returns the cells used.
fn put(buf: &mut Buffer, x: u16, y: u16, width: u16, text: &str, style: Style) -> u16 {
    if width == 0 {
        return 0;
    }
    let (end, _) = buf.set_stringn(x, y, text, usize::from(width), style);
    end.saturating_sub(x)
}

/// Writes spans from `(x, y)`, cut to `width` cells, and returns the cells used.
fn put_spans(buf: &mut Buffer, x: u16, y: u16, width: u16, spans: Vec<Span<'_>>) -> u16 {
    if width == 0 {
        return 0;
    }
    let (end, _) = buf.set_line(x, y, &Line::from(spans), width);
    end.saturating_sub(x)
}

/// Writes spans ending at the right edge of `width` cells from `x`.
fn put_right(buf: &mut Buffer, x: u16, y: u16, width: u16, spans: Vec<Span<'_>>) {
    let spans = fit(spans, usize::from(width));
    let used = spans_width(&spans) as u16;
    put_spans(buf, x + width.saturating_sub(used), y, used, spans);
}

fn spans_width(spans: &[Span<'_>]) -> usize {
    spans.iter().map(Span::width).sum()
}

/// Cuts spans to `width` cells, ending with "…" when something was cut.
fn fit(spans: Vec<Span<'_>>, width: usize) -> Vec<Span<'_>> {
    if spans_width(&spans) <= width {
        return spans;
    }
    let mut out = Vec::new();
    let mut used = 0;
    for span in spans {
        let span_width = span.width();
        if used + span_width < width {
            used += span_width;
            out.push(span);
            continue;
        }
        // More text follows this span, so it ends in "…" even when it would just fit.
        let cut = fmt::truncate(&format!("{}  ", span.content), width - used);
        if !cut.is_empty() {
            out.push(Span::styled(cut, span.style));
        }
        break;
    }
    out
}

/// The symbol and color of a worktree's state.
fn state_symbol(app: &App, state: State) -> (&'static str, Color) {
    let theme = &app.theme;
    let spin = SPINNER[app.frame % SPINNER.len()];
    match state {
        State::Removing => (spin, theme.error),
        State::Blocked => ("◆", theme.blocked),
        State::Working => (spin, theme.working),
        State::Idle => ("●", theme.idle),
        State::Busy => ("◎", theme.busy),
        State::Free => ("○", theme.free),
        State::Missing => ("✗", theme.missing),
    }
}

fn state_label(app: &App, state: State) -> &'static str {
    let text = app.text;
    match state {
        State::Removing => text.state_removing,
        State::Blocked => text.state_blocked,
        State::Working => text.state_working,
        State::Idle => text.state_idle,
        State::Busy => text.state_busy,
        State::Free => text.state_free,
        State::Missing => text.state_missing,
    }
}

const EIGHTHS: [&str; 8] = ["", "▏", "▎", "▍", "▌", "▋", "▊", "▉"];

/// A horizontal bar `fraction` full, drawn in eighths of a cell over a faint track.
fn bar(theme: &Theme, fraction: f64, width: u16, color: Color) -> Vec<Span<'static>> {
    let width = usize::from(width);
    if width == 0 {
        return Vec::new();
    }
    let fraction = fraction.clamp(0.0, 1.0);
    let mut eighths = (fraction * (width * 8) as f64).round() as usize;
    if fraction > 0.0 {
        eighths = eighths.max(1);
    }
    let full = eighths / 8;
    let part = EIGHTHS[eighths % 8];
    let filled = format!("{}{part}", "█".repeat(full));
    let used = full + usize::from(!part.is_empty());
    vec![
        Span::styled(filled, Style::new().fg(color)),
        Span::styled("·".repeat(width - used), theme.faint()),
    ]
}

/// A piece of the header: shorter forms come in when the window narrows, and the least
/// important pieces go first.
struct Chip<'a> {
    priority: u8,
    full: Vec<Span<'a>>,
    short: Vec<Span<'a>>,
    right: bool,
}

fn header(buf: &mut Buffer, area: Rect, app: &App, items: &[Item]) {
    let text = app.text;
    let theme = &app.theme;
    let totals = app.totals(items);
    let spin = SPINNER[app.frame % SPINNER.len()];
    let lang = app.lang;
    let logo = if theme.colored() {
        Span::styled(
            " grove ",
            Style::new().fg(Color::Black).bg(theme.accent).bold(),
        )
    } else {
        Span::styled(
            " grove ",
            Style::new().add_modifier(Modifier::REVERSED | Modifier::BOLD),
        )
    };
    let mut chips = vec![Chip {
        priority: 0,
        full: vec![logo.clone()],
        short: vec![logo],
        right: false,
    }];
    if app.found {
        chips.push(Chip {
            priority: 1,
            full: vec![Span::raw(format!("{} worktrees", totals.worktrees))],
            short: vec![Span::raw(format!("{} wt", totals.worktrees))],
            right: false,
        });
    }
    let mut count = |priority, n: usize, symbol: &'static str, color, label: &str| {
        if n > 0 {
            chips.push(Chip {
                priority,
                full: vec![
                    Span::styled(format!("{symbol} {n}"), Style::new().fg(color).bold()),
                    Span::styled(format!(" {label}"), Style::new().fg(color)),
                ],
                short: vec![Span::styled(
                    format!("{symbol}{n}"),
                    Style::new().fg(color).bold(),
                )],
                right: false,
            });
        }
    };
    count(2, totals.blocked, "◆", theme.blocked, text.blocked);
    count(3, totals.working, spin, theme.working, text.working);
    count(5, totals.idle, "●", theme.idle, text.idle_sessions);
    if totals.bytes > 0 {
        chips.push(Chip {
            priority: 4,
            full: vec![Span::styled(
                format!("Σ {}", fmt::size(totals.bytes, lang)),
                Style::new().bold(),
            )],
            short: vec![Span::styled(
                fmt::size_short(totals.bytes, lang),
                Style::new().bold(),
            )],
            right: false,
        });
    }
    if let Some(volume) = app.volume.filter(|volume| volume.total > 0) {
        let used = 1.0 - volume.free as f64 / volume.total as f64;
        let free = fmt::size(volume.free, lang);
        let mut full = vec![Span::styled(format!("{} ", text.disk), theme.muted())];
        full.extend(bar(theme, used, 8, theme.size_color(used)));
        full.push(Span::styled(
            format!(" {free} {}", text.free_space),
            theme.muted(),
        ));
        chips.push(Chip {
            priority: 6,
            full,
            short: vec![Span::styled(
                format!("{free} {}", text.free_space),
                theme.muted(),
            )],
            right: false,
        });
    }
    if !app.removing.is_empty() {
        let label = text
            .removing_count
            .replace("{n}", &app.removing.len().to_string());
        chips.push(Chip {
            priority: 1,
            full: vec![Span::styled(
                format!("{spin} {label}"),
                Style::new().fg(theme.error).bold(),
            )],
            short: vec![Span::styled(
                format!("{spin}{}", app.removing.len()),
                Style::new().fg(theme.error).bold(),
            )],
            right: true,
        });
    }
    if !app.found {
        chips.push(Chip {
            priority: 7,
            full: vec![Span::styled(
                format!("{spin} {}", text.searching),
                theme.muted(),
            )],
            short: vec![Span::styled(spin, theme.muted())],
            right: true,
        });
    } else if totals.measuring > 0 {
        let done = totals.worktrees.saturating_sub(totals.measuring);
        chips.push(Chip {
            priority: 7,
            full: vec![Span::styled(
                format!("{spin} {} {done}/{}", text.measuring, totals.worktrees),
                theme.muted(),
            )],
            short: vec![Span::styled(spin, theme.muted())],
            right: true,
        });
    }

    let width = usize::from(area.width);
    let mut short = vec![false; chips.len()];
    let mut shown = vec![true; chips.len()];
    // Two spaces between chips; the logo has none before it.
    let total = |short: &[bool], shown: &[bool]| -> usize {
        chips
            .iter()
            .enumerate()
            .filter(|(i, _)| shown[*i])
            .map(|(i, chip)| 2 + spans_width(if short[i] { &chip.short } else { &chip.full }))
            .sum::<usize>()
            - 2
    };
    while total(&short, &shown) > width {
        let shorten = (0..chips.len())
            .filter(|&i| {
                shown[i] && !short[i] && spans_width(&chips[i].short) < spans_width(&chips[i].full)
            })
            .max_by_key(|&i| chips[i].priority);
        if let Some(i) = shorten {
            short[i] = true;
            continue;
        }
        match (0..chips.len())
            .filter(|&i| shown[i] && chips[i].priority > 0)
            .max_by_key(|&i| chips[i].priority)
        {
            Some(i) => shown[i] = false,
            None => break,
        }
    }
    let mut left: Vec<Span> = Vec::new();
    let mut right: Vec<Span> = Vec::new();
    for (i, chip) in chips.into_iter().enumerate() {
        if !shown[i] {
            continue;
        }
        let spans = if short[i] { chip.short } else { chip.full };
        let side = if chip.right { &mut right } else { &mut left };
        if !side.is_empty() || chip.right {
            side.push(Span::raw("  "));
        }
        side.extend(spans);
    }
    put_spans(buf, area.x, area.y, area.width, fit(left, width));
    put_right(buf, area.x, area.y, area.width, right);
}

/// The key hints, or the filter being typed; returns where the hints are, to click them.
fn footer(buf: &mut Buffer, area: Rect, app: &App, items: &[Item]) -> Vec<(Rect, KeyCode)> {
    let text = app.text;
    let theme = &app.theme;
    let key_style = Style::new().fg(theme.accent).bold();
    if matches!(app.mode, Mode::Filter) {
        let matched = items
            .iter()
            .filter(|item| matches!(item, Item::Tree(..)))
            .count();
        let mut spans = vec![
            Span::styled(" / ", key_style),
            Span::raw(app.filter.clone()),
            Span::styled("▏", Style::new().fg(theme.accent)),
            Span::styled(
                format!(
                    "  {}",
                    text.matches
                        .replace("{n}", &matched.to_string())
                        .replace("{m}", &app.totals(&app.items()).worktrees.to_string())
                ),
                theme.muted(),
            ),
        ];
        spans.push(Span::raw("  "));
        let used = spans_width(&spans);
        let (hints, _) = key_hints(
            app,
            text.filter_keys,
            usize::from(area.width).saturating_sub(used),
        );
        spans.extend(hints);
        put_spans(
            buf,
            area.x,
            area.y,
            area.width,
            fit(spans, usize::from(area.width)),
        );
        return Vec::new();
    }
    let keys = match app.mode {
        Mode::Details => text.details_keys,
        _ => text.list_keys,
    };
    let mut right: Vec<Span> = Vec::new();
    if !app.filter.is_empty() {
        right.push(Span::styled(format!("{}: ", text.filter), theme.muted()));
        right.push(Span::styled(
            format!("{} ", app.filter),
            Style::new().fg(theme.accent).bold(),
        ));
    }
    let room = usize::from(area.width).saturating_sub(spans_width(&right));
    let (hints, places) = key_hints(app, keys, room);
    put_spans(buf, area.x, area.y, area.width, hints);
    put_right(buf, area.x, area.y, area.width, right);
    places
        .into_iter()
        .filter_map(|(x, width, key)| {
            let code = match key {
                "⏎" => KeyCode::Enter,
                "Esc" => KeyCode::Esc,
                key if key.chars().count() == 1 => KeyCode::Char(key.chars().next()?),
                _ => return None,
            };
            Some((Rect::new(area.x + x, area.y, width, 1), code))
        })
        .collect()
}

/// " key action" pairs, as many as fit in `width`, and where each one is.
fn key_hints<'a>(
    app: &App,
    keys: Keys,
    width: usize,
) -> (Vec<Span<'a>>, Vec<(u16, u16, &'static str)>) {
    let theme = &app.theme;
    let mut spans = Vec::new();
    let mut places = Vec::new();
    let mut used = 0;
    for (key, action) in keys {
        let action = if *key == "s" {
            format!("{action}: {}", app.text.sorts[app.sort.index()])
        } else {
            action.to_string()
        };
        let item = key.width() + action.width() + 3;
        if used + item > width {
            break;
        }
        places.push((used as u16, item as u16, *key));
        used += item;
        spans.push(Span::styled(
            format!(" {key}"),
            Style::new().fg(theme.accent).bold(),
        ));
        spans.push(Span::styled(format!(" {action} "), theme.muted()));
    }
    (spans, places)
}

fn empty(buf: &mut Buffer, area: Rect, app: &App) {
    let text = app.text;
    let theme = &app.theme;
    let spin = SPINNER[app.frame % SPINNER.len()];
    let lines: Vec<Line> = if !app.found {
        vec![Line::from(Span::styled(
            format!("{spin} {}", text.searching),
            theme.muted(),
        ))]
    } else if !app.filter.is_empty() {
        vec![Line::from(vec![
            Span::styled(format!("{} ", text.no_match), theme.muted()),
            Span::styled(format!("“{}”", app.filter), Style::new().fg(theme.accent)),
        ])]
    } else {
        let roots: Vec<String> = app
            .roots
            .iter()
            .map(|root| fmt::tilde(root, app.home.as_deref()))
            .collect();
        let hint = text.none_hint.replace("{roots}", &roots.join(", "));
        let mut lines = vec![
            Line::from(Span::styled(text.none_found, Style::new().bold())),
            Line::default(),
        ];
        lines.extend(
            fmt::wrap(&hint, usize::from(area.width.saturating_sub(4)), 4)
                .into_iter()
                .map(|line| Line::from(Span::styled(line, theme.muted()))),
        );
        lines
    };
    let top = area.y + area.height.saturating_sub(lines.len() as u16) / 2;
    for (i, line) in lines.into_iter().enumerate() {
        let y = top + i as u16;
        if y >= area.bottom() {
            break;
        }
        let line = line.centered();
        line.render(
            Rect {
                y,
                height: 1,
                ..area
            },
            buf,
        );
    }
}

fn toasts(buf: &mut Buffer, area: Rect, footer: Option<Rect>, app: &App) {
    let theme = &app.theme;
    let mut bottom = footer.map_or(area.bottom(), |footer| footer.y);
    for toast in app.toasts.iter().rev() {
        if bottom < area.y + 3 || area.width < 12 {
            break;
        }
        let (symbol, color) = match toast.kind {
            ToastKind::Ok => ("✓", theme.ok),
            ToastKind::Error => ("✗", theme.error),
            ToastKind::Info => ("·", theme.accent),
        };
        let message = format!("{symbol} {}", toast.text);
        let width = (message.width() as u16 + 4).min(area.width);
        let rect = Rect {
            x: area.right() - width,
            y: bottom - 3,
            width,
            height: 3,
        };
        Clear.render(rect, buf);
        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(Style::new().fg(color));
        let inner = block.inner(rect);
        block.render(rect, buf);
        put_spans(
            buf,
            inner.x + 1,
            inner.y,
            inner.width.saturating_sub(1),
            fit(
                vec![
                    Span::styled(format!("{symbol} "), Style::new().fg(color).bold()),
                    Span::raw(toast.text.clone()),
                ],
                usize::from(inner.width.saturating_sub(2)),
            ),
        );
        bottom -= 3;
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::*;
    use crate::demo;

    /// Draws the app on a `width`×`height` terminal and returns the screen as text.
    pub(crate) fn draw(app: &mut App, width: u16, height: u16) -> Buffer {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| render(frame, app)).unwrap();
        terminal.backend().buffer().clone()
    }

    pub(crate) fn text(buf: &Buffer) -> String {
        let mut out = String::new();
        for y in 0..buf.area.height {
            let mut line = String::new();
            for x in 0..buf.area.width {
                line.push_str(buf[(x, y)].symbol());
            }
            out.push_str(line.trim_end());
            out.push('\n');
        }
        out
    }

    #[test]
    fn draws_at_every_size_without_panicking() {
        let mut app = demo::app();
        for width in (0..=220).step_by(3) {
            for height in (0..=70).step_by(2) {
                draw(&mut app, width, height);
            }
        }
        for mode in 0..4 {
            let mut app = demo::app();
            app.move_by(2);
            app.mode = match mode {
                0 => Mode::Help,
                1 => Mode::Details,
                2 => Mode::Filter,
                _ => Mode::Confirm(crate::app::Confirm {
                    targets: app.idle_worktrees(),
                    idle: true,
                }),
            };
            app.toast(
                ToastKind::Ok,
                "checkout-coupons removed · 1.9 GB freed".into(),
            );
            for width in (0..=160).step_by(7) {
                for height in (0..=50).step_by(5) {
                    draw(&mut app, width, height);
                }
            }
        }
    }

    #[test]
    fn a_wide_window_shows_every_column_and_the_details() {
        let mut app = demo::app();
        app.move_by(3);
        let screen = text(&draw(&mut app, 240, 45));
        assert!(screen.contains(" grove "), "{screen}");
        for expected in [
            "worktrees",
            "order-pipeline",
            "⎇ feat/design-system",
            "pipeline optimization",
            "12.3 GB",
            "SESSIONS",
            "SPACE",
            "git -C ~/Workspace/board worktree remove --force",
        ] {
            assert!(
                screen.contains(expected),
                "missing {expected:?} in\n{screen}"
            );
        }
    }

    #[test]
    fn a_narrow_pane_keeps_names_states_and_sizes() {
        let mut app = demo::app();
        let screen = text(&draw(&mut app, 40, 30));
        assert!(screen.contains("checkout-coupons"), "{screen}");
        assert!(screen.contains("GB"), "{screen}");
        assert!(
            !screen.contains("SESSIONS"),
            "no room for details:\n{screen}"
        );
        let tiny = text(&draw(&mut app, 24, 6));
        assert!(tiny.contains("board"), "{tiny}");
    }
}
