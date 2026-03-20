use std::collections::{HashMap, HashSet};
use std::sync::mpsc;

use ratatui::widgets::ListState;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Protocol {
    Tcp,
    Udp,
}

// ── Sort mode ────────────────────────────────────────────────────────────────

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
            PortsSortMode::PortAsc => PortsSortMode::PortDesc,
            PortsSortMode::PortDesc => PortsSortMode::ProcessName,
            PortsSortMode::ProcessName => PortsSortMode::PidAsc,
            PortsSortMode::PidAsc => PortsSortMode::PortAsc,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            PortsSortMode::PortAsc => "Port \u{2191}",
            PortsSortMode::PortDesc => "Port \u{2193}",
            PortsSortMode::ProcessName => "Process",
            PortsSortMode::PidAsc => "PID",
        }
    }
}

// ── Kill messages ─────────────────────────────────────────────────────────────

pub enum KillMessage {
    Killing { port: u16, pid: u32, process: String },
    Killed { port: u16, pid: u32 },
    Error { port: u16, pid: u32, error: String },
    Complete,
}

// ── PortsState ────────────────────────────────────────────────────────────────

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

impl PortsState {
    pub fn new() -> Self {
        let mut list_state = ListState::default();
        list_state.select(Some(0));
        Self {
            items: Vec::new(),
            filtered_indices: Vec::new(),
            selected: Vec::new(),
            list_state,
            sort_mode: PortsSortMode::PortAsc,
            filter_text: String::new(),
            protocol_filter: None,
            protocol_filter_cursor: 0,
            dev_filter_active: false,
            dev_filter_ports: HashSet::new(),
            scan_complete: false,
            scan_rx: None,
            kill_rx: None,
            kill_progress: 0,
            kill_total: 0,
            kill_current: String::new(),
            kill_errors: Vec::new(),
        }
    }

    /// Start a port scan in a background thread.
    pub fn start_scan(&mut self, dev_filter: Option<HashSet<u16>>) {
        self.items.clear();
        self.filtered_indices.clear();
        self.selected.clear();
        self.scan_complete = false;

        let (tx, rx) = mpsc::channel();
        self.scan_rx = Some(rx);
        std::thread::spawn(move || scan_ports(tx, dev_filter));
    }

