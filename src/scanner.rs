use crate::targets::Target;
use rayon::prelude::*;
use std::collections::HashMap;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::time::SystemTime;

#[derive(Debug, Clone)]
pub struct ScanResult {
    pub path: PathBuf,
    pub target_name: String,
    pub size: u64,
    pub shared_size: u64,
    pub last_modified: Option<SystemTime>,
    pub file_count: u64,
    pub git_root: Option<PathBuf>,
}

#[derive(Debug)]
pub enum ScanMessage {
    Found(ScanResult),
    Progress {
        dirs_scanned: u64,
    },
    Complete,
    #[allow(dead_code)]
    Error(String),
}

pub struct DirStats {
    /// Disk usage freed by deleting the directory
    pub size: u64,
    /// Disk usage of hard-linked files that also live outside the directory
    pub shared: u64,
    pub files: u64,
}

/// Compute disk usage and file count of a directory recursively, like `du`.
/// Uses rayon for parallelism on large directories.
pub fn compute_dir_stats(path: &Path) -> DirStats {
    let walk = walk_dir(path);
    let mut stats = DirStats {
        size: walk.size,
        shared: 0,
        files: walk.files,
    };
    // A hard-linked file is only freed once all its links are gone
    for link in walk.links.into_values() {
        if link.seen >= link.nlink {
            stats.size += link.bytes;
        } else {
            stats.shared += link.bytes;
        }
    }
    stats
}

#[derive(Default)]
struct Walk {
    size: u64,
    files: u64,
    /// Files with several hard links, keyed by (device, inode)
    links: HashMap<(u64, u64), Link>,
}

struct Link {
    nlink: u64,
    seen: u64,
    bytes: u64,
}

impl Walk {
    fn merge(mut self, other: Walk) -> Walk {
        self.size += other.size;
        self.files += other.files;
        for (key, link) in other.links {
            self.links
                .entry(key)
                .and_modify(|l| l.seen += link.seen)
                .or_insert(link);
        }
        self
    }
}

