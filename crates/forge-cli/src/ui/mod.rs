mod dock;
mod file_tree;
pub(crate) mod layout;
pub(crate) mod terminal_pane;

use crate::app::App;
use ratatui::Frame;

/// Top-level render function.
pub fn render(frame: &mut Frame, app: &App) {
    layout::render(frame, app);
}
