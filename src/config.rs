use crate::targets::Target;
use serde::Deserialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Deserialize)]
pub struct Config {
    #[serde(default = "default_root")]
    pub root: String,
    #[serde(default)]
    pub skip: Vec<String>,
    #[serde(default)]
    pub targets: Vec<Target>,
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
}
