use serde::Deserialize;

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct Target {
    pub name: String,
    pub dirs: Vec<String>,
    #[serde(default)]
    pub indicator: Option<String>,
}

impl Target {
    pub fn matches_dir_name(&self, name: &str) -> bool {
        self.dirs.iter().any(|d| d == name)
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
            indicator: Some("package.json".to_string()),
        };
        assert!(target.matches_dir_name("node_modules"));
        assert!(!target.matches_dir_name("node_module"));
    }

    #[test]
    fn test_matches_multiple_dirs() {
        let target = Target {
            name: "Gradle cache".to_string(),
            dirs: vec![".gradle".to_string(), "build".to_string()],
            indicator: Some("build.gradle".to_string()),
        };
        assert!(target.matches_dir_name(".gradle"));
        assert!(target.matches_dir_name("build"));
        assert!(!target.matches_dir_name("gradle"));
    }
}
