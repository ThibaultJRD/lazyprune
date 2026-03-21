use crate::config::{parse_port_filter, Config};
use crate::ports::PortsState;
use crate::scanner::{ScanMessage, ScanResult};
use ratatui::widgets::ListState;
use std::sync::mpsc;

pub enum DeleteMessage {
    Deleting {
        path: String,
    },
    Deleted {
        idx: usize,
        size: u64,
    },
    Error {
        #[allow(dead_code)]
        idx: usize,
        err: String,
    },
    Complete,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SortMode {
    SizeDesc,
    SizeAsc,
    Name,
    DateDesc,
    DateAsc,
}

impl SortMode {
    pub fn next(self) -> Self {
        match self {
            SortMode::SizeDesc => SortMode::SizeAsc,
            SortMode::SizeAsc => SortMode::Name,
            SortMode::Name => SortMode::DateDesc,
            SortMode::DateDesc => SortMode::DateAsc,
            SortMode::DateAsc => SortMode::SizeDesc,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            SortMode::SizeDesc => "Size \u{2193}",
            SortMode::SizeAsc => "Size \u{2191}",
            SortMode::Name => "Name",
            SortMode::DateDesc => "Date \u{2193}",
            SortMode::DateAsc => "Date \u{2191}",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AppMode {
    Normal,
    Filter,
    SubFilter,
    Confirm,
    Processing,
    Help,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    Prune,
    Ports,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FocusPanel {
    List,
    Details,
}

#[derive(Debug, Clone)]
pub struct TreeEntry {
    pub name: String,
    pub is_dir: bool,
    pub is_last: bool,
    pub parent_is_last: Vec<bool>,
}

#[derive(Debug, Clone)]
pub struct TreeData {
    pub entries: Vec<TreeEntry>,
    pub top_dirs: Vec<(String, u64)>,
    pub project_type: Option<String>,
}

pub struct GroupInfo {
    pub name: String,
    pub path: std::path::PathBuf,
    pub total_size: u64,
    pub targets: Vec<(String, String, u64)>, // (target_name, relative_path, size)
}

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

pub struct App {
    pub active_tool: Tool,
    pub prune: PruneState,
    pub ports: Option<PortsState>,
    pub mode: AppMode,
    pub focus: FocusPanel,
    pub exit: bool,
    #[allow(dead_code)]
    pub config: Config,
}

impl App {
    pub fn new(scan_rx: mpsc::Receiver<ScanMessage>, config: Config) -> Self {
        let mut list_state = ListState::default();
        list_state.select(Some(0));
        *list_state.offset_mut() = 0;
        Self {
            prune: PruneState {
                items: Vec::new(),
                filtered_indices: Vec::new(),
                selected: Vec::new(),
                list_state,
                sort_mode: SortMode::SizeDesc,
                filter_text: String::new(),
                type_filter: None,
                scan_rx: Some(scan_rx),
                scan_complete: false,
                dirs_scanned: 0,
                total_deleted: 0,
                items_deleted: 0,
                scan_errors: 0,
                scan_tick: 0,
                available_types: Vec::new(),
                type_filter_cursor: 0,
                delete_rx: None,
                delete_total: 0,
                delete_progress: 0,
                delete_current_path: String::new(),
                delete_errors: Vec::new(),
                delete_done_indices: Vec::new(),
                group_separators: std::collections::HashSet::new(),
                project_grouping: false,
                tree_cache: std::collections::HashMap::new(),
                tree_rx: None,
                tree_loading: false,
                tree_scroll: 0,
                tree_debounce_at: None,
                tree_requested_path: None,
                path_index_map: std::collections::HashMap::new(),
            },
            active_tool: Tool::Prune,
            ports: None,
            mode: AppMode::Normal,
            focus: FocusPanel::List,
            exit: false,
            config,
        }
    }

    /// Lazily initialize PortsState and start the port scan if not yet done.
    pub fn ensure_ports_initialized(&mut self) {
        if self.ports.is_some() {
            return;
        }

        let dev_filter = if self.config.ports.dev_filter_enabled {
            Some(parse_port_filter(&self.config.ports.dev_filter))
        } else {
            None
        };

        let mut state = PortsState::new();
        if let Some(ref ports) = dev_filter {
            state.dev_filter_active = true;
            state.dev_filter_ports = ports.clone();
        }
        state.start_scan(dev_filter);
        self.ports = Some(state);
    }

    /// Drain the scan channel non-blocking, adding results and tracking progress.
    pub fn poll_scan_results(&mut self) {
        let rx = match self.prune.scan_rx.as_ref() {
            Some(rx) => rx,
            None => return,
        };

        loop {
            match rx.try_recv() {
                Ok(msg) => match msg {
                    ScanMessage::Found(result) => {
                        if !self.prune.available_types.contains(&result.target_name) {
                            self.prune.available_types.push(result.target_name.clone());
                        }
                        let idx = self.prune.items.len();
                        self.prune.path_index_map.insert(result.path.clone(), idx);
                        self.prune.items.push(result);
                        self.prune.selected.push(false);
                        // Append to filtered_indices if passes filter (no sort yet)
                        let item = &self.prune.items[idx];
                        if self.item_passes_filter(item) {
                            self.prune.filtered_indices.push(idx);
                        }
                    }
                    ScanMessage::Complete => {
                        self.prune.scan_complete = true;
                        self.prune.scan_rx = None;
                        self.apply_filter();
                        return;
                    }
                    ScanMessage::Progress { dirs_scanned } => {
                        self.prune.dirs_scanned = dirs_scanned;
                    }
                    ScanMessage::Error(_) => {
                        self.prune.scan_errors += 1;
                    }
                },
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.prune.scan_complete = true;
                    self.prune.scan_rx = None;
                    self.apply_filter();
                    return;
                }
            }
        }
    }

    /// Get the item currently under the cursor.
    pub fn current_item(&self) -> Option<&ScanResult> {
        let idx = self.prune.list_state.selected()?;
        if self.prune.group_separators.contains(&idx) {
            return None;
        }
        let &item_idx = self.prune.filtered_indices.get(idx)?;
        self.prune.items.get(item_idx)
    }

    /// Get group info when cursor is on a separator.
    pub fn current_group_info(&self) -> Option<GroupInfo> {
        let idx = self.prune.list_state.selected()?;
        if !self.prune.group_separators.contains(&idx) {
            return None;
        }

        let group_item_indices: Vec<usize> = self.prune.filtered_indices[idx + 1..]
            .iter()
            .take_while(|&&i| i != usize::MAX)
            .copied()
            .collect();

        if group_item_indices.is_empty() {
            return None;
        }

        let first_item = &self.prune.items[group_item_indices[0]];

        let project_path = first_item.git_root.clone().unwrap_or_else(|| {
            first_item
                .path
                .parent()
                .map(|p| p.to_path_buf())
                .unwrap_or_default()
        });

        let name = project_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("?")
            .to_string();

        let total_size: u64 = group_item_indices
            .iter()
            .map(|&i| self.prune.items[i].size)
            .sum();

        let targets: Vec<(String, String, u64)> = group_item_indices
            .iter()
            .map(|&i| {
                let item = &self.prune.items[i];
                let rel_path = item
                    .path
                    .strip_prefix(&project_path)
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_else(|_| item.path.to_string_lossy().to_string());
                (item.target_name.clone(), rel_path, item.size)
            })
            .collect();

        Some(GroupInfo {
            name,
            path: project_path,
            total_size,
            targets,
        })
    }

    /// Move cursor down with bounds clamping.
    pub fn next(&mut self) {
        if self.prune.filtered_indices.is_empty() {
            return;
        }
        let current = self.prune.list_state.selected().unwrap_or(0);
        let next = (current + 1).min(self.prune.filtered_indices.len() - 1);
        self.prune.list_state.select(Some(next));
        self.prune.tree_scroll = 0;
        if self.prune.group_separators.contains(&next) {
            self.request_group_tree_scan();
        } else {
            self.request_tree_scan();
        }
    }

    /// Move cursor up with bounds clamping.
    pub fn previous(&mut self) {
        if self.prune.filtered_indices.is_empty() {
            return;
        }
        let current = self.prune.list_state.selected().unwrap_or(0);
        let prev = current.saturating_sub(1);
        self.prune.list_state.select(Some(prev));
        self.prune.tree_scroll = 0;
        if self.prune.group_separators.contains(&prev) {
            self.request_group_tree_scan();
        } else {
            self.request_tree_scan();
        }
    }

    /// Jump to top of list.
    pub fn go_top(&mut self) {
        if !self.prune.filtered_indices.is_empty() {
            self.prune.list_state.select(Some(0));
        }
        self.prune.tree_scroll = 0;
        if self.prune.group_separators.contains(&0) {
            self.request_group_tree_scan();
        } else {
            self.request_tree_scan();
        }
    }

    /// Jump to bottom of list.
    pub fn go_bottom(&mut self) {
        if !self.prune.filtered_indices.is_empty() {
            let last = self.prune.filtered_indices.len() - 1;
            self.prune.list_state.select(Some(last));
        }
        self.prune.tree_scroll = 0;
        let pos = self.prune.list_state.selected().unwrap_or(0);
        if self.prune.group_separators.contains(&pos) {
            self.request_group_tree_scan();
        } else {
            self.request_tree_scan();
        }
    }

    /// Toggle selection of the current item.
    pub fn toggle_selection(&mut self) {
        let Some(idx) = self.prune.list_state.selected() else {
            return;
        };
        if self.prune.group_separators.contains(&idx) {
            // Find all items in this group (between this separator and the next)
            let group_items: Vec<usize> = self.prune.filtered_indices[idx + 1..]
                .iter()
                .take_while(|&&i| i != usize::MAX)
                .copied()
                .collect();
            let all_selected = group_items.iter().all(|&i| self.prune.selected[i]);
            for &i in &group_items {
                self.prune.selected[i] = !all_selected;
            }
            return;
        }
        if let Some(&item_idx) = self.prune.filtered_indices.get(idx) {
            self.prune.selected[item_idx] = !self.prune.selected[item_idx];
        }
    }

    /// Select all visible (filtered) items.
    pub fn select_all(&mut self) {
        for (pos, &idx) in self.prune.filtered_indices.iter().enumerate() {
            if !self.prune.group_separators.contains(&pos) && idx != usize::MAX {
                self.prune.selected[idx] = true;
            }
        }
    }

    /// Invert selection of all visible (filtered) items.
    pub fn invert_selection(&mut self) {
        for (pos, &idx) in self.prune.filtered_indices.iter().enumerate() {
            if !self.prune.group_separators.contains(&pos) && idx != usize::MAX {
                self.prune.selected[idx] = !self.prune.selected[idx];
            }
        }
    }

    /// Cycle to the next sort mode, re-sort, and re-filter.
    pub fn cycle_sort(&mut self) {
        self.prune.sort_mode = self.prune.sort_mode.next();
        self.apply_filter();
    }

    /// Toggle project grouping on/off, re-sort and re-filter.
    pub fn toggle_project_grouping(&mut self) {
        self.prune.project_grouping = !self.prune.project_grouping;
        self.apply_filter();
    }

    /// Return references to all selected items.
    pub fn selected_items(&self) -> Vec<&ScanResult> {
        self.prune
            .items
            .iter()
            .enumerate()
            .filter(|(i, _)| self.prune.selected.get(*i).copied().unwrap_or(false))
            .map(|(_, item)| item)
            .collect()
    }

    /// Total size of all selected items.
    pub fn selected_size(&self) -> u64 {
        self.selected_items().iter().map(|r| r.size).sum()
    }

    /// Start the deletion process: spawn a background thread that sends progress via channel.
    /// Indices are processed in descending order so that `items.remove(idx)` doesn't
    /// invalidate remaining indices.
    pub fn start_deleting(&mut self) {
        let mut indices: Vec<usize> = (0..self.prune.items.len())
            .filter(|&i| self.prune.selected[i])
            .collect();
        indices.reverse();

        if indices.is_empty() {
            return;
        }

        self.prune.delete_total = indices.len();
        self.prune.delete_progress = 0;
        self.prune.delete_current_path = String::new();
        self.prune.delete_errors = Vec::new();
        self.prune.delete_done_indices = Vec::new();

        let items_to_delete: Vec<(usize, std::path::PathBuf, u64)> = indices
            .iter()
            .map(|&i| {
                (
                    i,
                    self.prune.items[i].path.clone(),
                    self.prune.items[i].size,
                )
            })
            .collect();

        let (tx, rx) = mpsc::channel();
        self.prune.delete_rx = Some(rx);
        self.mode = AppMode::Processing;

        std::thread::spawn(move || {
            use rayon::prelude::*;
            items_to_delete.par_iter().for_each(|(idx, path, size)| {
                let path_str = path.to_string_lossy().to_string();
                let _ = tx.send(DeleteMessage::Deleting { path: path_str });
                match std::fs::remove_dir_all(path) {
                    Ok(()) => {
                        let _ = tx.send(DeleteMessage::Deleted {
                            idx: *idx,
                            size: *size,
                        });
                    }
                    Err(e) => {
                        let _ = tx.send(DeleteMessage::Error {
                            idx: *idx,
                            err: e.to_string(),
                        });
                    }
                }
            });
            let _ = tx.send(DeleteMessage::Complete);
        });
    }

    /// Poll the deletion channel for progress updates.
    pub fn poll_delete_results(&mut self) {
        let rx = match self.prune.delete_rx.as_ref() {
            Some(rx) => rx,
            None => return,
        };

        loop {
            match rx.try_recv() {
                Ok(msg) => match msg {
                    DeleteMessage::Deleting { path } => {
                        self.prune.delete_current_path = path;
                    }
                    DeleteMessage::Deleted { idx, size } => {
                        self.prune.total_deleted += size;
                        self.prune.items_deleted += 1;
                        self.prune.delete_progress += 1;
                        self.prune.delete_done_indices.push(idx);
                    }
                    DeleteMessage::Error { idx: _, err } => {
                        self.prune.delete_progress += 1;
                        self.prune.delete_errors.push(err);
                    }
                    DeleteMessage::Complete => {
                        self.prune.delete_rx = None;
                        // Remove deleted items from highest index to lowest
                        self.prune.delete_done_indices.sort_unstable();
                        self.prune.delete_done_indices.dedup();
                        for &idx in self.prune.delete_done_indices.iter().rev() {
                            self.prune.items.remove(idx);
                            self.prune.selected.remove(idx);
                        }
                        // Deselect all items (errored items stay in list but unselected)
                        for s in &mut self.prune.selected {
                            *s = false;
                        }
                        // path_index_map is stale after removal (indices shifted),
                        // but it's only used during scan which is already complete.
                        self.prune.path_index_map.clear();
                        self.prune.tree_cache.clear();
                        self.apply_filter();
                        self.mode = AppMode::Normal;
                        return;
                    }
                },
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.prune.delete_rx = None;
                    self.apply_filter();
                    self.mode = AppMode::Normal;
                    return;
                }
            }
        }

        self.apply_filter();
    }

    /// Count of selected items.
    pub fn selected_count(&self) -> usize {
        self.prune.selected.iter().filter(|&&s| s).count()
    }

    /// Check if a single item passes the current text and type filters.
    fn item_passes_filter(&self, item: &ScanResult) -> bool {
        if !self.prune.filter_text.is_empty() {
            let path_str = item.path.to_string_lossy().to_lowercase();
            if !path_str.contains(&self.prune.filter_text.to_lowercase()) {
                return false;
            }
        }
        if let Some(ref tf) = self.prune.type_filter {
            if item.target_name != *tf {
                return false;
            }
        }
        true
    }

    /// Rebuild filtered_indices: filter, sort, and optionally group.
    pub fn apply_filter(&mut self) {
        let filter_lower = self.prune.filter_text.to_lowercase();
        let mut base_indices: Vec<usize> = self
            .prune
            .items
            .iter()
            .enumerate()
            .filter(|(_, item)| {
                if !filter_lower.is_empty() {
                    let path_str = item.path.to_string_lossy().to_lowercase();
                    if !path_str.contains(&filter_lower) {
                        return false;
                    }
                }
                if let Some(ref tf) = self.prune.type_filter {
                    if item.target_name != *tf {
                        return false;
                    }
                }
                true
            })
            .map(|(i, _)| i)
            .collect();

        // Sort filtered indices by current sort mode (no item cloning)
        match self.prune.sort_mode {
            SortMode::SizeDesc => base_indices
                .sort_unstable_by(|&a, &b| self.prune.items[b].size.cmp(&self.prune.items[a].size)),
            SortMode::SizeAsc => base_indices
                .sort_unstable_by(|&a, &b| self.prune.items[a].size.cmp(&self.prune.items[b].size)),
            SortMode::Name => base_indices.sort_unstable_by(|&a, &b| {
                self.prune.items[a]
                    .path
                    .to_string_lossy()
                    .cmp(&self.prune.items[b].path.to_string_lossy())
            }),
            SortMode::DateDesc => base_indices.sort_unstable_by(|&a, &b| {
                self.prune.items[b]
                    .last_modified
                    .cmp(&self.prune.items[a].last_modified)
            }),
            SortMode::DateAsc => base_indices.sort_unstable_by(|&a, &b| {
                self.prune.items[a]
                    .last_modified
                    .cmp(&self.prune.items[b].last_modified)
            }),
        }

        self.prune.group_separators.clear();
        if self.prune.project_grouping && !base_indices.is_empty() {
            use std::collections::HashMap;

            // Group indices by project key (git_root or parent).
            // Iterating sorted base_indices preserves sort order within each group.
            let mut groups: HashMap<String, Vec<usize>> = HashMap::new();
            for &idx in &base_indices {
                let item = &self.prune.items[idx];
                let key = item
                    .git_root
                    .as_ref()
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_else(|| {
                        item.path
                            .parent()
                            .map(|p| p.to_string_lossy().to_string())
                            .unwrap_or_default()
                    });
                groups.entry(key).or_default().push(idx);
            }

            // Sort groups by active sort mode
            let mut group_list: Vec<(String, Vec<usize>)> = groups.into_iter().collect();
            match self.prune.sort_mode {
                SortMode::SizeDesc => {
                    group_list.sort_unstable_by(|a, b| {
                        let size_a: u64 = a.1.iter().map(|&i| self.prune.items[i].size).sum();
                        let size_b: u64 = b.1.iter().map(|&i| self.prune.items[i].size).sum();
                        size_b.cmp(&size_a).then_with(|| a.0.cmp(&b.0))
                    });
                }
                SortMode::SizeAsc => {
                    group_list.sort_unstable_by(|a, b| {
                        let size_a: u64 = a.1.iter().map(|&i| self.prune.items[i].size).sum();
                        let size_b: u64 = b.1.iter().map(|&i| self.prune.items[i].size).sum();
                        size_a.cmp(&size_b).then_with(|| a.0.cmp(&b.0))
                    });
                }
                SortMode::Name => {
                    group_list.sort_unstable_by(|a, b| a.0.cmp(&b.0));
                }
                SortMode::DateDesc => {
                    group_list.sort_unstable_by(|a, b| {
                        let date_a =
                            a.1.iter()
                                .filter_map(|&i| self.prune.items[i].last_modified)
                                .max();
                        let date_b =
                            b.1.iter()
                                .filter_map(|&i| self.prune.items[i].last_modified)
                                .max();
                        date_b.cmp(&date_a).then_with(|| a.0.cmp(&b.0))
                    });
                }
                SortMode::DateAsc => {
                    group_list.sort_unstable_by(|a, b| {
                        let date_a =
                            a.1.iter()
                                .filter_map(|&i| self.prune.items[i].last_modified)
                                .min();
                        let date_b =
                            b.1.iter()
                                .filter_map(|&i| self.prune.items[i].last_modified)
                                .min();
                        date_a.cmp(&date_b).then_with(|| a.0.cmp(&b.0))
                    });
                }
            }

            // Build filtered_indices with separators
            self.prune.filtered_indices = Vec::new();
            for (_, group_indices) in &group_list {
                self.prune
                    .group_separators
                    .insert(self.prune.filtered_indices.len());
                self.prune.filtered_indices.push(usize::MAX); // sentinel
                self.prune.filtered_indices.extend(group_indices);
            }
        } else {
            self.prune.filtered_indices = base_indices;
        }

        // Clamp cursor
        if self.prune.filtered_indices.is_empty() {
            self.prune.list_state.select(Some(0));
        } else {
            let current = self.prune.list_state.selected().unwrap_or(0);
            if current >= self.prune.filtered_indices.len() {
                self.prune
                    .list_state
                    .select(Some(self.prune.filtered_indices.len() - 1));
            }
        }
    }

    /// Build tree data for a directory: depth-2 tree entries, top 3 sub-dirs, project type.
    pub fn build_tree_data(path: &std::path::Path) -> TreeData {
        let mut entries = Vec::new();
        let mut dir_sizes: Vec<(String, u64)> = Vec::new();

        // Read depth-0 entries
        let mut children: Vec<(String, bool)> = Vec::new();
        if let Ok(rd) = std::fs::read_dir(path) {
            for entry in rd.filter_map(|e| e.ok()) {
                let name = entry.file_name().to_string_lossy().to_string();
                let is_dir = entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false);
                children.push((name, is_dir));
            }
        }

        // Sort: dirs first, then files, alphabetical within each group
        children.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));