fn walk_dir(path: &Path) -> Walk {
    let entries: Vec<_> = match fs::read_dir(path) {
        Ok(rd) => rd.filter_map(|e| e.ok()).collect(),
        Err(_) => return Walk::default(),
    };

    entries
        .par_iter()
        .map(|entry| {
            let Ok(file_type) = entry.file_type() else {
                return Walk::default();
            };
            if file_type.is_symlink() {
                return Walk::default();
            }
            if file_type.is_dir() {
                return walk_dir(&entry.path());
            }
            let mut walk = Walk {
                files: 1,
                ..Walk::default()
            };
            let Ok(meta) = entry.metadata() else {
                return walk;
            };
            let bytes = meta.blocks() * 512;
            if meta.nlink() > 1 {
                let link = Link {
                    nlink: meta.nlink(),
                    seen: 1,
                    bytes,
                };
                walk.links.insert((meta.dev(), meta.ino()), link);
            } else {
                walk.size = bytes;
            }
            walk
        })
        .reduce(Walk::default, Walk::merge)
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

struct ScanContext {
    root: PathBuf,
    targets: Vec<Target>,
    skip: Vec<String>,
    include_hidden: bool,
    tx: mpsc::Sender<ScanMessage>,
    dirs_scanned: Arc<AtomicU64>,
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
    let ctx = ScanContext {
        root: root.clone(),
        targets,
        skip,
        include_hidden,
        tx,
        dirs_scanned: Arc::new(AtomicU64::new(0)),
    };

    rayon::scope(|s| {
        scan_dir(root, &ctx, s);
    });

    let _ = ctx.tx.send(ScanMessage::Complete);
}

fn scan_dir<'scope>(path: PathBuf, ctx: &'scope ScanContext, scope: &rayon::Scope<'scope>) {
    let entries = match fs::read_dir(&path) {
        Ok(rd) => rd,
        Err(_) => return,
    };

    for entry in entries.filter_map(|e| e.ok()) {
        let file_type = match entry.file_type() {
            Ok(ft) => ft,
            Err(_) => continue,
        };

        if !file_type.is_dir() || file_type.is_symlink() {
            continue;
        }

        let name = entry.file_name();
        let name_str = match name.to_str() {
            Some(n) => n,
            None => continue,
        };

        // Skip list check: entries without '/' match by dir name,
        // entries with '/' match by relative path from root
        let entry_path = entry.path();
        if ctx.skip.iter().any(|s| {
            if s.contains('/') {
                entry_path
                    .strip_prefix(&ctx.root)
                    .map(|rel| rel == Path::new(s))
                    .unwrap_or(false)
            } else {
                s == name_str
            }
        }) {
            continue;
        }

        // Skip hidden dirs that don't match any target (unless --hidden)
        if !ctx.include_hidden
            && name_str.starts_with('.')
            && !ctx.targets.iter().any(|t| t.mentions_dir(name_str))
        {
            continue;
        }

        let matched_target = ctx.targets.iter().find(|t| t.matches(&entry_path));

        if let Some(target) = matched_target {
            // Found a target — compute stats in this branch (single-pass)
            // Nested targets are inherently excluded: we don't descend further
            let stats = compute_dir_stats(&entry_path);
            let last_modified = fs::metadata(&entry_path).and_then(|m| m.modified()).ok();
            let git_root = find_git_root(&entry_path);

            let _ = ctx.tx.send(ScanMessage::Found(ScanResult {
                path: entry_path,
                target_name: target.name.clone(),
                size: stats.size,
                shared_size: stats.shared,
                last_modified,
                file_count: stats.files,
                git_root,
            }));
        } else {
            // Not a target — spawn parallel exploration of this subtree
            let count = ctx.dirs_scanned.fetch_add(1, Ordering::Relaxed) + 1;
            if count.is_multiple_of(500) {
                let _ = ctx.tx.send(ScanMessage::Progress {
                    dirs_scanned: count,
                });
            }

            scope.spawn(move |s| {
                scan_dir(entry_path, ctx, s);
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::mpsc;
    use tempfile::TempDir;

    fn collect_scan_results(rx: mpsc::Receiver<ScanMessage>) -> Vec<ScanResult> {
        let mut results = Vec::new();
        for msg in rx {
            match msg {
                ScanMessage::Found(r) => results.push(r),
                ScanMessage::Complete => break,
                ScanMessage::Error(e) => panic!("Scan error: {}", e),
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
                indicators: vec!["package.json".to_string()],
            },
            Target {
                name: "Pods".to_string(),
                dirs: vec!["Pods".to_string()],
                indicators: vec!["Podfile".to_string()],
            },
            Target {
                name: "Gradle cache".to_string(),
                dirs: vec!["build".to_string()],
                indicators: vec!["build.gradle".to_string()],
            },
        ];

        let (tx, rx) = std::sync::mpsc::channel();
        scan(dir.path().to_path_buf(), targets, vec![], false, tx);

        let results = collect_scan_results(rx);

        assert_eq!(results.len(), 2);
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
            indicators: vec![],
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
            indicators: vec!["package.json".to_string()],
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
                indicators: vec![],
            },
            Target {
                name: "node_modules".to_string(),
                dirs: vec!["node_modules".to_string()],
                indicators: vec!["package.json".to_string()],
            },
        ];

        let (tx, rx) = std::sync::mpsc::channel();
        scan(dir.path().to_path_buf(), targets, vec![], false, tx);

        let results = collect_scan_results(rx);

        // Should find .pnpm-store but NOT the node_modules inside .cache
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].target_name, "pnpm store");
    }

    fn disk_usage(path: &Path) -> u64 {
        use std::os::unix::fs::MetadataExt;
        fs::metadata(path).unwrap().blocks() * 512
    }

    #[test]
    fn test_dir_size_counts_disk_blocks() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("sub")).unwrap();
        // A 1-byte file still occupies a full block on disk
        fs::write(dir.path().join("a.txt"), "x").unwrap();
        fs::write(dir.path().join("sub/b.txt"), "x".repeat(10_000)).unwrap();
        let expected =
            disk_usage(&dir.path().join("a.txt")) + disk_usage(&dir.path().join("sub/b.txt"));

        let stats = compute_dir_stats(dir.path());
        assert_eq!(stats.size, expected);
        assert_eq!(stats.files, 2);
        assert_eq!(stats.shared, 0);
    }

    #[test]
    fn test_hardlink_inside_dir_counted_once() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.bin");
        fs::write(&a, "x".repeat(50_000)).unwrap();
        fs::hard_link(&a, dir.path().join("b.bin")).unwrap();

        let stats = compute_dir_stats(dir.path());
        assert_eq!(stats.size, disk_usage(&a));
        assert_eq!(stats.files, 2);
        assert_eq!(stats.shared, 0);
    }

    #[test]
    fn test_hardlink_outside_dir_is_shared() {
        // pnpm-style: node_modules files hard-linked from a store elsewhere
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("store")).unwrap();
        fs::create_dir_all(dir.path().join("project/node_modules")).unwrap();
        let stored = dir.path().join("store/pkg.bin");
        fs::write(&stored, "x".repeat(50_000)).unwrap();
        fs::hard_link(&stored, dir.path().join("project/node_modules/pkg.bin")).unwrap();

        // Deleting node_modules alone frees nothing: the store keeps the data
        let stats = compute_dir_stats(&dir.path().join("project/node_modules"));
        assert_eq!(stats.size, 0);
        assert_eq!(stats.shared, disk_usage(&stored));
    }

    #[test]
    fn test_scan_reports_shared_size() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("store")).unwrap();
        fs::create_dir_all(root.join("app/node_modules")).unwrap();
        fs::write(root.join("app/package.json"), "{}").unwrap();
        let stored = root.join("store/pkg.bin");
        fs::write(&stored, "x".repeat(50_000)).unwrap();
        fs::hard_link(&stored, root.join("app/node_modules/pkg.bin")).unwrap();

        let targets = vec![Target {
            name: "node_modules".to_string(),
            dirs: vec!["node_modules".to_string()],
            indicators: vec!["package.json".to_string()],
        }];
        let (tx, rx) = std::sync::mpsc::channel();
        scan(root.to_path_buf(), targets, vec![], false, tx);

        let results = collect_scan_results(rx);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].size, 0);
        assert_eq!(results[0].shared_size, disk_usage(&stored));
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
            indicators: vec!["package.json".to_string()],
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
            indicators: vec![],
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
            indicators: vec![],
        }];

        // Without --hidden: should still find .cache because it matches a target
        let (tx, rx) = std::sync::mpsc::channel();
        scan(root.to_path_buf(), targets.clone(), vec![], false, tx);

        let results = collect_scan_results(rx);

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].target_name, ".cache");
    }

    #[test]
    fn test_scan_skips_by_relative_path() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();

        // Target inside a nested path that should be skipped
        fs::create_dir_all(root.join(".local/share/Steam/node_modules")).unwrap();
        fs::write(root.join(".local/share/Steam/package.json"), "{}").unwrap();

        // Target outside the skip path that should still be found
        fs::create_dir_all(root.join("project/node_modules/pkg")).unwrap();
        fs::write(root.join("project/package.json"), "{}").unwrap();
        fs::write(
            root.join("project/node_modules/pkg/index.js"),
            "x".repeat(100),
        )
        .unwrap();

        let targets = vec![Target {
            name: "node_modules".to_string(),
            dirs: vec!["node_modules".to_string()],
            indicators: vec!["package.json".to_string()],
        }];

        let (tx, rx) = std::sync::mpsc::channel();
        scan(
            root.to_path_buf(),
            targets,
            vec![".local/share/Steam".to_string()],
            true, // --hidden to enter .local
            tx,
        );

        let results = collect_scan_results(rx);

        assert_eq!(results.len(), 1);
        assert!(results[0].path.to_string_lossy().contains("project"));
    }

    #[test]
    fn test_scan_skip_path_requires_exact_match() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();

        // "share/Steam" exists at root but skip is "a/share/Steam"
        // — should NOT be skipped because the relative path doesn't match
        fs::create_dir_all(root.join("share/Steam/node_modules")).unwrap();
        fs::write(root.join("share/Steam/package.json"), "{}").unwrap();
        fs::write(
            root.join("share/Steam/node_modules/index.js"),
            "x".repeat(100),
        )
        .unwrap();

        let targets = vec![Target {
            name: "node_modules".to_string(),
            dirs: vec!["node_modules".to_string()],
            indicators: vec!["package.json".to_string()],
        }];

        let (tx, rx) = std::sync::mpsc::channel();
        scan(
            root.to_path_buf(),
            targets,
            vec!["a/share/Steam".to_string()],
            false,
            tx,
        );

        let results = collect_scan_results(rx);

        // "share/Steam" != "a/share/Steam", so the target inside should be found
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn test_default_targets_detect_common_layouts() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        // (project, indicator file, target dir)
        let layouts = [
            ("yarn-app", ".yarnrc.yml", ".yarn/cache"),
            ("next-app", "next.config.mjs", ".next"),
            ("nuxt-app", "nuxt.config.js", ".nuxt"),
            ("kts-app", "build.gradle.kts", "build"),
        ];
        for (project, indicator, target_dir) in layouts {
            fs::create_dir_all(root.join(project).join(target_dir)).unwrap();
            fs::write(root.join(project).join(indicator), "").unwrap();
        }

        let targets = crate::config::Config::load(None).unwrap().targets;
        let (tx, rx) = std::sync::mpsc::channel();
        scan(root.to_path_buf(), targets, vec![], false, tx);

        let mut found: Vec<(String, String)> = collect_scan_results(rx)
            .into_iter()
            .map(|r| {
                let rel = r.path.strip_prefix(root).unwrap();
                (rel.to_string_lossy().into_owned(), r.target_name)
            })
            .collect();
        found.sort();
        let expected = [
            ("kts-app/build", "Gradle cache"),
            ("next-app/.next", "Next.js build"),
            ("nuxt-app/.nuxt", "Nuxt build"),
            ("yarn-app/.yarn/cache", "Yarn cache"),
        ];
        let expected: Vec<(String, String)> = expected
            .iter()
            .map(|(p, n)| (p.to_string(), n.to_string()))
            .collect();
        assert_eq!(found, expected);
    }
}
