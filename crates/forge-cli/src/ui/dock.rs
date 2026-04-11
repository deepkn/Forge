//! Bottom dock — shows all subagents with status indicators.

use crate::app::App;
use forge_core::types::{AgentColor, AgentState};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Tabs};

/// Render the bottom dock bar.
pub fn render(frame: &mut Frame, area: Rect, app: &App) {
    let dock_title = if app.command_mode {
        " CMD: q=quit h/l=focus e=tree n=new-agent s/v=split z=zoom w=close f=full "
    } else {
        " Agents | Esc=command mode "
    };

    let border_color = if app.command_mode {
        Color::Yellow
    } else {
        Color::DarkGray
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color))
        .title(dock_title);

    if app.agents.is_empty() {
        let tabs = Tabs::new(vec!["No agents running"])
            .block(block)
            .style(Style::default().fg(Color::DarkGray));
        frame.render_widget(tabs, area);
        return;
    }

    let titles: Vec<Line> = app
        .agents
        .values()
        .enumerate()
        .map(|(i, agent)| {
            let state_icon = match agent.state {
                AgentState::Starting => "...",
                AgentState::Running => "▶",
                AgentState::Waiting => "●",
                AgentState::Done => "✓",
                AgentState::Error => "✗",
            };

            let state_color = match agent.state {
                AgentState::Starting => Color::DarkGray,
                AgentState::Running => Color::Green,
                AgentState::Waiting => Color::Red,
                AgentState::Done => Color::Green,
                AgentState::Error => Color::Red,
            };

            let agent_color = agent_color_to_ratatui(agent.color);

            Line::from(vec![
                Span::styled(format!("{} ", i + 1), Style::default().fg(Color::DarkGray)),
                Span::styled(
                    format!("{}:", &agent.label),
                    Style::default().fg(agent_color),
                ),
                Span::styled(
                    format!("{}", state_icon),
                    Style::default().fg(state_color),
                ),
            ])
        })
        .collect();

    let tabs = Tabs::new(titles)
        .block(block)
        .divider(Span::raw(" │ "));

    frame.render_widget(tabs, area);
}

fn agent_color_to_ratatui(color: AgentColor) -> Color {
    match color {
        AgentColor::Blue => Color::Blue,
        AgentColor::Green => Color::Green,
        AgentColor::Yellow => Color::Yellow,
        AgentColor::Red => Color::Red,
        AgentColor::Magenta => Color::Magenta,
        AgentColor::Cyan => Color::Cyan,
        AgentColor::Orange => Color::Rgb(255, 165, 0),
        AgentColor::Purple => Color::Rgb(128, 0, 255),
    }
}
