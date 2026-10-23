use serde::{Deserialize, Deserializer};
use std::path::Path;

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct Target {
    pub name: String,
    pub dirs: Vec<String>,
    /// Files confirming a real target when one of them exists in the project dir.
    /// A single string or a list in TOML; empty means always match.
    #[serde(default, rename = "indicator", deserialize_with = "one_or_many")]
    pub indicators: Vec<String>,
}

fn one_or_many<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<String>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum OneOrMany {
        One(String),
        Many(Vec<String>),
    }
    Ok(match OneOrMany::deserialize(deserializer)? {
        OneOrMany::One(s) => vec![s],
        OneOrMany::Many(v) => v,
    })
}

impl Target {
    /// True if `name` is a component of one of `dirs` (e.g. ".yarn" for ".yarn/cache"),
    /// so the scanner must descend into it even when hidden.
    pub fn mentions_dir(&self, name: &str) -> bool {
        self.dirs.iter().any(|d| d.split('/').any(|c| c == name))
    }

    /// True if `path` ends with one of `dirs` and, when indicators are set,
    /// one of them exists in the project dir containing it.
    pub fn matches(&self, path: &Path) -> bool {
        self.dirs.iter().any(|d| {
            let dir = Path::new(d);
            if !path.ends_with(dir) {
                return false;
            }
            if self.indicators.is_empty() {
                return true;
            }
            path.ancestors()
                .nth(dir.components().count())
                .is_some_and(|project| self.indicators.iter().any(|i| project.join(i).exists()))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_matches_dir_name() {
        let target = Target {
            name: "node_modules".to_string(),
            dirs: vec!["node_modules".to_string()],
            indicators: vec![],
        };
        assert!(target.matches(Path::new("/p/node_modules")));
        assert!(!target.matches(Path::new("/p/node_module")));
    }

    #[test]
    fn test_matches_multiple_dirs() {
        let target = Target {
            name: "Gradle cache".to_string(),
            dirs: vec![".gradle".to_string(), "build".to_string()],
            indicators: vec![],
        };
        assert!(target.matches(Path::new("/p/.gradle")));
        assert!(target.matches(Path::new("/p/build")));
        assert!(!target.matches(Path::new("/p/gradle")));
    }

    #[test]
    fn test_matches_any_indicator_in_project_dir() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("b.toml"), "").unwrap();
        let target = Target {
            name: "yarn".to_string(),
            dirs: vec![".yarn/cache".to_string()],
            indicators: vec!["a.toml".to_string(), "b.toml".to_string()],
        };
        // Indicator is looked up next to `.yarn`, not inside it
        assert!(target.matches(&dir.path().join(".yarn/cache")));
        assert!(!target.matches(&dir.path().join("sub/.yarn/cache")));
    }

    #[test]
    fn test_mentions_dir_includes_intermediate_components() {
        let target = Target {
            name: "yarn".to_string(),
            dirs: vec![".yarn/cache".to_string()],
            indicators: vec![],
        };
        assert!(target.mentions_dir(".yarn"));
        assert!(target.mentions_dir("cache"));
        assert!(!target.mentions_dir(".cache"));
    }

    #[test]
    fn test_indicator_accepts_string_or_list() {
        let one: Target = toml::from_str("name = 'a'\ndirs = ['x']\nindicator = 'f'").unwrap();
        assert_eq!(one.indicators, vec!["f"]);
        let many: Target =
            toml::from_str("name = 'a'\ndirs = ['x']\nindicator = ['f', 'g']").unwrap();
        assert_eq!(many.indicators, vec!["f", "g"]);
        let none: Target = toml::from_str("name = 'a'\ndirs = ['x']").unwrap();
        assert!(none.indicators.is_empty());
    }
}
