//! Writing to the host clipboard through OSC 52. The only place in `ui` allowed to touch the
//! terminal on behalf of a copy — `App` only ever hands text to `App::take_copy`.

use std::io;

use ratatui::crossterm::clipboard::CopyToClipboard;
use ratatui::crossterm::execute;

pub fn copy(text: &str) -> io::Result<()> {
    execute!(io::stdout(), CopyToClipboard::to_clipboard_from(text))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copying_to_a_pipe_rather_than_a_terminal_still_succeeds() {
        assert!(copy("hello").is_ok());
    }
}
