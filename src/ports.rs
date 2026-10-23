use std::collections::{HashMap, HashSet};
use std::sync::mpsc;

use nix::sys::signal::{self, Signal};
use nix::unistd::Pid;
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

#[allow(dead_code)]
pub enum KillMessage {
    Killing {
        port: u16,
        pid: u32,
        process: String,
    },
    Killed {
        port: u16,
        pid: u32,
    },
    Error {
        port: u16,
        pid: u32,
        error: String,
    },
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
    pub scan_error: Option<String>,
    pub scan_tick: u8,
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
            scan_error: None,
            scan_tick: 0,
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
        self.scan_error = None;

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
                    PortScanMessage::Error(e) => self.scan_error = Some(e),
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
        if let Some(proto) = self.protocol_filter
            && info.protocol != proto
        {
            return false;
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
                    self.items[a].process_name.cmp(&self.items[b].process_name)
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
            // Also restores a selection ratatui cleared while the list was empty
            let current = self.list_state.selected().unwrap_or(0);
            let last = self.filtered_indices.len() - 1;
            self.list_state.select(Some(current.min(last)));
        }
    }

    /// Toggle selection of the item at the given visible position.
    pub fn toggle_selection(&mut self, pos: usize) {
        if let Some(&item_idx) = self.filtered_indices.get(pos)
            && item_idx < self.selected.len()
        {
            self.selected[item_idx] = !self.selected[item_idx];
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

    // ── Kill support ──────────────────────────────────────────────────────────

    /// Spawn a kill thread for all selected items.
    pub fn start_killing(&mut self) {
        let targets: Vec<PortInfo> = self
            .selected
            .iter()
            .enumerate()
            .filter(|&(_, &sel)| sel)
            .map(|(i, _)| self.items[i].clone())
            .collect();

        if targets.is_empty() {
            return;
        }

        self.kill_total = targets.len();
        self.kill_progress = 0;
        self.kill_errors.clear();

        let (tx, rx) = mpsc::channel();
        self.kill_rx = Some(rx);
        std::thread::spawn(move || kill_ports(targets, tx));
    }

    /// Drain the kill channel. Returns `true` when kill is complete.
    pub fn poll_kill_results(&mut self) -> bool {
        let rx = match self.kill_rx.as_ref() {
            Some(rx) => rx,
            None => return false,
        };

        loop {
            match rx.try_recv() {
                Ok(msg) => match msg {
                    KillMessage::Killing { process, .. } => {
                        self.kill_current = process;
                    }
                    KillMessage::Killed { .. } => {
                        self.kill_progress += 1;
                    }
                    KillMessage::Error { port, pid, error } => {
                        self.kill_progress += 1;
                        self.kill_errors
                            .push(format!(":{port} (PID {pid}): {error}"));
                    }
                    KillMessage::Complete => {
                        self.kill_rx = None;
                        // Trigger a rescan to reflect the killed processes
                        let filter = if self.dev_filter_active {
                            Some(self.dev_filter_ports.clone())
                        } else {
                            None
                        };
                        self.start_scan(filter);
                        return true;
                    }
                },
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.kill_rx = None;
                    return true;
                }
            }
        }

        false
    }
}

// ── Kill logic ────────────────────────────────────────────────────────────────

/// Verify that `pid` still refers to `expected_name` by consulting `ps`.
fn verify_process(pid: u32, expected_name: &str) -> bool {
    let output = match std::process::Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "comm="])
        .output()
    {
        Ok(o) => o,
        Err(_) => return false,
    };
    let stdout = String::from_utf8_lossy(&output.stdout);
    let actual = stdout.trim();
    // ps may truncate the command name, so check if either string contains the other
    !actual.is_empty() && (actual.contains(expected_name) || expected_name.contains(actual))
}

/// True while `pid` exists, including processes owned by other users (EPERM).
fn is_alive(pid: u32) -> bool {
    !matches!(
        signal::kill(Pid::from_raw(pid as i32), None),
        Err(nix::errno::Errno::ESRCH)
    )
}

/// Time a process gets to exit after SIGTERM before being SIGKILLed.
const KILL_GRACE: std::time::Duration = std::time::Duration::from_secs(3);

