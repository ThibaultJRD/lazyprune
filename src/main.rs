mod app;
mod config;
mod ports;
mod scanner;
mod targets;
mod ui;

use app::{App, AppMode, Tool};
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

    /// Start in ports mode (list and kill processes by port)
    #[arg(short = 'p', long)]
    ports: bool,
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
        if cli.ports {
            let dev_filter = if config.ports.dev_filter_enabled {
                Some(config::parse_port_filter(&config.ports.dev_filter))
            } else {
                None
            };
            let (ptx, prx) = mpsc::channel();
            ports::scan_ports(ptx, dev_filter);
            let mut port_results: Vec<ports::PortInfo> = Vec::new();
            while let Ok(msg) = prx.recv() {
                match msg {
                    ports::PortScanMessage::Found(info) => port_results.push(info),
                    ports::PortScanMessage::Complete => break,
                    ports::PortScanMessage::Error(e) => eprintln!("Error: {}", e),
                }
            }
            port_results.sort_by_key(|p| p.port);
            println!(
                "{:<8} {:<6} {:<8} {:<16} STATE",
                "PORT", "PROTO", "PID", "PROCESS"
            );
            for p in &port_results {
                let proto = match p.protocol {
                    ports::Protocol::Tcp => "TCP",
                    ports::Protocol::Udp => "UDP",
                };
                println!(
                    "{:<8} {:<6} {:<8} {:<16} {}",
                    p.port, proto, p.pid, p.process_name, p.state
                );
            }
            return Ok(());
        }

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
    let mut app = App::new(rx, config);
    if cli.ports {
        app.active_tool = Tool::Ports;
        app.ensure_ports_initialized();
    }
    let result = run(&mut terminal, &mut app);
    ratatui::restore();

    if app.prune.items_deleted > 0 {
        println!(
            "lazyprune: deleted {} items, freed {}",
            app.prune.items_deleted,
            format_size(app.prune.total_deleted),
        );
    }

    result
}

