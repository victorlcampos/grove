//! Colors and symbols. Truecolor when the terminal says so, the 256-color palette otherwise,
//! and no color at all under `NO_COLOR`.

use ratatui::style::{Color, Modifier, Style};

use crate::model::Agent;

pub const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Palette {
    Rgb,
    Indexed,
    Mono,
}

#[derive(Clone, Copy, Debug)]
pub struct Theme {
    palette: Palette,
    pub accent: Color,
    pub muted: Color,
    pub faint: Color,
    pub selection: Color,
    pub working: Color,
    pub blocked: Color,
    pub idle: Color,
    pub busy: Color,
    pub free: Color,
    pub missing: Color,
    pub locked: Color,
    pub ok: Color,
    pub warn: Color,
    pub error: Color,
    claude: Color,
    codex: Color,
    opencode: Color,
}

const RGB: Theme = Theme {
    palette: Palette::Rgb,
    accent: Color::Rgb(126, 211, 135),
    muted: Color::Rgb(139, 148, 158),
    faint: Color::Rgb(72, 79, 88),
    selection: Color::Rgb(40, 46, 56),
    working: Color::Rgb(98, 214, 132),
    blocked: Color::Rgb(244, 114, 182),
    idle: Color::Rgb(229, 192, 123),
    busy: Color::Rgb(97, 175, 239),
    free: Color::Rgb(92, 99, 112),
    missing: Color::Rgb(224, 108, 117),
    locked: Color::Rgb(198, 120, 221),
    ok: Color::Rgb(98, 214, 132),
    warn: Color::Rgb(229, 192, 123),
    error: Color::Rgb(224, 108, 117),
    claude: Color::Rgb(217, 119, 87),
    codex: Color::Rgb(130, 170, 255),
    opencode: Color::Rgb(86, 212, 221),
};

const INDEXED: Theme = Theme {
    palette: Palette::Indexed,
    accent: Color::Indexed(114),
    muted: Color::Indexed(246),
    faint: Color::Indexed(239),
    selection: Color::Indexed(236),
    working: Color::Indexed(78),
    blocked: Color::Indexed(205),
    idle: Color::Indexed(179),
    busy: Color::Indexed(75),
    free: Color::Indexed(242),
    missing: Color::Indexed(167),
    locked: Color::Indexed(176),
    ok: Color::Indexed(78),
    warn: Color::Indexed(179),
    error: Color::Indexed(167),
    claude: Color::Indexed(173),
    codex: Color::Indexed(111),
    opencode: Color::Indexed(80),
};

const MONO: Theme = Theme {
    palette: Palette::Mono,
    accent: Color::Reset,
    muted: Color::Reset,
    faint: Color::Reset,
    selection: Color::Reset,
    working: Color::Reset,
    blocked: Color::Reset,
    idle: Color::Reset,
    busy: Color::Reset,
    free: Color::Reset,
    missing: Color::Reset,
    locked: Color::Reset,
    ok: Color::Reset,
    warn: Color::Reset,
    error: Color::Reset,
    claude: Color::Reset,
    codex: Color::Reset,
    opencode: Color::Reset,
};

impl Theme {
    pub fn detect() -> Self {
        let var = |name: &str| std::env::var(name).unwrap_or_default();
        Self::pick(&var("NO_COLOR"), &var("COLORTERM"))
    }

    fn pick(no_color: &str, colorterm: &str) -> Self {
        if !no_color.is_empty() {
            MONO
        } else if matches!(colorterm, "truecolor" | "24bit") {
            RGB
        } else {
            INDEXED
        }
    }

    #[cfg(test)]
    pub fn rgb() -> Self {
        RGB
    }

    pub fn colored(&self) -> bool {
        self.palette != Palette::Mono
    }

    pub fn muted(&self) -> Style {
        match self.palette {
            Palette::Mono => Style::new().add_modifier(Modifier::DIM),
            _ => Style::new().fg(self.muted),
        }
    }

    pub fn faint(&self) -> Style {
        match self.palette {
            Palette::Mono => Style::new().add_modifier(Modifier::DIM),
            _ => Style::new().fg(self.faint),
        }
    }

    /// The row under the cursor.
    pub fn selected(&self) -> Style {
        match self.palette {
            Palette::Mono => Style::new().add_modifier(Modifier::REVERSED),
            _ => Style::new().bg(self.selection),
        }
    }

    pub fn agent(&self, agent: Agent) -> Color {
        match agent {
            Agent::Claude => self.claude,
            Agent::Codex => self.codex,
            Agent::OpenCode => self.opencode,
        }
    }

    /// Color of a size bar, from teal for the small worktrees to red for the largest.
    pub fn size_color(&self, fraction: f64) -> Color {
        let fraction = fraction.clamp(0.0, 1.0);
        match self.palette {
            Palette::Mono => Color::Reset,
            Palette::Indexed => {
                const STEPS: [u8; 5] = [80, 114, 179, 173, 167];
                let step = (fraction * (STEPS.len() - 1) as f64).round() as usize;
                Color::Indexed(STEPS[step])
            }
            Palette::Rgb => {
                const LOW: (f64, f64, f64) = (86.0, 182.0, 194.0);
                const MID: (f64, f64, f64) = (229.0, 192.0, 123.0);
                const HIGH: (f64, f64, f64) = (224.0, 108.0, 117.0);
                let (from, to, t) = if fraction < 0.5 {
                    (LOW, MID, fraction * 2.0)
                } else {
                    (MID, HIGH, (fraction - 0.5) * 2.0)
                };
                let mix = |a: f64, b: f64| (a + (b - a) * t).round() as u8;
                Color::Rgb(mix(from.0, to.0), mix(from.1, to.1), mix(from.2, to.2))
            }
        }
    }
}

pub fn agent_glyph(agent: Agent) -> &'static str {
    match agent {
        Agent::Claude => "✻",
        Agent::Codex => "❯",
        Agent::OpenCode => "◈",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn follows_no_color_and_colorterm() {
        assert_eq!(Theme::pick("1", "truecolor").palette, Palette::Mono);
        assert_eq!(Theme::pick("", "truecolor").palette, Palette::Rgb);
        assert_eq!(Theme::pick("", "24bit").palette, Palette::Rgb);
        assert_eq!(Theme::pick("", "").palette, Palette::Indexed);
    }

    #[test]
    fn size_colors_run_from_teal_to_red() {
        let theme = RGB;
        assert_eq!(theme.size_color(0.0), Color::Rgb(86, 182, 194));
        assert_eq!(theme.size_color(0.5), Color::Rgb(229, 192, 123));
        assert_eq!(theme.size_color(1.0), Color::Rgb(224, 108, 117));
        assert_eq!(theme.size_color(7.0), Color::Rgb(224, 108, 117));
        assert_eq!(INDEXED.size_color(1.0), Color::Indexed(167));
    }
}
