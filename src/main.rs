mod app;
mod config;
mod scanner;
mod targets;
mod ui;

use app::{App, AppMode};
use clap::Parser;
use config::Config;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use ratatui::{DefaultTerminal, Frame};
use scanner::ScanMessage;
use std::io;
use std::path::PathBuf;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;
use targets::Target;

#[derive(Parser)]
#[command(
    name = "lazyprune",
    version,
    about = "Scan and delete cache/dependency directories"
)]
struct Cli {
    /// Root directory to scan (default: $HOME)
    path: Option<PathBuf>,

    /// Generate default config file at ~/.config/lazyprune/config.toml
    #[arg(long)]
    init_config: bool,

    /// List findings without TUI (stdout)
    #[arg(short, long)]
    dry_run: bool,

    /// Filter to a specific target type
    #[arg(short, long)]
    target: Option<String>,

    /// Also scan hidden directories (e.g. ~/.cache, ~/.local)
    #[arg(short = 'H', long)]
    hidden: bool,

    /// Scan for any directory with this name (ad-hoc, no config needed)
    #[arg(short = 'D', long, conflicts_with = "target")]
    dir: Option<Vec<String>>,
}

fn main() -> io::Result<()> {
    let cli = Cli::parse();

    // Handle --init-config
    if cli.init_config {
        let path = Config::user_config_path().expect("Could not determine config directory");
        if path.exists() {
            eprintln!("Config already exists at {}", path.display());
            std::process::exit(1);
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, Config::default_config_string())?;
        println!("Config written to {}", path.display());
        return Ok(());
    }

    let config = Config::load(Config::user_config_path().as_deref()).unwrap_or_else(|e| {
        eprintln!("Config error: {e}");
        std::process::exit(1);
    });

    let root = cli.path.unwrap_or_else(|| config.root_path());
    let skip = config.skip.clone();

    let mut targets = if let Some(ref dir_names) = cli.dir {
        dir_names
            .iter()
            .map(|d| Target {
                name: d.clone(),
                dirs: vec![d.clone()],
                indicator: None,
            })
            .collect()
    } else {
        config.targets.clone()
    };

    // Apply --target filter (only when using config targets, not --dir)
    if let Some(ref target_filter) = cli.target {
        let filter_lower = target_filter.to_lowercase();
        targets.retain(|t| {
            t.name.to_lowercase().contains(&filter_lower)
                || t.dirs
                    .iter()
                    .any(|d| d.to_lowercase().contains(&filter_lower))
        });
        if targets.is_empty() {
            eprintln!("No targets matching '{}'", target_filter);
            std::process::exit(1);
        }
    }

    let (tx, rx) = mpsc::channel();

    let hidden = cli.hidden;
    thread::spawn(move || {
        scanner::scan(root, targets, skip, hidden, tx);
    });

    // Handle --dry-run: accumulate results with stats, then print
    if cli.dry_run {
        let mut results: Vec<scanner::ScanResult> = Vec::new();
        for msg in rx {
            match msg {
                ScanMessage::Found(r) => results.push(r),
                ScanMessage::Complete => break,
                _ => {}
            }
        }
        results.sort_by(|a, b| b.size.cmp(&a.size));
        for result in &results {
            println!(
                "{}\t{}\t{}",
                format_size(result.size),
                result.target_name,
                result.path.display()
            );
        }
        return Ok(());
    }

    let mut terminal = ratatui::init();
    let mut app = App::new(rx);
    let result = run(&mut terminal, &mut app);
    ratatui::restore();

    if app.items_deleted > 0 {
        println!(
            "lazyprune: deleted {} items, freed {}",
            app.items_deleted,
            format_size(app.total_deleted),
        );
    }

    result
}

