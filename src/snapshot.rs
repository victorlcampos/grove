//! Pictures of the screen for checking the design: a drawn buffer as plain text or as an SVG
//! with its colors, drawn by cerne.

pub use cerne::snapshot::{parse_size, svg, text};

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use ratatui::crossterm::event::{KeyCode, KeyEvent};

    use crate::app::{Confirm, Mode, ToastKind, View};
    use crate::demo;
    use crate::ui::tests::draw;

    /// Writes pictures of the made-up computer at several window sizes into the folder in
    /// `GROVE_PICTURES`: `GROVE_PICTURES=/tmp/p cargo test pictures -- --ignored`.
    #[test]
    #[ignore = "writes files for a person to look at"]
    fn pictures() {
        let dir = PathBuf::from(std::env::var("GROVE_PICTURES").expect("set GROVE_PICTURES"));
        std::fs::create_dir_all(&dir).unwrap();
        let shots: [(&str, u16, u16, u8); 18] = [
            ("readme-list", 118, 27, 4),
            ("readme-sessions", 118, 25, 7),
            ("readme-routines", 118, 26, 10),
            ("sessions-wide", 200, 44, 8),
            ("sessions-pane", 46, 34, 9),
            ("readme-pane", 46, 34, 0),
            ("readme-confirm", 96, 24, 5),
            ("readme-idle", 96, 30, 2),
            ("readme-stops", 96, 26, 6),
            ("wide", 200, 44, 0),
            ("laptop", 140, 38, 0),
            ("half", 96, 50, 0),
            ("tall-narrow", 58, 60, 0),
            ("narrow", 40, 34, 0),
            ("tiny", 26, 10, 0),
            ("confirm", 120, 40, 1),
            ("idle", 100, 40, 2),
            ("help", 100, 34, 3),
        ];
        for (name, width, height, scene) in shots {
            let mut app = demo::app();
            app.frame = 3;
            app.move_by(3);
            let cli_release =
                PathBuf::from("/Users/me/Workspace/shop/.claude/worktrees/cli-release");
            match scene {
                1 => {
                    app.mode = Mode::Confirm(Confirm {
                        targets: vec![app.selected.clone().unwrap()],
                        idle: false,
                    })
                }
                2 => {
                    app.mode = Mode::Confirm(Confirm {
                        targets: app.idle_worktrees(),
                        idle: true,
                    });
                }
                3 => app.mode = Mode::Help,
                4 => app.show_details = false,
                5 => {
                    app.selected = Some(cli_release.clone());
                    app.mode = Mode::Confirm(Confirm {
                        targets: vec![cli_release],
                        idle: false,
                    });
                }
                6 => {
                    app.move_by(2);
                    app.mode = Mode::Confirm(Confirm {
                        targets: vec![app.selected.clone().unwrap()],
                        idle: false,
                    });
                }
                7..=9 => {
                    app.view = View::Sessions;
                    app.show_details = scene != 7;
                    app.move_by(5);
                    if scene == 8 {
                        app.on_key(KeyEvent::from(KeyCode::Enter));
                    }
                }
                10 => app.view = View::Routines,
                _ => {}
            }
            if name == "wide" {
                app.toast(
                    ToastKind::Ok,
                    "build-api-401-messages removed · 1.2 GB freed".into(),
                );
            }
            let buf = draw(&mut app, width, height);
            std::fs::write(dir.join(format!("{name}.svg")), svg(&buf)).unwrap();
            std::fs::write(dir.join(format!("{name}.txt")), text(&buf)).unwrap();
        }
    }
}
