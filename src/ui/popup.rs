use crate::app::{App, Tool};
use crate::format_size;
use ratatui::{
    layout::{Constraint, Flex, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
    Frame,
};

fn popup_area(area: Rect, width: u16, height: u16) -> Rect {
    let [area] = Layout::vertical([Constraint::Length(height)])
        .flex(Flex::Center)
        .areas(area);
    let [area] = Layout::horizontal([Constraint::Length(width)])
        .flex(Flex::Center)
        .areas(area);
    area
}

fn shorten_path(path: &std::path::Path) -> String {
    if let Some(home) = dirs::home_dir() {
        if let Ok(stripped) = path.strip_prefix(&home) {
            return format!("~/{}", stripped.display());
        }
    }
    path.display().to_string()
}

pub fn render_confirm(frame: &mut Frame, app: &App) {
    let selected = app.selected_items();
    let count = selected.len();
    let size_label = format_size(app.selected_size());

    let title = format!("Delete {} items? ({})", count, size_label);

    let max_show = 15;
    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::raw(""));

    for (i, item) in selected.iter().enumerate() {
        if i >= max_show {
            lines.push(Line::styled(
                format!("  ... and {} more", count - max_show),
                Style::default().fg(Color::DarkGray),
            ));
            break;
        }
        let short = shorten_path(&item.path);
        lines.push(Line::styled(
            format!("  {}", short),
            Style::default().fg(Color::White),
        ));
    }

    lines.push(Line::raw(""));
    lines.push(Line::from(vec![
        Span::styled("  [Enter]", Style::default().fg(Color::Yellow)),
        Span::raw(" Confirm  "),
        Span::styled("[Esc]", Style::default().fg(Color::Yellow)),
        Span::raw(" Cancel"),
    ]));

    // Calculate width: adapt to longest path
    let max_line_len = lines
        .iter()
        .map(|l| l.spans.iter().map(|s| s.content.len()).sum::<usize>())
        .max()
        .unwrap_or(0) as u16;
    let title_len = title.len() as u16 + 4; // borders + padding
    let terminal_width = frame.area().width;
    let width = max_line_len
        .max(title_len)
        .clamp(45, terminal_width.saturating_sub(10));
    let height = lines.len() as u16 + 2; // +2 for borders

    let area = popup_area(frame.area(), width, height);
    frame.render_widget(Clear, area);

    let block = Block::default()
        .title(format!(" {} ", title))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Red));

    let paragraph = Paragraph::new(lines).block(block);
    frame.render_widget(paragraph, area);
}

pub fn render_processing(frame: &mut Frame, app: &App) {
    let progress = app.prune.delete_progress;
    let total = app.prune.delete_total;
    let pct = if total > 0 {
        (progress as f64 / total as f64 * 100.0) as u16
    } else {
        0
    };

    let width: u16 = 50;
    let height: u16 = 7;
    let area = popup_area(frame.area(), width, height);
    frame.render_widget(Clear, area);

    let title = format!(" Deleting... {}/{} ({pct}%) ", progress, total);

    // Progress bar
    let bar_width = (width - 6) as usize; // minus borders and padding
    let filled = (bar_width as f64 * progress as f64 / total.max(1) as f64) as usize;
    let empty = bar_width - filled;
    let bar = format!("  {}{}", "█".repeat(filled), "░".repeat(empty));

    // Current file being deleted
    let current = if app.prune.delete_current_path.is_empty() {
        String::new()
    } else {
        let short = shorten_path(std::path::Path::new(&app.prune.delete_current_path));
        format!("  {}", short)
    };

    let lines = vec![
        Line::from(""),
        Line::styled(bar, Style::default().fg(Color::Red)),
        Line::from(""),
        Line::styled(current, Style::default().fg(Color::DarkGray)),
    ];

    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Red));

    frame.render_widget(Paragraph::new(lines).block(block), area);
}

