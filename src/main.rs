mod app;
mod config;
mod ports;
mod scanner;
mod targets;
mod ui;

use app::{App, AppMode, FocusPanel, Tool};
use clap::Parser;
use config::Config;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::DefaultTerminal;
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
                indicators: vec![],
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

    // Handle --dry-run --ports before starting the filesystem scan
    if cli.dry_run && cli.ports {
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
        results.sort_by_key(|r| std::cmp::Reverse(r.size));
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
        match (app.active_tool, app.mode == AppMode::Processing) {
            (Tool::Prune, true) => app.poll_delete_results(),
            (Tool::Ports, true) => app.poll_kill_results(),
            (Tool::Prune, false) => {
                app.prune.poll_scan_results();
                app.prune.poll_tree_results();
                app.prune.maybe_start_tree_scan();
                if !app.prune.scan_complete || app.prune.tree_loading {
                    app.prune.scan_tick = app.prune.scan_tick.wrapping_add(1);
                }
            }
            (Tool::Ports, false) => {
                if let Some(ports) = app.ports.as_mut() {
                    ports.poll_scan_results();
                    if !ports.scan_complete {
                        ports.scan_tick = ports.scan_tick.wrapping_add(1);
                    }
                }
            }
        }

        terminal.draw(|frame| ui::render(frame, app))?;

        if event::poll(Duration::from_millis(50))?
            && let Event::Key(key) = event::read()?
            && key.kind == KeyEventKind::Press
        {
            handle_key(app, key);
        }
    }
    Ok(())
}

fn handle_key(app: &mut App, key: KeyEvent) {
    if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
        app.exit = true;
        return;
    }
    match app.mode {
        AppMode::Normal => handle_normal_key(app, key),
        AppMode::Filter => match app.active_tool {
            Tool::Prune => handle_prune_filter_key(app, key.code),
            Tool::Ports => handle_ports_filter_key(app, key.code),
        },
        AppMode::SubFilter => match app.active_tool {
            Tool::Prune => handle_type_filter_key(app, key.code),
            Tool::Ports => handle_protocol_filter_key(app, key.code),
        },
        AppMode::Confirm => handle_confirm_key(app, key.code),
        AppMode::Help => handle_help_key(app, key.code),
        AppMode::Errors => handle_errors_key(app, key.code),
        // Only Ctrl+C is handled while deleting or killing
        AppMode::Processing => {}
    }
}

/// Keys shared by both tools, then the active panel's own keys.
fn handle_normal_key(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Tab => switch_tool(app, app.active_tool.other()),
        KeyCode::Char('1') => switch_tool(app, Tool::Prune),
        KeyCode::Char('2') => switch_tool(app, Tool::Ports),
        KeyCode::Char('q') => app.exit = true,
        KeyCode::Char('h') | KeyCode::Left | KeyCode::Esc if app.focus == FocusPanel::Details => {
            app.focus = FocusPanel::List;
        }
        KeyCode::Esc => app.exit = true,
        _ => match (app.active_tool, app.focus) {
            (Tool::Prune, FocusPanel::List) => handle_prune_list_key(app, key),
            (Tool::Prune, FocusPanel::Details) => handle_prune_details_key(app, key.code),
            (Tool::Ports, FocusPanel::List) => handle_ports_list_key(app, key),
            (Tool::Ports, FocusPanel::Details) => {}
        },
    }
}

fn switch_tool(app: &mut App, tool: Tool) {
    // Clear any active filter on the tool we're leaving
    match app.active_tool {
        Tool::Prune => {
            if !app.prune.filter_text.is_empty() {
                app.prune.filter_text.clear();
                app.prune.apply_filter();
            }
        }
        Tool::Ports => {
            if let Some(ref mut ports) = app.ports
                && !ports.filter_text.is_empty()
            {
                ports.filter_text.clear();
                ports.apply_filter();
            }
        }
    }
    app.active_tool = tool;
    app.mode = AppMode::Normal;
    app.focus = FocusPanel::List;
    if tool == Tool::Ports {
        app.ensure_ports_initialized();
    }
}

fn handle_confirm_key(app: &mut App, code: KeyCode) {
    match code {
        KeyCode::Enter => match app.active_tool {
            Tool::Prune => app.start_deleting(),
            Tool::Ports => app.start_killing(),
        },
        KeyCode::Esc => app.mode = AppMode::Normal,
        _ => {}
    }
}

