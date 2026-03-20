# Ports Mode Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a port management mode to lazyprune that lets users list, filter, select, and kill processes by port — togglable at runtime with the existing prune mode.

**Architecture:** Enum-based dual mode (`Tool::Prune` / `Tool::Ports`) with separate state structs (`PruneState` / `PortsState`). Shared event loop, layout, and popup system. Port scanning via `lsof` parsing in a background thread. Kill via `nix` crate signals (SIGTERM → SIGKILL).

**Tech Stack:** Rust, ratatui 0.30, crossterm 0.29, nix (new — signal feature), clap 4, lsof + ps (system commands)

**Spec:** `docs/superpowers/specs/2026-03-20-ports-mode-design.md`

---

## File Structure

### Modified files
- `Cargo.toml` — add `nix` dependency
- `config.default.toml` — add `[ports]` section
- `src/main.rs` — CLI flag, event loop dispatch, key handlers for ports mode, mode toggle
- `src/app.rs` — rename AppMode variants, extract PruneState, add Tool enum, active_tool, PortsState field
- `src/config.rs` — add PortsConfig parsing + field-by-field merge
- `src/ui/mod.rs` — mode-aware header/footer
- `src/ui/popup.rs` — mode-aware help, kill confirmation popup

### New files
- `src/ports.rs` — PortInfo, Protocol, PortsSortMode, PortsState, lsof scan, kill logic
- `src/ui/ports_list.rs` — ports list panel rendering
- `src/ui/ports_details.rs` — ports details panel rendering

---

## Task 1: Rename AppMode variants

Mechanical rename to make mode names tool-agnostic before adding ports support.

**Files:**
- Modify: `src/app.rs:52-60` (AppMode enum)
- Modify: `src/main.rs` (all AppMode references)
- Modify: `src/ui/mod.rs` (render dispatch)
- Modify: `src/ui/popup.rs` (popup rendering)

- [ ] **Step 1: Rename `TypeFilter` → `SubFilter` and `Deleting` → `Processing` in `src/app.rs`**

In the `AppMode` enum (line 52-60), rename:
- `TypeFilter` → `SubFilter`
- `Deleting` → `Processing`

Update all references within `app.rs` (search for `AppMode::TypeFilter` and `AppMode::Deleting`).

- [ ] **Step 2: Update all references in `src/main.rs`**

Search and replace:
- `AppMode::TypeFilter` → `AppMode::SubFilter`
- `AppMode::Deleting` → `AppMode::Processing`
- `handle_type_filter_key` → `handle_sub_filter_key`

- [ ] **Step 3: Update all references in `src/ui/mod.rs` and `src/ui/popup.rs`**

Same renames in UI rendering code. In `src/ui/popup.rs`:
- Rename function `render_type_filter` → `render_sub_filter`
- Rename function `render_deleting` → `render_processing`

In `src/ui/mod.rs`:
- Update call sites: `popup::render_type_filter` → `popup::render_sub_filter`
- Update call sites: `popup::render_deleting` → `popup::render_processing`
- Update `AppMode::TypeFilter` → `AppMode::SubFilter`
- Update `AppMode::Deleting` → `AppMode::Processing`

- [ ] **Step 4: Run tests**

Run: `cargo test`
Expected: All tests pass. No functional change.

Run: `cargo clippy`
Expected: No warnings.

- [ ] **Step 5: Commit**

```bash
git add -A && git commit -m "refactor: rename AppMode variants to be tool-agnostic

TypeFilter → SubFilter, Deleting → Processing — prepares for
multi-tool architecture where these modes are shared."
```

---

## Task 2: Extract PruneState from App

Move all prune-specific fields from `App` into a dedicated `PruneState` struct. This is the largest refactoring task — everything else builds on it.

**Files:**
- Modify: `src/app.rs:90-125` (App struct) — extract fields into PruneState
- Modify: `src/main.rs` — update all `app.field` to `app.prune.field` for prune-specific fields
- Modify: `src/ui/list.rs` — update references
- Modify: `src/ui/details.rs` — update references
- Modify: `src/ui/mod.rs` — update references
- Modify: `src/ui/popup.rs` — update references

- [ ] **Step 1: Define `PruneState` struct in `src/app.rs`**

Create `PruneState` struct containing all prune-specific fields from `App` (lines 91-124 of `src/app.rs`). Copy exact names and types:
```rust
pub struct PruneState {
    pub items: Vec<ScanResult>,
    pub filtered_indices: Vec<usize>,
    pub selected: Vec<bool>,
    pub list_state: ListState,
    pub sort_mode: SortMode,
    pub filter_text: String,
    pub type_filter: Option<String>,
    pub scan_rx: Option<mpsc::Receiver<ScanMessage>>,
    pub scan_complete: bool,
    pub dirs_scanned: u64,
    pub total_deleted: u64,
    pub items_deleted: usize,
    pub scan_errors: u64,
    pub scan_tick: u8,
    pub available_types: Vec<String>,
    pub type_filter_cursor: usize,
    pub delete_rx: Option<mpsc::Receiver<DeleteMessage>>,
    pub delete_total: usize,
    pub delete_progress: usize,
    pub delete_current_path: String,
    pub delete_errors: Vec<String>,
    pub delete_done_indices: Vec<usize>,
    pub group_separators: std::collections::HashSet<usize>,
    pub project_grouping: bool,
    pub tree_cache: std::collections::HashMap<std::path::PathBuf, TreeData>,
    pub tree_rx: Option<mpsc::Receiver<(std::path::PathBuf, TreeData)>>,
    pub tree_loading: bool,
    pub tree_scroll: u16,
    pub tree_debounce_at: Option<std::time::Instant>,
    pub tree_requested_path: Option<std::path::PathBuf>,
    pub path_index_map: std::collections::HashMap<std::path::PathBuf, usize>,
}
```

**Important:** The field names and types must match the current `App` struct exactly. Cross-reference `src/app.rs:90-125` during implementation.

- [ ] **Step 2: Add `PruneState` to `App`, remove extracted fields**

Replace the extracted fields on `App` with:
```rust
pub struct App {
    pub prune: PruneState,
    pub mode: AppMode,
    pub focus: FocusPanel,
    pub exit: bool,
    pub config: Config,
}
```

Keep `mode`, `focus`, `exit`, and `config` on `App` — they are shared.

