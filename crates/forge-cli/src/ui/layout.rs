//! Main three-pane layout with bottom dock.

use crate::app::{App, FocusTarget};
use crate::ui::{dock, file_tree, terminal_pane};
use forge_core::types::AgentId;
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Paragraph};
use std::collections::HashMap;

/// Render the main layout.
pub fn render(frame: &mut Frame, app: &App) {
    let area = frame.area();

    // Split: main area (top) + dock (bottom, 1 line + border)
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(3)])
        .split(area);

    let main_area = vertical[0];
    let dock_area = vertical[1];

    // Build horizontal constraints based on visible panes
    let mut constraints = Vec::new();
    let mut pane_types = Vec::new();

    if app.file_tree_visible {
        constraints.push(Constraint::Length(app.config.ui.file_tree_width));
        pane_types.push(PaneKind::FileTree);
    }

    // Center pane always exists
    constraints.push(Constraint::Min(1));
    pane_types.push(PaneKind::Center);

    if app.coordinator_visible {
        constraints.push(Constraint::Length(app.config.ui.coordinator_width));
        pane_types.push(PaneKind::Coordinator);
    }

    let horizontal = Layout::default()
        .direction(Direction::Horizontal)
        .constraints(constraints)
        .split(main_area);

    for (i, pane_type) in pane_types.iter().enumerate() {
        match pane_type {
            PaneKind::FileTree => {
                let focused = app.focus == FocusTarget::FileTree;
                file_tree::render(frame, horizontal[i], &app.workdir, &app.file_tree, focused);
            }
            PaneKind::Center => {
                render_center_pane(frame, horizontal[i], app);
            }
            PaneKind::Coordinator => {
                let focused = app.focus == FocusTarget::Coordinator;
                render_coordinator(frame, horizontal[i], app, focused);
            }
        }
    }

    // Render dock
    dock::render(frame, dock_area, app);
}

/// Compute the actual rendered size (cols, rows) for each agent pane.
/// Returns a map of agent_id -> (cols, rows) including the coordinator.
pub fn compute_pane_sizes(app: &App, total_area: Rect) -> HashMap<String, (u16, u16)> {
    let mut sizes = HashMap::new();

    // Same layout math as render
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(3)])
        .split(total_area);
    let main_area = vertical[0];

    let mut constraints = Vec::new();
    let mut pane_types = Vec::new();

    if app.file_tree_visible {
        constraints.push(Constraint::Length(app.config.ui.file_tree_width));
        pane_types.push(PaneKind::FileTree);
    }
    constraints.push(Constraint::Min(1));
    pane_types.push(PaneKind::Center);
    if app.coordinator_visible {
        constraints.push(Constraint::Length(app.config.ui.coordinator_width));
        pane_types.push(PaneKind::Coordinator);
    }

    let horizontal = Layout::default()
        .direction(Direction::Horizontal)
        .constraints(constraints)
        .split(main_area);

    for (i, pane_type) in pane_types.iter().enumerate() {
        match pane_type {
            PaneKind::Coordinator => {
                if let Some(ref coord_id) = app.coordinator_id {
                    // Inner area = area minus 2 for borders
                    let inner = Rect {
                        x: horizontal[i].x + 1,
                        y: horizontal[i].y + 1,
                        width: horizontal[i].width.saturating_sub(2),
                        height: horizontal[i].height.saturating_sub(2),
                    };
                    sizes.insert(coord_id.as_str().to_string(), (inner.width, inner.height));
                }
            }
            PaneKind::Center => {
                if let Some(root) = app.tiling.root() {
                    collect_tiling_sizes(root, horizontal[i], &mut sizes, &mut 0);
                }
            }
            PaneKind::FileTree => {}
        }
    }

    sizes
}

