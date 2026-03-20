# Ports Mode — Design Spec

## Overview

Add a port management mode to lazyprune. Users can list open ports, filter, select, and kill processes — the same interaction pattern as the existing prune mode. Entry via `lazyprune --ports`, or toggle at runtime with `Tab` / `1` / `2`.

## Architecture

### Approach: Enum mode + separate states

```
App
├── active_tool: Tool (Prune | Ports)
├── mode: AppMode (Normal, Filter, SubFilter, Confirm, Help, Processing)
├── focus: FocusPanel (List | Details)
├── prune: PruneState        ← extracted from current app.rs
│   ├── items: Vec<ScanResult>
│   ├── selected: Vec<bool>
│   ├── filtered_indices: Vec<usize>
│   ├── sort_mode, filter, type_filter...
│   └── scanner channel, tree channel...
│
├── ports: PortsState         ← new
│   ├── items: Vec<PortInfo>
│   ├── selected: Vec<bool>
│   ├── filtered_indices: Vec<usize>
│   ├── sort_mode, filter, protocol_filter...
│   └── scanner channel...
│
└── config: Config            ← shared, extended with [ports]
```

Each mode owns its state independently. Switching modes preserves cursor, selection, and filters.

### AppMode changes

Rename mode variants to be tool-agnostic:
- `TypeFilter` → `SubFilter` (dispatches to type filter in Prune, protocol filter in Ports based on `active_tool`)
- `Deleting` → `Processing` (covers both file deletion and port killing)

### Mode toggle

- Keys: `Tab`, `1` (Prune), `2` (Ports)
- **Only active in `Normal` mode** — ignored during Filter, SubFilter, Confirm, Help, and Processing modes
- On switch: mode resets to Normal, focus resets to List
- Lazy init: the inactive mode only scans when first switched to
- State preservation: switching back restores the full state

### Launch

- `lazyprune` — starts in Prune mode, auto-scans
- `lazyprune --ports` — starts in Ports mode, auto-scans
- `--dry-run --ports` — lists open ports to stdout (port, protocol, PID, process, state) without launching TUI, consistent with existing `--dry-run` behavior for prune

Note: no `-p` short flag to avoid conceptual overlap with the `p` TUI keybinding (project grouping in Prune mode).

## Port scanning

### PortInfo struct

```rust
struct PortInfo {
    port: u16,
    protocol: Protocol,    // TCP | UDP
    pid: u32,
    process_name: String,  // "node", "java"...
    command: String,       // full command path/args (from ps follow-up)
    user: String,
    state: String,         // "LISTEN", "ESTABLISHED", "" (UDP)
}
```

### Scan mechanism

Spawns a thread that:

1. Runs `lsof -i -n -P +c 0` to get full process names
2. Parses output line by line (whitespace-separated columns: COMMAND, PID, USER, FD, TYPE, DEVICE, SIZE/OFF, NODE, NAME)
3. Extracts port number and state from the NAME column (e.g., `*:3000 (LISTEN)` → port 3000, state LISTEN; UDP entries have no parenthetical state → empty string)
4. Deduplicates by `(port, protocol)`: if multiple rows exist for the same port/protocol pair (IPv4 + IPv6 listeners, LISTEN + ESTABLISHED), keep the LISTEN entry. Store connection count for display in details panel.
5. Runs `ps -p <pids> -o pid=,command=` as a single batch call to get full command lines for all discovered PIDs
6. Applies dev filter at parse time — ports outside configured ranges are not sent

Messages via `mpsc::channel`:
- `PortScanMessage::Found(PortInfo)`
- `PortScanMessage::Complete`
- `PortScanMessage::Error(String)`

Scan is near-instant (~50ms), no spinner or progress needed.

### Rescan lifecycle

When `r` is pressed or dev filter is toggled with `a`:
1. Drop the existing `scan_rx` channel
2. Reset `PortsState` items, selected, filtered_indices
3. Spawn a new scan thread with a fresh channel
4. Old thread will exit naturally when its `scan_tx` send fails

### Dev filter

Configurable list of port ranges. Enabled by default. Ports outside configured ranges are filtered out at scan time.

Toggle with `a` at runtime: flips the filter flag and triggers a rescan.

## Kill behavior

1. Collect PIDs from selected ports
2. Show confirmation popup listing ports, process names, and user (flag items owned by other users that may fail due to permissions)
3. On confirm: send SIGTERM via `nix::sys::signal::kill()` (new `nix` crate dependency)
4. Wait ~500ms, then for each PID that is still alive: verify it is still the same process (compare process name via `kill(pid, 0)` + process check) before sending SIGKILL
5. Report results: successful kills, permission errors, and already-exited processes (same pattern as `delete_errors` in prune mode)
6. Auto-refresh (rescan) after kill completes

