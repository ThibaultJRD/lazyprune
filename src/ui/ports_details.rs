use crate::app::{App, FocusPanel};
use crate::ports::Protocol;
use ratatui::{
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};

pub fn render(frame: &mut Frame, area: Rect, app: &App) {
    let border_color = if app.focus == FocusPanel::Details {
        Color::Cyan
    } else {
        Color::DarkGray
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Details ")
        .border_style(Style::default().fg(border_color));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let ports = match app.ports.as_ref() {
        Some(p) => p,
        None => {
            frame.render_widget(
                Paragraph::new(Line::styled(
                    "  No ports data",
                    Style::default().fg(Color::DarkGray),
                )),
                inner,
            );
            return;
        }
    };

    let item = match ports.current_item() {
        Some(item) => item,
        None => {
            frame.render_widget(
                Paragraph::new(Line::styled(
                    "  No item selected",
                    Style::default().fg(Color::DarkGray),
                )),
                inner,
            );
            return;
        }
    };

    let proto_str = match item.protocol {
        Protocol::Tcp => "TCP",
        Protocol::Udp => "UDP",
    };

    let state_color = if item.state == "LISTEN" {
        Color::Green
    } else {
        Color::White
    };

    let lines = vec![
        Line::from(vec![
            Span::styled("  Port:         ", Style::default().fg(Color::DarkGray)),
            Span::styled(item.port.to_string(), Style::default().fg(Color::Yellow)),
        ]),
        Line::from(vec![
            Span::styled("  Protocol:     ", Style::default().fg(Color::DarkGray)),
            Span::styled(proto_str, Style::default().fg(Color::Cyan)),
        ]),
        Line::from(vec![
            Span::styled("  State:        ", Style::default().fg(Color::DarkGray)),
            Span::styled(&item.state, Style::default().fg(state_color)),
        ]),
        Line::from(vec![
            Span::styled("  PID:          ", Style::default().fg(Color::DarkGray)),
            Span::styled(item.pid.to_string(), Style::default().fg(Color::White)),
        ]),
        Line::from(vec![
            Span::styled("  Process:      ", Style::default().fg(Color::DarkGray)),
            Span::styled(&item.process_name, Style::default().fg(Color::White)),
        ]),
        Line::from(vec![
            Span::styled("  Command:      ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                if item.command.is_empty() {
                    "—".to_string()
                } else {
                    item.command.clone()
                },
                Style::default().fg(Color::White),
            ),
        ]),
        Line::from(vec![
            Span::styled("  User:         ", Style::default().fg(Color::DarkGray)),
            Span::styled(&item.user, Style::default().fg(Color::White)),
        ]),
        Line::from(vec![
            Span::styled("  Connections:  ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                item.connections.to_string(),
                Style::default().fg(Color::White),
            ),
        ]),
    ];

    frame.render_widget(Paragraph::new(lines), inner);
}