fn collect_tiling_sizes(
    node: &crate::tiling::TilingNode,
    area: Rect,
    sizes: &mut HashMap<String, (u16, u16)>,
    leaf_index: &mut usize,
) {
    match node {
        crate::tiling::TilingNode::Leaf { agent_id } => {
            if let Some(id) = agent_id {
                // Inner area = area minus 2 for borders
                let inner_w = area.width.saturating_sub(2);
                let inner_h = area.height.saturating_sub(2);
                sizes.insert(id.as_str().to_string(), (inner_w, inner_h));
            }
            *leaf_index += 1;
        }
        crate::tiling::TilingNode::Split {
            direction,
            ratio,
            first,
            second,
        } => {
            let (dir, first_constraint) = match direction {
                crate::tiling::SplitDirection::Horizontal => (
                    Direction::Vertical,
                    Constraint::Percentage((*ratio * 100.0) as u16),
                ),
                crate::tiling::SplitDirection::Vertical => (
                    Direction::Horizontal,
                    Constraint::Percentage((*ratio * 100.0) as u16),
                ),
            };
            let chunks = Layout::default()
                .direction(dir)
                .constraints([
                    first_constraint,
                    Constraint::Percentage(100 - (*ratio * 100.0) as u16),
                ])
                .split(area);

            collect_tiling_sizes(first, chunks[0], sizes, leaf_index);
            collect_tiling_sizes(second, chunks[1], sizes, leaf_index);
        }
    }
}

#[derive(Debug)]
enum PaneKind {
    FileTree,
    Center,
    Coordinator,
}

fn render_center_pane(frame: &mut Frame, area: Rect, app: &App) {
    if !app.tiling.has_panes() {
        // Empty state
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray))
            .title(" Agents ");
        let text = Paragraph::new(
            "No agents running\n\nThe coordinator will spawn agents here.\nOr press Alt+Enter to create one manually.",
        )
        .block(block)
        .alignment(Alignment::Center)
        .style(Style::default().fg(Color::DarkGray));
        frame.render_widget(text, area);
        return;
    }

    // Render tiling layout
    if let Some(root) = app.tiling.root() {
        render_tiling_node(frame, area, root, app, &mut 0);
    }
}

fn render_tiling_node(
    frame: &mut Frame,
    area: Rect,
    node: &crate::tiling::TilingNode,
    app: &App,
    leaf_index: &mut usize,
) {
    match node {
        crate::tiling::TilingNode::Leaf { agent_id } => {
            let focused = *leaf_index == app.tiling.focused_index();
            let term_state = agent_id
                .as_ref()
                .and_then(|id| app.agent_terminal(id));
            terminal_pane::render(frame, area, agent_id.as_ref(), term_state, focused);
            *leaf_index += 1;
        }
        crate::tiling::TilingNode::Split {
            direction,
            ratio,
            first,
            second,
        } => {
            let (dir, first_constraint) = match direction {
                crate::tiling::SplitDirection::Horizontal => (
                    Direction::Vertical,
                    Constraint::Percentage((*ratio * 100.0) as u16),
                ),
                crate::tiling::SplitDirection::Vertical => (
                    Direction::Horizontal,
                    Constraint::Percentage((*ratio * 100.0) as u16),
                ),
            };

            let chunks = Layout::default()
                .direction(dir)
                .constraints([
                    first_constraint,
                    Constraint::Percentage(100 - (*ratio * 100.0) as u16),
                ])
                .split(area);

            render_tiling_node(frame, chunks[0], first, app, leaf_index);
            render_tiling_node(frame, chunks[1], second, app, leaf_index);
        }
    }
}

fn render_coordinator(frame: &mut Frame, area: Rect, app: &App, focused: bool) {
    let term_state = app.coordinator_terminal();

    let border_style = if focused {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .title(" Coordinator ");

    match term_state {
        Some(term) => {
            let inner = block.inner(area);
            frame.render_widget(block, area);
            // Reuse the terminal grid renderer from terminal_pane
            crate::ui::terminal_pane::render_terminal_content(frame, inner, term);
        }
        None => {
            let text = Paragraph::new("Starting coordinator...")
                .block(block)
                .style(Style::default().fg(Color::DarkGray));
            frame.render_widget(text, area);
        }
    }
}
