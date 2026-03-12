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
    TypeFilter,
    Confirm,
    Deleting,
    Help,
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
    pub all_sizes_ready: bool,
    pub targets: Vec<(String, String, u64, bool)>, // (target_name, relative_path, size, size_ready)
}

pub struct App {
    pub items: Vec<ScanResult>,
    pub filtered_indices: Vec<usize>,
    pub selected: Vec<bool>,
    pub list_state: ListState,
    pub sort_mode: SortMode,
    pub mode: AppMode,
    pub filter_text: String,
    pub type_filter: Option<String>,
    pub scan_rx: Option<mpsc::Receiver<ScanMessage>>,
    pub scan_complete: bool,
    pub dirs_scanned: u64,
    pub total_deleted: u64,
    pub items_deleted: usize,
    pub scan_errors: u64,
    pub scan_tick: u8,
    pub exit: bool,
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
    pub focus: FocusPanel,
    pub tree_cache: std::collections::HashMap<std::path::PathBuf, TreeData>,
    pub tree_rx: Option<mpsc::Receiver<(std::path::PathBuf, TreeData)>>,
    pub tree_loading: bool,
    pub tree_scroll: u16,
    pub tree_debounce_at: Option<std::time::Instant>,
    pub tree_requested_path: Option<std::path::PathBuf>,
}

impl App {
    pub fn new(scan_rx: mpsc::Receiver<ScanMessage>) -> Self {
        let mut list_state = ListState::default();
        list_state.select(Some(0));
        *list_state.offset_mut() = 0;
        Self {
            items: Vec::new(),
            filtered_indices: Vec::new(),
            selected: Vec::new(),
            list_state,
            sort_mode: SortMode::SizeDesc,
            mode: AppMode::Normal,
            filter_text: String::new(),
            type_filter: None,
            scan_rx: Some(scan_rx),
            scan_complete: false,
            dirs_scanned: 0,
            total_deleted: 0,
            items_deleted: 0,
            scan_errors: 0,
            scan_tick: 0,
            exit: false,
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
            focus: FocusPanel::List,
            tree_cache: std::collections::HashMap::new(),
            tree_rx: None,
            tree_loading: false,
            tree_scroll: 0,
            tree_debounce_at: None,
            tree_requested_path: None,
        }
    }

    /// Drain the scan channel non-blocking, adding results and tracking progress.
    pub fn poll_scan_results(&mut self) {
        let rx = match self.scan_rx.as_ref() {
            Some(rx) => rx,
            None => return,
        };

        let mut new_items = false;

        loop {
            match rx.try_recv() {
                Ok(msg) => match msg {
                    ScanMessage::Found(result) => {
                        // Track available types
                        if !self.available_types.contains(&result.target_name) {
                            self.available_types.push(result.target_name.clone());
                        }
                        self.items.push(result);
                        self.selected.push(false);
                        new_items = true;
                    }
                    ScanMessage::StatsReady { path, size, file_count } => {
                        if let Some(item) = self.items.iter_mut().find(|i| i.path == path) {
                            item.size = size;
                            item.file_count = file_count;
                            item.size_ready = true;
                        }
                        // Sizes update in-place (visible immediately in UI) but we
                        // skip re-sort here — items stay stable until new Found or Complete.
                    }
                    ScanMessage::Progress { dirs_scanned } => {
                        self.dirs_scanned = dirs_scanned;
                    }
                    ScanMessage::Complete => {
                        self.scan_complete = true;
                        self.scan_rx = None;
                        self.apply_sort();
                        self.apply_filter();
                        return;
                    }
                    ScanMessage::Error(_) => {
                        self.scan_errors += 1;
                    }
                },
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.scan_complete = true;
                    self.scan_rx = None;
                    self.apply_sort();
                    self.apply_filter();
                    return;
                }
            }
        }