fn run(terminal: &mut DefaultTerminal, app: &mut App) -> io::Result<()> {
    while !app.exit {
        if app.mode == AppMode::Processing {
            app.poll_delete_results();
            terminal.draw(|frame| render(frame, app))?;
            if event::poll(Duration::from_millis(50))? {
                if let Event::Key(key) = event::read()? {
                    if key.kind == crossterm::event::KeyEventKind::Press {
                        // Consume keys during deletion
                    }
                }
            }
            continue;
        }

        app.poll_scan_results();
        app.poll_tree_results();
        app.maybe_start_tree_scan();

        if !app.scan_complete || app.tree_loading {
            app.scan_tick = app.scan_tick.wrapping_add(1);
        }

        terminal.draw(|frame| render(frame, app))?;

        if event::poll(Duration::from_millis(50))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == crossterm::event::KeyEventKind::Press {
                    handle_key(app, key);
                }
            }
        }
    }
    Ok(())
}

fn handle_key(app: &mut App, key: KeyEvent) {
    match app.mode {
        AppMode::Normal => {
            if app.focus == app::FocusPanel::Details {
                handle_details_key(app, key.code);
            } else {
                handle_normal_key(app, key.code, key.modifiers);
            }
        }
        AppMode::Filter => handle_filter_key(app, key.code),
        AppMode::SubFilter => handle_sub_filter_key(app, key.code),
        AppMode::Confirm => handle_confirm_key(app, key.code),
        AppMode::Help => handle_help_key(app, key.code),
        AppMode::Processing => {} // No input during processing
    }
}

fn handle_normal_key(app: &mut App, code: KeyCode, modifiers: KeyModifiers) {
    match code {
        KeyCode::Char('q') | KeyCode::Esc => app.exit = true,
        KeyCode::Char('j') | KeyCode::Down => app.next(),
        KeyCode::Char('k') | KeyCode::Up => app.previous(),
        KeyCode::Char('g') => app.go_top(),
        KeyCode::Char('G') => app.go_bottom(),
        KeyCode::Char(' ') => app.toggle_selection(),
        KeyCode::Char('v') => app.invert_selection(),
        KeyCode::Char('a') if modifiers.contains(KeyModifiers::CONTROL) => app.select_all(),
        KeyCode::Char('s') => app.cycle_sort(),
        KeyCode::Char('p') => app.toggle_project_grouping(),
        KeyCode::Char('/') => app.mode = AppMode::Filter,
        KeyCode::Char('t') => {
            if !app.available_types.is_empty() {
                app.type_filter_cursor = 0;
                app.mode = AppMode::SubFilter;
            }
        }
        KeyCode::Char('d') => {
            if app.selected_count() > 0 {
                app.mode = AppMode::Confirm;
            }
        }
        KeyCode::Char('?') => app.mode = AppMode::Help,
        KeyCode::Char('l') | KeyCode::Right | KeyCode::Enter => {
            if app.current_item().is_some() {
                app.focus = app::FocusPanel::Details;
                app.request_tree_scan();
            } else if app.current_group_info().is_some() {
                app.focus = app::FocusPanel::Details;
                app.request_group_tree_scan();
            }
        }
        _ => {}
    }
}

fn handle_details_key(app: &mut App, code: KeyCode) {
    match code {
        KeyCode::Char('q') => app.exit = true,
        KeyCode::Char('h') | KeyCode::Left | KeyCode::Esc => {
            app.focus = app::FocusPanel::List;
        }
        KeyCode::Char('j') | KeyCode::Down => app.tree_scroll_down(),
        KeyCode::Char('k') | KeyCode::Up => app.tree_scroll_up(),
        KeyCode::Char('g') => app.tree_scroll_top(),
        KeyCode::Char('G') => app.tree_scroll_bottom(20),
        KeyCode::Char('y') => app.copy_path_to_clipboard(),
        _ => {}
    }
}

fn handle_filter_key(app: &mut App, code: KeyCode) {
    match code {
        KeyCode::Esc => {
            app.filter_text.clear();
            app.apply_filter();
            app.mode = AppMode::Normal;
        }
        KeyCode::Enter => {
            app.mode = AppMode::Normal;
        }
        KeyCode::Backspace => {
            app.filter_text.pop();
            app.apply_filter();
        }
        KeyCode::Char(c) => {
            app.filter_text.push(c);
            app.apply_filter();
        }
        _ => {}
    }
}

