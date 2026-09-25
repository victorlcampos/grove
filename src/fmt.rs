//! Sizes, counts, times and paths the way the interface shows them.

use std::path::Path;

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::i18n::Lang;

const UNITS: [&str; 6] = ["B", "kB", "MB", "GB", "TB", "PB"];
const SHORT_UNITS: [&str; 6] = ["B", "K", "M", "G", "T", "P"];

/// A disk size in powers of 1000, like macOS Finder shows it: "1.9 GB", "812 MB".
pub fn size(bytes: u64, lang: Lang) -> String {
    let (number, unit) = scaled(bytes, 99.95, lang);
    format!("{number} {}", UNITS[unit])
}

/// The same size in at most four cells: "1.9G", "812M".
pub fn size_short(bytes: u64, lang: Lang) -> String {
    let (number, unit) = scaled(bytes, 9.95, lang);
    format!("{number}{}", SHORT_UNITS[unit])
}

/// Divides by 1000 until the number fits three digits, with a decimal below `decimals_below`.
fn scaled(bytes: u64, decimals_below: f64, lang: Lang) -> (String, usize) {
    if bytes < 1000 {
        return (bytes.to_string(), 0);
    }
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 999.5 && unit < UNITS.len() - 1 {
        value /= 1000.0;
        unit += 1;
    }
    let number = if value < decimals_below {
        format!("{value:.1}")
    } else {
        format!("{value:.0}")
    };
    (localize_decimal(number, lang), unit)
}

fn localize_decimal(number: String, lang: Lang) -> String {
    match lang {
        Lang::En => number,
        Lang::Pt => number.replace('.', ","),
    }
}

/// A count with thousands separators: "212,340" or "212.340".
pub fn count(n: u64, lang: Lang) -> String {
    let digits = n.to_string();
    let separator = match lang {
        Lang::En => ',',
        Lang::Pt => '.',
    };
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, digit) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(separator);
        }
        out.push(digit);
    }
    out
}

/// Time since something, in a few cells: "now", "5m", "2h", "3d".
pub fn ago_short(seconds: u64, lang: Lang) -> String {
    match seconds {
        0..60 => match lang {
            Lang::En => "now".into(),
            Lang::Pt => "agora".into(),
        },
        60..3600 => format!("{}m", seconds / 60),
        3600..86_400 => format!("{}h", seconds / 3600),
        _ => format!("{}d", seconds / 86_400),
    }
}

/// Time since something, as words: "5 min ago", "há 3 dias".
pub fn ago(seconds: u64, lang: Lang) -> String {
    let (n, unit) = match seconds {
        0..60 => {
            return match lang {
                Lang::En => "just now".into(),
                Lang::Pt => "agora".into(),
            };
        }
        60..3600 => (seconds / 60, "min"),
        3600..86_400 => (seconds / 3600, "h"),
        _ => {
            let days = seconds / 86_400;
            let unit = match (lang, days) {
                (Lang::En, 1) => "day",
                (Lang::En, _) => "days",
                (Lang::Pt, 1) => "dia",
                (Lang::Pt, _) => "dias",
            };
            (days, unit)
        }
    };
    match lang {
        Lang::En => format!("{n} {unit} ago"),
        Lang::Pt => format!("há {n} {unit}"),
    }
}

/// The path with the home folder written as `~`.
pub fn tilde(path: &Path, home: Option<&Path>) -> String {
    if let Some(home) = home
        && let Ok(rest) = path.strip_prefix(home)
    {
        if rest.as_os_str().is_empty() {
            return "~".into();
        }
        return format!("~/{}", rest.display());
    }
    path.display().to_string()
}

/// Cuts the text to `width` cells, ending with "…" when something was cut.
pub fn truncate(text: &str, width: usize) -> String {
    if text.width() <= width {
        return text.to_string();
    }
    if width == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut used = 0;
    for ch in text.chars() {
        let w = ch.width().unwrap_or(0);
        if used + w > width - 1 {
            break;
        }
        out.push(ch);
        used += w;
    }
    out.push('…');
    out
}

