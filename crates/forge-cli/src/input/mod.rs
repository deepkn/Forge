//! Input handling — maps key events to application actions.
//!
//! Forge uses two input modes:
//! 1. **Normal mode** — all keys go to the focused PTY
//! 2. **Command mode** — activated by pressing `Esc`, next key is a forge command
//!
//! This avoids conflicts with terminal emulators eating Alt+key combinations.
//! Alt+key bindings are also supported for terminals that deliver them correctly.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// High-level input events.
#[derive(Debug, Clone)]
pub enum InputEvent {
    Quit,
    ToggleFileTree,
    ToggleCoordinator,
    FocusLeft,
    FocusRight,
    FocusUp,
    FocusDown,
    ZoomToggle,
    SplitHorizontal,
    SplitVertical,
    ClosePane,
    FocusDockItem(usize),
    ToggleFullscreen,
    ResizeLeft,
    ResizeRight,
    ResizeUp,
    ResizeDown,
    TreeUp,
    TreeDown,
    TreeExpand,
    TreeCollapse,
    TreeToggle,
    /// Spawn a new subagent in the center pane.
    SpawnNewAgent,
    /// Scroll the focused terminal up (into scrollback history).
    ScrollUp,
    /// Scroll the focused terminal down (toward current output).
    ScrollDown,
    /// Enters command mode (Esc prefix).
    EnterCommandMode,
    /// Raw input forwarded to focused PTY.
    RawInput(KeyEvent),
}

/// Tracks whether we're in command mode (Esc prefix was pressed).
pub struct InputState {
    pub command_mode: bool,
}

impl InputState {
    pub fn new() -> Self {
        Self {
            command_mode: false,
        }
    }
}

/// Map a key event to an InputEvent, considering current input state.
pub fn map_key_event(key: KeyEvent, state: &mut InputState) -> Option<InputEvent> {
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let shift = key.modifiers.contains(KeyModifiers::SHIFT);

    // ─── Command mode: Esc was pressed, interpret next key as command ────
    if state.command_mode {
        state.command_mode = false;

        return match key.code {
            KeyCode::Char('q') => Some(InputEvent::Quit),
            KeyCode::Char('e') => Some(InputEvent::ToggleFileTree),
            KeyCode::Char('c') => Some(InputEvent::ToggleCoordinator),
            KeyCode::Char('h') => Some(InputEvent::FocusLeft),
            KeyCode::Char('l') => Some(InputEvent::FocusRight),
            KeyCode::Char('j') => Some(InputEvent::FocusDown),
            KeyCode::Char('k') => Some(InputEvent::FocusUp),
            KeyCode::Char('z') => Some(InputEvent::ZoomToggle),
            KeyCode::Char('s') => Some(InputEvent::SplitHorizontal),
            KeyCode::Char('v') => Some(InputEvent::SplitVertical),
            KeyCode::Char('w') => Some(InputEvent::ClosePane),
            KeyCode::Char('f') => Some(InputEvent::ToggleFullscreen),
            KeyCode::Char('n') => Some(InputEvent::SpawnNewAgent),
            KeyCode::Char('H') => Some(InputEvent::ResizeLeft),
            KeyCode::Char('L') => Some(InputEvent::ResizeRight),
            KeyCode::Char('J') => Some(InputEvent::ResizeDown),
            KeyCode::Char('K') => Some(InputEvent::ResizeUp),
            KeyCode::Char(c) if c.is_ascii_digit() => {
                let n = c.to_digit(10).unwrap() as usize;
                Some(InputEvent::FocusDockItem(n))
            }
            // Esc+Esc sends a real Esc to the PTY
            KeyCode::Esc => Some(InputEvent::RawInput(KeyEvent::new(
                KeyCode::Esc,
                KeyModifiers::NONE,
            ))),
            // Unknown command key — drop it
            _ => None,
        };
    }

    // ─── Esc key: enter command mode ─────────────────────────────────────
    if key.code == KeyCode::Esc && key.modifiers.is_empty() {
        state.command_mode = true;
        return Some(InputEvent::EnterCommandMode);
    }

    // ─── Alt+Shift+<key> bindings (resize) ───────────────────────────────
    if alt && shift {
        return match key.code {
            KeyCode::Char('H') => Some(InputEvent::ResizeLeft),
            KeyCode::Char('L') => Some(InputEvent::ResizeRight),
            KeyCode::Char('J') => Some(InputEvent::ResizeDown),
            KeyCode::Char('K') => Some(InputEvent::ResizeUp),
            _ => None,
        };
    }

    // ─── Alt+<key> bindings (UI controls) ────────────────────────────────
    if alt {
        return match key.code {
            KeyCode::Char('q') => Some(InputEvent::Quit),
            KeyCode::Char('e') => Some(InputEvent::ToggleFileTree),
            KeyCode::Char('c') => Some(InputEvent::ToggleCoordinator),
            KeyCode::Char('h') => Some(InputEvent::FocusLeft),
            KeyCode::Char('l') => Some(InputEvent::FocusRight),
            KeyCode::Char('j') => Some(InputEvent::FocusDown),
            KeyCode::Char('k') => Some(InputEvent::FocusUp),
            KeyCode::Char('z') => Some(InputEvent::ZoomToggle),
            KeyCode::Char('s') => Some(InputEvent::SplitHorizontal),
            KeyCode::Char('v') => Some(InputEvent::SplitVertical),
            KeyCode::Char('w') => Some(InputEvent::ClosePane),
            KeyCode::Char('f') => Some(InputEvent::ToggleFullscreen),
            KeyCode::Char('n') => Some(InputEvent::SpawnNewAgent),
            KeyCode::Char(c) if c.is_ascii_digit() => {
                let n = c.to_digit(10).unwrap() as usize;
                Some(InputEvent::FocusDockItem(n))
            }
            _ => None,
        };
    }

    // ─── Ctrl+Q quit ─────────────────────────────────────────────────────
    if ctrl && key.code == KeyCode::Char('q') {
        return Some(InputEvent::Quit);
    }

    // ─── Shift+PageUp/PageDown: scroll terminal history ──────────────────
    if shift && !ctrl && !alt {
        match key.code {
            KeyCode::PageUp => return Some(InputEvent::ScrollUp),
            KeyCode::PageDown => return Some(InputEvent::ScrollDown),
            _ => {}
        }
    }

    // ─── Everything else: raw input to focused PTY ───────────────────────
    Some(InputEvent::RawInput(key))
}

/// Map raw input events to file tree actions (when file tree is focused).
pub fn map_tree_key(key: &KeyEvent) -> Option<InputEvent> {
    match key.code {
        KeyCode::Char('j') | KeyCode::Down => Some(InputEvent::TreeDown),
        KeyCode::Char('k') | KeyCode::Up => Some(InputEvent::TreeUp),
        KeyCode::Char('l') | KeyCode::Right => Some(InputEvent::TreeExpand),
        KeyCode::Char('h') | KeyCode::Left => Some(InputEvent::TreeCollapse),
        KeyCode::Enter => Some(InputEvent::TreeToggle),
        _ => None,
    }
}