        let max_entries = 15;
        let total_children = children.len();
        let truncated = total_children > max_entries;
        let show_children = if truncated {
            max_entries
        } else {
            total_children
        };

        for (i, (name, is_dir)) in children.iter().take(show_children).enumerate() {
            let is_last = !truncated && i == total_children - 1;
            entries.push(TreeEntry {
                name: name.clone(),
                is_dir: *is_dir,
                is_last,
                parent_is_last: vec![],
            });

            if *is_dir {
                let child_path = path.join(name);
                let (size, _) = crate::scanner::compute_dir_stats(&child_path);
                dir_sizes.push((name.clone(), size));

                let mut subchildren: Vec<(String, bool)> = Vec::new();
                if let Ok(rd) = std::fs::read_dir(&child_path) {
                    for entry in rd.filter_map(|e| e.ok()) {
                        let sname = entry.file_name().to_string_lossy().to_string();
                        let sis_dir = entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false);
                        subchildren.push((sname, sis_dir));
                    }
                }
                subchildren.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));

                let total_sub = subchildren.len();
                let sub_truncated = total_sub > max_entries;
                let show_sub = if sub_truncated {
                    max_entries
                } else {
                    total_sub
                };

                for (j, (sname, sis_dir)) in subchildren.iter().take(show_sub).enumerate() {
                    let sub_is_last = !sub_truncated && j == total_sub - 1;
                    entries.push(TreeEntry {
                        name: sname.clone(),
                        is_dir: *sis_dir,
                        is_last: sub_is_last,
                        parent_is_last: vec![is_last],
                    });
                }
                if sub_truncated {
                    let remaining = total_sub - max_entries;
                    entries.push(TreeEntry {
                        name: format!("... ({} more)", remaining),
                        is_dir: false,
                        is_last: true,
                        parent_is_last: vec![is_last],
                    });
                }
            }
        }
        if truncated {
            let remaining = total_children - max_entries;
            entries.push(TreeEntry {
                name: format!("... ({} more)", remaining),
                is_dir: false,
                is_last: true,
                parent_is_last: vec![],
            });
        }

        dir_sizes.sort_by(|a, b| b.1.cmp(&a.1));
        dir_sizes.truncate(3);

        let project_type = detect_project_type(path.parent().unwrap_or(path));

        TreeData {
            entries,
            top_dirs: dir_sizes,
            project_type,
        }
    }

    /// Request a tree scan for the current item with debounce.
    pub fn request_tree_scan(&mut self) {
        let item = match self.current_item() {
            Some(item) => item,
            None => return,
        };
        let path = item.path.clone();

        if self.prune.tree_cache.contains_key(&path) {
            return;
        }

        self.prune.tree_debounce_at =
            Some(std::time::Instant::now() + std::time::Duration::from_millis(200));
        self.prune.tree_requested_path = Some(path);
    }

    /// Request a tree scan for the current group's project root.
    pub fn request_group_tree_scan(&mut self) {
        let info = match self.current_group_info() {
            Some(info) => info,
            None => return,
        };

        if self.prune.tree_cache.contains_key(&info.path) {
            return;
        }

        self.prune.tree_debounce_at =
            Some(std::time::Instant::now() + std::time::Duration::from_millis(200));
        self.prune.tree_requested_path = Some(info.path);
    }

    /// Check if debounce timer has elapsed and launch the tree scan thread.
    pub fn maybe_start_tree_scan(&mut self) {
        let deadline = match self.prune.tree_debounce_at {
            Some(d) => d,
            None => return,
        };

        if std::time::Instant::now() < deadline {
            return;
        }

        let path = match self.prune.tree_requested_path.take() {
            Some(p) => p,
            None => return,
        };
        self.prune.tree_debounce_at = None;

        if self.prune.tree_cache.contains_key(&path) {
            return;
        }

        if self.prune.tree_loading {
            return;
        }

        self.prune.tree_loading = true;
        let (tx, rx) = mpsc::channel();
        self.prune.tree_rx = Some(rx);

        let scan_path = path.clone();
        std::thread::spawn(move || {
            let data = App::build_tree_data(&scan_path);
            let _ = tx.send((scan_path, data));
        });
    }

    pub fn tree_scroll_down(&mut self) {
        self.prune.tree_scroll = self.prune.tree_scroll.saturating_add(1);
    }

    pub fn tree_scroll_up(&mut self) {
        self.prune.tree_scroll = self.prune.tree_scroll.saturating_sub(1);
    }

    pub fn tree_scroll_top(&mut self) {
        self.prune.tree_scroll = 0;
    }

    pub fn tree_scroll_bottom(&mut self, visible_height: u16) {
        if let Some(item) = self.current_item() {
            if let Some(data) = self.prune.tree_cache.get(&item.path) {
                let total = data.entries.len() as u16;
                self.prune.tree_scroll = total.saturating_sub(visible_height);
            }
        }
    }

    pub fn copy_path_to_clipboard(&self) {
        if let Some(item) = self.current_item() {
            let path_str = item.path.to_string_lossy();
            let encoded = base64_encode(path_str.as_bytes());
            print!("\x1b]52;c;{}\x07", encoded);
        }
    }

    /// Poll for completed tree scan results.
    pub fn poll_tree_results(&mut self) {
        let rx = match self.prune.tree_rx.as_ref() {
            Some(rx) => rx,
            None => return,
        };

        match rx.try_recv() {
            Ok((path, data)) => {
                self.prune.tree_cache.insert(path, data);
                self.prune.tree_loading = false;
                self.prune.tree_rx = None;
            }
            Err(mpsc::TryRecvError::Empty) => {}
            Err(mpsc::TryRecvError::Disconnected) => {
                self.prune.tree_loading = false;
                self.prune.tree_rx = None;
            }
        }
    }
}

