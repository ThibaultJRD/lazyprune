use crate::app::{App, FocusPanel};
use crate::{format_duration, format_size};
use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};
use std::time::SystemTime;

pub fn render(frame: &mut Frame, app: &App, area: Rect) {
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

    // Check if we're on a group separator
    if let Some(group_info) = app.current_group_info() {
        render_group_details(frame, app, inner, &group_info);
        return;
    }

    let item = match app.current_item() {
        Some(item) => item,
        None => {
            let msg = Paragraph::new(Line::styled(
                "  No item selected",
                Style::default().fg(Color::DarkGray),
            ));
            frame.render_widget(msg, inner);
            return;
        }
    };

    // Split inner area: info top (fixed) + tree bottom (fill)
    let info_height = 11;
    let [info_area, tree_area] =
        Layout::vertical([Constraint::Length(info_height), Constraint::Fill(1)]).areas(inner);

    // --- Enriched info section ---
    let path_display = item.path.to_string_lossy();
    let home = dirs::home_dir().map(|h| h.to_string_lossy().to_string());
    let path_short = match &home {
        Some(h) if path_display.starts_with(h.as_str()) => {
            format!("~{}", &path_display[h.len()..])
        }
        _ => path_display.to_string(),
    };

    let age = item
        .last_modified
        .and_then(|t| SystemTime::now().duration_since(t).ok())
        .map(format_duration)
        .unwrap_or_else(|| "unknown".to_string());

    let tree_data = app.tree_cache.get(&item.path);

    let project_label = tree_data
        .and_then(|d| d.project_type.as_deref())
        .unwrap_or("—");

    let mut info_lines = vec![
        Line::from(vec![
            Span::styled("  Path: ", Style::default().fg(Color::DarkGray)),
            Span::styled(&path_short, Style::default().fg(Color::White)),
        ]),
        Line::from(vec![
            Span::styled("  Type: ", Style::default().fg(Color::DarkGray)),
            Span::styled(&item.target_name, Style::default().fg(Color::Cyan)),
        ]),
        Line::from(vec![
            Span::styled("  Project: ", Style::default().fg(Color::DarkGray)),
            Span::styled(project_label, Style::default().fg(Color::Magenta)),
        ]),
        Line::from(vec![
            Span::styled("  Size: ", Style::default().fg(Color::DarkGray)),
            Span::styled(format_size(item.size), Style::default().fg(Color::Yellow)),
            Span::styled("  ·  Files: ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                format!("{}", item.file_count),
                Style::default().fg(Color::White),
            ),
        ]),
        Line::from(vec![
            Span::styled("  Modified: ", Style::default().fg(Color::DarkGray)),
            Span::styled(format!("{} ago", age), Style::default().fg(Color::White)),
        ]),
        Line::from(""),
    ];

    // Top sub-dirs
    if let Some(data) = tree_data {
        if !data.top_dirs.is_empty() {
            info_lines.push(Line::from(Span::styled(
                "  Largest:",
                Style::default().fg(Color::DarkGray),
            )));
            for (name, size) in &data.top_dirs {
                info_lines.push(Line::from(vec![
                    Span::styled(
                        format!("   {:>8}  ", format_size(*size)),
                        Style::default().fg(Color::Yellow),
                    ),
                    Span::styled(format!("{}/", name), Style::default().fg(Color::White)),
                ]));
            }
        }
    }

    frame.render_widget(Paragraph::new(info_lines), info_area);

    // --- Tree preview section ---
    if tree_area.height < 2 {
        return;
    }

    // Horizontal separator
    let sep_line = "─".repeat(tree_area.width as usize);
    let [sep_area, tree_content_area] =
        Layout::vertical([Constraint::Length(1), Constraint::Fill(1)]).areas(tree_area);
    frame.render_widget(
        Paragraph::new(Line::styled(sep_line, Style::default().fg(Color::DarkGray))),
        sep_area,
    );

    if app.tree_loading {
        let spinner = super::SPINNER_FRAMES[(app.scan_tick as usize) % super::SPINNER_FRAMES.len()];
        let loading = Paragraph::new(Line::from(vec![
            Span::styled(
                format!("  {} ", spinner),
                Style::default().fg(Color::Yellow),
            ),
            Span::styled("Loading...", Style::default().fg(Color::DarkGray)),
        ]));
        frame.render_widget(loading, tree_content_area);
        return;
    }

    let tree_data = match app.tree_cache.get(&item.path) {
        Some(d) => d,
        None => {
            let hint = Paragraph::new(Line::styled(
                "  Press l/→/Enter to load tree",
                Style::default().fg(Color::DarkGray),
            ));
            frame.render_widget(hint, tree_content_area);
            return;
        }
    };

    let tree_lines = render_tree_lines(tree_data);

    // Apply scroll
    let scroll = app.tree_scroll as usize;
    let visible_height = tree_content_area.height as usize;
    let visible_lines: Vec<Line> = tree_lines
        .into_iter()
        .skip(scroll)
        .take(visible_height)
        .collect();

    frame.render_widget(Paragraph::new(visible_lines), tree_content_area);
}

