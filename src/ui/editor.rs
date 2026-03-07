// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Andrea Cervesato <andrea.cervesato@suse.com>
use anyhow::Result;
use crossterm::{
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{Terminal, backend::CrosstermBackend};
use std::io::{self, Write};
use std::process::Command;

/// Holds a text buffer that can be opened in the default system editor.
/// The TUI is suspended while the editor runs and resumed on exit.
pub struct Editor {
    data: String,
    cursor_line: usize,
}

impl Editor {
    pub fn new(data: String) -> Self {
        Self {
            data,
            cursor_line: 0,
        }
    }

    /// Position the cursor at the given line number when opening the editor.
    pub fn with_cursor(mut self, line: usize) -> Self {
        self.cursor_line = line;
        self
    }

    /// Open the default system editor (`$VISUAL` → `$EDITOR` → `vim`) on the
    /// current buffer. The TUI is suspended while the editor runs and resumed
    /// afterwards. `self.data` is replaced with the edited content.
    pub fn open(&mut self, terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> Result<()> {
        let tmp_path = std::env::temp_dir().join(format!("brew_reply_{}.eml", std::process::id()));
        {
            let mut f = std::fs::File::create(&tmp_path)?;
            f.write_all(self.data.as_bytes())?;
        }

        disable_raw_mode()?;
        execute!(terminal.backend_mut(), LeaveAlternateScreen)?;

        let editor_cmd = std::env::var("VISUAL")
            .or_else(|_| std::env::var("EDITOR"))
            .unwrap_or_else(|_| "vim".to_string());

        let mut cmd = Command::new(&editor_cmd);
        if self.cursor_line > 0 {
            cmd.arg(format!("+{}", self.cursor_line));
        }
        cmd.arg(&tmp_path).status()?;

        let edited = std::fs::read_to_string(&tmp_path)?;
        let _ = std::fs::remove_file(&tmp_path);

        enable_raw_mode()?;
        execute!(terminal.backend_mut(), EnterAlternateScreen)?;
        terminal.clear()?;

        self.data = edited;
        Ok(())
    }

    /// Consume the editor and return ownership of the buffer.
    pub fn into_data(self) -> String {
        self.data
    }
}
