pub mod details;
pub mod layout;
pub mod list;
pub mod popup;
pub mod ports_details;
pub mod ports_list;

pub const SPINNER_FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

use crate::app::{App, AppMode, Tool};
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

    render_tab_bar(frame, app, layout.tab_bar);
    render_header(frame, app, layout.header);

    match app.active_tool {
        Tool::Prune => {
            list::render(frame, app, layout.list);
            details::render(frame, app, layout.details);
        }
        Tool::Ports => {
            ports_list::render(frame, layout.list, app);
            ports_details::render(frame, layout.details, app);
        }
    }

    render_footer(frame, app, layout.footer);

    // Overlays
    match app.mode {
        AppMode::Confirm => match app.active_tool {
            Tool::Prune => popup::render_confirm(frame, app),
            Tool::Ports => popup::render_kill_confirm(frame, app),
        },
        AppMode::Processing => match app.active_tool {
            Tool::Prune => popup::render_processing(frame, app),
            Tool::Ports => popup::render_killing(frame, app),
        },
        AppMode::SubFilter => match app.active_tool {
            Tool::Prune => popup::render_sub_filter(frame, app),
            Tool::Ports => popup::render_protocol_filter(frame, app),
        },
        AppMode::Help => popup::render_help(frame, app),
        _ => {}
    }
}

fn render_tab_bar(frame: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let active_style = Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD);
    let inactive_style = Style::default().fg(Color::DarkGray);
    let indicator = Span::styled("● ", active_style);
    let no_indicator = Span::styled("  ", inactive_style);

    let (prune_indicator, prune_style, ports_indicator, ports_style) = match app.active_tool {
        Tool::Prune => (
            indicator.clone(),
            active_style,
            no_indicator,
            inactive_style,
        ),
        Tool::Ports => (no_indicator, inactive_style, indicator, active_style),
    };

    let spans = vec![
        Span::raw(" "),
        prune_indicator,
        Span::styled("Prune", prune_style),
        Span::styled("  │  ", Style::default().fg(Color::DarkGray)),
        ports_indicator,
        Span::styled("Ports", ports_style),
        Span::styled("     ", Style::default()),
        Span::styled("Tab/1/2: switch", Style::default().fg(Color::DarkGray)),
    ];

    frame.render_widget(ratatui::widgets::Paragraph::new(Line::from(spans)), area);
}