/// Kill the selected processes: SIGTERM, then SIGKILL survivors after a grace period.
pub fn kill_ports(targets: Vec<PortInfo>, tx: mpsc::Sender<KillMessage>) {
    kill_ports_with_grace(targets, tx, KILL_GRACE);
}

fn kill_ports_with_grace(
    targets: Vec<PortInfo>,
    tx: mpsc::Sender<KillMessage>,
    grace: std::time::Duration,
) {
    // One signal per process, even when it owns several selected ports
    let mut processes: Vec<(u32, String, Vec<u16>)> = Vec::new();
    for t in targets {
        match processes.iter_mut().find(|(pid, ..)| *pid == t.pid) {
            Some((_, _, ports)) => ports.push(t.port),
            None => processes.push((t.pid, t.process_name, vec![t.port])),
        }
    }

    let report = |pid: u32, ports: &[u16], error: Option<String>| {
        for &port in ports {
            let _ = tx.send(match &error {
                None => KillMessage::Killed { port, pid },
                Some(e) => KillMessage::Error {
                    port,
                    pid,
                    error: e.clone(),
                },
            });
        }
    };

    // SIGTERM everything first so processes shut down concurrently
    let mut terminating = Vec::new();
    for (pid, name, ports) in &processes {
        let _ = tx.send(KillMessage::Killing {
            port: ports[0],
            pid: *pid,
            process: name.clone(),
        });
        if !is_alive(*pid) {
            // Already exited: the port is free
            report(*pid, ports, None);
        } else if !verify_process(*pid, name) {
            report(*pid, ports, Some(format!("PID no longer runs {name}")));
        } else if let Err(e) = signal::kill(Pid::from_raw(*pid as i32), Signal::SIGTERM) {
            report(*pid, ports, Some(format!("SIGTERM failed: {e}")));
        } else {
            terminating.push((*pid, name, ports));
        }
    }

    let deadline = std::time::Instant::now() + grace;
    while terminating.iter().any(|(pid, ..)| is_alive(*pid)) && std::time::Instant::now() < deadline
    {
        std::thread::sleep(std::time::Duration::from_millis(50));
    }

    for (pid, name, ports) in terminating {
        // Re-check identity: the PID may have been reused after the process exited
        if is_alive(pid)
            && verify_process(pid, name)
            && let Err(e) = signal::kill(Pid::from_raw(pid as i32), Signal::SIGKILL)
        {
            report(pid, ports, Some(format!("SIGKILL failed: {e}")));
            continue;
        }
        report(pid, ports, None);
    }

    let _ = tx.send(KillMessage::Complete);
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
    /// NAME has a remote end (`local->remote`)
    pub connected: bool,
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
        let state_raw = name[idx..]
            .trim_matches(|c| c == '(' || c == ')')
            .trim()
            .to_string();
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

/// Decodes the `\xNN` escapes lsof uses for spaces and non-printable bytes.
fn unescape_lsof(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\'
            && bytes.get(i + 1) == Some(&b'x')
            && let (Some(hi), Some(lo)) = (
                bytes.get(i + 2).and_then(|b| (*b as char).to_digit(16)),
                bytes.get(i + 3).and_then(|b| (*b as char).to_digit(16)),
            )
        {
            out.push((hi * 16 + lo) as u8);
            i += 4;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Parses one line of `lsof -i -n -P +c 0` output into a `PortEntry`.
///
/// Expected columns (whitespace-separated):
/// COMMAND PID USER FD TYPE DEVICE SIZE/OFF NODE NAME
///
/// NODE is "TCP" or "UDP". Header lines and non-TCP/UDP lines are skipped.
pub fn parse_lsof_line(line: &str) -> Option<PortEntry> {
    let mut fields = line.split_whitespace();

    let process_name = unescape_lsof(fields.next()?);

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
        connected: name.contains("->"),
    })
}

/// Keeps only bound sockets (TCP LISTEN, unconnected UDP), deduplicated by
/// `(port, protocol, pid)` so IPv4/IPv6 listeners of one process merge while
/// distinct processes on the same port stay separate. Established TCP
/// connections are counted on their local port.
pub fn dedup_port_entries(entries: Vec<PortEntry>) -> Vec<PortInfo> {
    let mut map: HashMap<(u16, Protocol, u32), PortInfo> = HashMap::new();
    let mut connections: HashMap<(u16, Protocol), usize> = HashMap::new();

    for entry in entries {
        let bound = match entry.protocol {
            Protocol::Tcp => entry.state == "LISTEN",
            Protocol::Udp => !entry.connected,
        };
        if !bound {
            if entry.protocol == Protocol::Tcp && entry.connected {
                *connections.entry((entry.port, entry.protocol)).or_default() += 1;
            }
            continue;
        }
        map.entry((entry.port, entry.protocol, entry.pid))
            .or_insert(PortInfo {
                port: entry.port,
                protocol: entry.protocol,
                pid: entry.pid,
                process_name: entry.process_name,
                command: String::new(),
                user: entry.user,
                state: entry.state,
                connections: 0,
            });
    }

    for info in map.values_mut() {
        info.connections = connections
            .get(&(info.port, info.protocol))
            .copied()
            .unwrap_or(0);
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

    let entries: Vec<PortEntry> = stdout.lines().filter_map(parse_lsof_line).collect();

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
    fn test_parse_lsof_unescapes_command() {
        // lsof prints spaces and non-printable bytes in COMMAND as \xNN
        let line = r"Browser\x20Helper 49517 me 12u IPv4 0xdef 0t0 UDP *:5353";
        assert_eq!(
            parse_lsof_line(line).unwrap().process_name,
            "Browser Helper"
        );
        let line = r"caf\xc3\xa9 1 me 12u IPv4 0xdef 0t0 UDP *:5353";
        assert_eq!(parse_lsof_line(line).unwrap().process_name, "café");
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
        assert!(info.connected);
    }

    fn entry(port: u16, protocol: Protocol, pid: u32, state: &str, connected: bool) -> PortEntry {
        PortEntry {
            port,
            protocol,
            pid,
            process_name: "node".into(),
            user: "me".into(),
            state: state.into(),
            connected,
        }
    }

    #[test]
    fn test_dedup_ports() {
        let entries = vec![
            entry(3000, Protocol::Tcp, 123, "LISTEN", false),
            entry(3000, Protocol::Tcp, 123, "ESTABLISHED", true),
            entry(3000, Protocol::Tcp, 124, "ESTABLISHED", true),
        ];
        let deduped = dedup_port_entries(entries);
        assert_eq!(deduped.len(), 1);
        assert_eq!(deduped[0].state, "LISTEN");
        assert_eq!(deduped[0].pid, 123);
        assert_eq!(deduped[0].connections, 2);
    }

    #[test]
    fn test_dedup_ignores_outgoing_connections() {
        // Ephemeral local port of an outgoing connection is not a port to kill
        let entries = vec![entry(64231, Protocol::Tcp, 962, "ESTABLISHED", true)];
        assert!(dedup_port_entries(entries).is_empty());
    }

    #[test]
    fn test_dedup_keeps_distinct_processes_on_same_port() {
        let entries = vec![
            entry(3000, Protocol::Tcp, 1, "LISTEN", false), // IPv4
            entry(3000, Protocol::Tcp, 1, "LISTEN", false), // IPv6, same process
            entry(3000, Protocol::Tcp, 2, "LISTEN", false), // another process
        ];
        let mut pids: Vec<u32> = dedup_port_entries(entries).iter().map(|p| p.pid).collect();
        pids.sort();
        assert_eq!(pids, vec![1, 2]);
    }

    #[test]
    fn test_dedup_keeps_bound_udp_only() {
        let entries = vec![
            entry(5353, Protocol::Udp, 10, "", false),
            entry(58000, Protocol::Udp, 11, "", true), // connected (e.g. QUIC client)
        ];
        let deduped = dedup_port_entries(entries);
        assert_eq!(deduped.len(), 1);
        assert_eq!(deduped[0].port, 5353);
    }

    #[test]
    fn test_parse_port_from_name_column() {
        assert_eq!(
            parse_port_from_name("*:3000 (LISTEN)"),
            Some((3000, "LISTEN".to_string()))
        );
        assert_eq!(parse_port_from_name("*:5353"), Some((5353, "".to_string())));
        assert_eq!(
            parse_port_from_name("127.0.0.1:8080->127.0.0.1:52341 (ESTABLISHED)"),
            Some((8080, "ESTABLISHED".to_string()))
        );
        assert_eq!(
            parse_port_from_name("[::1]:3000 (LISTEN)"),
            Some((3000, "LISTEN".to_string()))
        );
    }

    #[test]
    fn test_dev_filter() {
        let filter: HashSet<u16> = (3000..=3009).collect();
        let entries = vec![
            PortInfo {
                port: 3000,
                protocol: Protocol::Tcp,
                pid: 1,
                process_name: "node".into(),
                command: "".into(),
                user: "me".into(),
                state: "LISTEN".into(),
                connections: 1,
            },
            PortInfo {
                port: 22,
                protocol: Protocol::Tcp,
                pid: 2,
                process_name: "sshd".into(),
                command: "".into(),
                user: "root".into(),
                state: "LISTEN".into(),
                connections: 1,
            },
        ];
        let filtered: Vec<_> = entries
            .into_iter()
            .filter(|p| filter.contains(&p.port))
            .collect();
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
    fn test_ports_scan_error_is_kept() {
        let mut state = PortsState::new();
        let (tx, rx) = mpsc::channel();
        state.scan_rx = Some(rx);
        tx.send(PortScanMessage::Error("lsof failed: not found".into()))
            .unwrap();
        drop(tx);
        state.poll_scan_results();
        assert!(state.scan_complete);
        assert_eq!(state.scan_error.as_deref(), Some("lsof failed: not found"));
    }

    #[test]
    fn test_ports_first_item_selected_after_empty_render() {
        let mut state = PortsState::new();
        // ratatui's List clears the selection when it renders an empty list
        state.list_state.select(None);
        state.items = vec![make_port_info(3000, "node"), make_port_info(3001, "node")];
        state.selected = vec![false; 2];
        state.apply_filter();
        assert_eq!(state.current_item().map(|p| p.port), Some(3000));
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

    // ── Kill tests ────────────────────────────────────────────────────────────

    #[test]
    fn test_ports_state_has_own_scan_tick() {
        let mut state = PortsState::new();
        assert_eq!(state.scan_tick, 0);
        state.scan_tick = state.scan_tick.wrapping_add(1);
        assert_eq!(state.scan_tick, 1);
        // Verify wrapping at u8 boundary
        state.scan_tick = 255;
        state.scan_tick = state.scan_tick.wrapping_add(1);
        assert_eq!(state.scan_tick, 0);
    }

    #[test]
    fn test_ports_state_select_all() {
        let mut state = PortsState::new();
        state.items = vec![
            make_port_info(3000, "node"),
            make_port_info(3001, "node"),
            make_port_info(8080, "java"),
        ];
        state.selected = vec![false; 3];
        state.apply_filter();
        state.select_all();
        assert_eq!(state.selected_count(), 3);
    }

    #[test]
    fn test_ports_state_invert_selection() {
        let mut state = PortsState::new();
        state.items = vec![make_port_info(3000, "node"), make_port_info(3001, "node")];
        state.selected = vec![true, false];
        state.apply_filter();
        state.invert_selection();
        assert!(!state.selected[0]);
        assert!(state.selected[1]);
    }

    #[test]
    fn test_ports_state_navigation() {
        let mut state = PortsState::new();
        state.items = vec![
            make_port_info(3000, "node"),
            make_port_info(3001, "node"),
            make_port_info(3002, "node"),
        ];
        state.selected = vec![false; 3];
        state.apply_filter();

        assert_eq!(state.list_state.selected(), Some(0));
        state.next();
        assert_eq!(state.list_state.selected(), Some(1));
        state.next();
        assert_eq!(state.list_state.selected(), Some(2));
        // Should clamp at the end
        state.next();
        assert_eq!(state.list_state.selected(), Some(2));

        state.previous();
        assert_eq!(state.list_state.selected(), Some(1));
        state.go_top();
        assert_eq!(state.list_state.selected(), Some(0));
        state.go_bottom();
        assert_eq!(state.list_state.selected(), Some(2));
    }

    #[test]
    fn test_ports_state_cycle_sort() {
        let mut state = PortsState::new();
        assert_eq!(state.sort_mode, PortsSortMode::PortAsc);
        state.items = vec![make_port_info(3000, "node")];
        state.selected = vec![false];
        state.cycle_sort();
        assert_eq!(state.sort_mode, PortsSortMode::PortDesc);
        state.cycle_sort();
        assert_eq!(state.sort_mode, PortsSortMode::ProcessName);
        state.cycle_sort();
        assert_eq!(state.sort_mode, PortsSortMode::PidAsc);
        state.cycle_sort();
        assert_eq!(state.sort_mode, PortsSortMode::PortAsc);
    }

    fn alive(pid: u32) -> bool {
        !matches!(
            signal::kill(Pid::from_raw(pid as i32), None),
            Err(nix::errno::Errno::ESRCH)
        )
    }

    fn wait_dead(pid: u32) -> bool {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while alive(pid) && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        !alive(pid)
    }

    /// Spawn `sleep` detached from the test so init reaps it once killed.
    fn spawn_sleep(prefix: &str) -> u32 {
        let out = std::process::Command::new("sh")
            .arg("-c")
            .arg(format!(
                "({prefix} exec sleep 30) >/dev/null 2>&1 & echo $!"
            ))
            .output()
            .unwrap();
        let pid: u32 = String::from_utf8_lossy(&out.stdout).trim().parse().unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while !verify_process(pid, "sleep") && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        pid
    }

    fn target(port: u16, pid: u32, process: &str) -> PortInfo {
        PortInfo {
            pid,
            ..make_port_info(port, process)
        }
    }

    fn run_kill(targets: Vec<PortInfo>, grace_ms: u64) -> (Vec<u16>, Vec<String>) {
        let (tx, rx) = mpsc::channel();
        kill_ports_with_grace(targets, tx, std::time::Duration::from_millis(grace_ms));
        let (mut killed, mut errors) = (Vec::new(), Vec::new());
        for msg in rx {
            match msg {
                KillMessage::Killed { port, .. } => killed.push(port),
                KillMessage::Error { error, .. } => errors.push(error),
                _ => {}
            }
        }
        killed.sort();
        (killed, errors)
    }

    #[test]
    fn test_kill_terminates_process() {
        let pid = spawn_sleep("");
        let (killed, errors) = run_kill(vec![target(3000, pid, "sleep")], 2000);
        assert!(wait_dead(pid));
        assert_eq!(killed, vec![3000]);
        assert!(errors.is_empty(), "{errors:?}");
    }

    #[test]
    fn test_kill_escalates_to_sigkill() {
        let pid = spawn_sleep("trap '' TERM;");
        let (killed, errors) = run_kill(vec![target(3000, pid, "sleep")], 200);
        assert!(wait_dead(pid));
        assert_eq!(killed, vec![3000]);
        assert!(errors.is_empty(), "{errors:?}");
    }

    #[test]
    fn test_kill_signals_each_process_once() {
        // One process listening on two selected ports
        let pid = spawn_sleep("");
        let targets = vec![target(3000, pid, "sleep"), target(3001, pid, "sleep")];
        let (killed, errors) = run_kill(targets, 2000);
        assert!(wait_dead(pid));
        assert_eq!(killed, vec![3000, 3001]);
        assert!(errors.is_empty(), "{errors:?}");
    }

    #[test]
    fn test_kill_spares_pid_running_another_process() {
        // Scanned as "node", but the PID now runs something else
        let pid = spawn_sleep("");
        let (killed, errors) = run_kill(vec![target(3000, pid, "node")], 200);
        let still_alive = alive(pid);
        let _ = signal::kill(Pid::from_raw(pid as i32), Signal::SIGKILL);
        assert!(
            still_alive,
            "no signal must reach a PID that changed process"
        );
        assert!(killed.is_empty());
        assert_eq!(errors.len(), 1);
    }

    #[test]
    fn test_kill_counts_exited_process_as_killed() {
        let pid = spawn_sleep("");
        let _ = signal::kill(Pid::from_raw(pid as i32), Signal::SIGKILL);
        assert!(wait_dead(pid));
        let (killed, errors) = run_kill(vec![target(3000, pid, "sleep")], 200);
        assert_eq!(killed, vec![3000]);
        assert!(errors.is_empty(), "{errors:?}");
    }

    #[test]
    fn test_kill_message_types() {
        let msg = KillMessage::Killing {
            port: 3000,
            pid: 123,
            process: "node".into(),
        };
        match msg {
            KillMessage::Killing { port, pid, .. } => {
                assert_eq!(port, 3000);
                assert_eq!(pid, 123);
            }
            _ => panic!("wrong variant"),
        }
    }
}
