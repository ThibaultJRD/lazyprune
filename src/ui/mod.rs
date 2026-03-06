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
    let max_width = area.width as usize;

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
        spans.push(Span::styled(
            "[Project \u{2713}] ",
            Style::default().fg(Color::Cyan),
        ));
    }

    if app.scan_complete {
        let total_size: u64 = app.items.iter().map(|i| i.size).sum();
        let total_span_text = format!("Total: {}", format_size(total_size));

        // Calculate base width (prefix + total)
        let prefix_width: usize = spans.iter().map(|s| s.content.len()).sum();
        let total_section_width = 3 + total_span_text.len(); // " | " + total text

        let available_for_types = max_width.saturating_sub(prefix_width + total_section_width);

        // Try to fit type stats
        let mut type_spans: Vec<Span> = Vec::new();
        let mut types_shown = 0;
        let mut current_type_width = 0;

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
            let text = format!("{}: {} \u{00b7} {}", type_name, count, format_size(size));
            let separator_width = if i > 0 { 3 } else { 0 }; // " | "
            let entry_width = separator_width + text.len();

            // Reserve space for potential "... +N more"
            let remaining_types = app.available_types.len() - i - 1;
            let reserve = if remaining_types > 0 { 17 } else { 0 };

            if current_type_width + entry_width + reserve > available_for_types && i > 0 {
                let remaining = app.available_types.len() - i;
                type_spans.push(Span::styled(" | ", Style::default().fg(Color::DarkGray)));
                type_spans.push(Span::styled(
                    format!("... +{} more", remaining),
                    Style::default().fg(Color::DarkGray),
                ));
                break;
            }

            if i > 0 {
                type_spans.push(Span::styled(" | ", Style::default().fg(Color::DarkGray)));
            }
            type_spans.push(Span::styled(text, Style::default().fg(Color::White)));
            current_type_width += entry_width;
            types_shown += 1;
        }

        spans.extend(type_spans);

        if types_shown > 0 || app.available_types.is_empty() {
            spans.push(Span::styled(" | ", Style::default().fg(Color::DarkGray)));
        }
        spans.push(Span::styled(
            total_span_text,
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        ));
    } else {
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
            Span::styled(": project  ", Style::default().fg(Color::DarkGray)),
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