fn render_header(frame: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let max_width = area.width as usize;

    if app.active_tool == Tool::Ports {
        let mut spans: Vec<Span> = vec![Span::raw(" ")];

        if let Some(ports) = &app.ports {
            let port_count = ports.filtered_indices.len();
            let sort_label = ports.sort_mode.label();
            let dev_filter_str = if ports.dev_filter_active { "ON" } else { "OFF" };
            spans.push(Span::styled(
                format!("{port_count} ports"),
                Style::default().fg(Color::White),
            ));
            spans.push(Span::styled(" │ ", Style::default().fg(Color::DarkGray)));
            spans.push(Span::styled("Sort: ", Style::default().fg(Color::DarkGray)));
            spans.push(Span::styled(sort_label, Style::default().fg(Color::Cyan)));
            spans.push(Span::styled(" │ ", Style::default().fg(Color::DarkGray)));
            spans.push(Span::styled(
                "Dev filter: ",
                Style::default().fg(Color::DarkGray),
            ));
            spans.push(Span::styled(
                dev_filter_str,
                if ports.dev_filter_active {
                    Style::default().fg(Color::Green)
                } else {
                    Style::default().fg(Color::DarkGray)
                },
            ));

            if !ports.scan_complete {
                let spinner = SPINNER_FRAMES[(ports.scan_tick as usize) % SPINNER_FRAMES.len()];
                spans.push(Span::styled(" │ ", Style::default().fg(Color::DarkGray)));
                spans.push(Span::styled(
                    format!("{spinner} Scanning..."),
                    Style::default().fg(Color::Yellow),
                ));
            }
        } else {
            spans.push(Span::styled(
                "Ports not initialized",
                Style::default().fg(Color::DarkGray),
            ));
        }

        frame.render_widget(ratatui::widgets::Paragraph::new(Line::from(spans)), area);
        return;
    }

    let mut spans: Vec<Span> = vec![Span::raw(" ")];

    let type_label = match &app.prune.type_filter {
        Some(t) => t.as_str(),
        None => "All",
    };

    spans.extend(vec![
        Span::styled("[Targets: ", Style::default().fg(Color::DarkGray)),
        Span::styled(type_label, Style::default().fg(Color::Cyan)),
        Span::styled("] ", Style::default().fg(Color::DarkGray)),
        Span::styled("[Sort: ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            app.prune.sort_mode.label(),
            Style::default().fg(Color::Cyan),
        ),
        Span::styled("]  ", Style::default().fg(Color::DarkGray)),
    ]);

    if app.prune.project_grouping {
        spans.push(Span::styled(
            "[Project \u{2713}] ",
            Style::default().fg(Color::Cyan),
        ));
    }

    if app.prune.scan_complete {
        let total_size: u64 = app.prune.items.iter().map(|i| i.size).sum();
        let total_span_text = format!("Total: {}", format_size(total_size));

        // Calculate base width (prefix + total)
        let prefix_width: usize = spans.iter().map(|s| s.content.len()).sum();
        let total_section_width = 3 + total_span_text.len(); // " | " + total text

        let available_for_types = max_width.saturating_sub(prefix_width + total_section_width);

        // Try to fit type stats
        let mut type_spans: Vec<Span> = Vec::new();
        let mut types_shown = 0;
        let mut current_type_width = 0;

        for (i, type_name) in app.prune.available_types.iter().enumerate() {
            let count = app
                .prune
                .items
                .iter()
                .filter(|item| item.target_name == *type_name)
                .count();
            let size: u64 = app
                .prune
                .items
                .iter()
                .filter(|item| item.target_name == *type_name)
                .map(|i| i.size)
                .sum();
            let text = format!("{}: {} \u{00b7} {}", type_name, count, format_size(size));
            let separator_width = if i > 0 { 3 } else { 0 }; // " | "
            let entry_width = separator_width + text.len();

            // Reserve space for potential "... +N more"
            let remaining_types = app.prune.available_types.len() - i - 1;
            let reserve = if remaining_types > 0 { 17 } else { 0 };

            if current_type_width + entry_width + reserve > available_for_types && i > 0 {
                let remaining = app.prune.available_types.len() - i;
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

        if types_shown > 0 || app.prune.available_types.is_empty() {
            spans.push(Span::styled(" | ", Style::default().fg(Color::DarkGray)));
        }
        spans.push(Span::styled(
            total_span_text,
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        ));
    } else {
        let spinner = SPINNER_FRAMES[(app.prune.scan_tick as usize) % SPINNER_FRAMES.len()];
        spans.push(Span::styled(
            format!("{} Scanning... {} dirs", spinner, app.prune.dirs_scanned),
            Style::default().fg(Color::Yellow),
        ));
        let found = app.prune.items.len();
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
    let [line1_area, line2_area] =
        Layout::vertical([Constraint::Length(1), Constraint::Length(1)]).areas(area);

    if app.active_tool == Tool::Ports {
        let line1 = match app.mode {
            AppMode::Filter => Line::from(vec![
                Span::styled(" /", Style::default().fg(Color::Cyan)),
                Span::raw(
                    app.ports
                        .as_ref()
                        .map(|p| p.filter_text.as_str())
                        .unwrap_or(""),
                ),
                Span::styled("\u{2588}", Style::default().fg(Color::White)),
                Span::styled(
                    "  (Enter: apply, Esc: cancel)",
                    Style::default().fg(Color::DarkGray),
                ),
            ]),
            _ => Line::from(vec![
                Span::styled(" Tab", Style::default().fg(Color::Cyan)),
                Span::styled(":toggle  ", Style::default().fg(Color::DarkGray)),
                Span::styled("/", Style::default().fg(Color::Cyan)),
                Span::styled(":filter  ", Style::default().fg(Color::DarkGray)),
                Span::styled("s", Style::default().fg(Color::Cyan)),
                Span::styled(":sort  ", Style::default().fg(Color::DarkGray)),
                Span::styled("t", Style::default().fg(Color::Cyan)),
                Span::styled(":proto  ", Style::default().fg(Color::DarkGray)),
                Span::styled("a", Style::default().fg(Color::Cyan)),
                Span::styled(":dev filter  ", Style::default().fg(Color::DarkGray)),
                Span::styled("d", Style::default().fg(Color::Cyan)),
                Span::styled(":kill  ", Style::default().fg(Color::DarkGray)),
                Span::styled("r", Style::default().fg(Color::Cyan)),
                Span::styled(":refresh  ", Style::default().fg(Color::DarkGray)),
                Span::styled("?", Style::default().fg(Color::Cyan)),
                Span::styled(":help", Style::default().fg(Color::DarkGray)),
            ]),
        };

        let line2 = if let Some(ports) = &app.ports {
            let selected = ports.selected_count();
            if selected > 0 {
                Line::from(vec![Span::styled(
                    format!(" Selected: {selected} ports"),
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                )])
            } else {
                Line::from(Span::styled(
                    format!(" {} total", ports.filtered_indices.len()),
                    Style::default().fg(Color::DarkGray),
                ))
            }
        } else {
            Line::from(Span::styled("", Style::default()))
        };

        frame.render_widget(Paragraph::new(line1), line1_area);
        frame.render_widget(Paragraph::new(line2), line2_area);
        return;
    }

    // Prune footer (original behavior)
    let line1 = match app.mode {
        AppMode::Filter => Line::from(vec![
            Span::styled(" /", Style::default().fg(Color::Cyan)),
            Span::raw(&app.prune.filter_text),
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
                format_size(app.prune.items.iter().map(|i| i.size).sum::<u64>())
            ),
            Style::default().fg(Color::DarkGray),
        ))
    };

    frame.render_widget(Paragraph::new(line1), line1_area);
    frame.render_widget(Paragraph::new(line2), line2_area);
}
