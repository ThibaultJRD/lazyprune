pub mod details;
pub mod layout;
pub mod list;
pub mod popup;

use crate::app::{App, AppMode};
use crate::format_size;
use ratatui::{
    layout::{Constraint, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

pub fn render(frame: &mut Frame, app: &mut App) {
    let layout = layout::build(frame.area());

    render_header(frame, app, layout.header);
    list::render(frame, app, layout.list);
    details::render(frame, app, layout.details);
    render_footer(frame, app, layout.footer);

    // Overlays
    match app.mode {
        AppMode::Confirm => popup::render_confirm(frame, app),
        AppMode::Deleting => popup::render_deleting(frame, app),
        AppMode::TypeFilter => popup::render_type_filter(frame, app),
        AppMode::Help => popup::render_help(frame),
        _ => {}
    }
}

fn render_header(frame: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let type_label = match &app.type_filter {
        Some(t) => t.as_str(),
        None => "All",
    };

    let mut spans = vec![
        Span::styled(" [Targets: ", Style::default().fg(Color::DarkGray)),
        Span::styled(type_label, Style::default().fg(Color::Cyan)),
        Span::styled("] ", Style::default().fg(Color::DarkGray)),
        Span::styled("[Sort: ", Style::default().fg(Color::DarkGray)),
        Span::styled(app.sort_mode.label(), Style::default().fg(Color::Cyan)),
        Span::styled("]  ", Style::default().fg(Color::DarkGray)),
    ];

    if app.project_grouping {
        spans.push(Span::styled("[Project \u{2713}] ", Style::default().fg(Color::Cyan)));
    }

    if app.scan_complete {
        // Stats summary: count + size per type, then total
        let total_size: u64 = app.items.iter().map(|i| i.size).sum();
        for (i, type_name) in app.available_types.iter().enumerate() {
            let count = app
                .items
                .iter()
                .filter(|item| item.target_name == *type_name)
                .count();
            let size: u64 = app
                .items
                .iter()
                .filter(|item| item.target_name == *type_name)
                .map(|i| i.size)
                .sum();
            if i > 0 {
                spans.push(Span::styled(" | ", Style::default().fg(Color::DarkGray)));
            }
            spans.push(Span::styled(
                format!("{}: {} \u{00b7} {}", type_name, count, format_size(size)),
                Style::default().fg(Color::White),
            ));
        }
        spans.push(Span::styled(" | ", Style::default().fg(Color::DarkGray)));
        spans.push(Span::styled(
            format!("Total: {}", format_size(total_size)),
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        ));
    } else {
        // Animated spinner
        let frames = [
            "\u{280b}", "\u{2819}", "\u{2839}", "\u{2838}", "\u{283c}", "\u{2834}", "\u{2826}",
            "\u{2827}", "\u{2807}", "\u{280f}",
        ];
        let spinner = frames[(app.scan_tick as usize) % frames.len()];
        spans.push(Span::styled(
            format!("{} Scanning... {} dirs", spinner, app.dirs_scanned),
            Style::default().fg(Color::Yellow),
        ));
        let found = app.items.len();
        if found > 0 {
            spans.push(Span::styled(
                format!(" | {} found", found),
                Style::default().fg(Color::Green),
            ));
        }
    }

    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn render_footer(frame: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let line1 = match app.mode {
        AppMode::Filter => Line::from(vec![
            Span::styled(" /", Style::default().fg(Color::Cyan)),
            Span::raw(&app.filter_text),
            Span::styled("\u{2588}", Style::default().fg(Color::White)),
            Span::styled(
                "  (Enter: apply, Esc: cancel)",
                Style::default().fg(Color::DarkGray),
            ),
        ]),
        _ => Line::from(vec![
            Span::styled(" j/k", Style::default().fg(Color::Cyan)),
            Span::styled(": navigate  ", Style::default().fg(Color::DarkGray)),
            Span::styled("space", Style::default().fg(Color::Cyan)),
            Span::styled(": select  ", Style::default().fg(Color::DarkGray)),
            Span::styled("d", Style::default().fg(Color::Cyan)),
            Span::styled(": delete  ", Style::default().fg(Color::DarkGray)),
            Span::styled("/", Style::default().fg(Color::Cyan)),
            Span::styled(": filter  ", Style::default().fg(Color::DarkGray)),
            Span::styled("s", Style::default().fg(Color::Cyan)),
            Span::styled(": sort  ", Style::default().fg(Color::DarkGray)),
            Span::styled("p", Style::default().fg(Color::Cyan)),
            Span::styled(": group  ", Style::default().fg(Color::DarkGray)),
            Span::styled("?", Style::default().fg(Color::Cyan)),
            Span::styled(": help  ", Style::default().fg(Color::DarkGray)),
            Span::styled("l", Style::default().fg(Color::Cyan)),
            Span::styled(": details  ", Style::default().fg(Color::DarkGray)),
            Span::styled("q", Style::default().fg(Color::Cyan)),
            Span::styled(": quit", Style::default().fg(Color::DarkGray)),
        ]),
    };

    let selected = app.selected_count();
    let line2 = if selected > 0 {
        Line::from(vec![Span::styled(
            format!(
                " Selected: {} ({})",
                selected,
                format_size(app.selected_size())
            ),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )])
    } else {
        Line::from(Span::styled(
            format!(
                " {} total",
                format_size(app.items.iter().map(|i| i.size).sum::<u64>())
            ),
            Style::default().fg(Color::DarkGray),
        ))
    };

    let [line1_area, line2_area] =
        Layout::vertical([Constraint::Length(1), Constraint::Length(1)]).areas(area);

    frame.render_widget(Paragraph::new(line1), line1_area);
    frame.render_widget(Paragraph::new(line2), line2_area);
}