Update `App::new()` signature to accept `Config`:
```rust
pub fn new(scan_rx: mpsc::Receiver<ScanMessage>, config: Config) -> Self
```
Initialize `PruneState` with all extracted fields inside `new()`. Update the call site in `src/main.rs` (currently `App::new(rx)` at line ~135) to pass config: `App::new(rx, config)`.

- [ ] **Step 3: Update all methods in `src/app.rs`**

Every method that accessed `self.items`, `self.selected`, etc. now accesses `self.prune.items`, `self.prune.selected`, etc. This is mechanical. Key methods to update:
- `new()`, `poll_scan_results()`, `current_item()`, `current_group_info()`
- `next()`, `previous()`, `go_top()`, `go_bottom()`
- `toggle_selection()`, `select_all()`, `invert_selection()`
- `cycle_sort()`, `toggle_project_grouping()`
- `selected_items()`, `selected_size()`, `selected_count()`
- `start_deleting()`, `poll_delete_results()`
- `item_passes_filter()`, `apply_filter()`
- `request_tree_scan()`, `maybe_start_tree_scan()`, `poll_tree_results()`
- Tree scroll methods, `copy_path_to_clipboard()`

- [ ] **Step 4: Update `src/main.rs`**

Update all references from `app.field` to `app.prune.field` for prune-specific fields. The key areas:
- `run()` function: `app.prune.scan_rx`, `app.prune.scan_complete`, etc.
- `handle_normal_key()`: `app.prune.list_state`, `app.prune.filter_text`, etc.
- `handle_filter_key()`: `app.prune.filter_text`
- `handle_sub_filter_key()`: `app.prune.type_filter_cursor`, `app.prune.available_types`
- `handle_confirm_key()`: `app.start_deleting()`
- `render()`: passes `app` unchanged to `ui::render`

Keep `app.mode`, `app.focus`, `app.exit` as-is (they stay on App).

**Don't forget `main()` post-run summary** (lines ~139-145 of `src/main.rs`): this code accesses `app.items_deleted` and `app.total_deleted` after `run()` returns. Update to `app.prune.items_deleted` and `app.prune.total_deleted`.

- [ ] **Step 5: Update all UI files**

In `src/ui/mod.rs`, `src/ui/list.rs`, `src/ui/details.rs`, `src/ui/popup.rs`:
- Replace `app.items` → `app.prune.items`
- Replace `app.filtered_indices` → `app.prune.filtered_indices`
- Replace `app.selected` → `app.prune.selected`
- Replace `app.list_state` → `app.prune.list_state`
- And so on for all prune-specific fields.

The render functions already receive `&App` or `&mut App` — the signature doesn't change.

- [ ] **Step 6: Update tests in `src/app.rs`**

All test code that creates an `App` or accesses its fields needs updating to use `app.prune.field`. The App::new() constructor will initialize `PruneState` internally.

- [ ] **Step 7: Run tests**

Run: `cargo test`
Expected: All tests pass. No functional change.

Run: `cargo clippy`
Expected: No warnings.

- [ ] **Step 8: Commit**

```bash
git add -A && git commit -m "refactor: extract PruneState from App

Move all prune-specific fields (items, selected, filtered_indices,
scan channels, tree cache, delete progress) into PruneState struct.
Prepares App for multi-tool architecture."
```

---

## Task 3: Add Tool enum and mode toggle

Add the `Tool` enum, `active_tool` field, and `Tab`/`1`/`2` keybindings. Ports mode is a placeholder for now (no scan, no UI — just the toggle infrastructure).

**Files:**
- Modify: `src/app.rs` — add Tool enum, active_tool field
- Modify: `src/main.rs` — add toggle key handling, CLI `--ports` flag

