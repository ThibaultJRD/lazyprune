use std::collections::{HashMap, HashSet};
use std::sync::mpsc;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Protocol {
    Tcp,
    Udp,
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
}