        if new_items {
            self.apply_sort();
            self.apply_filter();
        }
    }

    /// Get the item currently under the cursor.
    pub fn current_item(&self) -> Option<&ScanResult> {
        let idx = self.list_state.selected()?;
        if self.group_separators.contains(&idx) {
            return None;
        }
        let &item_idx = self.filtered_indices.get(idx)?;
        self.items.get(item_idx)
    }

    /// Get group info when cursor is on a separator.
    pub fn current_group_info(&self) -> Option<GroupInfo> {
        let idx = self.list_state.selected()?;
        if !self.group_separators.contains(&idx) {
            return None;
        }

        let group_item_indices: Vec<usize> = self.filtered_indices[idx + 1..]
            .iter()
            .take_while(|&&i| i != usize::MAX)
            .copied()
            .collect();

        if group_item_indices.is_empty() {
            return None;
        }

        let first_item = &self.items[group_item_indices[0]];

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

        let total_size: u64 = group_item_indices.iter().map(|&i| self.items[i].size).sum();
        let all_sizes_ready = group_item_indices.iter().all(|&i| self.items[i].size_ready);

        let targets: Vec<(String, String, u64, bool)> = group_item_indices
            .iter()
            .map(|&i| {
                let item = &self.items[i];
                let rel_path = item
                    .path
                    .strip_prefix(&project_path)
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_else(|_| item.path.to_string_lossy().to_string());
                (item.target_name.clone(), rel_path, item.size, item.size_ready)
            })
            .collect();

        Some(GroupInfo {
            name,
            path: project_path,
            total_size,
            all_sizes_ready,
            targets,
        })
    }

    /// Move cursor down with bounds clamping.
    pub fn next(&mut self) {
        if self.filtered_indices.is_empty() {
            return;
        }
        let current = self.list_state.selected().unwrap_or(0);
        let next = (current + 1).min(self.filtered_indices.len() - 1);
        self.list_state.select(Some(next));
        self.tree_scroll = 0;
        if self.group_separators.contains(&next) {
            self.request_group_tree_scan();
        } else {
            self.request_tree_scan();
        }
    }

    /// Move cursor up with bounds clamping.
    pub fn previous(&mut self) {
        if self.filtered_indices.is_empty() {
            return;
        }
        let current = self.list_state.selected().unwrap_or(0);
        let prev = current.saturating_sub(1);
        self.list_state.select(Some(prev));
        self.tree_scroll = 0;
        if self.group_separators.contains(&prev) {
            self.request_group_tree_scan();
        } else {
            self.request_tree_scan();
        }
    }

    /// Jump to top of list.
    pub fn go_top(&mut self) {
        if !self.filtered_indices.is_empty() {
            self.list_state.select(Some(0));
        }
        self.tree_scroll = 0;
        if self.group_separators.contains(&0) {
            self.request_group_tree_scan();
        } else {
            self.request_tree_scan();
        }
    }

    /// Jump to bottom of list.
    pub fn go_bottom(&mut self) {
        if !self.filtered_indices.is_empty() {
            let last = self.filtered_indices.len() - 1;
            self.list_state.select(Some(last));
        }
        self.tree_scroll = 0;
        let pos = self.list_state.selected().unwrap_or(0);
        if self.group_separators.contains(&pos) {
            self.request_group_tree_scan();
        } else {
            self.request_tree_scan();
        }
    }

    /// Toggle selection of the current item.
    pub fn toggle_selection(&mut self) {
        let Some(idx) = self.list_state.selected() else {
            return;
        };
        if self.group_separators.contains(&idx) {
            // Find all items in this group (between this separator and the next)
            let group_items: Vec<usize> = self.filtered_indices[idx + 1..]
                .iter()
                .take_while(|&&i| i != usize::MAX)
                .copied()
                .collect();
            let all_selected = group_items.iter().all(|&i| self.selected[i]);
            for &i in &group_items {
                self.selected[i] = !all_selected;
            }
            return;
        }
        if let Some(&item_idx) = self.filtered_indices.get(idx) {
            self.selected[item_idx] = !self.selected[item_idx];
        }
    }

    /// Select all visible (filtered) items.
    pub fn select_all(&mut self) {
        for (pos, &idx) in self.filtered_indices.iter().enumerate() {
            if !self.group_separators.contains(&pos) && idx != usize::MAX {
                self.selected[idx] = true;
            }
        }
    }

    /// Invert selection of all visible (filtered) items.
    pub fn invert_selection(&mut self) {
        for (pos, &idx) in self.filtered_indices.iter().enumerate() {
            if !self.group_separators.contains(&pos) && idx != usize::MAX {
                self.selected[idx] = !self.selected[idx];
            }
        }
    }

    /// Cycle to the next sort mode, re-sort, and re-filter.
    pub fn cycle_sort(&mut self) {
        self.sort_mode = self.sort_mode.next();
        self.apply_sort();
        self.apply_filter();
    }

    /// Toggle project grouping on/off, re-sort and re-filter.
    pub fn toggle_project_grouping(&mut self) {
        self.project_grouping = !self.project_grouping;
        self.apply_sort();
        self.apply_filter();
    }

    /// Return references to all selected items.
    pub fn selected_items(&self) -> Vec<&ScanResult> {
        self.items
            .iter()
            .enumerate()
            .filter(|(i, _)| self.selected.get(*i).copied().unwrap_or(false))
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
        let mut indices: Vec<usize> = (0..self.items.len())
            .filter(|&i| self.selected[i])
            .collect();
        indices.reverse();

        if indices.is_empty() {
            return;
        }

        self.delete_total = indices.len();
        self.delete_progress = 0;
        self.delete_current_path = String::new();
        self.delete_errors = Vec::new();
        self.delete_done_indices = Vec::new();

        let items_to_delete: Vec<(usize, std::path::PathBuf, u64)> = indices
            .iter()
            .map(|&i| (i, self.items[i].path.clone(), self.items[i].size))
            .collect();

        let (tx, rx) = mpsc::channel();
        self.delete_rx = Some(rx);
        self.mode = AppMode::Deleting;

        std::thread::spawn(move || {
            use rayon::prelude::*;
            items_to_delete
                .par_iter()
                .for_each(|(idx, path, size)| {
                    let path_str = path.to_string_lossy().to_string();
                    let _ = tx.send(DeleteMessage::Deleting { path: path_str });
                    match std::fs::remove_dir_all(path) {
                        Ok(()) => {
                            let _ = tx.send(DeleteMessage::Deleted { idx: *idx, size: *size });
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
        let rx = match self.delete_rx.as_ref() {
            Some(rx) => rx,
            None => return,
        };

        loop {
            match rx.try_recv() {
                Ok(msg) => match msg {
                    DeleteMessage::Deleting { path } => {
                        self.delete_current_path = path;
                    }
                    DeleteMessage::Deleted { idx, size } => {
                        self.total_deleted += size;
                        self.items_deleted += 1;
                        self.delete_progress += 1;
                        self.delete_done_indices.push(idx);
                    }
                    DeleteMessage::Error { idx: _, err } => {
                        self.delete_progress += 1;
                        self.delete_errors.push(err);
                    }
                    DeleteMessage::Complete => {
                        self.delete_rx = None;
                        // Remove deleted items from highest index to lowest
                        self.delete_done_indices.sort_unstable();
                        self.delete_done_indices.dedup();
                        for &idx in self.delete_done_indices.iter().rev() {
                            self.items.remove(idx);
                            self.selected.remove(idx);
                        }
                        // Deselect all items (errored items stay in list but unselected)
                        for s in &mut self.selected {
                            *s = false;
                        }
                        self.tree_cache.clear();
                        self.apply_filter();
                        self.mode = AppMode::Normal;
                        return;
                    }
                },
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.delete_rx = None;
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
        self.selected.iter().filter(|&&s| s).count()
    }

    /// Sort items vec according to current sort_mode.
    /// Remaps the selected array to stay in sync.
    pub fn apply_sort(&mut self) {
        // Build index permutation
        let mut indices: Vec<usize> = (0..self.items.len()).collect();
        match self.sort_mode {
            SortMode::SizeDesc => {
                indices.sort_by(|&a, &b| self.items[b].size.cmp(&self.items[a].size))
            }
            SortMode::SizeAsc => {
                indices.sort_by(|&a, &b| self.items[a].size.cmp(&self.items[b].size))
            }
            SortMode::Name => indices.sort_by(|&a, &b| {
                self.items[a]
                    .path
                    .to_string_lossy()
                    .cmp(&self.items[b].path.to_string_lossy())
            }),
            SortMode::DateDesc => indices.sort_by(|&a, &b| {
                self.items[b]
                    .last_modified
                    .cmp(&self.items[a].last_modified)
            }),
            SortMode::DateAsc => indices.sort_by(|&a, &b| {
                self.items[a]
                    .last_modified
                    .cmp(&self.items[b].last_modified)
            }),
        }

        // Reorder items and selected by the permutation
        let new_items: Vec<ScanResult> = indices.iter().map(|&i| self.items[i].clone()).collect();
        let new_selected: Vec<bool> = indices
            .iter()
            .map(|&i| self.selected.get(i).copied().unwrap_or(false))
            .collect();
        self.items = new_items;
        self.selected = new_selected;
    }

    /// Rebuild filtered_indices based on filter_text and type_filter.
    /// Clamps cursor to valid range.
    pub fn apply_filter(&mut self) {
        let filter_lower = self.filter_text.to_lowercase();
        let base_indices: Vec<usize> = self
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
                if let Some(ref tf) = self.type_filter {
                    if item.target_name != *tf {
                        return false;
                    }
                }
                true
            })
            .map(|(i, _)| i)
            .collect();

        self.group_separators.clear();
        if self.project_grouping && !base_indices.is_empty() {
            use std::collections::HashMap;

            // Group indices by project key (git_root or parent)
            let mut groups: HashMap<String, Vec<usize>> = HashMap::new();
            for &idx in &base_indices {
                let item = &self.items[idx];
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
            match self.sort_mode {
                SortMode::SizeDesc => {
                    group_list.sort_by(|a, b| {
                        let size_a: u64 = a.1.iter().map(|&i| self.items[i].size).sum();
                        let size_b: u64 = b.1.iter().map(|&i| self.items[i].size).sum();
                        size_b.cmp(&size_a).then_with(|| a.0.cmp(&b.0))
                    });
                }
                SortMode::SizeAsc => {
                    group_list.sort_by(|a, b| {
                        let size_a: u64 = a.1.iter().map(|&i| self.items[i].size).sum();
                        let size_b: u64 = b.1.iter().map(|&i| self.items[i].size).sum();
                        size_a.cmp(&size_b).then_with(|| a.0.cmp(&b.0))
                    });
                }
                SortMode::Name => {
                    group_list.sort_by(|a, b| a.0.cmp(&b.0));
                }
                SortMode::DateDesc => {
                    group_list.sort_by(|a, b| {
                        let date_a =
                            a.1.iter()
                                .filter_map(|&i| self.items[i].last_modified)
                                .max();
                        let date_b =
                            b.1.iter()
                                .filter_map(|&i| self.items[i].last_modified)
                                .max();
                        date_b.cmp(&date_a).then_with(|| a.0.cmp(&b.0))
                    });
                }
                SortMode::DateAsc => {
                    group_list.sort_by(|a, b| {
                        let date_a =
                            a.1.iter()
                                .filter_map(|&i| self.items[i].last_modified)
                                .min();
                        let date_b =
                            b.1.iter()
                                .filter_map(|&i| self.items[i].last_modified)
                                .min();
                        date_a.cmp(&date_b).then_with(|| a.0.cmp(&b.0))
                    });
                }
            }

            // Build filtered_indices with separators
            self.filtered_indices = Vec::new();
            for (_, group_indices) in &group_list {
                self.group_separators.insert(self.filtered_indices.len());
                self.filtered_indices.push(usize::MAX); // sentinel
                self.filtered_indices.extend(group_indices);
            }
        } else {
            self.filtered_indices = base_indices;
        }

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

        if self.tree_cache.contains_key(&path) {
            return;
        }

        self.tree_debounce_at =
            Some(std::time::Instant::now() + std::time::Duration::from_millis(200));
        self.tree_requested_path = Some(path);
    }

    /// Request a tree scan for the current group's project root.
    pub fn request_group_tree_scan(&mut self) {
        let info = match self.current_group_info() {
            Some(info) => info,
            None => return,
        };

        if self.tree_cache.contains_key(&info.path) {
            return;
        }

        self.tree_debounce_at =
            Some(std::time::Instant::now() + std::time::Duration::from_millis(200));
        self.tree_requested_path = Some(info.path);
    }

    /// Check if debounce timer has elapsed and launch the tree scan thread.
    pub fn maybe_start_tree_scan(&mut self) {
        let deadline = match self.tree_debounce_at {
            Some(d) => d,
            None => return,
        };

        if std::time::Instant::now() < deadline {
            return;
        }

        let path = match self.tree_requested_path.take() {
            Some(p) => p,
            None => return,
        };
        self.tree_debounce_at = None;

        if self.tree_cache.contains_key(&path) {
            return;
        }

        if self.tree_loading {
            return;
        }

        self.tree_loading = true;
        let (tx, rx) = mpsc::channel();
        self.tree_rx = Some(rx);

        let scan_path = path.clone();
        std::thread::spawn(move || {
            let data = App::build_tree_data(&scan_path);
            let _ = tx.send((scan_path, data));
        });
    }

    pub fn tree_scroll_down(&mut self) {
        self.tree_scroll = self.tree_scroll.saturating_add(1);
    }

    pub fn tree_scroll_up(&mut self) {
        self.tree_scroll = self.tree_scroll.saturating_sub(1);
    }

    pub fn tree_scroll_top(&mut self) {
        self.tree_scroll = 0;
    }

    pub fn tree_scroll_bottom(&mut self, visible_height: u16) {
        if let Some(item) = self.current_item() {
            if let Some(data) = self.tree_cache.get(&item.path) {
                let total = data.entries.len() as u16;
                self.tree_scroll = total.saturating_sub(visible_height);
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
        let rx = match self.tree_rx.as_ref() {
            Some(rx) => rx,
            None => return,
        };

        match rx.try_recv() {
            Ok((path, data)) => {
                self.tree_cache.insert(path, data);
                self.tree_loading = false;
                self.tree_rx = None;
            }
            Err(mpsc::TryRecvError::Empty) => {}
            Err(mpsc::TryRecvError::Disconnected) => {
                self.tree_loading = false;
                self.tree_rx = None;
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
            size_ready: true,
        }
    }

    fn make_test_app(items: Vec<ScanResult>) -> App {
        let (tx, rx) = mpsc::channel();
        drop(tx);
        let n = items.len();
        let mut app = App::new(rx);
        app.items = items;
        app.selected = vec![false; n];
        app.apply_sort();
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

        app.sort_mode = SortMode::SizeDesc;
        app.apply_sort();
        app.apply_filter();

        assert_eq!(app.items[0].size, 500);
        assert_eq!(app.items[1].size, 200);
        assert_eq!(app.items[2].size, 100);
    }

    #[test]
    fn test_filter_by_text() {
        let mut app = make_test_app(vec![
            make_result("node_modules", "/projects/web/node_modules", 100),
            make_result("node_modules", "/projects/api/node_modules", 200),
            make_result("Pods", "/projects/ios/Pods", 300),
        ]);

        app.filter_text = "api".to_string();
        app.apply_filter();

        assert_eq!(app.filtered_indices.len(), 1);
        assert_eq!(
            app.items[app.filtered_indices[0]].path.to_string_lossy(),
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

        app.type_filter = Some("Pods".to_string());
        app.apply_filter();

        assert_eq!(app.filtered_indices.len(), 1);
        assert_eq!(app.items[app.filtered_indices[0]].target_name, "Pods");
    }

    #[test]
    fn test_toggle_selection() {
        let mut app = make_test_app(vec![
            make_result("node_modules", "/a/node_modules", 100),
            make_result("node_modules", "/b/node_modules", 200),
        ]);

        // Cursor is at 0
        app.list_state.select(Some(0));
        app.toggle_selection();
        let idx = app.filtered_indices[0];
        assert!(app.selected[idx]);

        // Toggle again to deselect
        app.toggle_selection();
        assert!(!app.selected[idx]);
    }

    #[test]
    fn test_invert_selection() {
        let mut app = make_test_app(vec![
            make_result("node_modules", "/a/node_modules", 100),
            make_result("node_modules", "/b/node_modules", 200),
            make_result("Pods", "/c/Pods", 300),
        ]);

        // Select first item
        let first_idx = app.filtered_indices[0];
        app.selected[first_idx] = true;

        app.invert_selection();

        assert!(!app.selected[app.filtered_indices[0]]);
        assert!(app.selected[app.filtered_indices[1]]);
        assert!(app.selected[app.filtered_indices[2]]);
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
        app.tree_cache.insert(
            std::path::PathBuf::from("/a/node_modules"),
            TreeData {
                entries: vec![],
                top_dirs: vec![],
                project_type: None,
            },
        );
        assert!(!app.tree_cache.is_empty());
        assert!(app
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
        app.project_grouping = true;
        app.apply_filter();

        // my-app group (600) first, then other (200)
        assert_eq!(app.group_separators.len(), 2);

        let real: Vec<usize> = (0..app.filtered_indices.len())
            .filter(|i| !app.group_separators.contains(i))
            .map(|i| app.filtered_indices[i])
            .collect();
        // my-app items first (sorted by size desc: 500, 100), then other (200)
        assert_eq!(app.items[real[0]].size, 500);
        assert_eq!(app.items[real[1]].size, 100);
        assert_eq!(app.items[real[2]].size, 200);
    }

    #[test]
    fn test_project_grouping_fallback_no_git() {
        let items = vec![
            make_result("node_modules", "/a/node_modules", 100),
            make_result("node_modules", "/b/node_modules", 300),
        ];

        let mut app = make_test_app(items);
        app.project_grouping = true;
        app.apply_filter();

        // Two separate groups (different parents)
        assert_eq!(app.group_separators.len(), 2);
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
        app.project_grouping = false;
        app.apply_filter();

        assert!(app.group_separators.is_empty());
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
        app.sort_mode = SortMode::Name;
        app.project_grouping = true;
        app.apply_sort();
        app.apply_filter();

        // Groups should be sorted alphabetically: alpha first, then zebra
        let real: Vec<usize> = (0..app.filtered_indices.len())
            .filter(|i| !app.group_separators.contains(i))
            .map(|i| app.filtered_indices[i])
            .collect();
        assert!(app.items[real[0]].path.to_string_lossy().contains("alpha"));
        assert!(app.items[real[1]].path.to_string_lossy().contains("zebra"));
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
        app.project_grouping = true;
        app.apply_filter();

        // Cursor on first separator (position 0)
        app.list_state.select(Some(0));
        assert!(app.group_separators.contains(&0));

        // Toggle selects all items in the first group
        app.toggle_selection();

        let group_items: Vec<usize> = (1..app.filtered_indices.len())
            .take_while(|i| !app.group_separators.contains(i))
            .map(|i| app.filtered_indices[i])
            .collect();
        assert!(group_items.iter().all(|&i| app.selected[i]));

        // Second group items should NOT be selected
        let second_group_item = app.filtered_indices[app.filtered_indices.len() - 1];
        assert!(!app.selected[second_group_item]);

        // Toggle again deselects all items in the group
        app.toggle_selection();
        assert!(group_items.iter().all(|&i| !app.selected[i]));
    }
}
