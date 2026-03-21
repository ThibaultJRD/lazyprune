use crate::targets::Target;
use serde::Deserialize;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

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
                "3000-3009".into(),
                "4000-4009".into(),
                "5173-5174".into(),
                "8080-8090".into(),
            ],
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct Config {
    #[serde(default = "default_root")]
    pub root: String,
    #[serde(default)]
    pub skip: Vec<String>,
    #[serde(default)]
    pub targets: Vec<Target>,
    #[serde(default)]
    pub ports: PortsConfig,
}

/// Expand a list of port range strings (e.g. "3000-3009", "5173") into a set of u16 port numbers.
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

fn default_root() -> String {
    "~".to_string()
}

const DEFAULT_CONFIG: &str = include_str!("../config.default.toml");

impl Config {
    /// Load config: embedded defaults merged with user config if it exists.
    pub fn load(user_config_path: Option<&Path>) -> Result<Self, String> {
        let mut config: Config = toml::from_str(DEFAULT_CONFIG)
            .map_err(|e| format!("Failed to parse default config: {e}"))?;

        if let Some(path) = user_config_path {
            if path.exists() {
                let user_str = std::fs::read_to_string(path)
                    .map_err(|e| format!("Failed to read {}: {e}", path.display()))?;
                let user_config: UserConfigOverride = toml::from_str(&user_str)
                    .map_err(|e| format!("Failed to parse {}: {e}", path.display()))?;
                user_config.apply_to(&mut config);
            }
        }

        Ok(config)
    }

    /// Resolve root path (expand ~)
    pub fn root_path(&self) -> PathBuf {
        if self.root.starts_with('~') {
            if let Some(home) = dirs::home_dir() {
                let rest = self.root.get(2..).unwrap_or("");
                return home.join(rest);
            }
        }
        PathBuf::from(&self.root)
    }

    /// Return default config as string for --init-config
    pub fn default_config_string() -> &'static str {
        DEFAULT_CONFIG
    }

    /// Return platform config file path: ~/.config/lazyprune/config.toml
    pub fn user_config_path() -> Option<PathBuf> {
        dirs::config_dir().map(|d| d.join("lazyprune").join("config.toml"))
    }
}

/// Partial override: only fields present in user config are applied
#[derive(Debug, Deserialize, Default)]
struct UserConfigOverride {
    root: Option<String>,
    skip: Option<Vec<String>>,
    targets: Option<Vec<Target>>,
    #[serde(default)]
    ports: Option<UserPortsOverride>,
}

#[derive(Debug, Deserialize)]
struct UserPortsOverride {
    dev_filter_enabled: Option<bool>,
    dev_filter: Option<Vec<String>>,
}

impl UserConfigOverride {
    fn apply_to(self, config: &mut Config) {
        if let Some(root) = self.root {
            config.root = root;
        }
        if let Some(skip) = self.skip {
            config.skip = skip;
        }
        if let Some(targets) = self.targets {
            config.targets = targets;
        }
        if let Some(ref ports) = self.ports {
            if let Some(enabled) = ports.dev_filter_enabled {
                config.ports.dev_filter_enabled = enabled;
            }
            if let Some(ref filter) = ports.dev_filter {
                config.ports.dev_filter = filter.clone();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_load_defaults() {
        let config = Config::load(None).unwrap();
        assert_eq!(config.root, "~");
        assert!(!config.targets.is_empty());
        assert!(config.targets.iter().any(|t| t.name == "node_modules"));
    }

    #[test]
    fn test_load_with_user_override() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.toml");
        std::fs::write(
            &config_path,
            r#"
            root = "~/Projects"
            skip = [".Trash"]
        "#,
        )
        .unwrap();

        let config = Config::load(Some(&config_path)).unwrap();
        assert_eq!(config.root, "~/Projects");
        assert_eq!(config.skip, vec![".Trash"]);
        assert!(!config.targets.is_empty());
    }

    #[test]
    fn test_load_nonexistent_user_config() {
        let path = Path::new("/tmp/nonexistent_prune_config.toml");
        let config = Config::load(Some(path)).unwrap();
        assert_eq!(config.root, "~");
    }

    #[test]
    fn test_root_path_expands_tilde() {
        let config = Config::load(None).unwrap();
        let path = config.root_path();
        assert!(!path.to_string_lossy().contains('~'));
        assert!(path.is_absolute());
    }

    #[test]
    fn test_ports_config_defaults() {
        let config = Config::load(None).unwrap();
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
        let default = Config::load(None).unwrap();
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
}
