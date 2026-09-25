//! Splits the window between the header, the list, the details and the key hints, and picks
//! how much each list row shows, so the screen works from a full monitor down to a sliver of
//! a tiling window manager.

use ratatui::layout::Rect;

#[derive(Debug, PartialEq, Eq)]
pub struct Screen {
    pub header: Option<Rect>,
    pub list: Rect,
    pub details: Option<Rect>,
    pub footer: Option<Rect>,
}

/// Narrowest details panel beside the list, and the list it leaves.
const SIDE_DETAILS: (u16, u16) = (46, 72);
const SIDE_MIN_LIST: u16 = 70;
/// Shortest details panel under the list, and the list it leaves.
const BOTTOM_DETAILS: (u16, u16) = (12, 18);
const BOTTOM_MIN_LIST: u16 = 12;

pub fn split(area: Rect, details: bool) -> Screen {
    let header = (area.height >= 3).then_some(Rect { height: 1, ..area });
    let footer = (area.height >= 8 && area.width >= 20).then(|| Rect {
        y: area.bottom() - 1,
        height: 1,
        ..area
    });
    // A blank row under the header when the window is tall enough to spare it.
    let top = match header {
        Some(_) if area.height >= 24 => 2,
        Some(_) => 1,
        None => 0,
    };
    let body = Rect {
        y: area.y + top,
        height: area.height - top - u16::from(footer.is_some()),
        ..area
    };
    let (list, details) = if !details {
        (body, None)
    } else if body.width >= SIDE_DETAILS.0 + 1 + SIDE_MIN_LIST && body.height >= BOTTOM_DETAILS.0 {
        let width = (body.width * 2 / 5).clamp(SIDE_DETAILS.0, SIDE_DETAILS.1);
        (
            Rect {
                width: body.width - width - 1,
                ..body
            },
            Some(Rect {
                x: body.right() - width,
                width,
                ..body
            }),
        )
    } else if body.height >= BOTTOM_DETAILS.0 + BOTTOM_MIN_LIST + 6 && body.width >= 44 {
        let height = (body.height * 2 / 5).clamp(BOTTOM_DETAILS.0, BOTTOM_DETAILS.1);
        (
            Rect {
                height: body.height - height,
                ..body
            },
            Some(Rect {
                y: body.bottom() - height,
                height,
                ..body
            }),
        )
    } else {
        (body, None)
    };
    Screen {
        header,
        list,
        details,
        footer,
    }
}

/// How much a list row shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Density {
    /// One line per worktree, with as many columns as fit.
    Table(Columns),
    /// Two lines per worktree: name and size, then sessions and a size bar.
    Cards,
    /// One line per worktree: name and size.
    Minimal,
}

/// `longest_name` is the width of the longest worktree name on the list, tags included.
pub fn density(list: Rect, longest_name: u16) -> Density {
    if list.width >= 64 {
        Density::Table(columns(list.width, longest_name))
    } else if list.width >= 34 && list.height >= 10 {
        Density::Cards
    } else {
        Density::Minimal
    }
}

/// Widths of the table columns; zero means the column is left out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Columns {
    pub name: u16,
    pub branch: u16,
    pub git: u16,
    pub agents: u16,
    pub size: u16,
    pub bar: u16,
    pub age: u16,
}

/// Cells before the name (selection mark, state symbol and a space) and after the last column
/// (the scrollbar).
pub const LEAD: u16 = 3;
pub const TRAIL: u16 = 1;
pub const SIZE: u16 = 8;
const NAME_MIN: u16 = 16;
/// The sessions column never gets less than this.
#[cfg(test)]
const AGENTS_MIN: u16 = 16;
/// The sessions column keeps this much before a branch column is added.
const AGENTS_BESIDE_BRANCH: u16 = 24;
const BRANCH_RANGE: (u16, u16) = (12, 36);

/// Names get the room the longest one needs, up to a share of the row; the branches and the
/// sessions split what is left, the sessions taking the most.
pub fn columns(width: u16, longest_name: u16) -> Columns {
    // What a narrower row gives up first: the branch (the details show it), then the last
    // use, then the git state.
    let (bar, git, age, with_branch) = match width {
        140.. => (14, 9, 5, true),
        110.. => (10, 9, 5, true),
        88.. => (8, 9, 0, false),
        _ => (6, 0, 0, false),
    };
    let optional = |width: u16| if width > 0 { width + 1 } else { 0 };
    let fixed = LEAD + 2 + (1 + SIZE) + (1 + bar) + optional(git) + optional(age) + TRAIL;
    let flex = width.saturating_sub(fixed);
    let split = |share: u16| {
        let name = longest_name
            .max(NAME_MIN)
            .min(share.max(NAME_MIN))
            .min(flex);
        (name, flex - name)
    };
    let (mut name, mut rest) = split(flex * 9 / 20);
    let mut branch = 0;
    if with_branch {
        branch = (rest / 3)
            .clamp(BRANCH_RANGE.0, BRANCH_RANGE.1)
            .min(rest.saturating_sub(AGENTS_BESIDE_BRANCH + 1));
        // A branch column too narrow to read gives its room back to the names.
        if branch < BRANCH_RANGE.0 + 6 {
            branch = 0;
            (name, rest) = split(flex * 3 / 5);
        }
    } else {
        (name, rest) = split(flex * 3 / 5);
    }
    let agents = rest - optional(branch);
    Columns {
        name,
        branch,
        git,
        agents,
        size: SIZE,
        bar,
        age,
    }
}

