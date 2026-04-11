//! Terminal pane — renders a single agent's terminal using alacritty_terminal.

use crate::terminal::TerminalState;
use forge_core::types::AgentId;
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Paragraph};

/// Render a terminal pane for an agent.
pub fn render(
    frame: &mut Frame,
    area: Rect,
    agent_id: Option<&AgentId>,
    terminal: Option<&TerminalState>,
    focused: bool,
) {
    let border_style = if focused {
        Style::default().fg(Color::Green)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let title = match agent_id {
        Some(id) => format!(" Agent {} ", id),
        None => " Empty Pane ".to_string(),
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .title(title);

    match terminal {
        Some(term) => {
            let inner = block.inner(area);
            frame.render_widget(block, area);
            render_terminal_grid(frame, inner, term);
        }
        None => {
            let content = match agent_id {
                Some(id) => format!("Agent {} — connecting...", id),
                None => "No agent assigned to this pane.".to_string(),
            };
            let text = Paragraph::new(content)
                .block(block)
                .style(Style::default().fg(Color::DarkGray));
            frame.render_widget(text, area);
        }
    }
}

/// Public entry point for rendering terminal content (used by layout.rs for coordinator).
pub fn render_terminal_content(frame: &mut Frame, area: Rect, term_state: &TerminalState) {
    render_terminal_grid(frame, area, term_state);
}

/// Convert alacritty_terminal grid cells into ratatui buffer cells.
fn render_terminal_grid(frame: &mut Frame, area: Rect, term_state: &TerminalState) {
    let content = term_state.renderable_content();
    let cursor = content.cursor;
    let buf = frame.buffer_mut();

    for indexed in content.display_iter {
        let point = indexed.point;
        let cell = &indexed.cell;

        // Map grid coordinates to buffer coordinates
        let x = area.x + point.column.0 as u16;
        let y = area.y + point.line.0 as u16;

        // Skip cells outside the visible area
        if x >= area.x + area.width || y >= area.y + area.height {
            continue;
        }

        // Skip wide char spacers (second cell of a wide character)
        if cell
            .flags
            .contains(alacritty_terminal::term::cell::Flags::WIDE_CHAR_SPACER)
        {
            continue;
        }

        let buf_cell = &mut buf[(x, y)];

        // Set character
        let ch = cell.c;
        if ch == ' ' || ch == '\0' {
            buf_cell.set_char(' ');
        } else {
            buf_cell.set_char(ch);
        }

        // Map alacritty colors → ratatui colors
        let fg = convert_color(cell.fg, true);
        let bg = convert_color(cell.bg, false);

        // Handle INVERSE flag
        let flags = cell.flags;
        if flags.contains(alacritty_terminal::term::cell::Flags::INVERSE) {
            buf_cell.set_fg(bg);
            buf_cell.set_bg(fg);
        } else {
            buf_cell.set_fg(fg);
            buf_cell.set_bg(bg);
        }

        // Map text attributes
        let mut modifier = Modifier::empty();
        if flags.contains(alacritty_terminal::term::cell::Flags::BOLD) {
            modifier |= Modifier::BOLD;
        }
        if flags.contains(alacritty_terminal::term::cell::Flags::ITALIC) {
            modifier |= Modifier::ITALIC;
        }
        if flags.contains(alacritty_terminal::term::cell::Flags::UNDERLINE) {
            modifier |= Modifier::UNDERLINED;
        }
        if flags.contains(alacritty_terminal::term::cell::Flags::DIM) {
            modifier |= Modifier::DIM;
        }
        if flags.contains(alacritty_terminal::term::cell::Flags::HIDDEN) {
            modifier |= Modifier::HIDDEN;
        }
        buf_cell.set_style(Style::default().add_modifier(modifier));
    }

    // Render cursor
    let cx = area.x + cursor.point.column.0 as u16;
    let cy = area.y + cursor.point.line.0 as u16;
    if cx < area.x + area.width && cy < area.y + area.height {
        let buf_cell = &mut buf[(cx, cy)];
        // Invert cursor cell
        let fg = buf_cell.fg;
        let bg = buf_cell.bg;
        buf_cell.set_fg(bg);
        buf_cell.set_bg(if fg == Color::Reset {
            Color::White
        } else {
            fg
        });
    }
}

/// Convert alacritty Color → ratatui Color.
fn convert_color(color: alacritty_terminal::vte::ansi::Color, is_foreground: bool) -> Color {
    use alacritty_terminal::vte::ansi::{Color as AColor, NamedColor};

    match color {
        AColor::Named(named) => match named {
            NamedColor::Black | NamedColor::DimBlack => Color::Black,
            NamedColor::Red | NamedColor::DimRed => Color::Red,
            NamedColor::Green | NamedColor::DimGreen => Color::Green,
            NamedColor::Yellow | NamedColor::DimYellow => Color::Yellow,
            NamedColor::Blue | NamedColor::DimBlue => Color::Blue,
            NamedColor::Magenta | NamedColor::DimMagenta => Color::Magenta,
            NamedColor::Cyan | NamedColor::DimCyan => Color::Cyan,
            NamedColor::White | NamedColor::DimWhite => Color::White,
            NamedColor::BrightBlack => Color::DarkGray,
            NamedColor::BrightRed => Color::LightRed,
            NamedColor::BrightGreen => Color::LightGreen,
            NamedColor::BrightYellow => Color::LightYellow,
            NamedColor::BrightBlue => Color::LightBlue,
            NamedColor::BrightMagenta => Color::LightMagenta,
            NamedColor::BrightCyan => Color::LightCyan,
            NamedColor::BrightWhite => Color::White,
            NamedColor::Foreground => {
                if is_foreground {
                    Color::Reset
                } else {
                    Color::White
                }
            }
            NamedColor::Background => {
                if is_foreground {
                    Color::Black
                } else {
                    Color::Reset
                }
            }
            _ => Color::Reset,
        },
        AColor::Spec(rgb) => Color::Rgb(rgb.r, rgb.g, rgb.b),
        AColor::Indexed(idx) => Color::Indexed(idx),
    }
}