fn handle_sub_filter_key(app: &mut App, code: KeyCode) {
    match code {
        KeyCode::Esc | KeyCode::Char('t') => {
            app.mode = AppMode::Normal;
        }
        KeyCode::Char('j') | KeyCode::Down => {
            if !app.available_types.is_empty() {
                // cursor 0 = "All", then 1..=len for each type
                let max = app.available_types.len();
                app.type_filter_cursor = (app.type_filter_cursor + 1).min(max);
            }
        }
        KeyCode::Char('k') | KeyCode::Up => {
            app.type_filter_cursor = app.type_filter_cursor.saturating_sub(1);
        }
        KeyCode::Enter => {
            if app.type_filter_cursor == 0 {
                app.type_filter = None;
            } else {
                let idx = app.type_filter_cursor - 1;
                if let Some(t) = app.available_types.get(idx) {
                    app.type_filter = Some(t.clone());
                }
            }
            app.apply_filter();
            app.mode = AppMode::Normal;
        }
        _ => {}
    }
}

fn handle_confirm_key(app: &mut App, code: KeyCode) {
    match code {
        KeyCode::Enter => {
            app.start_deleting();
        }
        KeyCode::Esc => {
            app.mode = AppMode::Normal;
        }
        _ => {}
    }
}

fn handle_help_key(app: &mut App, code: KeyCode) {
    match code {
        KeyCode::Esc | KeyCode::Char('?') | KeyCode::Char('q') => {
            app.mode = AppMode::Normal;
        }
        _ => {}
    }
}

fn render(frame: &mut Frame, app: &mut App) {
    ui::render(frame, app);
}

pub fn format_duration(d: std::time::Duration) -> String {
    let secs = d.as_secs();
    if secs >= 86400 * 365 {
        format!("{}y", secs / (86400 * 365))
    } else if secs >= 86400 * 30 {
        format!("{}mo", secs / (86400 * 30))
    } else if secs >= 86400 {
        format!("{}d", secs / 86400)
    } else if secs >= 3600 {
        format!("{}h", secs / 3600)
    } else {
        format!("{}m", secs / 60)
    }
}

pub fn format_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * KB;
    const GB: u64 = 1024 * MB;

    if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_size_bytes() {
        assert_eq!(format_size(0), "0 B");
        assert_eq!(format_size(500), "500 B");
        assert_eq!(format_size(1023), "1023 B");
    }

    #[test]
    fn test_format_size_kb() {
        assert_eq!(format_size(1024), "1.0 KB");
        assert_eq!(format_size(1536), "1.5 KB");
    }

    #[test]
    fn test_format_size_mb() {
        assert_eq!(format_size(1_048_576), "1.0 MB");
        assert_eq!(format_size(524_288_000), "500.0 MB");
    }

    #[test]
    fn test_format_size_gb() {
        assert_eq!(format_size(1_073_741_824), "1.0 GB");
        assert_eq!(format_size(2_684_354_560), "2.5 GB");
    }

    #[test]
    fn test_cli_parses_dir_option() {
        use clap::Parser;
        let cli = Cli::try_parse_from(["lazyprune", "-D", "src"]).unwrap();
        assert_eq!(cli.dir, Some(vec!["src".to_string()]));
    }

    #[test]
    fn test_cli_parses_multiple_dir_options() {
        use clap::Parser;
        let cli = Cli::try_parse_from(["lazyprune", "-D", "src", "-D", "docs"]).unwrap();
        assert_eq!(cli.dir, Some(vec!["src".to_string(), "docs".to_string()]));
    }

    #[test]
    fn test_cli_dir_and_target_conflict() {
        use clap::Parser;
        let result = Cli::try_parse_from(["lazyprune", "-D", "src", "-t", "node"]);
        assert!(result.is_err());
    }
}
