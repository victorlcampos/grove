//! Puts text on the clipboard: with the system's own tool when there is one, else through
//! the terminal (the OSC 52 sequence), which also reaches the clipboard over SSH.

use std::io::Write;
use std::process::{Command, Stdio};

/// Where copied text goes: the clipboard, or a list tests read.
#[derive(Debug)]
pub enum Clipboard {
    System,
    #[cfg(test)]
    Memory(Vec<String>),
}

impl Clipboard {
    pub fn copy(&mut self, text: &str) -> Result<(), String> {
        match self {
            Clipboard::System => system(text),
            #[cfg(test)]
            Clipboard::Memory(copied) => {
                copied.push(text.to_string());
                Ok(())
            }
        }
    }

    #[cfg(test)]
    pub fn last(&self) -> Option<&str> {
        match self {
            Clipboard::System => None,
            Clipboard::Memory(copied) => copied.last().map(String::as_str),
        }
    }
}

fn system(text: &str) -> Result<(), String> {
    // Over SSH the tools here would fill this computer's clipboard, not the one in front of
    // the person.
    let remote = ["SSH_CONNECTION", "SSH_TTY"]
        .iter()
        .any(|name| std::env::var_os(name).is_some_and(|value| !value.is_empty()));
    if !remote
        && tools()
            .into_iter()
            .any(|(program, args)| pipe(program, args, text))
    {
        return Ok(());
    }
    let mut out = std::io::stdout();
    write!(out, "\x1b]52;c;{}\x07", base64(text.as_bytes()))
        .and_then(|()| out.flush())
        .map_err(|error| error.to_string())
}

/// The clipboard tools this system may have, in the order to try them.
fn tools() -> Vec<(&'static str, &'static [&'static str])> {
    if cfg!(target_os = "macos") {
        return vec![("pbcopy", &[])];
    }
    if cfg!(windows) {
        return vec![("clip", &[])];
    }
    let set = |name: &str| std::env::var_os(name).is_some_and(|value| !value.is_empty());
    let mut tools: Vec<(&'static str, &'static [&'static str])> = Vec::new();
    if set("WAYLAND_DISPLAY") {
        tools.push(("wl-copy", &[]));
    }
    if set("DISPLAY") {
        tools.push(("xclip", &["-selection", "clipboard"]));
        tools.push(("xsel", &["--clipboard", "--input"]));
    }
    tools
}

/// Hands `text` to a clipboard tool; whether it took it.
fn pipe(program: &str, args: &[&str], text: &str) -> bool {
    let Ok(mut child) = Command::new(program)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    else {
        return false;
    };
    let written = child
        .stdin
        .take()
        .is_some_and(|mut stdin| stdin.write_all(text.as_bytes()).is_ok());
    child.wait().is_ok_and(|status| status.success()) && written
}

fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, &byte)| n | (u32::from(byte) << (16 - 8 * i)));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(char::from(ALPHABET[((n >> (18 - 6 * i)) & 63) as usize]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_base64_with_padding() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
        assert_eq!(
            base64("cd ~/ação && claude --resume 1".as_bytes()),
            "Y2Qgfi9hw6fDo28gJiYgY2xhdWRlIC0tcmVzdW1lIDE="
        );
    }

    #[cfg(unix)]
    #[test]
    fn hands_the_text_to_a_tool_and_tells_whether_it_took_it() {
        assert!(pipe("cat", &[], "cd ~/code && claude --resume 1"));
        assert!(!pipe("false", &[], "x"), "a tool that fails");
        assert!(!pipe("grove-no-such-tool", &[], "x"));
    }

    #[test]
    fn memory_keeps_what_was_copied() {
        let mut clipboard = Clipboard::Memory(Vec::new());
        clipboard.copy("one").unwrap();
        clipboard.copy("two").unwrap();
        assert_eq!(clipboard.last(), Some("two"));
    }
}