- [ ] **Step 1: Add `Tool` enum and `active_tool` to `App` in `src/app.rs`**

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    Prune,
    Ports,
}
```

Add to `App`:
```rust
pub struct App {
    pub active_tool: Tool,
    pub prune: PruneState,
    // ports: PortsState will come in Task 7
    pub mode: AppMode,
    pub focus: FocusPanel,
    pub exit: bool,
    pub config: Config,
}
```

Default `active_tool` to `Tool::Prune` in `new()`.

- [ ] **Step 2: Add `--ports` CLI flag in `src/main.rs`**

Add to the `Cli` struct:
```rust
/// Start in ports mode (list and kill processes by port)
#[arg(long)]
ports: bool,
```

After creating the App, set `app.active_tool = Tool::Ports` if the flag is set.

- [ ] **Step 3: Add toggle key handling in `handle_normal_key()`**

In `handle_normal_key()`, add cases for `Tab`, `1`, `2` — only when `app.mode == AppMode::Normal`:
```rust
KeyCode::Tab => {
    app.active_tool = match app.active_tool {
        Tool::Prune => Tool::Ports,
        Tool::Ports => Tool::Prune,
    };
    app.mode = AppMode::Normal;
    app.focus = FocusPanel::List;
}
KeyCode::Char('1') => {
    app.active_tool = Tool::Prune;
    app.mode = AppMode::Normal;
    app.focus = FocusPanel::List;
}
KeyCode::Char('2') => {
    app.active_tool = Tool::Ports;
    app.mode = AppMode::Normal;
    app.focus = FocusPanel::List;
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test`
Expected: All tests pass.

Run: `cargo clippy`
Expected: No warnings.

- [ ] **Step 5: Commit**

```bash
git add -A && git commit -m "feat: add Tool enum and mode toggle (Tab/1/2)

Add Tool::Prune/Ports enum with active_tool on App. Tab toggles,
1/2 switch directly. --ports CLI flag starts in ports mode.
Ports mode is a placeholder for now."
```

---

## Task 4: Config extension

Add `[ports]` section to config with dev_filter support and port range parsing.

**Files:**
- Modify: `config.default.toml` — add `[ports]` section with comments
- Modify: `src/config.rs` — add PortsConfig struct, parsing, merge
- Test: `src/config.rs` (inline tests)

- [ ] **Step 1: Write failing tests for port config parsing**

Add tests in `src/config.rs`:
```rust
#[test]
fn test_ports_config_defaults() {
    let config = Config::load(None);
    assert!(config.ports.dev_filter_enabled);
    assert!(!config.ports.dev_filter.is_empty());
}

#[test]
fn test_ports_config_parse_range() {
    let ranges = parse_port_filter(&["3000-3009".to_string(), "5173".to_string()]);
    assert!(ranges.contains(&3000));
    assert!(ranges.contains(&3009));
    assert!(ranges.contains(&5173));
    assert!(!ranges.contains(&3010));
}

#[test]
fn test_ports_config_user_override_partial() {
    // User overrides only dev_filter, dev_filter_enabled keeps default
    let default = Config::load(None);
    let toml_str = r#"
[ports]
dev_filter = ["8080"]
"#;
    let user: UserConfigOverride = toml::from_str(toml_str).unwrap();
    let mut config = default;
    user.apply_to(&mut config);
    assert!(config.ports.dev_filter_enabled); // kept default
    assert_eq!(config.ports.dev_filter, vec!["8080"]);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test test_ports_config`
Expected: FAIL — PortsConfig doesn't exist yet.

- [ ] **Step 3: Add `PortsConfig` struct and parsing in `src/config.rs`**

```rust
#[derive(Debug, Deserialize, Clone)]
pub struct PortsConfig {
    pub dev_filter_enabled: bool,
    pub dev_filter: Vec<String>,
}

impl Default for PortsConfig {
    fn default() -> Self {
        Self {
            dev_filter_enabled: true,
            dev_filter: vec![
                "3000-3009".into(), "4000-4009".into(),
                "5173-5174".into(), "8080-8090".into(),
            ],
        }
    }
}
```

Add `ports: PortsConfig` to `Config` struct with `#[serde(default)]`.

Add `parse_port_filter()` helper:
```rust
pub fn parse_port_filter(ranges: &[String]) -> HashSet<u16> {
    let mut ports = HashSet::new();
    for entry in ranges {
        if let Some((start, end)) = entry.split_once('-') {
            if let (Ok(s), Ok(e)) = (start.parse::<u16>(), end.parse::<u16>()) {
                for p in s..=e {
                    ports.insert(p);
                }
            }
        } else if let Ok(p) = entry.parse::<u16>() {
            ports.insert(p);
        }
    }
    ports
}
```

Update `UserConfigOverride` with optional ports fields:
```rust
pub struct UserConfigOverride {
    // ... existing fields ...
    #[serde(default)]
    ports: Option<UserPortsOverride>,
}

#[derive(Debug, Deserialize)]
struct UserPortsOverride {
    dev_filter_enabled: Option<bool>,
    dev_filter: Option<Vec<String>>,
}
```

Update `apply_to()` to merge ports fields individually:
```rust
// Inside apply_to(&self, config: &mut Config)
if let Some(ref ports) = self.ports {
    if let Some(enabled) = ports.dev_filter_enabled {
        config.ports.dev_filter_enabled = enabled;
    }
    if let Some(ref filter) = ports.dev_filter {
        config.ports.dev_filter = filter.clone();
    }
}
```

- [ ] **Step 4: Add `[ports]` section to `config.default.toml`**

Append to end of file:
```toml

# --- Ports mode ---
[ports]
# Only show ports matching these ranges on startup (toggle with 'a' in TUI)
dev_filter_enabled = true
# Port ranges to show when dev filter is active (supports "PORT" and "START-END")
dev_filter = ["3000-3009", "4000-4009", "5173-5174", "8080-8090"]
```

- [ ] **Step 5: Run tests**

Run: `cargo test`
Expected: All tests pass including new port config tests.

- [ ] **Step 6: Commit**

```bash
git add -A && git commit -m "feat: add [ports] config section with dev_filter

PortsConfig with dev_filter_enabled and dev_filter (port range syntax).
Field-by-field merge for user overrides. parse_port_filter() helper
expands ranges like '3000-3009' into port sets."
```

---

## Task 5: Add nix dependency

**Files:**
- Modify: `Cargo.toml`

- [ ] **Step 1: Add nix to Cargo.toml**

Add to `[dependencies]`:
```toml
nix = { version = "0.29", features = ["signal", "process"] }
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo check`
Expected: Compiles successfully.

- [ ] **Step 3: Commit**

```bash
git add Cargo.toml Cargo.lock && git commit -m "chore: add nix dependency for signal handling"
```

---

## Task 6: Port scanner — PortInfo, lsof parsing, scan thread

Core port scanning logic. Self-contained module with its own tests.

**Files:**
- Create: `src/ports.rs`
- Modify: `src/main.rs:1-5` — add `mod ports;`
- Test: inline in `src/ports.rs`

- [ ] **Step 1: Write failing tests for lsof output parsing**

In `src/ports.rs`, write tests:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_lsof_listen_line() {
        let line = "node      12345 thibault   23u  IPv6 0xabc      0t0  TCP *:3000 (LISTEN)";
        let result = parse_lsof_line(line);
        assert!(result.is_some());
        let info = result.unwrap();
        assert_eq!(info.port, 3000);
        assert_eq!(info.pid, 12345);
        assert_eq!(info.process_name, "node");
        assert_eq!(info.user, "thibault");
        assert_eq!(info.protocol, Protocol::Tcp);
        assert_eq!(info.state, "LISTEN");
    }

    #[test]
    fn test_parse_lsof_udp_no_state() {
        let line = "mDNSRespo   123 _mdnsresponder   12u  IPv4 0xdef      0t0  UDP *:5353";
        let result = parse_lsof_line(line);
        assert!(result.is_some());
        let info = result.unwrap();
        assert_eq!(info.port, 5353);
        assert_eq!(info.protocol, Protocol::Udp);
        assert_eq!(info.state, "");
    }

    #[test]
    fn test_parse_lsof_skip_header() {
        let line = "COMMAND     PID   USER   FD   TYPE             DEVICE SIZE/OFF NODE NAME";
        assert!(parse_lsof_line(line).is_none());
    }

    #[test]
    fn test_parse_lsof_established() {
        let line = "node      12345 thibault   24u  IPv6 0xabc      0t0  TCP 127.0.0.1:3000->127.0.0.1:52341 (ESTABLISHED)";
        let result = parse_lsof_line(line);
        assert!(result.is_some());
        let info = result.unwrap();
        assert_eq!(info.port, 3000);
        assert_eq!(info.state, "ESTABLISHED");
    }

    #[test]
    fn test_dedup_ports() {
        let entries = vec![
            PortEntry { port: 3000, protocol: Protocol::Tcp, pid: 123,
                        process_name: "node".into(), user: "me".into(), state: "LISTEN".into() },
            PortEntry { port: 3000, protocol: Protocol::Tcp, pid: 123,
                        process_name: "node".into(), user: "me".into(), state: "ESTABLISHED".into() },
            PortEntry { port: 3000, protocol: Protocol::Tcp, pid: 124,
                        process_name: "node".into(), user: "me".into(), state: "ESTABLISHED".into() },
        ];
        let deduped = dedup_port_entries(entries);
        assert_eq!(deduped.len(), 1);
        assert_eq!(deduped[0].state, "LISTEN");
        assert_eq!(deduped[0].connections, 3);
    }

    #[test]
    fn test_parse_port_from_name_column() {
        assert_eq!(parse_port_from_name("*:3000 (LISTEN)"), Some((3000, "LISTEN".to_string())));
        assert_eq!(parse_port_from_name("*:5353"), Some((5353, "".to_string())));
        assert_eq!(parse_port_from_name("127.0.0.1:8080->127.0.0.1:52341 (ESTABLISHED)"), Some((8080, "ESTABLISHED".to_string())));
        assert_eq!(parse_port_from_name("[::1]:3000 (LISTEN)"), Some((3000, "LISTEN".to_string())));
    }

    #[test]
    fn test_dev_filter() {
        let filter: HashSet<u16> = (3000..=3009).collect();
        let entries = vec![
            PortInfo { port: 3000, protocol: Protocol::Tcp, pid: 1, process_name: "node".into(),
                       command: "".into(), user: "me".into(), state: "LISTEN".into(), connections: 1 },
            PortInfo { port: 22, protocol: Protocol::Tcp, pid: 2, process_name: "sshd".into(),
                       command: "".into(), user: "root".into(), state: "LISTEN".into(), connections: 1 },
        ];
        let filtered: Vec<_> = entries.into_iter().filter(|p| filter.contains(&p.port)).collect();
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].port, 3000);
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test test_parse_lsof`
Expected: FAIL — module doesn't exist yet.

- [ ] **Step 3: Implement PortInfo, Protocol, and parsing logic**

In `src/ports.rs`:
```rust
use std::collections::{HashMap, HashSet};
use std::process::Command;
use std::sync::mpsc;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Protocol {
    Tcp,
    Udp,
}

// Internal struct from lsof parsing before dedup
#[derive(Debug)]
struct PortEntry {
    port: u16,
    protocol: Protocol,
    pid: u32,
    process_name: String,
    user: String,
    state: String,
}

#[derive(Debug, Clone)]
pub struct PortInfo {
    pub port: u16,
    pub protocol: Protocol,
    pub pid: u32,
    pub process_name: String,
    pub command: String,
    pub user: String,
    pub state: String,
    pub connections: usize,
}

pub enum PortScanMessage {
    Found(PortInfo),
    Complete,
    Error(String),
}

fn parse_port_from_name(name: &str) -> Option<(u16, String)> {
    // Extract port from NAME column: "*:3000 (LISTEN)", "127.0.0.1:8080->...", "[::1]:3000"
    // Port is after the last ':' before any '->' or space
    let local = name.split("->").next()?;
    let port_str = local.rsplit(':').next()?;
    let port_str = port_str.split_whitespace().next()?;
    let port = port_str.parse::<u16>().ok()?;

    let state = if let Some(start) = name.rfind('(') {
        name[start + 1..].trim_end_matches(')').to_string()
    } else {
        String::new()
    };

    Some((port, state))
}

fn parse_lsof_line(line: &str) -> Option<PortEntry> {
    let fields: Vec<&str> = line.split_whitespace().collect();
    if fields.len() < 9 || fields[0] == "COMMAND" {
        return None;
    }

    let process_name = fields[0].to_string();
    let pid = fields[1].parse::<u32>().ok()?;
    let user = fields[2].to_string();
    let node = fields[7]; // TCP or UDP
    let protocol = match node {
        "TCP" => Protocol::Tcp,
        "UDP" => Protocol::Udp,
        _ => return None,
    };

    // NAME is everything from field 8 onward (may contain spaces in state)
    let name = fields[8..].join(" ");
    let (port, state) = parse_port_from_name(&name)?;

    Some(PortEntry { port, protocol, pid, process_name, user, state })
}

fn dedup_port_entries(entries: Vec<PortEntry>) -> Vec<PortInfo> {
    let mut map: HashMap<(u16, Protocol), (PortEntry, usize)> = HashMap::new();
    for entry in entries {
        let key = (entry.port, entry.protocol);
        match map.entry(key) {
            std::collections::hash_map::Entry::Occupied(mut e) => {
                let (existing, count) = e.get_mut();
                *count += 1;
                // Prefer LISTEN over other states
                if entry.state == "LISTEN" && existing.state != "LISTEN" {
                    *existing = entry;
                }
            }
            std::collections::hash_map::Entry::Vacant(e) => {
                e.insert((entry, 1));
            }
        }
    }
    map.into_values()
        .map(|(e, count)| PortInfo {
            port: e.port, protocol: e.protocol, pid: e.pid,
            process_name: e.process_name, command: String::new(),
            user: e.user, state: e.state, connections: count,
        })
        .collect()
}

fn fetch_commands(pids: &[u32]) -> HashMap<u32, String> {
    if pids.is_empty() {
        return HashMap::new();
    }
    let pid_args: Vec<String> = pids.iter().map(|p| p.to_string()).collect();
    let output = Command::new("ps")
        .args(["-p", &pid_args.join(","), "-o", "pid=,command="])
        .output();
    let mut map = HashMap::new();
    if let Ok(output) = output {
        let stdout = String::from_utf8_lossy(&output.stdout);
        for line in stdout.lines() {
            let trimmed = line.trim();
            if let Some(idx) = trimmed.find(|c: char| !c.is_ascii_digit()) {
                let pid: u32 = trimmed[..idx].trim().parse().unwrap_or(0);
                let cmd = trimmed[idx..].trim().to_string();
                if pid > 0 {
                    map.insert(pid, cmd);
                }
            }
        }
    }
    map
}

pub fn scan_ports(
    tx: mpsc::Sender<PortScanMessage>,
    dev_filter: Option<HashSet<u16>>,
) {
    let output = Command::new("lsof")
        .args(["-i", "-n", "-P", "+c", "0"])
        .output();

    match output {
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let entries: Vec<PortEntry> = stdout.lines()
                .filter_map(parse_lsof_line)
                .collect();
            let mut infos = dedup_port_entries(entries);

            // Apply dev filter
            if let Some(ref filter) = dev_filter {
                infos.retain(|p| filter.contains(&p.port));
            }

            // Batch fetch commands
            let pids: Vec<u32> = infos.iter().map(|p| p.pid).collect();
            let commands = fetch_commands(&pids);
            for info in &mut infos {
                if let Some(cmd) = commands.get(&info.pid) {
                    info.command = cmd.clone();
                }
            }

            for info in infos {
                if tx.send(PortScanMessage::Found(info)).is_err() {
                    return;
                }
            }
            let _ = tx.send(PortScanMessage::Complete);
        }
        Err(e) => {
            let _ = tx.send(PortScanMessage::Error(e.to_string()));
            let _ = tx.send(PortScanMessage::Complete);
        }
    }
}
```

- [ ] **Step 4: Add `mod ports;` to `src/main.rs`**

Add `mod ports;` to the module declarations at the top of `src/main.rs`.

- [ ] **Step 5: Run tests**

Run: `cargo test`
Expected: All tests pass including new port parsing tests.

- [ ] **Step 6: Commit**

```bash
git add -A && git commit -m "feat: add port scanner with lsof parsing