pub fn render_sub_filter(frame: &mut Frame, app: &App) {
    let type_count = app.prune.available_types.len();
    // "All" + each type + blank line + instructions line
    let height = (type_count as u16) + 1 + 2 + 1; // +1 for "All", +2 borders, +1 instructions

    let width: u16 = 30;
    let area = popup_area(frame.area(), width, height);
    frame.render_widget(Clear, area);

    let mut lines: Vec<Line> = Vec::new();

    // "All" option at cursor 0
    let all_marker = if app.prune.type_filter_cursor == 0 {
        "> "
    } else {
        "  "
    };
    let all_style = if app.prune.type_filter.is_none() {
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::White)
    };
    lines.push(Line::styled(format!("{}All", all_marker), all_style));

    // Each type
    for (i, t) in app.prune.available_types.iter().enumerate() {
        let cursor_idx = i + 1;
        let marker = if app.prune.type_filter_cursor == cursor_idx {
            "> "
        } else {
            "  "
        };
        let style = if app.prune.type_filter.as_deref() == Some(t.as_str()) {
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::White)
        };
        lines.push(Line::styled(format!("{}{}", marker, t), style));
    }

    lines.push(Line::styled(
        "Enter: select  Esc: close",
        Style::default().fg(Color::DarkGray),
    ));

    let block = Block::default()
        .title(" Filter by type ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    let paragraph = Paragraph::new(lines).block(block);
    frame.render_widget(paragraph, area);
}

pub fn render_help(frame: &mut Frame, app: &App) {
    let lines = if app.active_tool == Tool::Ports {
        let width: u16 = 50;
        let height: u16 = 18;
        let area = popup_area(frame.area(), width, height);
        frame.render_widget(Clear, area);

        let lines = vec![
            Line::raw(""),
            help_line("j/k \u{2191}/\u{2193}", "Navigate"),
            help_line("g/G", "Jump top/bottom"),
            help_line("Space", "Toggle selection"),
            help_line("v", "Invert selection"),
            help_line("Ctrl+a", "Select all"),
            help_line("d", "Kill selected"),
            help_line("/", "Filter by port/process"),
            help_line("s", "Cycle sort"),
            help_line("t", "Filter by protocol"),
            help_line("a", "Toggle dev filter"),
            help_line("r", "Refresh port list"),
            help_line("Tab", "Switch to Prune mode"),
            help_line("?", "Toggle help"),
            help_line("q", "Quit"),
            Line::raw(""),
        ];

        let block = Block::default()
            .title(" Help — Ports ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan));

        let paragraph = Paragraph::new(lines).block(block);
        frame.render_widget(paragraph, area);
        return;
    } else {
        vec![
            Line::raw(""),
            help_line("j/k \u{2191}/\u{2193}", "Navigate"),
            help_line("g/G", "Jump top/bottom"),
            help_line("Space", "Toggle selection"),
            help_line("v", "Invert selection"),
            help_line("Ctrl+a", "Select all"),
            help_line("d", "Delete selected"),
            help_line("/", "Filter by path"),
            help_line("s", "Cycle sort (size/name/date)"),
            help_line("p", "Toggle project grouping"),
            help_line("t", "Filter by type"),
            help_line("l/\u{2192}/Enter", "Open details panel"),
            help_line("h/\u{2190}/Esc", "Back to list"),
            help_line("y", "Copy path (in details)"),
            help_line("Tab", "Switch to Ports mode"),
            help_line("?", "Toggle help"),
            help_line("q", "Quit"),
            Line::raw(""),
        ]
    };

    let width: u16 = 50;
    let height: u16 = lines.len() as u16 + 2;
    let area = popup_area(frame.area(), width, height);
    frame.render_widget(Clear, area);

    let block = Block::default()
        .title(" Help ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    let paragraph = Paragraph::new(lines).block(block);
    frame.render_widget(paragraph, area);
}

pub fn render_kill_confirm(frame: &mut Frame, app: &App) {
    let ports = match app.ports.as_ref() {
        Some(p) => p,
        None => return,
    };

    let selected = ports.selected_items();
    let count = selected.len();

    let max_show = 15;
    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::raw(""));

    for (i, item) in selected.iter().enumerate() {
        if i >= max_show {
            lines.push(Line::styled(
                format!("  ... and {} more", count - max_show),
                Style::default().fg(Color::DarkGray),
            ));
            break;
        }

        // Warn if owned by another user
        let current_user = std::env::var("USER").unwrap_or_default();
        let warn = if !current_user.is_empty() && item.user != current_user {
            " ⚠ "
        } else {
            "   "
        };

        lines.push(Line::from(vec![
            Span::styled(warn, Style::default().fg(Color::Yellow)),
            Span::styled(
                format!(":{}", item.port),
                Style::default().fg(Color::Yellow),
            ),
            Span::raw("  "),
            Span::styled(&item.process_name, Style::default().fg(Color::White)),
            Span::styled(
                format!("  ({})", item.user),
                Style::default().fg(Color::DarkGray),
            ),
        ]));
    }

    lines.push(Line::raw(""));
    lines.push(Line::from(vec![
        Span::styled("  [Enter]", Style::default().fg(Color::Yellow)),
        Span::raw(" Confirm  "),
        Span::styled("[Esc]", Style::default().fg(Color::Yellow)),
        Span::raw(" Cancel"),
    ]));

    let title = format!(" Kill {count} process{}? ", if count == 1 { "" } else { "es" });

    let max_line_len = lines
        .iter()
        .map(|l| l.spans.iter().map(|s| s.content.len()).sum::<usize>())
        .max()
        .unwrap_or(0) as u16;
    let title_len = title.len() as u16 + 4;
    let terminal_width = frame.area().width;
    let width = max_line_len
        .max(title_len)
        .clamp(45, terminal_width.saturating_sub(10));
    let height = lines.len() as u16 + 2;

    let area = popup_area(frame.area(), width, height);
    frame.render_widget(Clear, area);

    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Red));

    let paragraph = Paragraph::new(lines).block(block);
    frame.render_widget(paragraph, area);
}