#[cfg(test)]
impl Columns {
    /// Total width, which is the width the columns were made for.
    pub fn width(&self) -> u16 {
        let optional = |width: u16| if width > 0 { width + 1 } else { 0 };
        LEAD + self.name
            + optional(self.branch)
            + optional(self.git)
            + (2 + self.agents)
            + (1 + self.size)
            + (1 + self.bar)
            + optional(self.age)
            + TRAIL
    }
}

/// A box of at most `width`×`height` centered in `area`.
pub fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + (area.height - height) / 2,
        width,
        height,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inside(inner: Rect, outer: Rect) -> bool {
        inner.is_empty() || outer.union(inner) == outer
    }

    #[test]
    fn every_window_size_gets_a_consistent_layout() {
        for width in 0..=260 {
            for height in (0..=90).step_by(3) {
                let area = Rect::new(3, 2, width, height);
                for details in [false, true] {
                    let screen = split(area, details);
                    let parts: Vec<Rect> = [
                        screen.header,
                        Some(screen.list),
                        screen.details,
                        screen.footer,
                    ]
                    .into_iter()
                    .flatten()
                    .collect();
                    for (i, part) in parts.iter().enumerate() {
                        assert!(inside(*part, area), "{part:?} outside {area:?}");
                        for other in &parts[i + 1..] {
                            assert!(!part.intersects(*other), "{part:?} overlaps {other:?}");
                        }
                    }
                    if let Some(panel) = screen.details {
                        assert!(panel.width >= 44 && panel.height >= BOTTOM_DETAILS.0);
                        assert!(screen.list.width >= 44 && screen.list.height >= 10);
                    }
                }
            }
        }
    }

    #[test]
    fn table_columns_fill_the_row_exactly() {
        for width in 64..=400 {
            for longest in [0, 10, 24, 36, 80, 500] {
                let columns = columns(width, longest);
                assert_eq!(columns.width(), width, "{columns:?}");
                assert!(columns.name >= NAME_MIN, "{width}: {columns:?}");
                assert!(columns.agents >= AGENTS_MIN, "{width}: {columns:?}");
            }
        }
    }

    #[test]
    fn names_take_what_they_need_and_sessions_the_rest() {
        let short = columns(127, 20);
        let long = columns(127, 36);
        assert_eq!(long.name, 36);
        assert_eq!(short.name, 20);
        // The room short names leave brings the branch column in.
        assert!(short.branch >= 18 && long.branch == 0, "{short:?} {long:?}");
        assert!(long.agents >= 48, "{long:?}");
        assert!(
            columns(92, 90).name < 50,
            "a long name does not take the whole row"
        );
        for width in 64..=400 {
            let branch = columns(width, 30).branch;
            assert!(
                branch == 0 || branch >= 18,
                "{width}: a {branch}-cell branch is unreadable"
            );
        }
    }

    #[test]
    fn rows_show_less_as_the_list_narrows() {
        let at = |width, height| density(Rect::new(0, 0, width, height), 30);
        assert!(matches!(at(160, 40), Density::Table(c) if c.branch >= 18 && c.age > 0));
        assert!(
            matches!(at(120, 40), Density::Table(c) if c.branch == 0 && c.age > 0 && c.git > 0)
        );
        assert!(
            matches!(at(96, 40), Density::Table(c) if c.branch == 0 && c.age == 0 && c.git > 0 && c.name == 30)
        );
        assert!(matches!(at(70, 40), Density::Table(c) if c.branch == 0));
        assert_eq!(at(50, 40), Density::Cards);
        assert_eq!(at(50, 8), Density::Minimal);
        assert_eq!(at(30, 40), Density::Minimal);
    }

    #[test]
    fn puts_the_details_beside_under_or_nowhere() {
        let beside = split(Rect::new(0, 0, 200, 50), true);
        let panel = beside.details.unwrap();
        assert_eq!(panel.y, beside.list.y);
        assert!(panel.x > beside.list.right());
        assert_eq!(panel.width, 72);

        let under = split(Rect::new(0, 0, 90, 50), true);
        let panel = under.details.unwrap();
        assert_eq!((panel.x, panel.width), (0, 90));
        assert_eq!(panel.y, under.list.bottom());

        assert!(split(Rect::new(0, 0, 90, 24), true).details.is_none());
        assert!(split(Rect::new(0, 0, 40, 60), true).details.is_none());
        assert!(split(Rect::new(0, 0, 200, 50), false).details.is_none());
    }

    #[test]
    fn keeps_the_list_in_tiny_windows() {
        let screen = split(Rect::new(0, 0, 18, 2), true);
        assert_eq!(screen.header, None);
        assert_eq!(screen.footer, None);
        assert_eq!(screen.list, Rect::new(0, 0, 18, 2));
        let screen = split(Rect::new(0, 0, 80, 24), true);
        assert_eq!(screen.header, Some(Rect::new(0, 0, 80, 1)));
        assert_eq!(screen.list.y, 2, "a blank row under the header");
        assert_eq!(screen.footer, Some(Rect::new(0, 23, 80, 1)));
    }
}
