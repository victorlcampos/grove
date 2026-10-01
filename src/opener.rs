//! Opens links, like the `claude://` ones that bring a session up in Claude Desktop, with the
//! system's own tool.

use std::process::{Command, Stdio};

/// Where opened links go: the system, or a list tests read.
#[derive(Debug)]
pub enum Opener {
    System,
    #[cfg(test)]
    Memory(Vec<String>),
}

impl Opener {
    pub fn open(&mut self, link: &str) -> Result<(), String> {
        match self {
            Opener::System => system(link),
            #[cfg(test)]
            Opener::Memory(opened) => {
                opened.push(link.to_string());
                Ok(())
            }
        }
    }

    #[cfg(test)]
    pub fn last(&self) -> Option<&str> {
        match self {
            Opener::System => None,
            Opener::Memory(opened) => opened.last().map(String::as_str),
        }
    }
}

/// Over SSH, what opens is on this computer, not in front of the person.
pub fn remote() -> bool {
    ["SSH_CONNECTION", "SSH_TTY"]
        .iter()
        .any(|name| std::env::var_os(name).is_some_and(|value| !value.is_empty()))
}

fn system(link: &str) -> Result<(), String> {
    // `start` would go through cmd, which takes `&` in a link for its own; the URL handler
    // takes the link as it is, Microsoft Store apps' protocols included.
    let (program, args): (&str, &[&str]) = if cfg!(target_os = "macos") {
        ("open", &[])
    } else if cfg!(windows) {
        ("rundll32", &["url.dll,FileProtocolHandler"])
    } else {
        ("xdg-open", &[])
    };
    let status = Command::new(program)
        .args(args)
        .arg(link)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|error| format!("{program}: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{program} {status}"))
    }
}