    /// Drain the scan channel non-blocking.
    pub fn poll_scan_results(&mut self) {
        let rx = match self.scan_rx.as_ref() {
            Some(rx) => rx,
            None => return,
        };

        loop {
            match rx.try_recv() {
                Ok(msg) => match msg {
                    PortScanMessage::Found(info) => {
                        self.items.push(info);
                        self.selected.push(false);
                    }
                    PortScanMessage::Complete => {
                        self.scan_complete = true;
                        self.scan_rx = None;
                        self.apply_filter();
                        return;
                    }
                    PortScanMessage::Error(_) => {}
                },
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.scan_complete = true;
                    self.scan_rx = None;
                    self.apply_filter();
                    return;
                }
            }
        }
    }

    /// Return the item currently under the cursor.
    pub fn current_item(&self) -> Option<&PortInfo> {
        let idx = self.list_state.selected()?;
        let &item_idx = self.filtered_indices.get(idx)?;
        self.items.get(item_idx)
    }

    /// Check if a single item passes the current text and protocol filters.
    pub fn item_passes_filter(&self, info: &PortInfo) -> bool {
        if !self.filter_text.is_empty() {
            let lower = self.filter_text.to_lowercase();
            let port_str = info.port.to_string();
            let pid_str = info.pid.to_string();
            let name_lower = info.process_name.to_lowercase();
            if !port_str.contains(&lower)
                && !name_lower.contains(&lower)
                && !pid_str.contains(&lower)
            {
                return false;
            }
        }
        if let Some(proto) = self.protocol_filter {
            if info.protocol != proto {
                return false;
            }
        }
        true
    }

    /// Rebuild filtered_indices: filter then sort.
    pub fn apply_filter(&mut self) {
        let mut indices: Vec<usize> = self
            .items
            .iter()
            .enumerate()
            .filter(|(_, info)| self.item_passes_filter(info))
            .map(|(i, _)| i)
            .collect();

        match self.sort_mode {
            PortsSortMode::PortAsc => {
                indices.sort_unstable_by_key(|&i| self.items[i].port);
            }
            PortsSortMode::PortDesc => {
                indices.sort_unstable_by(|&a, &b| self.items[b].port.cmp(&self.items[a].port));
            }
            PortsSortMode::ProcessName => {
                indices.sort_unstable_by(|&a, &b| {
                    self.items[a]
                        .process_name
                        .cmp(&self.items[b].process_name)
                });
            }
            PortsSortMode::PidAsc => {
                indices.sort_unstable_by_key(|&i| self.items[i].pid);
            }
        }

        self.filtered_indices = indices;

        // Clamp cursor
        if self.filtered_indices.is_empty() {
            self.list_state.select(Some(0));
        } else {
            let current = self.list_state.selected().unwrap_or(0);
            if current >= self.filtered_indices.len() {
                self.list_state
                    .select(Some(self.filtered_indices.len() - 1));
            }
        }
    }

    /// Toggle selection of the item at the given visible position.
    pub fn toggle_selection(&mut self, pos: usize) {
        if let Some(&item_idx) = self.filtered_indices.get(pos) {
            if item_idx < self.selected.len() {
                self.selected[item_idx] = !self.selected[item_idx];
            }
        }
    }

    /// Select all visible (filtered) items.
    pub fn select_all(&mut self) {
        for &idx in &self.filtered_indices {
            if idx < self.selected.len() {
                self.selected[idx] = true;
            }
        }
    }

    /// Invert selection of all visible (filtered) items.
    pub fn invert_selection(&mut self) {
        for &idx in &self.filtered_indices {
            if idx < self.selected.len() {
                self.selected[idx] = !self.selected[idx];
            }
        }
    }

    /// Count of selected items.
    pub fn selected_count(&self) -> usize {
        self.selected.iter().filter(|&&s| s).count()
    }

    /// Move cursor down.
    pub fn next(&mut self) {
        if self.filtered_indices.is_empty() {
            return;
        }
        let current = self.list_state.selected().unwrap_or(0);
        let next = (current + 1).min(self.filtered_indices.len() - 1);
        self.list_state.select(Some(next));
    }

    /// Move cursor up.
    pub fn previous(&mut self) {
        if self.filtered_indices.is_empty() {
            return;
        }
        let current = self.list_state.selected().unwrap_or(0);
        let prev = current.saturating_sub(1);
        self.list_state.select(Some(prev));
    }

    /// Jump to the top of the list.
    pub fn go_top(&mut self) {
        if !self.filtered_indices.is_empty() {
            self.list_state.select(Some(0));
        }
    }

    /// Jump to the bottom of the list.
    pub fn go_bottom(&mut self) {
        if !self.filtered_indices.is_empty() {
            let last = self.filtered_indices.len() - 1;
            self.list_state.select(Some(last));
        }
    }

    /// Cycle sort mode and re-apply filter.
    pub fn cycle_sort(&mut self) {
        self.sort_mode = self.sort_mode.next();
        self.apply_filter();
    }

    /// Return references to all selected items.
    pub fn selected_items(&self) -> Vec<&PortInfo> {
        self.items
            .iter()
            .enumerate()
            .filter(|(i, _)| self.selected.get(*i).copied().unwrap_or(false))
            .map(|(_, item)| item)
            .collect()
    }

}

/// Internal struct used during lsof parsing, before deduplication.
#[derive(Debug, Clone)]
pub struct PortEntry {
    pub port: u16,
    pub protocol: Protocol,
    pub pid: u32,
    pub process_name: String,
    pub user: String,
    pub state: String,
}

/// Public struct representing a unique port with aggregated connection info.
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