fn handle_help_key(app: &mut App, code: KeyCode) {
    if matches!(code, KeyCode::Esc | KeyCode::Char('?') | KeyCode::Char('q')) {
        app.mode = AppMode::Normal;
    }
}

fn handle_errors_key(app: &mut App, code: KeyCode) {
    if matches!(code, KeyCode::Enter | KeyCode::Esc | KeyCode::Char('q')) {
        app.mode = AppMode::Normal;
    }
}

// ── Prune key handlers ──────────────────────────────────────────────────────

fn handle_prune_list_key(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Char('j') | KeyCode::Down => app.prune.next(),
        KeyCode::Char('k') | KeyCode::Up => app.prune.previous(),
        KeyCode::Char('g') | KeyCode::Home => app.prune.go_top(),
        KeyCode::Char('G') | KeyCode::End => app.prune.go_bottom(),
        KeyCode::Char(' ') => app.prune.toggle_selection(),
        KeyCode::Char('v') => app.prune.invert_selection(),
        KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.prune.select_all()
        }
        KeyCode::Char('s') => app.prune.cycle_sort(),
        KeyCode::Char('p') => app.prune.toggle_project_grouping(),
        KeyCode::Char('/') => app.mode = AppMode::Filter,
        KeyCode::Char('t') if !app.prune.available_types.is_empty() => {
            app.prune.type_filter_cursor = 0;
            app.mode = AppMode::SubFilter;
        }
        KeyCode::Char('d') if app.prune.selected_count() > 0 => app.mode = AppMode::Confirm,
        KeyCode::Char('?') => app.mode = AppMode::Help,
        KeyCode::Char('l') | KeyCode::Right | KeyCode::Enter => {
            if app.prune.current_item().is_some() {
                app.focus = FocusPanel::Details;
                app.prune.request_tree_scan();
            } else if app.prune.current_group_info().is_some() {
                app.focus = FocusPanel::Details;
                app.prune.request_group_tree_scan();
            }
        }
        _ => {}
    }
}

fn handle_prune_details_key(app: &mut App, code: KeyCode) {
    match code {
        KeyCode::Char('j') | KeyCode::Down => app.prune.tree_scroll_down(),
        KeyCode::Char('k') | KeyCode::Up => app.prune.tree_scroll_up(),
        KeyCode::Char('g') => app.prune.tree_scroll_top(),
        KeyCode::Char('G') => app.prune.tree_scroll_bottom(),
        KeyCode::Char('y') => app.prune.copy_path_to_clipboard(),
        _ => {}
    }
}

fn handle_prune_filter_key(app: &mut App, code: KeyCode) {
    match code {
        KeyCode::Esc => {
            app.prune.filter_text.clear();
            app.prune.apply_filter();
            app.mode = AppMode::Normal;
        }
        KeyCode::Enter => app.mode = AppMode::Normal,
        KeyCode::Backspace => {
            app.prune.filter_text.pop();
            app.prune.apply_filter();
        }
        KeyCode::Char(c) => {
            app.prune.filter_text.push(c);
            app.prune.apply_filter();
        }
        _ => {}
    }
}

fn handle_type_filter_key(app: &mut App, code: KeyCode) {
    match code {
        KeyCode::Esc | KeyCode::Char('t') => app.mode = AppMode::Normal,
        KeyCode::Char('j') | KeyCode::Down if !app.prune.available_types.is_empty() => {
            let max = app.prune.available_types.len();
            app.prune.type_filter_cursor = (app.prune.type_filter_cursor + 1).min(max);
        }
        KeyCode::Char('k') | KeyCode::Up => {
            app.prune.type_filter_cursor = app.prune.type_filter_cursor.saturating_sub(1);
        }
        KeyCode::Enter => {
            // Cursor 0 is "All", then one entry per type
            app.prune.type_filter = match app.prune.type_filter_cursor {
                0 => None,
                n => app.prune.available_types.get(n - 1).cloned(),
            };
            app.prune.apply_filter();
            app.mode = AppMode::Normal;
        }
        _ => {}
    }
}

// ── Ports key handlers ──────────────────────────────────────────────────────

