/// Integration test: verify alacritty_terminal processes ANSI bytes correctly.

#[test]
fn test_terminal_processes_ansi_output() {
    use alacritty_terminal::event::VoidListener;
    use alacritty_terminal::grid::Dimensions;
    use alacritty_terminal::term::{Config as TermConfig, Term};
    use alacritty_terminal::vte::ansi::Processor;

    struct TermSize {
        columns: usize,
        screen_lines: usize,
    }

    impl Dimensions for TermSize {
        fn total_lines(&self) -> usize { self.screen_lines }
        fn screen_lines(&self) -> usize { self.screen_lines }
        fn columns(&self) -> usize { self.columns }
    }

    let size = TermSize { columns: 80, screen_lines: 24 };
    let config = TermConfig { scrolling_history: 100, ..Default::default() };
    let mut term: Term<VoidListener> = Term::new(config, &size, VoidListener);
    let mut parser: Processor = Processor::new();

    // Feed "Hello, World!" with ANSI green color then reset
    let input = b"\x1b[32mHello\x1b[0m, World!\r\n";
    for &byte in input.iter() {
        parser.advance(&mut term, byte);
    }

    // Check grid contents at row 0
    use alacritty_terminal::index::{Column, Line};
    let grid = term.grid();
    assert_eq!(grid[Line(0)][Column(0)].c, 'H');
    assert_eq!(grid[Line(0)][Column(1)].c, 'e');
    assert_eq!(grid[Line(0)][Column(4)].c, 'o');
    assert_eq!(grid[Line(0)][Column(7)].c, 'W');

    // Verify 'H' has green foreground
    let h_cell = &grid[Line(0)][Column(0)];
    match h_cell.fg {
        alacritty_terminal::vte::ansi::Color::Named(named) => {
            assert_eq!(named, alacritty_terminal::vte::ansi::NamedColor::Green);
        }
        other => panic!("Expected Named(Green), got {:?}", other),
    }
}

#[test]
fn test_terminal_cursor_movement() {
    use alacritty_terminal::event::VoidListener;
    use alacritty_terminal::grid::Dimensions;
    use alacritty_terminal::term::{Config as TermConfig, Term};
    use alacritty_terminal::vte::ansi::Processor;

    struct TermSize { columns: usize, screen_lines: usize }
    impl Dimensions for TermSize {
        fn total_lines(&self) -> usize { self.screen_lines }
        fn screen_lines(&self) -> usize { self.screen_lines }
        fn columns(&self) -> usize { self.columns }
    }

    let size = TermSize { columns: 80, screen_lines: 24 };
    let config = TermConfig::default();
    let mut term: Term<VoidListener> = Term::new(config, &size, VoidListener);
    let mut parser: Processor = Processor::new();

    // Write on line 0, then move cursor to line 2, col 5 and write "X"
    let input = b"Line0\x1b[3;6HX";
    for &byte in input.iter() {
        parser.advance(&mut term, byte);
    }

    use alacritty_terminal::index::{Column, Line};
    let grid = term.grid();
    // Line 0 should have "Line0"
    assert_eq!(grid[Line(0)][Column(0)].c, 'L');
    // Line 2 (0-indexed), Col 5 (0-indexed from 1-indexed "6") should have "X"
    assert_eq!(grid[Line(2)][Column(5)].c, 'X');
}