/// Parses the NAME column from an lsof line and returns `(port, state)`.
///
/// Handles formats:
/// - `*:3000 (LISTEN)`
/// - `*:5353`
/// - `127.0.0.1:8080->127.0.0.1:52341 (ESTABLISHED)`
/// - `[::1]:3000 (LISTEN)`
pub fn parse_port_from_name(name: &str) -> Option<(u16, String)> {
    // Extract optional trailing state like "(LISTEN)" or "(ESTABLISHED)"
    let (addr_part, state) = if let Some(idx) = name.rfind('(') {
        let state_raw = name[idx..].trim_matches(|c| c == '(' || c == ')').trim().to_string();
        (name[..idx].trim(), state_raw)
    } else {
        (name.trim(), String::new())
    };

    // For connection lines like "src->dst", take the source part only
    let local_part = if let Some(arrow_idx) = addr_part.find("->") {
        &addr_part[..arrow_idx]
    } else {
        addr_part
    };

    // Find the last ':' to split host and port
    let colon_idx = local_part.rfind(':')?;
    let port_str = &local_part[colon_idx + 1..];
    let port: u16 = port_str.trim().parse().ok()?;

    Some((port, state))
}

/// Parses one line of `lsof -i -n -P +c 0` output into a `PortEntry`.
///
/// Expected columns (whitespace-separated):
/// COMMAND PID USER FD TYPE DEVICE SIZE/OFF NODE NAME
///
/// NODE is "TCP" or "UDP". Header lines and non-TCP/UDP lines are skipped.
pub fn parse_lsof_line(line: &str) -> Option<PortEntry> {
    let mut fields = line.split_whitespace();

    let process_name = fields.next()?.to_string();

    // Skip header line
    if process_name == "COMMAND" {
        return None;
    }

    let pid_str = fields.next()?;
    let pid: u32 = pid_str.parse().ok()?;

    let user = fields.next()?.to_string();
    let _fd = fields.next()?;
    let _type_field = fields.next()?;
    let _device = fields.next()?;
    let _size_off = fields.next()?;
    let node = fields.next()?;

    let protocol = match node.to_uppercase().as_str() {
        "TCP" => Protocol::Tcp,
        "UDP" => Protocol::Udp,
        _ => return None,
    };

    // Remaining tokens form the NAME column
    let name: String = fields.collect::<Vec<_>>().join(" ");
    if name.is_empty() {
        return None;
    }

    let (port, state) = parse_port_from_name(&name)?;

    Some(PortEntry {
        port,
        protocol,
        pid,
        process_name,
        user,
        state,
    })
}

/// Deduplicates `PortEntry` list by `(port, protocol)`.
///
/// Keeps the entry with the LISTEN state when available. Tracks total
/// connection count across all entries for the same (port, protocol) key.
pub fn dedup_port_entries(entries: Vec<PortEntry>) -> Vec<PortInfo> {
    let mut map: HashMap<(u16, Protocol), PortInfo> = HashMap::new();

    for entry in entries {
        let key = (entry.port, entry.protocol);
        match map.entry(key) {
            std::collections::hash_map::Entry::Vacant(v) => {
                v.insert(PortInfo {
                    port: entry.port,
                    protocol: entry.protocol,
                    pid: entry.pid,
                    process_name: entry.process_name,
                    command: String::new(),
                    user: entry.user,
                    state: entry.state,
                    connections: 1,
                });
            }
            std::collections::hash_map::Entry::Occupied(mut o) => {
                let existing = o.get_mut();
                existing.connections += 1;
                // Prefer the LISTEN state entry as the canonical one
                if entry.state == "LISTEN" && existing.state != "LISTEN" {
                    existing.pid = entry.pid;
                    existing.process_name = entry.process_name;
                    existing.user = entry.user;
                    existing.state = entry.state;
                }
            }
        }
    }

    map.into_values().collect()
}

/// Batch-fetches full command strings for the given PIDs via `ps`.
///
/// Runs `ps -p <pid,...> -o pid=,command=` and parses each output line.
pub fn fetch_commands(pids: &[u32]) -> HashMap<u32, String> {
    if pids.is_empty() {
        return HashMap::new();
    }

    let pid_list: Vec<String> = pids.iter().map(|p| p.to_string()).collect();
    let pid_arg = pid_list.join(",");

    let output = match std::process::Command::new("ps")
        .args(["-p", &pid_arg, "-o", "pid=,command="])
        .output()
    {
        Ok(o) => o,
        Err(_) => return HashMap::new(),
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut result = HashMap::new();

    for line in stdout.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        // First token is PID, rest is the command
        if let Some(space_idx) = trimmed.find(char::is_whitespace) {
            let pid_str = &trimmed[..space_idx];
            let command = trimmed[space_idx..].trim().to_string();
            if let Ok(pid) = pid_str.parse::<u32>() {
                result.insert(pid, command);
            }
        }
    }

    result
}