PortInfo struct, Protocol enum, lsof output parsing with dedup by
(port, protocol) preferring LISTEN state. Batch ps follow-up for
full command lines. Dev filter applied at scan time."
```

---

## Task 7: PortsState — state management, filter, sort, selection

Parallel to PruneState but for ports. Includes PortsSortMode and all list interaction logic.

**Files:**
- Modify: `src/ports.rs` — add PortsState, PortsSortMode, filtering/sorting/selection methods
- Modify: `src/app.rs` — add `ports: Option<PortsState>` to App (lazy init)
- Test: inline in `src/ports.rs`

- [ ] **Step 1: Write failing tests for PortsState**

```rust
fn make_port_info(port: u16, process: &str) -> PortInfo {
    PortInfo {
        port, protocol: Protocol::Tcp, pid: port as u32,
        process_name: process.into(), command: String::new(),
        user: "test".into(), state: "LISTEN".into(), connections: 1,
    }
}

fn make_port_info_udp(port: u16, process: &str) -> PortInfo {
    PortInfo {
        port, protocol: Protocol::Udp, pid: port as u32,
        process_name: process.into(), command: String::new(),
        user: "test".into(), state: String::new(), connections: 1,
    }
}

#[test]
fn test_ports_state_filter_text() {
    let mut state = PortsState::new();
    state.items = vec![
        make_port_info(3000, "node"),
        make_port_info(8080, "java"),
    ];
    state.filter_text = "node".into();
    state.apply_filter();
    assert_eq!(state.filtered_indices.len(), 1);
    assert_eq!(state.items[state.filtered_indices[0]].port, 3000);
}

