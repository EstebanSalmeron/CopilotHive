//! Project list from a TOML file: `[[projects]]` with `name`, `path` and optional
//! `allow_all`, `model` and `[projects.agent_models]` (see docs/config.example.toml).
use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Deserialize)]
pub struct Project {
    pub name: String,
    pub path: PathBuf,
    /// Start the session with ACP `allow_all` on (dangerous; off by default).
    #[serde(default)]
    pub allow_all: bool,
    /// Model applied at startup through `session/set_config_option`.
    pub model: Option<String>,
    /// Mandatory subagent models, written to the Copilot user settings at startup.
    #[serde(default)]
    pub agent_models: BTreeMap<String, String>,
}

#[derive(Deserialize)]
struct Config {
    projects: Vec<Project>,
}

pub fn default_path() -> Result<PathBuf> {
    let home = std::env::var_os("HOME").context("HOME is not set")?;
    Ok(PathBuf::from(home).join(".config/copilot-cockpit/projects.toml"))
}

pub fn parse(text: &str) -> Result<Vec<Project>> {
    let c: Config = toml::from_str(text).context("invalid projects config")?;
    if c.projects.is_empty() {
        bail!("config has no projects");
    }
    Ok(c.projects)
}

pub fn load(path: &std::path::Path) -> Result<Vec<Project>> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("cannot read config {}", path.display()))?;
    parse(&text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_projects_and_rejects_empty() {
        let p = parse("[[projects]]\nname = \"a\"\npath = \"/x\"\n").unwrap();
        assert_eq!((p[0].name.as_str(), p[0].path.to_str()), ("a", Some("/x")));
        assert!(parse("projects = []").is_err());
        assert!(parse("[[projects]]\nname = \"a\"").is_err());
    }

    #[test]
    fn parses_optional_safety_and_model_fields() {
        let p = parse("[[projects]]\nname=\"a\"\npath=\"/x\"\nallow_all=true\nmodel=\"m\"\n[projects.agent_models]\nexplore=\"h\"\n").unwrap();
        assert!(p[0].allow_all);
        assert_eq!(p[0].model.as_deref(), Some("m"));
        assert_eq!(p[0].agent_models["explore"], "h");
        let p = parse("[[projects]]\nname=\"a\"\npath=\"/x\"").unwrap();
        assert!(!p[0].allow_all && p[0].model.is_none() && p[0].agent_models.is_empty());
    }

    #[test]
    fn rejects_empty_and_malformed_toml() {
        assert!(parse("").is_err());
        assert!(parse("[[projects").is_err());
    }
}