/// Scans open ports by running `lsof -i -n -P +c 0`, parses, deduplicates,
/// optionally filters to a set of ports, fetches full commands, then sends
/// each `PortInfo` over the channel followed by `PortScanMessage::Complete`.
pub fn scan_ports(tx: mpsc::Sender<PortScanMessage>, dev_filter: Option<HashSet<u16>>) {
    let output = match std::process::Command::new("lsof")
        .args(["-i", "-n", "-P", "+c", "0"])
        .output()
    {
        Ok(o) => o,
        Err(e) => {
            let _ = tx.send(PortScanMessage::Error(format!("lsof failed: {e}")));
            return;
        }
    };

    let stdout = String::from_utf8_lossy(&output.stdout);

    let entries: Vec<PortEntry> = stdout
        .lines()
        .filter_map(parse_lsof_line)
        .collect();

    let mut port_infos = dedup_port_entries(entries);

    // Apply optional dev filter
    if let Some(ref filter) = dev_filter {
        port_infos.retain(|p| filter.contains(&p.port));
    }

    // Fetch full command strings for all unique PIDs
    let pids: Vec<u32> = port_infos.iter().map(|p| p.pid).collect();
    let commands = fetch_commands(&pids);

    for mut info in port_infos {
        if let Some(cmd) = commands.get(&info.pid) {
            info.command = cmd.clone();
        }
        let _ = tx.send(PortScanMessage::Found(info));
    }

    let _ = tx.send(PortScanMessage::Complete);
}

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

    // ── PortsState tests ──────────────────────────────────────────────────────

    fn make_port_info(port: u16, process: &str) -> PortInfo {
        PortInfo {
            port,
            protocol: Protocol::Tcp,
            pid: port as u32,
            process_name: process.into(),
            command: String::new(),
            user: "test".into(),
            state: "LISTEN".into(),
            connections: 1,
        }
    }

    fn make_port_info_udp(port: u16, process: &str) -> PortInfo {
        PortInfo {
            port,
            protocol: Protocol::Udp,
            pid: port as u32,
            process_name: process.into(),
            command: String::new(),
            user: "test".into(),
            state: String::new(),
            connections: 1,
        }
    }

    #[test]
    fn test_ports_state_filter_text() {
        let mut state = PortsState::new();
        state.items = vec![make_port_info(3000, "node"), make_port_info(8080, "java")];
        state.selected = vec![false; 2];
        state.filter_text = "node".into();
        state.apply_filter();
        assert_eq!(state.filtered_indices.len(), 1);
        assert_eq!(state.items[state.filtered_indices[0]].port, 3000);
    }

    #[test]
    fn test_ports_state_sort_by_port() {
        let mut state = PortsState::new();
        state.items = vec![make_port_info(8080, "java"), make_port_info(3000, "node")];
        state.selected = vec![false; 2];
        state.sort_mode = PortsSortMode::PortAsc;
        state.apply_filter();
        assert_eq!(state.items[state.filtered_indices[0]].port, 3000);
        assert_eq!(state.items[state.filtered_indices[1]].port, 8080);
    }

    #[test]
    fn test_ports_state_protocol_filter() {
        let mut state = PortsState::new();
        state.items = vec![
            make_port_info(3000, "node"),
            make_port_info_udp(5353, "mDNSResponder"),
        ];
        state.selected = vec![false; 2];
        state.protocol_filter = Some(Protocol::Tcp);
        state.apply_filter();
        assert_eq!(state.filtered_indices.len(), 1);
    }

    #[test]
    fn test_ports_state_selection() {
        let mut state = PortsState::new();
        state.items = vec![make_port_info(3000, "node"), make_port_info(3001, "node")];
        state.selected = vec![false; 2];
        state.apply_filter();
        state.toggle_selection(0);
        assert!(state.selected[state.filtered_indices[0]]);
        assert_eq!(state.selected_count(), 1);
    }

}
