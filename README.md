# lazyprune

[![Crates.io](https://img.shields.io/crates/v/lazyprune)](https://crates.io/crates/lazyprune)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)

A TUI tool with two modes:
- **Prune** -- find and delete heavy cache/dependency directories (`node_modules`, `Pods`, `.gradle`, `target/`, etc.)
- **Ports** -- list and kill processes by port (dev servers, stale listeners, etc.)

Switch between modes with `Tab`, `1`, or `2`. Vim-style keybindings throughout.

![lazyprune demo](assets/demo.png)

## Install

```bash
cargo install lazyprune
```

Or build from source:

```bash
git clone https://github.com/ThibaultJRD/lazyprune.git
cd lazyprune
cargo install --path .
```

## Usage

```bash
lazyprune                            # Start in prune mode (default)
lazyprune --ports                    # Start in ports mode
lazyprune ~/Develop                  # Scan a specific directory
lazyprune -t / --target node_modules # Only look for node_modules
lazyprune -d / --dry-run             # Print results to stdout, no TUI
lazyprune --dry-run --ports          # Print open ports to stdout
lazyprune -H / --hidden              # Also scan hidden directories (e.g. ~/.cache)
lazyprune -D / --dir vendor          # Scan for arbitrary directory names (ad-hoc)
lazyprune --init-config              # Generate config at ~/.config/lazyprune/config.toml
```

## Keybindings

### Shared

| Key | Action |
|-----|--------|
| `Tab` | Toggle between Prune and Ports |
| `1` / `2` | Switch to Prune / Ports |
| `j/k` `↑/↓` | Navigate |
| `g/G` | Jump top/bottom |
| `Space` | Toggle selection |
| `v` | Invert selection |
| `Ctrl+a` | Select all |
| `/` | Filter |
| `s` | Cycle sort |
| `l/→/Enter` | Open details panel |
| `h/←/Esc` | Back to list |
| `?` | Help |
| `q` | Quit |

### Prune mode

| Key | Action |
|-----|--------|
| `d` | Delete selected directories |
| `t` | Filter by target type |
| `p` | Toggle project grouping |
| `y` | Copy path (in details) |

### Ports mode

| Key | Action |
|-----|--------|
| `d` | Kill selected ports |
| `t` | Filter by protocol (TCP/UDP) |
| `a` | Toggle dev port filter |
| `r` | Refresh port list |

## Config

Override defaults with `~/.config/lazyprune/config.toml`:

```toml
root = "~"
skip = [".Trash", "Library", "Applications"]

[[targets]]
name = "node_modules"
dirs = ["node_modules"]
indicator = "package.json"

# --- Ports mode ---
[ports]
# Only show ports matching these ranges on startup (toggle with 'a' in TUI)
dev_filter_enabled = true
# Port ranges to show when dev filter is active (supports "PORT" and "START-END")
dev_filter = ["3000-3009", "4000-4009", "5173-5174", "8080-8090"]
```

**Targets** have:
- `dirs` -- directory names to look for
- `indicator` (optional) -- a file that must exist in the parent to confirm it's a real target

Default targets: `node_modules`, `Pods`, `.gradle`/`build`, `.pnpm-store`, `.yarn/cache`, `.next`, `.nuxt`, `target` (Rust), `dist`.

**Ports** config:
- `dev_filter_enabled` -- only show ports in the configured ranges on startup
- `dev_filter` -- list of port ranges (`"3000-3009"`) and individual ports (`"5173"`)

## How it works

**Prune mode:**
- Walks the filesystem, computes directory sizes in parallel with [rayon](https://crates.io/crates/rayon)
- Skips hidden directories unless they match a target
- Never follows symlinks
- Deletion requires explicit confirmation

**Ports mode:**
- Lists open ports via `lsof`, deduplicates by port/protocol
- Dev filter hides system ports, showing only your dev servers
- Kill sends SIGTERM, then SIGKILL after 500ms if the process survives
- Kill requires explicit confirmation

## License

MIT