fn render_tree_lines(tree_data: &crate::app::TreeData) -> Vec<Line<'static>> {
    tree_data
        .entries
        .iter()
        .map(|entry| {
            let mut prefix = String::from("  ");
            for &ancestor_is_last in &entry.parent_is_last {
                if ancestor_is_last {
                    prefix.push_str("    ");
                } else {
                    prefix.push_str("│   ");
                }
            }
            if entry.is_last {
                prefix.push_str("└── ");
            } else {
                prefix.push_str("├── ");
            }

            let name_style = if entry.name.starts_with("...") {
                Style::default().fg(Color::DarkGray)
            } else if entry.is_dir {
                Style::default().fg(Color::Cyan)
            } else {
                Style::default().fg(Color::White)
            };

            let display_name = if entry.is_dir && !entry.name.starts_with("...") {
                format!("{}/", entry.name)
            } else {
                entry.name.clone()
            };

            Line::from(vec![
                Span::styled(prefix, Style::default().fg(Color::DarkGray)),
                Span::styled(display_name, name_style),
            ])
        })
        .collect()
}

fn render_group_details(
    frame: &mut Frame,
    app: &App,
    inner: Rect,
    group_info: &crate::app::GroupInfo,
) {
    let home = dirs::home_dir().map(|h| h.to_string_lossy().to_string());
    let path_display = group_info.path.to_string_lossy();
    let path_short = match &home {
        Some(h) if path_display.starts_with(h.as_str()) => {
            format!("~{}", &path_display[h.len()..])
        }
        _ => path_display.to_string(),
    };

    let info_height = (7 + group_info.targets.len()) as u16;
    let [info_area, tree_area] =
        Layout::vertical([Constraint::Length(info_height), Constraint::Fill(1)]).areas(inner);

    let mut info_lines = vec![
        Line::from(vec![
            Span::styled("  Project: ", Style::default().fg(Color::DarkGray)),
            Span::styled(&group_info.name, Style::default().fg(Color::Magenta)),
        ]),
        Line::from(vec![
            Span::styled("  Path: ", Style::default().fg(Color::DarkGray)),
            Span::styled(&path_short, Style::default().fg(Color::White)),
        ]),
        Line::from(vec![
            Span::styled("  Size: ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                format_size(group_info.total_size),
                Style::default().fg(Color::Yellow),
            ),
        ]),
        Line::from(vec![
            Span::styled("  Targets: ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                format!("{}", group_info.targets.len()),
                Style::default().fg(Color::White),
            ),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            "  Contents:",
            Style::default().fg(Color::DarkGray),
        )),
    ];

    for (target_name, rel_path, size) in &group_info.targets {
        let size_text = format_size(*size);
        info_lines.push(Line::from(vec![
            Span::styled(
                format!("   {:>8}  ", size_text),
                Style::default().fg(Color::Yellow),
            ),
            Span::styled(target_name, Style::default().fg(Color::Cyan)),
            Span::styled(
                format!("  {}", rel_path),
                Style::default().fg(Color::DarkGray),
            ),
        ]));
    }

    frame.render_widget(Paragraph::new(info_lines), info_area);

    // Tree section
    if tree_area.height < 2 {
        return;
    }

    let sep_line = "─".repeat(tree_area.width as usize);
    let [sep_area, tree_content_area] =
        Layout::vertical([Constraint::Length(1), Constraint::Fill(1)]).areas(tree_area);
    frame.render_widget(
        Paragraph::new(Line::styled(sep_line, Style::default().fg(Color::DarkGray))),
        sep_area,
    );

    if app.tree_loading {
        let spinner = super::SPINNER_FRAMES[(app.scan_tick as usize) % super::SPINNER_FRAMES.len()];
        let loading = Paragraph::new(Line::from(vec![
            Span::styled(
                format!("  {} ", spinner),
                Style::default().fg(Color::Yellow),
            ),
            Span::styled("Loading...", Style::default().fg(Color::DarkGray)),
        ]));
        frame.render_widget(loading, tree_content_area);
        return;
    }

    let tree_data = match app.tree_cache.get(&group_info.path) {
        Some(d) => d,
        None => {
            let hint = Paragraph::new(Line::styled(
                "  Loading tree...",
                Style::default().fg(Color::DarkGray),
            ));
            frame.render_widget(hint, tree_content_area);
            return;
        }
    };

    let tree_lines = render_tree_lines(tree_data);

    let scroll = app.tree_scroll as usize;
    let visible_height = tree_content_area.height as usize;
    let visible_lines: Vec<Line> = tree_lines
        .into_iter()
        .skip(scroll)
        .take(visible_height)
        .collect();

    frame.render_widget(Paragraph::new(visible_lines), tree_content_area);
}