#[test]
fn test_ports_state_sort_by_port() {
    let mut state = PortsState::new();
    state.items = vec![
        make_port_info(8080, "java"),
        make_port_info(3000, "node"),
    ];
    state.sort_mode = PortsSortMode::PortAsc;
    state.apply_filter();
    assert_eq!(state.items[state.filtered_indices[0]].port, 3000);
    assert_eq!(state.items[state.filtered_indices[1]].port, 8080);
}

#[test]
fn test_ports_state_protocol_filter() {
    let mut state = PortsState::new();
    state.items = vec![
        make_port_info(3000, "node"),  // TCP
        make_port_info_udp(5353, "mDNSResponder"),
    ];
    state.protocol_filter = Some(Protocol::Tcp);
    state.apply_filter();
    assert_eq!(state.filtered_indices.len(), 1);
}

#[test]
fn test_ports_state_selection() {
    let mut state = PortsState::new();
    state.items = vec![
        make_port_info(3000, "node"),
        make_port_info(3001, "node"),
    ];
    state.selected = vec![false; 2];
    state.apply_filter();
    state.toggle_selection(0);
    assert!(state.selected[state.filtered_indices[0]]);
    assert_eq!(state.selected_count(), 1);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test test_ports_state`
Expected: FAIL.

- [ ] **Step 3: Implement PortsState**

Add to `src/ports.rs`:
```rust
use ratatui::widgets::ListState;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortsSortMode {
    PortAsc,
    PortDesc,
    ProcessName,
    PidAsc,
}

impl PortsSortMode {
    pub fn next(self) -> Self {
        match self {
            Self::PortAsc => Self::PortDesc,
            Self::PortDesc => Self::ProcessName,
            Self::ProcessName => Self::PidAsc,
            Self::PidAsc => Self::PortAsc,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::PortAsc => "Port ↑",
            Self::PortDesc => "Port ↓",
            Self::ProcessName => "Process",
            Self::PidAsc => "PID",
        }
    }
}

pub struct PortsState {
    pub items: Vec<PortInfo>,
    pub filtered_indices: Vec<usize>,
    pub selected: Vec<bool>,
    pub list_state: ListState,
    pub sort_mode: PortsSortMode,
    pub filter_text: String,
    pub protocol_filter: Option<Protocol>,
    pub protocol_filter_cursor: usize,
    pub dev_filter_active: bool,
    pub dev_filter_ports: HashSet<u16>,
    pub scan_complete: bool,
    pub scan_rx: Option<mpsc::Receiver<PortScanMessage>>,
    pub kill_rx: Option<mpsc::Receiver<KillMessage>>,
    pub kill_progress: usize,
    pub kill_total: usize,
    pub kill_current: String,
    pub kill_errors: Vec<String>,
}
```

Implement methods mirroring PruneState:
- `new()` — initialize with empty state
- `poll_scan_results()` — drain scan_rx channel
- `current_item()` — get item at cursor
- `apply_filter()` — text filter + protocol filter + sort
- `toggle_selection()`, `select_all()`, `invert_selection()`, `selected_count()`
- `next()`, `previous()`, `go_top()`, `go_bottom()`
- `cycle_sort()`
- `start_scan()` — spawn scan thread
- `item_passes_filter()` — check text + protocol

- [ ] **Step 4: Add `ports: Option<PortsState>` to `App` in `src/app.rs`**

Add the field and lazy init helper:
```rust
pub struct App {
    pub active_tool: Tool,
    pub prune: PruneState,
    pub ports: Option<PortsState>,
    pub mode: AppMode,
    pub focus: FocusPanel,
    pub exit: bool,
    pub config: Config,
}
```

Add `ensure_ports_initialized()` method that creates PortsState and starts scan if None.

- [ ] **Step 5: Run tests**

Run: `cargo test`
Expected: All tests pass.

- [ ] **Step 6: Commit**

```bash
git add -A && git commit -m "feat: add PortsState with filter, sort, and selection

PortsSortMode (PortAsc/Desc, ProcessName, PidAsc), protocol filter,
text filter, selection. Mirrors PruneState interaction patterns.
Lazy-initialized on App when ports mode is first entered."
```

---

## Task 8: Kill logic

SIGTERM → SIGKILL with nix crate. Integrated into PortsState.

**Files:**
- Modify: `src/ports.rs` — add kill functions and KillMessage enum

- [ ] **Step 1: Write failing tests for kill logic**

```rust
#[test]
fn test_kill_message_types() {
    // Test that KillMessage enum variants exist and work
    let msg = KillMessage::Killing { port: 3000, pid: 123, process: "node".into() };
    match msg {
        KillMessage::Killing { port, pid, .. } => {
            assert_eq!(port, 3000);
            assert_eq!(pid, 123);
        }
        _ => panic!("wrong variant"),
    }
}
```

- [ ] **Step 2: Implement kill logic**

Add to `src/ports.rs`:
```rust
use nix::sys::signal::{self, Signal};
use nix::unistd::Pid;

pub enum KillMessage {
    Killing { port: u16, pid: u32, process: String },
    Killed { port: u16, pid: u32 },
    Error { port: u16, pid: u32, error: String },
    Complete,
}

pub fn kill_ports(
    targets: Vec<PortInfo>,
    tx: mpsc::Sender<KillMessage>,
) {
    for target in &targets {
        let _ = tx.send(KillMessage::Killing {
            port: target.port,
            pid: target.pid,
            process: target.process_name.clone(),
        });

        let pid = Pid::from_raw(target.pid as i32);

        // Send SIGTERM
        if let Err(e) = signal::kill(pid, Signal::SIGTERM) {
            let _ = tx.send(KillMessage::Error {
                port: target.port, pid: target.pid,
                error: format!("SIGTERM failed: {}", e),
            });
            continue;
        }

        // Wait 500ms, then check if still alive
        std::thread::sleep(std::time::Duration::from_millis(500));

        // Check if process is still alive
        match signal::kill(pid, None) {
            Ok(_) => {
                // Still alive — verify it's the same process before SIGKILL
                if verify_process(target.pid, &target.process_name) {
                    let _ = signal::kill(pid, Signal::SIGKILL);
                }
            }
            Err(_) => {
                // Process already dead — success
            }
        }

        let _ = tx.send(KillMessage::Killed {
            port: target.port, pid: target.pid,
        });
    }
    let _ = tx.send(KillMessage::Complete);
}

fn verify_process(pid: u32, expected_name: &str) -> bool {
    let output = Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "comm="])
        .output();
    match output {
        Ok(out) => {
            let name = String::from_utf8_lossy(&out.stdout).trim().to_string();
            name == expected_name
        }
        Err(_) => false,
    }
}
```

Add `start_killing()` and `poll_kill_results()` methods to `PortsState`:

```rust
// In PortsState impl
pub fn start_killing(&mut self) {
    let targets: Vec<PortInfo> = self.selected.iter().enumerate()
        .filter(|(_, &sel)| sel)
        .map(|(i, _)| self.items[i].clone())
        .collect();
    if targets.is_empty() { return; }
    self.kill_total = targets.len();
    self.kill_progress = 0;
    self.kill_errors.clear();
    let (tx, rx) = mpsc::channel();
    self.kill_rx = Some(rx);
    std::thread::spawn(move || kill_ports(targets, tx));
}