/// Cuts the middle of the text, keeping more of the end: paths end in the part that matters.
pub fn truncate_middle(text: &str, width: usize) -> String {
    if text.width() <= width {
        return text.to_string();
    }
    if width < 5 {
        return truncate(text, width);
    }
    let head_width = (width - 1) / 3;
    let tail_width = width - 1 - head_width;
    let mut head = String::new();
    let mut used = 0;
    for ch in text.chars() {
        let w = ch.width().unwrap_or(0);
        if used + w > head_width {
            break;
        }
        head.push(ch);
        used += w;
    }
    let mut tail: Vec<char> = Vec::new();
    let mut used = 0;
    for ch in text.chars().rev() {
        let w = ch.width().unwrap_or(0);
        if used + w > tail_width {
            break;
        }
        tail.push(ch);
        used += w;
    }
    tail.reverse();
    format!("{head}…{}", tail.into_iter().collect::<String>())
}

/// Breaks the text into lines of at most `width` cells, at spaces when it can, and cuts what
/// does not fit in `max_lines`.
pub fn wrap(text: &str, width: usize, max_lines: usize) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    if width == 0 || max_lines == 0 {
        return lines;
    }
    let mut line = String::new();
    let words: Vec<&str> = text.split_whitespace().collect();
    let mut i = 0;
    while i < words.len() {
        let word = words[i];
        let needed = if line.is_empty() {
            word.width()
        } else {
            line.width() + 1 + word.width()
        };
        if needed <= width {
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(word);
            i += 1;
            continue;
        }
        if line.is_empty() {
            // A word longer than the line: cut it.
            lines.push(truncate(word, width));
            i += 1;
        } else {
            lines.push(std::mem::take(&mut line));
        }
        if lines.len() == max_lines {
            break;
        }
    }
    if !line.is_empty() && lines.len() < max_lines {
        lines.push(line);
    }
    if i < words.len() {
        let rest = words[i..].join(" ");
        if let Some(last) = lines.last_mut() {
            *last = truncate(&format!("{last} {rest}"), width);
        }
    }
    lines
}