fn base64_encode(data: &[u8]) -> String {
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut result = String::new();
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = if chunk.len() > 1 { chunk[1] as u32 } else { 0 };
        let b2 = if chunk.len() > 2 { chunk[2] as u32 } else { 0 };
        let triple = (b0 << 16) | (b1 << 8) | b2;
        result.push(CHARS[((triple >> 18) & 0x3F) as usize] as char);
        result.push(CHARS[((triple >> 12) & 0x3F) as usize] as char);
        if chunk.len() > 1 {
            result.push(CHARS[((triple >> 6) & 0x3F) as usize] as char);
        } else {
            result.push('=');
        }
        if chunk.len() > 2 {
            result.push(CHARS[(triple & 0x3F) as usize] as char);
        } else {
            result.push('=');
        }
    }
    result
}

pub(crate) fn detect_project_type(dir: &std::path::Path) -> Option<String> {
    let checks: &[(&str, &str)] = &[
        ("package.json", "Node.js"),
        ("Cargo.toml", "Rust"),
        ("Podfile", "iOS (CocoaPods)"),
        ("build.gradle", "Android/Java"),
        ("build.gradle.kts", "Android/Kotlin"),
        ("pyproject.toml", "Python"),
        ("requirements.txt", "Python"),
        ("go.mod", "Go"),
        ("Gemfile", "Ruby"),
    ];
    for (file, label) in checks {
        if dir.join(file).exists() {
            return Some(label.to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::mpsc;

    fn make_result(name: &str, path: &str, size: u64) -> ScanResult {
        ScanResult {
            path: PathBuf::from(path),
            target_name: name.to_string(),
            size,
            last_modified: None,
            file_count: 0,
            git_root: None,
        }
    }

    fn make_test_app(items: Vec<ScanResult>) -> App {
        let (tx, rx) = mpsc::channel();
        drop(tx);
        let n = items.len();
        let config = Config::load(None).unwrap();
        let mut app = App::new(rx, config);
        app.prune.items = items;
        app.prune.selected = vec![false; n];
        app.apply_filter();
        app
    }

    #[test]
    fn test_sort_by_size_desc() {
        let mut app = make_test_app(vec![
            make_result("node_modules", "/a/node_modules", 100),
            make_result("node_modules", "/b/node_modules", 500),
            make_result("node_modules", "/c/node_modules", 200),
        ]);

        app.prune.sort_mode = SortMode::SizeDesc;
        app.apply_filter();

        assert_eq!(app.prune.items[app.prune.filtered_indices[0]].size, 500);
        assert_eq!(app.prune.items[app.prune.filtered_indices[1]].size, 200);
        assert_eq!(app.prune.items[app.prune.filtered_indices[2]].size, 100);
    }

    #[test]
    fn test_filter_by_text() {
        let mut app = make_test_app(vec![
            make_result("node_modules", "/projects/web/node_modules", 100),
            make_result("node_modules", "/projects/api/node_modules", 200),
            make_result("Pods", "/projects/ios/Pods", 300),
        ]);

        app.prune.filter_text = "api".to_string();
        app.apply_filter();

        assert_eq!(app.prune.filtered_indices.len(), 1);
        assert_eq!(
            app.prune.items[app.prune.filtered_indices[0]]
                .path
                .to_string_lossy(),
            "/projects/api/node_modules"
        );
    }

    #[test]
    fn test_filter_by_type() {
        let mut app = make_test_app(vec![
            make_result("node_modules", "/a/node_modules", 100),
            make_result("Pods", "/b/Pods", 200),
            make_result("node_modules", "/c/node_modules", 300),
        ]);

        app.prune.type_filter = Some("Pods".to_string());
        app.apply_filter();

        assert_eq!(app.prune.filtered_indices.len(), 1);
        assert_eq!(
            app.prune.items[app.prune.filtered_indices[0]].target_name,
            "Pods"
        );
    }

    #[test]
    fn test_toggle_selection() {
        let mut app = make_test_app(vec![
            make_result("node_modules", "/a/node_modules", 100),
            make_result("node_modules", "/b/node_modules", 200),
        ]);

        // Cursor is at 0
        app.prune.list_state.select(Some(0));
        app.toggle_selection();
        let idx = app.prune.filtered_indices[0];
        assert!(app.prune.selected[idx]);

        // Toggle again to deselect
        app.toggle_selection();
        assert!(!app.prune.selected[idx]);
    }

    #[test]
    fn test_invert_selection() {
        let mut app = make_test_app(vec![
            make_result("node_modules", "/a/node_modules", 100),
            make_result("node_modules", "/b/node_modules", 200),
            make_result("Pods", "/c/Pods", 300),
        ]);

        // Select first item
        let first_idx = app.prune.filtered_indices[0];
        app.prune.selected[first_idx] = true;

        app.invert_selection();

        assert!(!app.prune.selected[app.prune.filtered_indices[0]]);
        assert!(app.prune.selected[app.prune.filtered_indices[1]]);
        assert!(app.prune.selected[app.prune.filtered_indices[2]]);
    }

    #[test]
    fn test_tree_data_from_dir() {
        use tempfile::TempDir;
        let dir = TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join("alpha/sub1")).unwrap();
        std::fs::create_dir_all(dir.path().join("beta")).unwrap();
        std::fs::write(dir.path().join("alpha/sub1/file.txt"), "x".repeat(1000)).unwrap();
        std::fs::write(dir.path().join("gamma.txt"), "y".repeat(500)).unwrap();

        let data = App::build_tree_data(dir.path());
        assert!(!data.entries.is_empty());
        assert!(!data.top_dirs.is_empty());
        assert_eq!(data.top_dirs[0].0, "alpha");
    }

    #[test]
    fn test_base64_encode() {
        assert_eq!(base64_encode(b"hello"), "aGVsbG8=");
        assert_eq!(
            base64_encode(b"/Users/test/path"),
            "L1VzZXJzL3Rlc3QvcGF0aA=="
        );
    }

    #[test]
    fn test_tree_cache_persists_across_navigation() {
        let mut app = make_test_app(vec![make_result("node_modules", "/a/node_modules", 100)]);
        app.prune.tree_cache.insert(
            std::path::PathBuf::from("/a/node_modules"),
            TreeData {
                entries: vec![],
                top_dirs: vec![],
                project_type: None,
            },
        );
        assert!(!app.prune.tree_cache.is_empty());
        assert!(app
            .prune
            .tree_cache
            .contains_key(&std::path::PathBuf::from("/a/node_modules")));
    }

    #[test]
    fn test_detect_project_type_node() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("package.json"), "{}").unwrap();
        let result = detect_project_type(dir.path());
        assert_eq!(result, Some("Node.js".to_string()));
    }

    #[test]
    fn test_detect_project_type_rust() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("Cargo.toml"), "").unwrap();
        let result = detect_project_type(dir.path());
        assert_eq!(result, Some("Rust".to_string()));
    }

    #[test]
    fn test_detect_project_type_none() {
        let dir = tempfile::tempdir().unwrap();
        let result = detect_project_type(dir.path());
        assert_eq!(result, None);
    }

    #[test]
    fn test_project_grouping_by_git_root() {
        let mut items = vec![
            make_result("node_modules", "/projects/my-app/node_modules", 100),
            make_result("Pods", "/projects/my-app/ios/Pods", 500),
            make_result("node_modules", "/projects/other/node_modules", 200),
        ];
        items[0].git_root = Some(PathBuf::from("/projects/my-app"));
        items[1].git_root = Some(PathBuf::from("/projects/my-app"));
        items[2].git_root = Some(PathBuf::from("/projects/other"));

        let mut app = make_test_app(items);
        app.prune.project_grouping = true;
        app.apply_filter();

        // my-app group (600) first, then other (200)
        assert_eq!(app.prune.group_separators.len(), 2);

        let real: Vec<usize> = (0..app.prune.filtered_indices.len())
            .filter(|i| !app.prune.group_separators.contains(i))
            .map(|i| app.prune.filtered_indices[i])
            .collect();
        // my-app items first (sorted by size desc: 500, 100), then other (200)
        assert_eq!(app.prune.items[real[0]].size, 500);
        assert_eq!(app.prune.items[real[1]].size, 100);
        assert_eq!(app.prune.items[real[2]].size, 200);
    }

    #[test]
    fn test_project_grouping_fallback_no_git() {
        let items = vec![
            make_result("node_modules", "/a/node_modules", 100),
            make_result("node_modules", "/b/node_modules", 300),
        ];

        let mut app = make_test_app(items);
        app.prune.project_grouping = true;
        app.apply_filter();

        // Two separate groups (different parents)
        assert_eq!(app.prune.group_separators.len(), 2);
    }

    #[test]
    fn test_project_grouping_off_no_separators() {
        let mut items = vec![
            make_result("node_modules", "/projects/my-app/node_modules", 100),
            make_result("Pods", "/projects/my-app/ios/Pods", 500),
        ];
        items[0].git_root = Some(PathBuf::from("/projects/my-app"));
        items[1].git_root = Some(PathBuf::from("/projects/my-app"));

        let mut app = make_test_app(items);
        app.prune.project_grouping = false;
        app.apply_filter();

        assert!(app.prune.group_separators.is_empty());
    }

    #[test]
    fn test_toggle_selection_on_separator_selects_group() {
        let mut items = vec![
            make_result("node_modules", "/projects/my-app/node_modules", 100),
            make_result("Pods", "/projects/my-app/ios/Pods", 500),
            make_result("node_modules", "/projects/other/node_modules", 200),
        ];
        items[0].git_root = Some(PathBuf::from("/projects/my-app"));
        items[1].git_root = Some(PathBuf::from("/projects/my-app"));
        items[2].git_root = Some(PathBuf::from("/projects/other"));

        let mut app = make_test_app(items);
        app.prune.project_grouping = true;
        app.apply_filter();

        // Cursor on first separator (position 0)
        app.prune.list_state.select(Some(0));
        assert!(app.prune.group_separators.contains(&0));

        // Toggle selects all items in the first group
        app.toggle_selection();

        let group_items: Vec<usize> = (1..app.prune.filtered_indices.len())
            .take_while(|i| !app.prune.group_separators.contains(i))
            .map(|i| app.prune.filtered_indices[i])
            .collect();
        assert!(group_items.iter().all(|&i| app.prune.selected[i]));

        // Second group items should NOT be selected
        let second_group_item = app.prune.filtered_indices[app.prune.filtered_indices.len() - 1];
        assert!(!app.prune.selected[second_group_item]);

        // Toggle again deselects all items in the group
        app.toggle_selection();
        assert!(group_items.iter().all(|&i| !app.prune.selected[i]));
    }

    #[test]
    fn test_poll_scan_results_streaming() {
        let (tx, rx) = mpsc::channel();
        let config = Config::load(None).unwrap();
        let mut app = App::new(rx, config);

        tx.send(ScanMessage::Found(ScanResult {
            path: PathBuf::from("/a/node_modules"),
            target_name: "node_modules".to_string(),
            size: 500,
            last_modified: None,
            file_count: 10,
            git_root: None,
        }))
        .unwrap();
        tx.send(ScanMessage::Found(ScanResult {
            path: PathBuf::from("/b/node_modules"),
            target_name: "node_modules".to_string(),
            size: 200,
            last_modified: None,
            file_count: 5,
            git_root: None,
        }))
        .unwrap();
        tx.send(ScanMessage::Complete).unwrap();
        drop(tx);

        app.poll_scan_results();

        assert_eq!(app.prune.items.len(), 2);
        assert_eq!(app.prune.items[0].size, 500);
        assert_eq!(app.prune.items[1].size, 200);
        assert_eq!(app.prune.path_index_map.len(), 2);
        assert!(app.prune.scan_complete);
        // Sorted by size desc after Complete triggered apply_filter
        assert_eq!(app.prune.items[app.prune.filtered_indices[0]].size, 500);
        assert_eq!(app.prune.items[app.prune.filtered_indices[1]].size, 200);
    }

    #[test]
    fn test_project_grouping_sorted_by_name() {
        let mut items = vec![
            make_result("node_modules", "/projects/zebra/node_modules", 500),
            make_result("node_modules", "/projects/alpha/node_modules", 100),
        ];
        items[0].git_root = Some(PathBuf::from("/projects/zebra"));
        items[1].git_root = Some(PathBuf::from("/projects/alpha"));

        let mut app = make_test_app(items);
        app.prune.sort_mode = SortMode::Name;
        app.prune.project_grouping = true;
        app.apply_filter();

        // alpha group first, then zebra
        assert_eq!(app.prune.group_separators.len(), 2);
        let real: Vec<usize> = (0..app.prune.filtered_indices.len())
            .filter(|i| !app.prune.group_separators.contains(i))
            .map(|i| app.prune.filtered_indices[i])
            .collect();
        assert_eq!(app.prune.items[real[0]].size, 100); // alpha
        assert_eq!(app.prune.items[real[1]].size, 500); // zebra
    }
}