No force-kill option — the SIGTERM → SIGKILL fallback handles all cases.

## UI

### Layout

Same two-panel layout as Prune mode (55/45 split), same header/footer structure.

### Header

Displays active mode indicator (e.g., `[Prune] Ports` or `Prune [Ports]`), port count, dev filter status (e.g., `Dev filter: ON`).

### List panel

```
  PORT   PROTO  PID     PROCESS     STATE
● 3000   TCP    12345   node        LISTEN
  3001   TCP    12346   node        LISTEN
● 5173   TCP    14201   node        LISTEN
  8080   TCP    9832    java        LISTEN
```

- Selection marker `●` like Prune mode
- Color by state: green for LISTEN, gray for others

### Details panel

```
Port:         3000
Protocol:     TCP
State:        LISTEN
PID:          12345
Process:      node
Command:      /usr/local/bin/node ./server.js
User:         thibault
Connections:  3 (1 LISTEN, 2 ESTABLISHED)
```

No tree view — not relevant for ports. The "Connections" line shows the count from the dedup step when multiple lsof rows existed for this port.

### Keybindings (Ports mode)

| Key | Action |
|-----|--------|
| `j/k` `↑/↓` | Navigate |
| `Space` | Toggle selection |
| `v` | Invert selection |
| `Ctrl+A` | Select all |
| `/` | Text filter (port, process, PID) |
| `s` | Cycle sort (port asc → port desc → process name → PID asc) |
| `t` | Protocol filter (TCP/UDP/All) |
| `a` | Toggle dev filter (show all ports) |
| `r` | Refresh (rescan) |
| `d` | Kill selected ports |
| `Tab` `1` `2` | Toggle mode (Normal mode only) |
| `?` | Help |
| `q` | Quit |

### Sort modes

Separate `PortsSortMode` enum (distinct from the prune `SortMode`):
- `PortAsc` (default)
- `PortDesc`
- `ProcessName`
- `PidAsc`

`s` cycles through them in order.

### Help popup

Mode-aware: displays different keybinding lists depending on `active_tool`. The `a` key (dev filter toggle) and `r` key (refresh) are specific to Ports mode and must be shown only there.

### Confirmation popup

Same pattern as Prune delete confirm — centered modal listing ports/processes to kill, Enter to confirm, Esc to cancel. Items owned by other users are flagged with a warning indicator.

## Config

Extension of `config.default.toml`:

```toml
# --- Ports mode ---
[ports]
# Only show ports matching these ranges on startup (toggle with 'a' in TUI)
dev_filter_enabled = true
# Port ranges to show when dev filter is active (supports "PORT" and "START-END")
dev_filter = ["3000-3009", "4000-4009", "5173-5174", "8080-8090"]
```

User override at `~/.config/lazyprune/config.toml` can override `[ports]` fields independently — field-by-field merge (if user specifies only `dev_filter`, `dev_filter_enabled` keeps its default value). This is more ergonomic than the atomic section replacement used for `targets`.

Kill behavior (SIGTERM → SIGKILL) is not configurable — fixed behavior.

## Dependencies

New crate dependency:
- `nix` — for `kill()` signal sending (SIGTERM, SIGKILL). Only the `signal` feature is needed.

## Refactoring plan

### app.rs

Extract all prune-specific state into `PruneState`. Methods like `toggle_selection()`, `apply_filter()`, `select_all()` that operate on `items`/`selected`/`filtered_indices` are duplicated in `PortsState` — same pattern, different types. No shared trait (YAGNI).

### main.rs

Key dispatch and channel polling become `match app.active_tool`. Event loop skeleton stays the same. Mode-toggle keys (`Tab`, `1`, `2`) are only processed in `Normal` mode.

### ui/

- `list.rs`, `details.rs`: second render path for Ports mode
- `layout.rs`: unchanged
- `popup.rs`: confirmation popup adapts content based on active tool; help popup is mode-aware
- Header: display active mode indicator and mode-specific stats

### config.rs

Add `PortsConfig` to `Config` struct. Parse `[ports]` section with field-by-field merge for user overrides.

### New files

- `src/ports.rs` — `PortsState`, `PortInfo`, `PortsSortMode`, scan logic (lsof parsing + ps follow-up), kill logic
- `src/ui/ports_list.rs` — Ports list rendering
- `src/ui/ports_details.rs` — Ports details panel rendering