pub fn poll_kill_results(&mut self) -> bool {
    let rx = match &self.kill_rx {
        Some(rx) => rx,
        None => return false,
    };
    while let Ok(msg) = rx.try_recv() {
        match msg {
            KillMessage::Killing { port, process, .. } => {
                self.kill_current = format!(":{} ({})", port, process);
            }
            KillMessage::Killed { .. } => {
                self.kill_progress += 1;
            }
            KillMessage::Error { port, error, .. } => {
                self.kill_progress += 1;
                self.kill_errors.push(format!("Port {}: {}", port, error));
            }
            KillMessage::Complete => {
                self.kill_rx = None;
                // Trigger a rescan instead of removing items
                // (ports may be re-bound immediately)
                let filter = if self.dev_filter_active {
                    Some(self.dev_filter_ports.clone())
                } else {
                    None
                };
                self.start_scan(filter);
                return true;
            }
        }
    }
    false
}
```

**Key difference from prune mode:** After kill completes, trigger a full rescan rather than removing items from the list. This is because ports can be re-bound immediately by other processes, so a fresh scan gives the most accurate view.

Also add `kill_rx: Option<mpsc::Receiver<KillMessage>>` to `PortsState`.

- [ ] **Step 3: Run tests**

Run: `cargo test`
Expected: All tests pass.

- [ ] **Step 4: Commit**

```bash
git add -A && git commit -m "feat: add port kill logic with SIGTERM/SIGKILL

Kill flow: SIGTERM → 500ms wait → verify PID still same process →
SIGKILL if needed. KillMessage enum for progress reporting.
start_killing()/poll_kill_results() on PortsState."
```

---

## Task 9: UI — ports list panel

**Files:**
- Create: `src/ui/ports_list.rs`
- Modify: `src/ui/mod.rs:1` — add `mod ports_list;`

- [ ] **Step 1: Implement ports list rendering**

Create `src/ui/ports_list.rs`:
```rust
use ratatui::prelude::*;
use ratatui::widgets::*;
use crate::app::App;
use crate::ports::Protocol;