fn handle_ports_list_key(app: &mut App, key: KeyEvent) {
    let Some(ports) = app.ports.as_mut() else {
        return;
    };
    match key.code {
        KeyCode::Char('j') | KeyCode::Down => ports.next(),
        KeyCode::Char('k') | KeyCode::Up => ports.previous(),
        KeyCode::Char('g') | KeyCode::Home => ports.go_top(),
        KeyCode::Char('G') | KeyCode::End => ports.go_bottom(),
        KeyCode::Char(' ') => {
            let pos = ports.list_state.selected().unwrap_or(0);
            ports.toggle_selection(pos);
        }
        KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::CONTROL) => ports.select_all(),
        KeyCode::Char('v') => ports.invert_selection(),
        KeyCode::Char('s') => ports.cycle_sort(),
        KeyCode::Char('a') => {
            ports.dev_filter_active = !ports.dev_filter_active;
            ports.rescan();
        }
        KeyCode::Char('r') => ports.rescan(),
        KeyCode::Char('/') => app.mode = AppMode::Filter,
        KeyCode::Char('t') => {
            ports.protocol_filter_cursor = 0;
            app.mode = AppMode::SubFilter;
        }
        KeyCode::Char('d') if ports.selected_count() > 0 => app.mode = AppMode::Confirm,
        KeyCode::Char('l') | KeyCode::Right | KeyCode::Enter if ports.current_item().is_some() => {
            app.focus = FocusPanel::Details;
        }
        KeyCode::Char('?') => app.mode = AppMode::Help,
        _ => {}
    }
}

fn handle_ports_filter_key(app: &mut App, code: KeyCode) {
    let Some(ports) = app.ports.as_mut() else {
        return;
    };
    match code {
        KeyCode::Esc => {
            ports.filter_text.clear();
            ports.apply_filter();
            app.mode = AppMode::Normal;
        }
        KeyCode::Enter => app.mode = AppMode::Normal,
        KeyCode::Backspace => {
            ports.filter_text.pop();
            ports.apply_filter();
        }
        KeyCode::Char(c) => {
            ports.filter_text.push(c);
            ports.apply_filter();
        }
        _ => {}
    }
}