fn run(terminal: &mut DefaultTerminal, app: &mut App) -> io::Result<()> {
    while !app.exit {
        match app.active_tool {
            Tool::Prune => {
                if app.mode == AppMode::Processing {
                    app.poll_delete_results();
                    terminal.draw(|frame| render(frame, app))?;
                    if event::poll(Duration::from_millis(50))? {
                        if let Event::Key(key) = event::read()? {
                            if key.kind == crossterm::event::KeyEventKind::Press {
                                // Consume keys during deletion
                                let _ = key;
                            }
                        }
                    }
                    continue;
                }

                app.poll_scan_results();
                app.poll_tree_results();
                app.maybe_start_tree_scan();

                if !app.prune.scan_complete || app.prune.tree_loading {
                    app.prune.scan_tick = app.prune.scan_tick.wrapping_add(1);
                }
            }
            Tool::Ports => {
                if app.mode == AppMode::Processing {
                    if let Some(ref mut ports) = app.ports {
                        let done = ports.poll_kill_results();
                        terminal.draw(|frame| render(frame, app))?;
                        if event::poll(Duration::from_millis(50))? {
                            if let Event::Key(key) = event::read()? {
                                if key.kind == crossterm::event::KeyEventKind::Press {
                                    let _ = key;
                                }
                            }
                        }
                        if done {
                            app.mode = AppMode::Normal;
                        }
                    }
                    continue;
                }

                if let Some(ref mut ports) = app.ports {
                    ports.poll_scan_results();
                    if !ports.scan_complete {
                        app.prune.scan_tick = app.prune.scan_tick.wrapping_add(1);
                    }
                }
            }
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
    match app.active_tool {
        Tool::Prune => handle_prune_key(app, key),
        Tool::Ports => handle_ports_key(app, key),
    }
}

// ── Prune key handlers ──────────────────────────────────────────────────────

fn handle_prune_key(app: &mut App, key: KeyEvent) {
    match app.mode {
        AppMode::Normal => {
            if app.focus == app::FocusPanel::Details {
                handle_prune_details_key(app, key.code);
            } else {
                handle_prune_normal_key(app, key.code, key.modifiers);
            }
        }
        AppMode::Filter => handle_prune_filter_key(app, key.code),
        AppMode::SubFilter => handle_prune_sub_filter_key(app, key.code),
        AppMode::Confirm => handle_prune_confirm_key(app, key.code),
        AppMode::Help => handle_prune_help_key(app, key.code),
        AppMode::Processing => {}
    }
}

fn switch_tool(app: &mut App, tool: Tool) {
    app.active_tool = tool;
    app.mode = AppMode::Normal;
    app.focus = app::FocusPanel::List;
    if tool == Tool::Ports {
        app.ensure_ports_initialized();
    }
}

fn handle_prune_normal_key(app: &mut App, code: KeyCode, modifiers: KeyModifiers) {
    match code {
        KeyCode::Char('q') | KeyCode::Esc => app.exit = true,
        KeyCode::Char('j') | KeyCode::Down => app.next(),
        KeyCode::Char('k') | KeyCode::Up => app.previous(),
        KeyCode::Char('g') | KeyCode::Home => app.go_top(),
        KeyCode::Char('G') | KeyCode::End => app.go_bottom(),
        KeyCode::Char(' ') => app.toggle_selection(),
        KeyCode::Char('v') => app.invert_selection(),
        KeyCode::Char('a') if modifiers.contains(KeyModifiers::CONTROL) => app.select_all(),
        KeyCode::Char('s') => app.cycle_sort(),
        KeyCode::Char('p') => app.toggle_project_grouping(),
        KeyCode::Char('/') => app.mode = AppMode::Filter,
        KeyCode::Char('t') => {
            if !app.prune.available_types.is_empty() {
                app.prune.type_filter_cursor = 0;
                app.mode = AppMode::SubFilter;
            }
        }
        KeyCode::Char('d') => {
            if app.selected_count() > 0 {
                app.mode = AppMode::Confirm;
            }
        }
        KeyCode::Tab => {
            let next = match app.active_tool {
                Tool::Prune => Tool::Ports,
                Tool::Ports => Tool::Prune,
            };
            switch_tool(app, next);
        }
        KeyCode::Char('1') => switch_tool(app, Tool::Prune),
        KeyCode::Char('2') => switch_tool(app, Tool::Ports),
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

fn handle_prune_details_key(app: &mut App, code: KeyCode) {
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
        KeyCode::Tab => {
            let next = match app.active_tool {
                Tool::Prune => Tool::Ports,
                Tool::Ports => Tool::Prune,
            };
            switch_tool(app, next);
        }
        KeyCode::Char('1') => switch_tool(app, Tool::Prune),
        KeyCode::Char('2') => switch_tool(app, Tool::Ports),
        _ => {}
    }
}

fn handle_prune_filter_key(app: &mut App, code: KeyCode) {
    match code {
        KeyCode::Esc => {
            app.prune.filter_text.clear();
            app.apply_filter();
            app.mode = AppMode::Normal;
        }
        KeyCode::Enter => {
            app.mode = AppMode::Normal;
        }
        KeyCode::Backspace => {
            app.prune.filter_text.pop();
            app.apply_filter();
        }
        KeyCode::Char(c) => {
            app.prune.filter_text.push(c);
            app.apply_filter();
        }
        _ => {}
    }
}

fn handle_prune_sub_filter_key(app: &mut App, code: KeyCode) {
    match code {
        KeyCode::Esc | KeyCode::Char('t') => {
            app.mode = AppMode::Normal;
        }
        KeyCode::Char('j') | KeyCode::Down => {
            if !app.prune.available_types.is_empty() {
                let max = app.prune.available_types.len();
                app.prune.type_filter_cursor = (app.prune.type_filter_cursor + 1).min(max);
            }
        }
        KeyCode::Char('k') | KeyCode::Up => {
            app.prune.type_filter_cursor = app.prune.type_filter_cursor.saturating_sub(1);
        }
        KeyCode::Enter => {
            if app.prune.type_filter_cursor == 0 {
                app.prune.type_filter = None;
            } else {
                let idx = app.prune.type_filter_cursor - 1;
                if let Some(t) = app.prune.available_types.get(idx) {
                    app.prune.type_filter = Some(t.clone());
                }
            }
            app.apply_filter();
            app.mode = AppMode::Normal;
        }
        _ => {}
    }
}

fn handle_prune_confirm_key(app: &mut App, code: KeyCode) {
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

fn handle_prune_help_key(app: &mut App, code: KeyCode) {
    match code {
        KeyCode::Esc | KeyCode::Char('?') | KeyCode::Char('q') => {
            app.mode = AppMode::Normal;
        }
        _ => {}
    }
}

// ── Ports key handlers ──────────────────────────────────────────────────────

fn handle_ports_key(app: &mut App, key: KeyEvent) {
    match app.mode {
        AppMode::Normal => handle_ports_normal_key(app, key.code, key.modifiers),
        AppMode::Filter => handle_ports_filter_key(app, key.code),
        AppMode::SubFilter => handle_ports_sub_filter_key(app, key.code),
        AppMode::Confirm => handle_ports_confirm_key(app, key.code),
        AppMode::Help => handle_ports_help_key(app, key.code),
        AppMode::Processing => {}
    }
}

fn handle_ports_normal_key(app: &mut App, code: KeyCode, modifiers: KeyModifiers) {
    if app.focus == app::FocusPanel::Details {
        match code {
            KeyCode::Char('h') | KeyCode::Left | KeyCode::Esc => {
                app.focus = app::FocusPanel::List;
            }
            KeyCode::Char('q') => app.exit = true,
            KeyCode::Tab => {
                let next = match app.active_tool {
                    Tool::Prune => Tool::Ports,
                    Tool::Ports => Tool::Prune,
                };
                switch_tool(app, next);
            }
            KeyCode::Char('1') => switch_tool(app, Tool::Prune),
            KeyCode::Char('2') => switch_tool(app, Tool::Ports),
            _ => {}
        }
        return;
    }

    match code {
        KeyCode::Char('q') | KeyCode::Esc => app.exit = true,
        KeyCode::Char('j') | KeyCode::Down => {
            if let Some(ref mut ports) = app.ports {
                ports.next();
            }
        }
        KeyCode::Char('k') | KeyCode::Up => {
            if let Some(ref mut ports) = app.ports {
                ports.previous();
            }
        }
        KeyCode::Char('g') | KeyCode::Home => {
            if let Some(ref mut ports) = app.ports {
                ports.go_top();
            }
        }
        KeyCode::Char('G') | KeyCode::End => {
            if let Some(ref mut ports) = app.ports {
                ports.go_bottom();
            }
        }
        KeyCode::Char(' ') => {
            if let Some(ref mut ports) = app.ports {
                let pos = ports.list_state.selected().unwrap_or(0);
                ports.toggle_selection(pos);
            }
        }
        KeyCode::Char('a') if modifiers.contains(KeyModifiers::CONTROL) => {
            if let Some(ref mut ports) = app.ports {
                ports.select_all();
            }
        }
        KeyCode::Char('v') => {
            if let Some(ref mut ports) = app.ports {
                ports.invert_selection();
            }
        }
        KeyCode::Char('/') => app.mode = AppMode::Filter,
        KeyCode::Char('s') => {
            if let Some(ref mut ports) = app.ports {
                ports.cycle_sort();
            }
        }
        KeyCode::Char('t') => {
            if let Some(ref mut ports) = app.ports {
                ports.protocol_filter_cursor = 0;
            }
            app.mode = AppMode::SubFilter;
        }
        KeyCode::Char('a') => {
            if let Some(ref mut ports) = app.ports {
                ports.dev_filter_active = !ports.dev_filter_active;
                let filter = if ports.dev_filter_active {
                    Some(ports.dev_filter_ports.clone())
                } else {
                    None
                };
                ports.start_scan(filter);
            }
        }
        KeyCode::Char('r') => {
            if let Some(ref mut ports) = app.ports {
                let filter = if ports.dev_filter_active {
                    Some(ports.dev_filter_ports.clone())
                } else {
                    None
                };
                ports.start_scan(filter);
            }
        }
        KeyCode::Char('d') => {
            if let Some(ref ports) = app.ports {
                if ports.selected_count() > 0 {
                    app.mode = AppMode::Confirm;
                }
            }
        }
        KeyCode::Char('l') | KeyCode::Right | KeyCode::Enter => {
            if app
                .ports
                .as_ref()
                .and_then(|p| p.current_item())
                .is_some()
            {
                app.focus = app::FocusPanel::Details;
            }
        }
        KeyCode::Tab => {
            let next = match app.active_tool {
                Tool::Prune => Tool::Ports,
                Tool::Ports => Tool::Prune,
            };
            switch_tool(app, next);
        }
        KeyCode::Char('1') => switch_tool(app, Tool::Prune),
        KeyCode::Char('2') => switch_tool(app, Tool::Ports),
        KeyCode::Char('?') => app.mode = AppMode::Help,
        _ => {}
    }
}

fn handle_ports_filter_key(app: &mut App, code: KeyCode) {
    match code {
        KeyCode::Esc => {
            if let Some(ref mut ports) = app.ports {
                ports.filter_text.clear();
                ports.apply_filter();
            }
            app.mode = AppMode::Normal;
        }
        KeyCode::Enter => {
            app.mode = AppMode::Normal;
        }
        KeyCode::Backspace => {
            if let Some(ref mut ports) = app.ports {
                ports.filter_text.pop();
                ports.apply_filter();
            }
        }
        KeyCode::Char(c) => {
            if let Some(ref mut ports) = app.ports {
                ports.filter_text.push(c);
                ports.apply_filter();
            }
        }
        _ => {}
    }
}

fn handle_ports_sub_filter_key(app: &mut App, code: KeyCode) {
    match code {
        KeyCode::Esc | KeyCode::Char('t') => {
            app.mode = AppMode::Normal;
        }
        KeyCode::Char('j') | KeyCode::Down => {
            if let Some(ref mut ports) = app.ports {
                ports.protocol_filter_cursor = (ports.protocol_filter_cursor + 1).min(2);
            }
        }
        KeyCode::Char('k') | KeyCode::Up => {
            if let Some(ref mut ports) = app.ports {
                ports.protocol_filter_cursor = ports.protocol_filter_cursor.saturating_sub(1);
            }
        }
        KeyCode::Enter => {
            if let Some(ref mut ports) = app.ports {
                match ports.protocol_filter_cursor {
                    0 => ports.protocol_filter = None,
                    1 => ports.protocol_filter = Some(ports::Protocol::Tcp),
                    2 => ports.protocol_filter = Some(ports::Protocol::Udp),
                    _ => {}
                }
                ports.apply_filter();
            }
            app.mode = AppMode::Normal;
        }
        _ => {}
    }
}

fn handle_ports_confirm_key(app: &mut App, code: KeyCode) {
    match code {
        KeyCode::Enter => {
            if let Some(ref mut ports) = app.ports {
                ports.start_killing();
            }
            app.mode = AppMode::Processing;
        }
        KeyCode::Esc => {
            app.mode = AppMode::Normal;
        }
        _ => {}
    }
}

fn handle_ports_help_key(app: &mut App, code: KeyCode) {
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