pub fn render(frame: &mut Frame, area: Rect, app: &mut App) {
    let ports = match &app.ports {
        Some(p) => p,
        None => {
            // Render empty state
            let block = Block::default()
                .borders(Borders::ALL)
                .title(" Ports ")
                .border_style(Style::default().fg(Color::DarkGray));
            frame.render_widget(block, area);
            return;
        }
    };

    let items: Vec<ListItem> = ports.filtered_indices.iter().map(|&idx| {
        let info = &ports.items[idx];
        let selected_marker = if ports.selected[idx] { "● " } else { "  " };
        let proto = match info.protocol {
            Protocol::Tcp => "TCP",
            Protocol::Udp => "UDP",
        };
        let state_style = if info.state == "LISTEN" {
            Style::default().fg(Color::Green)
        } else {
            Style::default().fg(Color::DarkGray)
        };

        let line = Line::from(vec![
            Span::styled(selected_marker, Style::default().fg(Color::Cyan)),
            Span::styled(format!("{:<6}", info.port), Style::default().fg(Color::Cyan)),
            Span::styled(format!("{:<5}", proto), Style::default().fg(Color::Blue)),
            Span::styled(format!("{:<8}", info.pid), Style::default().fg(Color::DarkGray)),
            Span::styled(format!("{:<12}", info.process_name), Style::default().fg(Color::White)),
            Span::styled(&info.state, state_style),
        ]);
        ListItem::new(line)
    }).collect();

    let title = format!(" Ports ({}) ", ports.filtered_indices.len());
    let border_color = if app.focus == crate::app::FocusPanel::List {
        Color::Cyan
    } else {
        Color::DarkGray
    };

    let list = List::new(items)
        .block(Block::default()
            .borders(Borders::ALL)
            .title(title)
            .border_style(Style::default().fg(border_color)))
        .highlight_style(Style::default().bg(Color::DarkGray));

    frame.render_stateful_widget(
        list, area,
        &mut app.ports.as_mut().unwrap().list_state,
    );
}
```

- [ ] **Step 2: Register module in `src/ui/mod.rs`**

Add `mod ports_list;` and `pub use ports_list;` in `src/ui/mod.rs`.

- [ ] **Step 3: Verify it compiles**

Run: `cargo check`
Expected: Compiles successfully.

- [ ] **Step 4: Commit**

```bash
git add -A && git commit -m "feat: add ports list panel rendering

Selection markers, color by state (green LISTEN), port/proto/PID/
process/state columns. Follows same patterns as prune list panel."
```

---

## Task 10: UI — ports details panel

**Files:**
- Create: `src/ui/ports_details.rs`
- Modify: `src/ui/mod.rs` — add `mod ports_details;`

- [ ] **Step 1: Implement ports details rendering**

Create `src/ui/ports_details.rs` showing: Port, Protocol, State, PID, Process, Command, User, Connections. Same structure as `details.rs` but simpler (no tree view).

- [ ] **Step 2: Register module and verify**

Run: `cargo check`
Expected: Compiles.

- [ ] **Step 3: Commit**

```bash
git add -A && git commit -m "feat: add ports details panel rendering

Shows port, protocol, state, PID, process, command, user, and
connection count for the selected port."
```

---

## Task 11: UI — mode-aware header, footer, and help

**Files:**
- Modify: `src/ui/mod.rs` — header and footer dispatch on active_tool
- Modify: `src/ui/popup.rs` — mode-aware help, kill confirmation, kill progress

- [ ] **Step 1: Update header in `src/ui/mod.rs`**

Update `render_header()` to:
- Show mode indicator: `[Prune] Ports` or `Prune [Ports]` (styled with bg highlight on active)
- When active_tool is Ports: show port count, dev filter status, sort mode
- When active_tool is Prune: existing behavior unchanged

- [ ] **Step 2: Update footer in `src/ui/mod.rs`**

Update `render_footer()` to show mode-specific help text:
- Ports mode: `Tab:toggle | /:filter | s:sort | t:proto | a:dev filter | d:kill | r:refresh | ?:help`
- Prune mode: existing behavior unchanged

- [ ] **Step 3: Update help popup in `src/ui/popup.rs`**

Update `render_help()` signature from `render_help(frame: &mut Frame)` to `render_help(frame: &mut Frame, app: &App)` and show different keybindings based on `app.active_tool`:
- Ports mode help includes: `a` (dev filter), `r` (refresh), `d` (kill)
- Prune mode help: existing keybindings unchanged

- [ ] **Step 4: Add kill confirmation popup**

Add `render_kill_confirm()` in `src/ui/popup.rs`:
- List ports and process names to be killed
- Flag items owned by other users with warning
- Enter to confirm, Esc to cancel

- [ ] **Step 5: Add kill progress popup**

Add `render_killing()` in `src/ui/popup.rs`:
- Progress bar, current port/process, kill count

- [ ] **Step 6: Run tests and verify**

Run: `cargo test && cargo clippy`
Expected: All pass.

- [ ] **Step 7: Commit**

```bash
git add -A && git commit -m "feat: mode-aware header, footer, help, and kill popups

Header shows [Prune]/[Ports] indicator with mode-specific stats.
Footer shows mode-specific keybindings. Help popup adapts to active
tool. Kill confirmation and progress popups for ports mode."
```

---

## Task 12: Main render dispatch

Wire up the render function to dispatch to the correct list/details panels based on active_tool.

**Files:**
- Modify: `src/ui/mod.rs` — render() dispatches based on app.active_tool

- [ ] **Step 1: Update render() dispatch**

In `src/ui/mod.rs`, update the `render()` function:
```rust
pub fn render(frame: &mut Frame, app: &mut App) {
    let layout = AppLayout::build(frame.area());
    render_header(frame, layout.header, app);

    match app.active_tool {
        Tool::Prune => {
            list::render(frame, layout.list, app);
            details::render(frame, layout.details, app);
        }
        Tool::Ports => {
            ports_list::render(frame, layout.list, app);
            ports_details::render(frame, layout.details, app);
        }
    }

    render_footer(frame, layout.footer, app);

    // Popups (shared across modes)
    match app.mode {
        AppMode::SubFilter => popup::render_sub_filter(frame, app),
        AppMode::Confirm => popup::render_confirm(frame, app),
        AppMode::Processing => popup::render_processing(frame, app),
        AppMode::Help => popup::render_help(frame, app),
        _ => {}
    }
}
```

The popup rendering functions need to dispatch internally based on `app.active_tool` for mode-specific behavior (confirm shows delete items vs kill targets, processing shows delete progress vs kill progress).

- [ ] **Step 2: Verify it compiles**

Run: `cargo check`
Expected: Compiles.

- [ ] **Step 3: Commit**

```bash
git add -A && git commit -m "feat: wire render dispatch for Prune/Ports modes