pub fn render_killing(frame: &mut Frame, app: &App) {
    let ports = match app.ports.as_ref() {
        Some(p) => p,
        None => return,
    };

    let progress = ports.kill_progress;
    let total = ports.kill_total;
    let pct = if total > 0 {
        (progress as f64 / total as f64 * 100.0) as u16
    } else {
        0
    };

    let width: u16 = 50;
    let height: u16 = 7;
    let area = popup_area(frame.area(), width, height);
    frame.render_widget(Clear, area);

    let title = format!(" Killing... {}/{} ({pct}%) ", progress, total);

    // Progress bar
    let bar_width = (width - 6) as usize;
    let filled = (bar_width as f64 * progress as f64 / total.max(1) as f64) as usize;
    let empty = bar_width - filled;
    let bar = format!("  {}{}", "\u{2588}".repeat(filled), "\u{2591}".repeat(empty));

    let current = if ports.kill_current.is_empty() {
        String::new()
    } else {
        format!("  {}", ports.kill_current)
    };

    let mut lines = vec![
        Line::from(""),
        Line::styled(bar, Style::default().fg(Color::Red)),
        Line::from(""),
        Line::styled(current, Style::default().fg(Color::DarkGray)),
    ];

    // Show first error if any
    if let Some(err) = ports.kill_errors.first() {
        lines.push(Line::styled(
            format!("  Error: {err}"),
            Style::default().fg(Color::Red),
        ));
    }

    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Red));

    frame.render_widget(Paragraph::new(lines).block(block), area);
}

pub fn render_protocol_filter(frame: &mut Frame, app: &App) {
    let ports = match app.ports.as_ref() {
        Some(p) => p,
        None => return,
    };

    let options = ["All", "TCP", "UDP"];
    let height: u16 = options.len() as u16 + 2 + 1; // options + borders + instructions
    let width: u16 = 30;
    let area = popup_area(frame.area(), width, height);
    frame.render_widget(Clear, area);

    let mut lines: Vec<Line> = Vec::new();

    for (i, label) in options.iter().enumerate() {
        let marker = if ports.protocol_filter_cursor == i {
            "> "
        } else {
            "  "
        };

        let is_active = match i {
            0 => ports.protocol_filter.is_none(),
            1 => ports.protocol_filter == Some(crate::ports::Protocol::Tcp),
            2 => ports.protocol_filter == Some(crate::ports::Protocol::Udp),
            _ => false,
        };

        let style = if is_active {
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::White)
        };

        lines.push(Line::styled(format!("{}{}", marker, label), style));
    }

    lines.push(Line::styled(
        "Enter: select  Esc: close",
        Style::default().fg(Color::DarkGray),
    ));

    let block = Block::default()
        .title(" Filter by protocol ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    let paragraph = Paragraph::new(lines).block(block);
    frame.render_widget(paragraph, area);
}

fn help_line<'a>(key: &'a str, desc: &'a str) -> Line<'a> {
    Line::from(vec![
        Span::styled(format!("  {:14}", key), Style::default().fg(Color::Cyan)),
        Span::styled(desc, Style::default().fg(Color::White)),
    ])
}
