//! Terminal emulation state — wraps alacritty_terminal for VT100 processing.

use alacritty_terminal::event::VoidListener;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::term::{Config as TermConfig, Term};
use alacritty_terminal::vte::ansi::Processor;

/// Terminal dimensions that implements alacritty's Dimensions trait.
struct TermSize {
    columns: usize,
    screen_lines: usize,
}

impl Dimensions for TermSize {
    fn total_lines(&self) -> usize {
        self.screen_lines
    }

    fn screen_lines(&self) -> usize {
        self.screen_lines
    }

    fn columns(&self) -> usize {
        self.columns
    }
}

/// Wraps an alacritty_terminal::Term with its VTE parser.
/// Feed raw PTY bytes in, read the rendered grid out.
pub struct TerminalState {
    term: Term<VoidListener>,
    parser: Processor,
}

impl TerminalState {
    /// Create a new terminal state with the given dimensions.
    pub fn new(cols: u16, rows: u16) -> Self {
        let size = TermSize {
            columns: cols as usize,
            screen_lines: rows as usize,
        };

        let config = TermConfig {
            scrolling_history: 10_000,
            ..Default::default()
        };

        let term = Term::new(config, &size, VoidListener);
        let parser = Processor::new();

        Self { term, parser }
    }

    /// Feed raw bytes from PTY output into the terminal emulator.
    pub fn process_bytes(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.parser.advance(&mut self.term, byte);
        }
    }

    /// Resize the terminal to new dimensions.
    pub fn resize(&mut self, cols: u16, rows: u16) {
        let size = TermSize {
            columns: cols as usize,
            screen_lines: rows as usize,
        };
        self.term.resize(size);
    }

    /// Get renderable content for the UI to draw.
    pub fn renderable_content(&self) -> alacritty_terminal::term::RenderableContent<'_> {
        self.term.renderable_content()
    }

    /// Number of columns.
    pub fn columns(&self) -> usize {
        self.term.columns()
    }

    /// Number of screen lines.
    pub fn screen_lines(&self) -> usize {
        self.term.screen_lines()
    }
}
