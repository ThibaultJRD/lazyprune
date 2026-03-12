use std::time::SystemTime;

use crate::app::App;
use crate::{format_duration, format_size};
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem},
    Frame,
};

/// Shorten a path to show ~/ prefix and last 2-3 components.
/// e.g. /home/alice/projects/old-project -> ~/projects/old-project
fn shorten_path(path: Option<&std::path::Path>) -> String {
    let path = match path {
        Some(p) => p,
        None => return "?".to_string(),
    };

    let path_str = path.to_string_lossy();
    let home = dirs::home_dir().map(|h| h.to_string_lossy().to_string());

    // Replace home dir with ~
    let shortened = match &home {
        Some(h) if path_str.starts_with(h.as_str()) => {
            format!("~{}", &path_str[h.len()..])
        }
        _ => path_str.to_string(),
    };

    // If still too long, keep ~ prefix + last 2 components
    let parts: Vec<&str> = shortened.split('/').filter(|s| !s.is_empty()).collect();
    if parts.len() > 4 {
        let tail = &parts[parts.len() - 2..];
        format!("~/…/{}", tail.join("/"))
    } else {
        shortened
    }
}

pub fn render(frame: &mut Frame, app: &mut App, area: Rect) {
    let selected_pos = app.list_state.selected();

    let items: Vec<ListItem> = app
        .filtered_indices
        .iter()
        .enumerate()
        .map(|(pos, &idx)| {
            // Render separator line
            if app.group_separators.contains(&pos) {
                let next_idx = app
                    .filtered_indices
                    .iter()
                    .skip(pos + 1)
                    .find(|&&i| i != usize::MAX);
                let (label, group_size) = match next_idx {
                    Some(&item_idx) => {
                        let item = &app.items[item_idx];
                        let project_label = item
                            .git_root
                            .as_ref()
                            .and_then(|p| p.file_name())
                            .and_then(|n| n.to_str())
                            .map(|s| s.to_string())
                            .unwrap_or_else(|| shorten_path(item.path.parent()));

                        let mut size = 0u64;
                        let mut count = 0usize;
                        let mut all_ready = true;
                        for i in (pos + 1)..app.filtered_indices.len() {
                            if app.group_separators.contains(&i) {
                                break;
                            }
                            let gi = app.filtered_indices[i];
                            size += app.items[gi].size;
                            count += 1;
                            if !app.items[gi].size_ready {
                                all_ready = false;
                            }
                        }
                        let size_label = if all_ready {
                            crate::format_size(size)
                        } else {
                            super::SPINNER_FRAMES[(app.scan_tick as usize) % super::SPINNER_FRAMES.len()].to_string()
                        };
                        (
                            project_label,
                            format!("{} targets, {}", count, size_label),
                        )
                    }
                    None => ("?".to_string(), String::new()),
                };
                let sep_text = format!("── {} ({}) ──", label, group_size);
                return ListItem::new(Line::styled(sep_text, Style::default().fg(Color::Cyan)));
            }

            let item = &app.items[idx];
            let is_selected = app.selected[idx];
            let is_highlighted = selected_pos == Some(pos);

            let marker = if is_selected { "● " } else { "  " };
            let size_str = if item.size_ready {
                format_size(item.size)
            } else {
                super::SPINNER_FRAMES[(app.scan_tick as usize) % super::SPINNER_FRAMES.len()].to_string()
            };
            let dir_name = item
                .path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("?");

            let parent_path = shorten_path(item.path.parent());

            let size_color = if !item.size_ready {
                Color::DarkGray
            } else if item.size >= 1_073_741_824 {
                Color::Red
            } else if item.size >= 524_288_000 {
                Color::Yellow
            } else {
                Color::Green
            };

            let marker_style = if is_selected {
                Style::default().fg(Color::Cyan)
            } else {
                Style::default().fg(Color::DarkGray)
            };

            let parent_color = if is_highlighted {
                Color::Gray
            } else {
                Color::DarkGray
            };

            let date_str = item
                .last_modified
                .and_then(|t| SystemTime::now().duration_since(t).ok())
                .map(format_duration)
                .unwrap_or_else(|| "?".to_string());

            let line = Line::from(vec![
                Span::styled(marker, marker_style),
                Span::styled(
                    format!("{:>8}", size_str),
                    Style::default().fg(size_color).add_modifier(Modifier::BOLD),
                ),
                Span::raw("  "),
                Span::styled(dir_name, Style::default().fg(Color::White)),
                Span::styled(
                    format!("  {:>4}", date_str),
                    Style::default().fg(age_color(item.last_modified)),
                ),
                Span::styled(
                    format!("  ({})", parent_path),
                    Style::default().fg(parent_color),
                ),
            ]);

            ListItem::new(line)
        })
        .collect();

    let item_count = app.filtered_indices.len() - app.group_separators.len();
    let title = format!(" {} items ", item_count);
    let border_color = if app.focus == crate::app::FocusPanel::List {
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

    frame.render_stateful_widget(list, area, &mut app.list_state);
}

fn age_color(last_modified: Option<SystemTime>) -> Color {
    let Some(modified) = last_modified else {
        return Color::White;
    };
    let Ok(elapsed) = SystemTime::now().duration_since(modified) else {
        return Color::White;
    };
    let days = elapsed.as_secs() / 86400;
    if days < 7 {
        Color::Green
    } else if days < 30 {
        Color::White
    } else if days < 90 {
        Color::Yellow
    } else {
        Color::Red
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn test_age_color_none() {
        assert_eq!(age_color(None), Color::White);
    }

    #[test]
    fn test_age_color_recent() {
        let t = SystemTime::now() - Duration::from_secs(86400 * 2);
        assert_eq!(age_color(Some(t)), Color::Green);
    }

    #[test]
    fn test_age_color_week_old() {
        let t = SystemTime::now() - Duration::from_secs(86400 * 14);
        assert_eq!(age_color(Some(t)), Color::White);
    }

    #[test]
    fn test_age_color_stale() {
        let t = SystemTime::now() - Duration::from_secs(86400 * 60);
        assert_eq!(age_color(Some(t)), Color::Yellow);
    }

    #[test]
    fn test_age_color_very_old() {
        let t = SystemTime::now() - Duration::from_secs(86400 * 120);
        assert_eq!(age_color(Some(t)), Color::Red);
    }
}
