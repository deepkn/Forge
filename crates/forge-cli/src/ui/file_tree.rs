//! File tree pane — renders directory structure with agent lock indicators.

use crate::file_tree::FileTreeState;
use forge_core::types::LockMode;
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, List, ListItem};
use std::path::Path;

/// Render the file tree pane.
pub fn render(
    frame: &mut Frame,
    area: Rect,
    workdir: &Path,
    tree_state: &FileTreeState,
    focused: bool,
) {
    let border_style = if focused {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let dir_name = workdir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(".");

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .title(format!(" {} ", dir_name));

    if tree_state.visible.is_empty() {
        let items = vec![ListItem::new(Line::from(vec![Span::styled(
            "  (empty)",
            Style::default().fg(Color::DarkGray).italic(),
        )]))];
        let list = List::new(items).block(block);
        frame.render_widget(list, area);
        return;
    }

    let inner_height = block.inner(area).height as usize;

    // Calculate scroll offset to keep selection visible
    let scroll_offset = if tree_state.selected >= inner_height {
        tree_state.selected - inner_height + 1
    } else {
        0
    };

    let items: Vec<ListItem> = tree_state
        .visible
        .iter()
        .skip(scroll_offset)
        .take(inner_height)
        .enumerate()
        .map(|(display_idx, node)| {
            let actual_idx = display_idx + scroll_offset;
            let is_selected = actual_idx == tree_state.selected;

            let mut spans = Vec::new();

            // Section headers for remote hosts get distinct rendering
            if node.is_section_header {
                let header_style = if is_selected && focused {
                    Style::default().fg(Color::White).bg(Color::DarkGray).bold()
                } else {
                    Style::default().fg(Color::Cyan).bold()
                };
                let icon = if node.expanded { "▼ " } else { "▶ " };
                spans.push(Span::styled(icon, Style::default().fg(Color::Cyan)));
                spans.push(Span::styled(&node.name, header_style));
                return ListItem::new(Line::from(spans));
            }

            // Indent
            let indent = "  ".repeat(node.depth);
            spans.push(Span::raw(indent));

            // Tree icon
            if node.is_dir {
                let icon = if node.expanded { "▼ " } else { "▶ " };
                spans.push(Span::styled(icon, Style::default().fg(Color::Yellow)));
            } else {
                spans.push(Span::raw("  "));
            }

            // File/dir name
            let name_style = if is_selected && focused {
                Style::default().fg(Color::White).bg(Color::DarkGray).bold()
            } else if node.is_dir {
                Style::default().fg(Color::Blue).bold()
            } else {
                Style::default().fg(Color::Gray)
            };
            spans.push(Span::styled(&node.name, name_style));

            // Lock indicators
            let path_str = node.path.to_string_lossy();
            if let Some(locks) = tree_state.locks.get(path_str.as_ref()) {
                spans.push(Span::raw(" "));
                for lock in locks {
                    let (icon, color) = match lock.mode {
                        LockMode::Write => ("W", Color::Red),
                        LockMode::Read => ("R", Color::Green),
                    };
                    let agent_short = &lock.agent_id.0[..lock.agent_id.0.len().min(4)];
                    spans.push(Span::styled(
                        format!("[{}·{}]", agent_short, icon),
                        Style::default().fg(color),
                    ));
                }
            }

            // Active edit indicator
            if let Some(agent_id) = tree_state.active_edits.get(path_str.as_ref()) {
                let agent_short = &agent_id.0[..agent_id.0.len().min(4)];
                spans.push(Span::styled(
                    format!(" *{}", agent_short),
                    Style::default().fg(Color::Yellow),
                ));
            }

            ListItem::new(Line::from(spans))
        })
        .collect();

    let list = List::new(items).block(block);
    frame.render_widget(list, area);
}
