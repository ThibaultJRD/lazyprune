use crate::app::{App, FocusPanel};
use crate::ports::Protocol;
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem},
    Frame,
};

pub fn render(frame: &mut Frame, area: Rect, app: &mut App) {
    if app.ports.is_none() {
        let block = Block::default()
            .borders(Borders::ALL)
            .title(" Ports ")
            .border_style(Style::default().fg(Color::DarkGray));
        frame.render_widget(block, area);
        return;
    }

    // Build list items from an immutable borrow first
    let (items, item_count) = {
        let ports = app.ports.as_ref().unwrap();
        let selected_pos = ports.list_state.selected();

        let items: Vec<ListItem> = ports
            .filtered_indices
            .iter()
            .enumerate()
            .map(|(pos, &idx)| {
                let info = &ports.items[idx];
                let is_selected = ports.selected.get(idx).copied().unwrap_or(false);
                let is_highlighted = selected_pos == Some(pos);

                let marker = if is_selected { "● " } else { "  " };
                let marker_style = if is_selected {
                    Style::default().fg(Color::Cyan)
                } else {
                    Style::default().fg(Color::DarkGray)
                };

                let proto_str = match info.protocol {
                    Protocol::Tcp => "TCP",
                    Protocol::Udp => "UDP",
                };

                let state_style = if info.state == "LISTEN" {
                    Style::default().fg(Color::Green)
                } else {
                    Style::default().fg(Color::DarkGray)
                };

                let process_color = if is_highlighted {
                    Color::White
                } else {
                    Color::Gray
                };

                let line = Line::from(vec![
                    Span::styled(marker, marker_style),
                    Span::styled(
                        format!("{:>5}", info.port),
                        Style::default()
                            .fg(Color::Yellow)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::raw("  "),
                    Span::styled(
                        format!("{:<3}", proto_str),
                        Style::default().fg(Color::Cyan),
                    ),
                    Span::raw("  "),
                    Span::styled(
                        format!("{:>6}", info.pid),
                        Style::default().fg(Color::DarkGray),
                    ),
                    Span::raw("  "),
                    Span::styled(
                        format!("{:<20}", truncate(&info.process_name, 20)),
                        Style::default().fg(process_color),
                    ),
                    Span::raw("  "),
                    Span::styled(info.state.clone(), state_style),
                ]);

                ListItem::new(line)
            })
            .collect();

        let count = ports.filtered_indices.len();
        (items, count)
    };

    let title = format!(" Ports ({item_count}) ");
    let border_color = if app.focus == FocusPanel::List {
        Color::Cyan
    } else {
        Color::DarkGray
    };

    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(title)
                .border_style(Style::default().fg(border_color)),
        )
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        );

    frame.render_stateful_widget(list, area, &mut app.ports.as_mut().unwrap().list_state);
}

fn truncate(s: &str, max_chars: usize) -> &str {
    match s.char_indices().nth(max_chars) {
        None => s,
        Some((idx, _)) => &s[..idx],
    }
}