/// Breaks a command into lines of `width` cells: at spaces when the words fit, inside a word
/// (a long path) when they do not, ending in "…" when it needs more than `max_lines`.
pub fn hard_wrap(text: &str, width: usize, max_lines: usize) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    if width == 0 || max_lines == 0 {
        return lines;
    }
    let mut line = String::new();
    for word in text.split(' ') {
        let gap = usize::from(!line.is_empty());
        if line.width() + gap + word.width() <= width {
            if gap == 1 {
                line.push(' ');
            }
            line.push_str(word);
            continue;
        }
        if word.width() <= width {
            lines.push(std::mem::take(&mut line));
            line.push_str(word);
            continue;
        }
        // A word longer than a line: fill the current line, then break it by cells.
        if gap == 1 {
            if line.width() + 1 < width {
                line.push(' ');
            } else {
                lines.push(std::mem::take(&mut line));
            }
        }
        for ch in word.chars() {
            let w = ch.width().unwrap_or(0);
            if line.width() + w > width {
                lines.push(std::mem::take(&mut line));
            }
            line.push(ch);
        }
    }
    if !line.is_empty() {
        lines.push(line);
    }
    if lines.len() > max_lines {
        lines.truncate(max_lines);
        if let Some(last) = lines.last_mut() {
            *last = truncate(&format!("{last}  "), width);
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hard_wraps_long_commands() {
        assert_eq!(hard_wrap("abcdefgh", 3, 5), ["abc", "def", "gh"]);
        assert_eq!(hard_wrap("abcdefgh", 3, 2), ["abc", "de…"]);
        assert_eq!(hard_wrap("abc", 3, 1), ["abc"]);
        assert!(hard_wrap("abc", 0, 1).is_empty());
        assert_eq!(
            hard_wrap("$ git -C ~/code/app worktree remove ~/code/app/wt", 20, 5),
            ["$ git -C ~/code/app", "worktree remove", "~/code/app/wt"]
        );
        assert_eq!(
            hard_wrap("rm ~/a/very/long/path/here", 10, 5),
            ["rm ~/a/ver", "y/long/pat", "h/here"]
        );
        for width in 1..40 {
            for line in hard_wrap(
                "$ git -C ~/Workspace/shop worktree remove --force ~/Workspace/shop/.claude/worktrees/x",
                width,
                9,
            ) {
                assert!(line.width() <= width, "{line:?} wider than {width}");
            }
        }
    }

    #[test]
    fn sizes_use_powers_of_1000_and_the_language_decimal_mark() {
        assert_eq!(size(0, Lang::En), "0 B");
        assert_eq!(size(999, Lang::En), "999 B");
        assert_eq!(size(1_000, Lang::En), "1.0 kB");
        assert_eq!(size(1_900_000_000, Lang::En), "1.9 GB");
        assert_eq!(size(1_900_000_000, Lang::Pt), "1,9 GB");
        assert_eq!(size(99_949, Lang::En), "99.9 kB");
        assert_eq!(size(99_950, Lang::En), "100 kB");
        assert_eq!(size(812_300_000, Lang::En), "812 MB");
        assert_eq!(size(999_600, Lang::En), "1.0 MB");
    }

    #[test]
    fn short_sizes_fit_four_cells() {
        assert_eq!(size_short(1_900_000_000, Lang::En), "1.9G");
        assert_eq!(size_short(1_900_000_000, Lang::Pt), "1,9G");
        assert_eq!(size_short(812_300_000, Lang::En), "812M");
        assert_eq!(size_short(12_000, Lang::En), "12K");
        assert_eq!(size_short(999, Lang::En), "999B");
        for bytes in [
            0,
            5,
            999,
            1_000,
            9_949,
            9_950,
            999_499,
            999_500,
            45_000_000_000,
        ] {
            assert!(size_short(bytes, Lang::Pt).width() <= 4, "{bytes}");
        }
    }

    #[test]
    fn counts_group_thousands() {
        assert_eq!(count(0, Lang::En), "0");
        assert_eq!(count(999, Lang::En), "999");
        assert_eq!(count(212_340, Lang::En), "212,340");
        assert_eq!(count(1_212_340, Lang::Pt), "1.212.340");
    }

    #[test]
    fn times_read_naturally() {
        assert_eq!(ago_short(5, Lang::En), "now");
        assert_eq!(ago_short(5 * 60, Lang::Pt), "5m");
        assert_eq!(ago_short(3 * 86_400, Lang::Pt), "3d");
        assert_eq!(ago(2 * 3600, Lang::En), "2 h ago");
        assert_eq!(ago(2 * 3600, Lang::Pt), "há 2 h");
        assert_eq!(ago(86_400, Lang::Pt), "há 1 dia");
        assert_eq!(ago(3 * 86_400, Lang::En), "3 days ago");
    }

    #[test]
    fn home_becomes_a_tilde() {
        let home = Path::new("/Users/me");
        assert_eq!(tilde(Path::new("/Users/me/code/x"), Some(home)), "~/code/x");
        assert_eq!(tilde(Path::new("/Users/me"), Some(home)), "~");
        assert_eq!(tilde(Path::new("/Users/meh"), Some(home)), "/Users/meh");
        assert_eq!(tilde(Path::new("/tmp"), None), "/tmp");
    }

    #[test]
    fn truncation_respects_cell_widths() {
        assert_eq!(truncate("worktree", 20), "worktree");
        assert_eq!(truncate("worktree", 5), "work…");
        assert_eq!(truncate("worktree", 1), "…");
        assert_eq!(truncate("worktree", 0), "");
        assert_eq!(truncate("ação", 3), "aç…");
        let cut = truncate_middle("~/Workspace/shop/.claude/worktrees/coupons-613", 24);
        assert_eq!(cut.width(), 24);
        assert!(
            cut.starts_with("~/Work") && cut.ends_with("coupons-613"),
            "{cut}"
        );
    }

    #[test]
    fn wraps_at_spaces_and_cuts_the_rest() {
        assert_eq!(wrap("a bb ccc", 4, 3), vec!["a bb", "ccc"]);
        assert_eq!(wrap("one two three four", 9, 1), vec!["one two …"]);
        assert_eq!(wrap("supercalifragilistic", 6, 2), vec!["super…"]);
        assert!(wrap("", 10, 2).is_empty());
        for width in 1..30 {
            for line in wrap(
                "bug hunter scanning PR #123 (3 lenses); CI in progress",
                width,
                2,
            ) {
                assert!(line.width() <= width, "{line:?} wider than {width}");
            }
        }
    }
}