render() dispatches to correct list/details panels based on
active_tool. Popup rendering adapts to active mode."
```

---

## Task 13: Event loop integration

Wire up the full event loop: key dispatch, channel polling, mode toggle, rescan, and kill flow for ports mode.

**Files:**
- Modify: `src/main.rs` — event loop, key handlers for ports mode

- [ ] **Step 1: Update `run()` to poll ports channels**

In the event loop, add ports channel polling alongside prune:
```rust
match app.active_tool {
    Tool::Prune => {
        // existing prune polling
        app.poll_scan_results();
        app.poll_tree_results();
        app.maybe_start_tree_scan();
    }
    Tool::Ports => {
        if let Some(ref mut ports) = app.ports {
            ports.poll_scan_results();
            ports.poll_kill_results();
        }
    }
}
```

- [ ] **Step 2: Add ports key handlers**

Add `handle_ports_normal_key()`:
- `j/k/↑/↓/g/G/Home/End` — navigation on PortsState
- `Space` — toggle selection
- `v` — invert selection
- `Ctrl+A` — select all
- `/` — enter filter mode
- `s` — cycle sort
- `t` — enter SubFilter mode (protocol)
- `a` — toggle dev filter + rescan
- `r` — rescan
- `d` — enter Confirm mode (if items selected)
- `l/Enter` — switch focus to details
- `Tab/1/2` — mode toggle (from Task 3)
- `?` — help
- `q` — quit

Add `handle_ports_details_key()`:
- `h/Esc` — back to list
- `j/k` — scroll details (if needed)
- `y` — copy path/port info

Add `handle_ports_filter_key()` — text input for filter, same pattern as prune.

Add `handle_ports_sub_filter_key()` — protocol selection (All, TCP, UDP).

Add `handle_ports_confirm_key()` — Enter to start kill, Esc to cancel.

- [ ] **Step 3: Update `handle_key()` dispatch**

```rust
fn handle_key(app: &mut App, key: KeyEvent) {
    match app.active_tool {
        Tool::Prune => handle_prune_key(app, key),
        Tool::Ports => handle_ports_key(app, key),
    }
}
```

Where `handle_prune_key` contains the existing dispatch logic, and `handle_ports_key` is the new one.

- [ ] **Step 4: Lazy init ports on first switch**

In the mode toggle handler, when switching to Ports for the first time:
```rust
Tool::Ports => {
    if app.ports.is_none() {
        let dev_filter = if app.config.ports.dev_filter_enabled {
            Some(parse_port_filter(&app.config.ports.dev_filter))
        } else {
            None
        };
        let mut ports = PortsState::new();
        ports.dev_filter_active = app.config.ports.dev_filter_enabled;
        ports.dev_filter_ports = dev_filter.unwrap_or_default();
        ports.start_scan(dev_filter);
        app.ports = Some(ports);
    }
}
```

- [ ] **Step 5: Handle `--ports` flag at startup**

In `main()`, if `cli.ports` is set, initialize ports immediately and start scan:
```rust
if cli.ports {
    app.active_tool = Tool::Ports;
    // Initialize and start scan
    app.ensure_ports_initialized();
}
```

- [ ] **Step 6: Run full test suite**

Run: `cargo test && cargo clippy`
Expected: All pass.

- [ ] **Step 7: Manual test**

Run: `cargo run -- --ports`
Expected: TUI launches in ports mode, shows open ports.

Test `Tab` to switch to prune mode and back.
Test `r` to refresh, `a` to toggle dev filter.
Test selecting ports and killing them with `d`.

- [ ] **Step 8: Commit**

```bash
git add -A && git commit -m "feat: integrate ports mode into event loop

Full key dispatch for ports mode (navigation, selection, filter, sort,
protocol filter, dev filter toggle, rescan, kill). Lazy port scan
initialization on first mode switch. --ports flag starts in ports mode."
```

---

## Task 14: Dry-run support for ports

**Files:**
- Modify: `src/main.rs` — `--dry-run --ports` prints ports to stdout

- [ ] **Step 1: Add dry-run ports output in `main()`**

In the existing `--dry-run` block (lines 113-132), add a branch for ports mode:
```rust
if cli.dry_run {
    if cli.ports {
        // Scan ports synchronously
        let dev_filter = if config.ports.dev_filter_enabled {
            Some(parse_port_filter(&config.ports.dev_filter))
        } else {
            None
        };
        let (tx, rx) = mpsc::channel();
        scan_ports(tx, dev_filter);
        let mut ports: Vec<PortInfo> = Vec::new();
        while let Ok(msg) = rx.recv() {
            match msg {
                PortScanMessage::Found(info) => ports.push(info),
                PortScanMessage::Complete => break,
                PortScanMessage::Error(e) => eprintln!("Error: {}", e),
            }
        }
        ports.sort_by_key(|p| p.port);
        println!("{:<8} {:<6} {:<8} {:<16} {}", "PORT", "PROTO", "PID", "PROCESS", "STATE");
        for p in &ports {
            let proto = match p.protocol {
                Protocol::Tcp => "TCP",
                Protocol::Udp => "UDP",
            };
            println!("{:<8} {:<6} {:<8} {:<16} {}", p.port, proto, p.pid, p.process_name, p.state);
        }
        return Ok(());
    }
    // ... existing prune dry-run logic ...
}
```

- [ ] **Step 2: Test**

Run: `cargo run -- --dry-run --ports`
Expected: Prints a table of open ports to stdout and exits.

- [ ] **Step 3: Commit**

```bash
git add -A && git commit -m "feat: add --dry-run --ports support

Prints port table to stdout without launching TUI. Respects dev
filter from config."
```

---

## Task 15: Final polish and integration test

**Files:**
- All files — final review pass

- [ ] **Step 1: Run full test suite**

Run: `cargo test`
Expected: All tests pass.

- [ ] **Step 2: Run clippy**

Run: `cargo clippy -- -D warnings`
Expected: No warnings.

- [ ] **Step 3: Manual end-to-end test**

Test the following scenarios:
1. `cargo run` — prune mode works as before
2. `cargo run -- --ports` — ports mode shows dev ports
3. `Tab` to switch between modes — state preserved
4. `1` and `2` to switch directly
5. `/` to filter ports by name
6. `t` to filter by protocol
7. `a` to toggle dev filter
8. `s` to cycle sort modes
9. Select ports with `Space`, kill with `d`
10. `r` to refresh after kill
11. `cargo run -- --dry-run` — prune dry-run unchanged
12. `cargo run -- --dry-run --ports` — ports dry-run works

- [ ] **Step 4: Commit**

```bash
git add -A && git commit -m "test: verify ports mode integration"
```
