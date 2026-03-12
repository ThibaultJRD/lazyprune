use crate::targets::Target;
use ignore::WalkBuilder;
use rayon::prelude::*;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

#[derive(Debug, Clone)]
pub struct ScanResult {
    pub path: PathBuf,
    pub target_name: String,
    pub size: u64,
    pub last_modified: Option<SystemTime>,
    pub file_count: u64,
    pub git_root: Option<PathBuf>,
    pub size_ready: bool,
}

#[derive(Debug)]
pub enum ScanMessage {
    Found(ScanResult),
    StatsReady {
        path: PathBuf,
        size: u64,
        file_count: u64,
    },
    Progress { dirs_scanned: u64 },
    Complete,
    Error(#[allow(dead_code)] String),
}

/// Compute total size and file count of a directory recursively.
/// Uses rayon for parallelism on large directories.
pub fn compute_dir_stats(path: &Path) -> (u64, u64) {
    let entries: Vec<_> = match fs::read_dir(path) {
        Ok(rd) => rd.filter_map(|e| e.ok()).collect(),
        Err(_) => return (0, 0),
    };

    entries
        .par_iter()
        .map(|entry| {
            let p = entry.path();
            if p.is_symlink() {
                return (0, 0);
            }
            if p.is_dir() {
                compute_dir_stats(&p)
            } else {
                let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
                (size, 1)
            }
        })
        .reduce(|| (0, 0), |(s1, c1), (s2, c2)| (s1 + s2, c1 + c2))
}

/// Walk up from `path` looking for a `.git` directory.
fn find_git_root(path: &Path) -> Option<PathBuf> {
    let mut current = path.parent()?;
    loop {
        if current.join(".git").exists() {
            return Some(current.to_path_buf());
        }
        current = current.parent()?;
    }
}

/// Check if `path` is inside any directory in `found` by walking ancestors.
/// O(path_depth) with HashSet lookups instead of O(found.len()) with starts_with.
fn is_under_found_target(path: &Path, found: &HashSet<PathBuf>) -> bool {
    let mut ancestor = path.to_path_buf();
    while ancestor.pop() {
        if found.contains(&ancestor) {
            return true;
        }
    }
    false
}

/// Run the scan synchronously. Sends results via channel as they're found.
/// Called from a spawned thread.
pub fn scan(
    root: PathBuf,
    targets: Vec<Target>,
    skip: Vec<String>,
    include_hidden: bool,
    tx: mpsc::Sender<ScanMessage>,
) {
    let found_targets: Arc<Mutex<HashSet<PathBuf>>> = Arc::new(Mutex::new(HashSet::new()));

    // Use filter_entry to prevent descending into:
    // - directories in the skip list
    // - hidden directories that don't match any target
    // - directories inside already-found targets
    let targets_for_filter = targets.clone();
    let skip_for_filter = skip.clone();
    let found_for_filter = Arc::clone(&found_targets);
    let walker = WalkBuilder::new(&root)
        .hidden(false) // don't skip hidden dirs — we need .pnpm-store, .gradle etc.
        .git_ignore(false)
        .follow_links(false)
        .filter_entry(move |entry| {
            let Some(name) = entry.file_name().to_str() else {
                return true;
            };
            // Only filter directories
            if !entry.file_type().is_some_and(|ft| ft.is_dir()) {
                return true;
            }
            // Skip directories in the skip list
            if skip_for_filter.iter().any(|s| s == name) {
                return false;
            }
            // Skip hidden dirs that don't match any target (unless --hidden)
            if !include_hidden
                && name.starts_with('.')
                && !targets_for_filter.iter().any(|t| t.matches_dir_name(name))
            {
                return false;
            }
            // Skip subdirs of already-found targets (prevents descending into node_modules/...)
            if let Ok(found) = found_for_filter.lock() {
                if is_under_found_target(entry.path(), &found) {
                    return false;
                }
            }
            true
        })
        .build();

    let mut dirs_scanned: u64 = 0;

    rayon::scope(|s| {
        for entry in walker {
            let entry = match entry {
                Ok(e) => e,
                Err(e) => {
                    let _ = tx.send(ScanMessage::Error(e.to_string()));
                    continue;
                }
            };

            let path = entry.path();
            if !path.is_dir() {
                continue;
            }

            // Skip if this path is inside an already-found target directory.
            if let Ok(found) = found_targets.lock() {
                if is_under_found_target(path, &found) {
                    continue;
                }
            }

            let dir_name = match path.file_name().and_then(|n| n.to_str()) {
                Some(n) => n.to_string(),
                None => continue,
            };

            dirs_scanned += 1;
            if dirs_scanned.is_multiple_of(500) {
                let _ = tx.send(ScanMessage::Progress { dirs_scanned });
            }

            for target in &targets {
                if !target.matches_dir_name(&dir_name) {
                    continue;
                }

                if let Some(ref indicator) = target.indicator {
                    let parent = match path.parent() {
                        Some(p) => p,
                        None => continue,
                    };
                    if !parent.join(indicator).exists() {
                        continue;
                    }
                }

                let last_modified = fs::metadata(path).and_then(|m| m.modified()).ok();
                let git_root = find_git_root(path);
                let path_buf = path.to_path_buf();

                if let Ok(mut found) = found_targets.lock() {
                    found.insert(path_buf.clone());
                }

                let _ = tx.send(ScanMessage::Found(ScanResult {
                    path: path_buf.clone(),
                    target_name: target.name.clone(),
                    size: 0,
                    last_modified,
                    file_count: 0,
                    git_root,
                    size_ready: false,
                }));

                // Compute stats in parallel on rayon threadpool
                let tx = tx.clone();
                s.spawn(move |_| {
                    let (size, file_count) = compute_dir_stats(&path_buf);
                    let _ = tx.send(ScanMessage::StatsReady {
                        path: path_buf,
                        size,
                        file_count,
                    });
                });

                break;
            }
        }
        // rayon::scope waits for all spawned stats tasks here
    });

    let _ = tx.send(ScanMessage::Complete);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::mpsc;
    use tempfile::TempDir;

    /// Collect scan results, applying StatsReady updates, and return final results.
    fn collect_scan_results(rx: mpsc::Receiver<ScanMessage>) -> Vec<ScanResult> {
        let mut results = Vec::new();
        for msg in rx {
            match msg {
                ScanMessage::Found(r) => results.push(r),
                ScanMessage::StatsReady {
                    path,
                    size,
                    file_count,
                } => {
                    if let Some(r) = results.iter_mut().find(|r| r.path == path) {
                        r.size = size;
                        r.file_count = file_count;
                        r.size_ready = true;
                    }
                }
                ScanMessage::Complete => break,
                _ => {}
            }
        }
        results
    }

    fn setup_test_tree() -> TempDir {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();

        // project-a with node_modules and package.json
        fs::create_dir_all(root.join("project-a/node_modules/some-pkg")).unwrap();
        fs::write(root.join("project-a/package.json"), "{}").unwrap();
        fs::write(
            root.join("project-a/node_modules/some-pkg/index.js"),
            "x".repeat(1000),
        )
        .unwrap();

        // project-b with Pods and Podfile
        fs::create_dir_all(root.join("project-b/Pods/SomePod")).unwrap();
        fs::write(root.join("project-b/Podfile"), "").unwrap();
        fs::write(root.join("project-b/Pods/SomePod/lib.a"), "x".repeat(5000)).unwrap();

        // random dir called "build" with NO indicator -> should be ignored
        fs::create_dir_all(root.join("random/build")).unwrap();
        fs::write(root.join("random/build/output.txt"), "hello").unwrap();

        dir
    }

    #[test]
    fn test_scan_finds_targets_with_indicators() {
        let dir = setup_test_tree();
        let targets = vec![
            Target {
                name: "node_modules".to_string(),
                dirs: vec!["node_modules".to_string()],
                indicator: Some("package.json".to_string()),
            },
            Target {
                name: "Pods".to_string(),
                dirs: vec!["Pods".to_string()],
                indicator: Some("Podfile".to_string()),
            },
            Target {
                name: "Gradle cache".to_string(),
                dirs: vec!["build".to_string()],
                indicator: Some("build.gradle".to_string()),
            },
        ];

        let (tx, rx) = std::sync::mpsc::channel();
        scan(dir.path().to_path_buf(), targets, vec![], false, tx);

        let results = collect_scan_results(rx);

        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|r| r.size_ready));
        let names: Vec<&str> = results.iter().map(|r| r.target_name.as_str()).collect();
        assert!(names.contains(&"node_modules"));
        assert!(names.contains(&"Pods"));
    }

    #[test]
    fn test_scan_target_without_indicator_always_matches() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("project/.pnpm-store/v3")).unwrap();
        fs::write(
            dir.path().join("project/.pnpm-store/v3/data"),
            "x".repeat(100),
        )
        .unwrap();

        let targets = vec![Target {
            name: "pnpm store".to_string(),
            dirs: vec![".pnpm-store".to_string()],
            indicator: None,
        }];

        let (tx, rx) = std::sync::mpsc::channel();
        scan(dir.path().to_path_buf(), targets, vec![], false, tx);

        let results = collect_scan_results(rx);

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].target_name, "pnpm store");
    }

    #[test]
    fn test_scan_skips_directories() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("skip-me/node_modules")).unwrap();
        fs::write(dir.path().join("skip-me/package.json"), "{}").unwrap();

        let targets = vec![Target {
            name: "node_modules".to_string(),
            dirs: vec!["node_modules".to_string()],
            indicator: Some("package.json".to_string()),
        }];

        let (tx, rx) = std::sync::mpsc::channel();
        scan(
            dir.path().to_path_buf(),
            targets,
            vec!["skip-me".to_string()],
            false,
            tx,
        );

        let results = collect_scan_results(rx);

        assert_eq!(results.len(), 0);
    }

    #[test]
    fn test_scan_skips_hidden_non_target_dirs() {
        let dir = tempfile::tempdir().unwrap();
        // Hidden dir that IS a target -> should be found
        fs::create_dir_all(dir.path().join("project/.pnpm-store/data")).unwrap();
        fs::write(
            dir.path().join("project/.pnpm-store/data/file"),
            "x".repeat(100),
        )
        .unwrap();
        // Hidden dir that is NOT a target -> should be skipped
        fs::create_dir_all(dir.path().join("project/.cache/some-tool/node_modules")).unwrap();
        fs::write(
            dir.path().join("project/.cache/some-tool/package.json"),
            "{}",
        )
        .unwrap();

        let targets = vec![
            Target {
                name: "pnpm store".to_string(),
                dirs: vec![".pnpm-store".to_string()],
                indicator: None,
            },
            Target {
                name: "node_modules".to_string(),
                dirs: vec!["node_modules".to_string()],
                indicator: Some("package.json".to_string()),
            },
        ];

        let (tx, rx) = std::sync::mpsc::channel();
        scan(dir.path().to_path_buf(), targets, vec![], false, tx);

        let results = collect_scan_results(rx);

        // Should find .pnpm-store but NOT the node_modules inside .cache
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].target_name, "pnpm store");
    }

    #[test]
    fn test_dir_size_computes_correctly() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("sub")).unwrap();
        fs::write(dir.path().join("a.txt"), "x".repeat(1000)).unwrap();
        fs::write(dir.path().join("sub/b.txt"), "x".repeat(2000)).unwrap();

        let (size, count) = compute_dir_stats(dir.path());
        assert_eq!(size, 3000);
        assert_eq!(count, 2);
    }

    #[test]
    fn test_find_git_root_found() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join(".git")).unwrap();
        fs::create_dir_all(root.join("src/deep/path")).unwrap();
        let result = find_git_root(&root.join("src/deep/path"));
        assert_eq!(result, Some(root.to_path_buf()));
    }

    #[test]
    fn test_find_git_root_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("src/deep")).unwrap();
        let result = find_git_root(&root.join("src/deep"));
        assert_ne!(result, Some(root.join("src/deep")));
    }

    #[test]
    fn test_scan_populates_git_root() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();

        // Create a project with .git
        fs::create_dir_all(root.join("my-app/.git")).unwrap();
        fs::create_dir_all(root.join("my-app/node_modules/pkg")).unwrap();
        fs::write(root.join("my-app/package.json"), "{}").unwrap();
        fs::write(
            root.join("my-app/node_modules/pkg/index.js"),
            "x".repeat(100),
        )
        .unwrap();

        // Create a project without .git
        fs::create_dir_all(root.join("no-git/node_modules/pkg")).unwrap();
        fs::write(root.join("no-git/package.json"), "{}").unwrap();
        fs::write(
            root.join("no-git/node_modules/pkg/index.js"),
            "y".repeat(50),
        )
        .unwrap();

        let targets = vec![Target {
            name: "node_modules".to_string(),
            dirs: vec!["node_modules".to_string()],
            indicator: Some("package.json".to_string()),
        }];

        let (tx, rx) = std::sync::mpsc::channel();
        scan(root.to_path_buf(), targets, vec![], false, tx);

        let results = collect_scan_results(rx);

        assert_eq!(results.len(), 2);

        let with_git = results
            .iter()
            .find(|r| r.path.to_string_lossy().contains("my-app"))
            .unwrap();
        assert_eq!(with_git.git_root, Some(root.join("my-app")));

        let no_git = results
            .iter()
            .find(|r| r.path.to_string_lossy().contains("no-git"))
            .unwrap();
        // no-git dir has no .git, so git_root should NOT be the no-git dir itself
        assert_ne!(no_git.git_root, Some(root.join("no-git")));
    }

    #[test]
    fn test_scan_adhoc_dir_no_indicator() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("project-a/src/utils")).unwrap();
        fs::write(root.join("project-a/src/utils/mod.rs"), "x".repeat(200)).unwrap();
        fs::create_dir_all(root.join("project-b/src")).unwrap();
        fs::write(root.join("project-b/src/main.rs"), "x".repeat(100)).unwrap();
        // Non-matching dir
        fs::create_dir_all(root.join("project-c/lib")).unwrap();
        fs::write(root.join("project-c/lib/foo.rs"), "x".repeat(50)).unwrap();

        let targets = vec![Target {
            name: "src".to_string(),
            dirs: vec!["src".to_string()],
            indicator: None,
        }];

        let (tx, rx) = std::sync::mpsc::channel();
        scan(root.to_path_buf(), targets, vec![], false, tx);

        let results = collect_scan_results(rx);

        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|r| r.target_name == "src"));
    }

    #[test]
    fn test_scan_adhoc_hidden_dir() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        // .cache is hidden but IS our ad-hoc target
        fs::create_dir_all(root.join("project/.cache/stuff")).unwrap();
        fs::write(root.join("project/.cache/stuff/data"), "x".repeat(100)).unwrap();

        let targets = vec![Target {
            name: ".cache".to_string(),
            dirs: vec![".cache".to_string()],
            indicator: None,
        }];

        // Without --hidden: should still find .cache because it matches a target
        let (tx, rx) = std::sync::mpsc::channel();
        scan(root.to_path_buf(), targets.clone(), vec![], false, tx);

        let results = collect_scan_results(rx);

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].target_name, ".cache");
    }
}