fn handle_protocol_filter_key(app: &mut App, code: KeyCode) {
    let Some(ports) = app.ports.as_mut() else {
        return;
    };
    match code {
        KeyCode::Esc | KeyCode::Char('t') => app.mode = AppMode::Normal,
        KeyCode::Char('j') | KeyCode::Down => {
            ports.protocol_filter_cursor = (ports.protocol_filter_cursor + 1).min(2);
        }
        KeyCode::Char('k') | KeyCode::Up => {
            ports.protocol_filter_cursor = ports.protocol_filter_cursor.saturating_sub(1);
        }
        KeyCode::Enter => {
            // Same order as the popup: All, TCP, UDP
            ports.protocol_filter = match ports.protocol_filter_cursor {
                1 => Some(ports::Protocol::Tcp),
                2 => Some(ports::Protocol::Udp),
                _ => None,
            };
            ports.apply_filter();
            app.mode = AppMode::Normal;
        }
        _ => {}
    }
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

    use ports::{PortInfo, PortsState, Protocol};

    fn press(app: &mut App, code: KeyCode) {
        handle_key(app, KeyEvent::new(code, KeyModifiers::NONE));
    }

    /// App with two prune items and two ports, no background scans running.
    fn test_app(tool: Tool) -> App {
        let (_tx, rx) = mpsc::channel();
        let mut app = App::new(rx, Config::load(None).unwrap());
        app.prune.items = ["/a/node_modules", "/b/node_modules"]
            .iter()
            .map(|p| scanner::ScanResult {
                path: PathBuf::from(p),
                target_name: "node_modules".into(),
                size: 1,
                shared_size: 0,
                last_modified: None,
                file_count: 1,
                git_root: None,
            })
            .collect();
        app.prune.selected = vec![false; 2];
        app.prune.available_types = vec!["node_modules".into()];
        app.prune.apply_filter();

        let mut ports = PortsState::new();
        ports.items = [(3000, 1), (3001, 2)]
            .iter()
            .map(|&(port, pid)| PortInfo {
                port,
                protocol: Protocol::Tcp,
                pid,
                process_name: "node".into(),
                command: String::new(),
                user: "me".into(),
                state: "LISTEN".into(),
                connections: 0,
            })
            .collect();
        ports.selected = vec![false; 2];
        ports.scan_complete = true;
        ports.apply_filter();
        app.ports = Some(ports);
        app.active_tool = tool;
        app
    }

    #[test]
    fn test_tool_switch_keys_from_every_panel() {
        for tool in [Tool::Prune, Tool::Ports] {
            for focus in [FocusPanel::List, FocusPanel::Details] {
                for (code, expected) in [
                    (
                        KeyCode::Tab,
                        if tool == Tool::Prune {
                            Tool::Ports
                        } else {
                            Tool::Prune
                        },
                    ),
                    (KeyCode::Char('1'), Tool::Prune),
                    (KeyCode::Char('2'), Tool::Ports),
                ] {
                    let mut app = test_app(tool);
                    app.focus = focus;
                    press(&mut app, code);
                    assert_eq!(app.active_tool, expected, "{tool:?} {focus:?} {code:?}");
                    assert_eq!(app.focus, FocusPanel::List);
                    assert_eq!(app.mode, AppMode::Normal);
                }
            }
        }
    }

    #[test]
    fn test_quit_and_back_keys() {
        for tool in [Tool::Prune, Tool::Ports] {
            for focus in [FocusPanel::List, FocusPanel::Details] {
                let mut app = test_app(tool);
                app.focus = focus;
                press(&mut app, KeyCode::Char('q'));
                assert!(app.exit, "q quits ({tool:?} {focus:?})");
            }
            let mut app = test_app(tool);
            press(&mut app, KeyCode::Esc);
            assert!(app.exit, "Esc quits from the list ({tool:?})");
            for back in [KeyCode::Esc, KeyCode::Char('h'), KeyCode::Left] {
                let mut app = test_app(tool);
                app.focus = FocusPanel::Details;
                press(&mut app, back);
                assert!(!app.exit);
                assert_eq!(app.focus, FocusPanel::List, "{tool:?} {back:?}");
            }
        }
    }

    #[test]
    fn test_ports_list_keys() {
        let mut app = test_app(Tool::Ports);
        let cursor = |app: &App| app.ports.as_ref().unwrap().list_state.selected();
        press(&mut app, KeyCode::Char('j'));
        assert_eq!(cursor(&app), Some(1));
        press(&mut app, KeyCode::Char('g'));
        assert_eq!(cursor(&app), Some(0));
        press(&mut app, KeyCode::Char('G'));
        assert_eq!(cursor(&app), Some(1));
        press(&mut app, KeyCode::Char('k'));
        assert_eq!(cursor(&app), Some(0));

        press(&mut app, KeyCode::Char('d'));
        assert_eq!(app.mode, AppMode::Normal, "d needs a selection");
        press(&mut app, KeyCode::Char(' '));
        assert_eq!(app.ports.as_ref().unwrap().selected, vec![true, false]);
        press(&mut app, KeyCode::Char('v'));
        assert_eq!(app.ports.as_ref().unwrap().selected, vec![false, true]);
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL),
        );
        assert_eq!(app.ports.as_ref().unwrap().selected, vec![true, true]);
        press(&mut app, KeyCode::Char('d'));
        assert_eq!(app.mode, AppMode::Confirm);
        press(&mut app, KeyCode::Esc);
        assert_eq!(app.mode, AppMode::Normal);

        press(&mut app, KeyCode::Char('s'));
        assert_eq!(
            app.ports.as_ref().unwrap().sort_mode,
            ports::PortsSortMode::PortDesc
        );

        press(&mut app, KeyCode::Char('t'));
        assert_eq!(app.mode, AppMode::SubFilter);
        press(&mut app, KeyCode::Char('j'));
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.mode, AppMode::Normal);
        assert_eq!(
            app.ports.as_ref().unwrap().protocol_filter,
            Some(Protocol::Tcp)
        );

        press(&mut app, KeyCode::Char('/'));
        assert_eq!(app.mode, AppMode::Filter);
        press(&mut app, KeyCode::Char('3'));
        press(&mut app, KeyCode::Char('0'));
        press(&mut app, KeyCode::Char('0'));
        press(&mut app, KeyCode::Char('1'));
        assert_eq!(app.ports.as_ref().unwrap().filter_text, "3001");
        assert_eq!(app.ports.as_ref().unwrap().filtered_indices.len(), 1);
        press(&mut app, KeyCode::Backspace);
        assert_eq!(app.ports.as_ref().unwrap().filter_text, "300");
        press(&mut app, KeyCode::Esc);
        assert_eq!(app.mode, AppMode::Normal);
        assert_eq!(app.ports.as_ref().unwrap().filter_text, "");

        press(&mut app, KeyCode::Char('?'));
        assert_eq!(app.mode, AppMode::Help);
        press(&mut app, KeyCode::Char('q'));
        assert_eq!(app.mode, AppMode::Normal);
        assert!(!app.exit, "q closes help");

        press(&mut app, KeyCode::Enter);
        assert_eq!(app.focus, FocusPanel::Details);
    }

    #[test]
    fn test_ports_rescan_keys() {
        let mut app = test_app(Tool::Ports);
        press(&mut app, KeyCode::Char('r'));
        assert!(!app.ports.as_ref().unwrap().scan_complete);
        let before = app.ports.as_ref().unwrap().dev_filter_active;
        press(&mut app, KeyCode::Char('a'));
        assert_eq!(app.ports.as_ref().unwrap().dev_filter_active, !before);
    }

    #[test]
    fn test_prune_list_keys() {
        let mut app = test_app(Tool::Prune);
        press(&mut app, KeyCode::Char('d'));
        assert_eq!(app.mode, AppMode::Normal, "d needs a selection");
        press(&mut app, KeyCode::Char(' '));
        press(&mut app, KeyCode::Char('d'));
        assert_eq!(app.mode, AppMode::Confirm);
        press(&mut app, KeyCode::Esc);
        assert_eq!(app.mode, AppMode::Normal);

        press(&mut app, KeyCode::Char('t'));
        assert_eq!(app.mode, AppMode::SubFilter);
        press(&mut app, KeyCode::Char('j'));
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.prune.type_filter.as_deref(), Some("node_modules"));
        press(&mut app, KeyCode::Char('t'));
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.prune.type_filter, None);

        press(&mut app, KeyCode::Char('p'));
        assert!(app.prune.project_grouping);
        press(&mut app, KeyCode::Char('s'));
        assert_eq!(app.prune.sort_mode, app::SortMode::SizeAsc);

        press(&mut app, KeyCode::Char('/'));
        press(&mut app, KeyCode::Char('b'));
        press(&mut app, KeyCode::Char('/'));
        assert_eq!(app.prune.filter_text, "b/");
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.mode, AppMode::Normal);
        assert_eq!(app.prune.filter_text, "b/", "Enter keeps the filter");

        press(&mut app, KeyCode::Char('?'));
        assert_eq!(app.mode, AppMode::Help);
        press(&mut app, KeyCode::Esc);
        assert_eq!(app.mode, AppMode::Normal);

        press(&mut app, KeyCode::Char('l'));
        assert_eq!(app.focus, FocusPanel::Details);
    }

    #[test]
    fn test_errors_popup_keys() {
        for code in [KeyCode::Enter, KeyCode::Esc, KeyCode::Char('q')] {
            let mut app = test_app(Tool::Prune);
            app.mode = AppMode::Errors;
            press(&mut app, code);
            assert_eq!(app.mode, AppMode::Normal, "{code:?}");
            assert!(!app.exit);
        }
    }

    #[test]
    fn test_ctrl_c_quits_from_any_mode() {
        let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        for tool in [Tool::Prune, Tool::Ports] {
            for mode in [
                AppMode::Normal,
                AppMode::Filter,
                AppMode::SubFilter,
                AppMode::Confirm,
                AppMode::Processing,
                AppMode::Help,
                AppMode::Errors,
            ] {
                let (_tx, rx) = mpsc::channel();
                let mut app = App::new(rx, Config::load(None).unwrap());
                app.active_tool = tool;
                app.mode = mode;
                handle_key(&mut app, ctrl_c);
                assert!(app.exit, "Ctrl+C should quit ({tool:?}, {mode:?})");
            }
        }
    }
}
