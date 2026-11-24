//! Subagent discovery (custom agent files + built-ins) and mandatory model assignment.
//!
//! The mandatory-model mechanism is Copilot's own: `subagents.agents.<name>` in the user
//! `~/.copilot/settings.json` with `modelPolicy: "required"` (what `/subagents` writes).
//! A `model:` in an agent's frontmatter is only a default, so we read it but never edit it.
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub const BUILTIN: [&str; 6] = [
    "explore",
    "task",
    "general-purpose",
    "code-review",
    "research",
    "rubber-duck",
];

#[derive(Debug, PartialEq, Clone)]
pub struct AgentDef {
    pub name: String,
    pub description: String,
    /// `model:` from the frontmatter (default only; settings.json overrides it).
    pub model: Option<String>,
    pub builtin: bool,
}

/// `key: value` pairs of the leading `---` block (no nested YAML; quotes stripped).
pub fn parse_frontmatter(text: &str) -> HashMap<String, String> {
    let mut lines = text.lines();
    let mut out = HashMap::new();
    if lines.next().map(str::trim) != Some("---") {
        return out;
    }
    for l in lines {
        if l.trim() == "---" {
            break;
        }
        if let Some((k, v)) = l.split_once(':')
            && !k.starts_with([' ', '\t'])
        {
            let v = v.trim().trim_matches(['"', '\'']);
            out.insert(k.trim().to_string(), v.to_string());
        }
    }
    out
}

fn read_dir_defs(dir: &Path, out: &mut Vec<AgentDef>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    let mut files: Vec<_> = rd.flatten().map(|e| e.path()).collect();
    files.sort();
    for p in files {
        let Some(file) = p.file_name().and_then(|f| f.to_str()) else {
            continue;
        };
        let Some(stem) = file.strip_suffix(".md") else {
            continue;
        };
        let Ok(text) = std::fs::read_to_string(&p) else {
            continue;
        };
        let fm = parse_frontmatter(&text);
        let name = fm
            .get("name")
            .cloned()
            .unwrap_or_else(|| stem.trim_end_matches(".agent").to_string());
        if out.iter().any(|d| d.name == name) {
            continue; // project agents win over user ones
        }
        out.push(AgentDef {
            description: fm.get("description").cloned().unwrap_or_default(),
            model: fm.get("model").cloned(),
            name,
            builtin: false,
        });
    }
}

/// Custom agents of the project and the user, then the built-ins not shadowed by them.
pub fn discover(project: &Path, home: Option<&Path>) -> Vec<AgentDef> {
    let mut out = vec![];
    read_dir_defs(&project.join(".github/agents"), &mut out);
    if let Some(h) = home {
        read_dir_defs(&h.join(".copilot/agents"), &mut out);
    }
    for b in BUILTIN {
        if !out.iter().any(|d| d.name == b) {
            out.push(AgentDef {
                name: b.into(),
                description: "built-in".into(),
                model: None,
                builtin: true,
            });
        }
    }
    out
}

pub fn settings_path() -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var_os("HOME")?).join(".copilot/settings.json"))
}

/// Models assigned in `subagents.agents.<name>.model`; a missing file means none.
pub fn load_models(path: &Path) -> HashMap<String, String> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return HashMap::new();
    };
    let v: Value = serde_json::from_str(&text).unwrap_or_default();
    v["subagents"]["agents"]
        .as_object()
        .into_iter()
        .flatten()
        .filter_map(|(k, e)| Some((k.clone(), e["model"].as_str()?.to_string())))
        .collect()
}

/// Sets `subagents.agents.<agent>` to `{model, modelPolicy: "required"}`, keeping every
/// other key (and their order). Refuses to touch a file that is not plain JSON.
pub fn set_model(path: &Path, agent: &str, model: &str) -> Result<()> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => "{}".into(),
        Err(e) => return Err(e).with_context(|| format!("cannot read {}", path.display())),
    };
    let mut root: Value = serde_json::from_str(&text)
        .with_context(|| format!("{} is not plain JSON; edit it by hand", path.display()))?;
    let Some(obj) = root.as_object_mut() else {
        bail!("{} is not a JSON object", path.display());
    };
    let agents = obj
        .entry("subagents")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .and_then(|s| {
            s.entry("agents")
                .or_insert_with(|| json!({}))
                .as_object_mut()
        })
        .context("`subagents.agents` is not an object")?;
    let entry = agents
        .entry(agent)
        .or_insert_with(|| json!({"effortLevel":"inherit","contextTier":"inherit"}));
    let Some(e) = entry.as_object_mut() else {
        bail!("`subagents.agents.{agent}` is not an object");
    };
    e.insert("model".into(), Value::String(model.into()));
    e.insert("modelPolicy".into(), Value::String("required".into()));
    let mut out = serde_json::to_string_pretty(&root)?;
    if text.ends_with('\n') {
        out.push('\n');
    }
    if let Some(dir) = path.parent()
        && !dir.as_os_str().is_empty()
    {
        std::fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
    }
    // Write beside the target and rename so a crash never leaves a half-written file.
    let tmp = path.with_extension(format!("json.cockpit-tmp-{}", std::process::id()));
    std::fs::write(&tmp, out)?;
    if let Ok(meta) = std::fs::metadata(path) {
        std::fs::set_permissions(&tmp, meta.permissions())?;
    }
    std::fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_frontmatter() {
        let fm = parse_frontmatter(
            "---\nname: dev\ndescription: \"Does: things\"\nmodel: 'x-1'\ntools:\n  - a\n---\nbody: no",
        );
        assert_eq!(fm["name"], "dev");
        assert_eq!(fm["description"], "Does: things");
        assert_eq!(fm["model"], "x-1");
        assert!(!fm.contains_key("a") && !fm.contains_key("body"));
        assert!(parse_frontmatter("no frontmatter").is_empty());
    }

    #[test]
    fn discovers_custom_before_builtin_and_dedups() {
        let dir = std::env::temp_dir().join(format!("cockpit-disc-{}", std::process::id()));
        let ag = dir.join(".github/agents");
        std::fs::create_dir_all(&ag).unwrap();
        std::fs::write(
            ag.join("dev.agent.md"),
            "---\ndescription: D\nmodel: m1\n---\n",
        )
        .unwrap();
        std::fs::write(
            ag.join("explore.md"),
            "---\nname: explore\ndescription: mine\n---\n",
        )
        .unwrap();
        std::fs::write(ag.join("notes.txt"), "x").unwrap();
        let v = discover(&dir, None);
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(
            (v[0].name.as_str(), v[0].model.as_deref()),
            ("dev", Some("m1"))
        );
        assert_eq!(v.iter().filter(|d| d.name == "explore").count(), 1);
        assert!(!v.iter().find(|d| d.name == "explore").unwrap().builtin);
        assert!(v.iter().any(|d| d.name == "rubber-duck" && d.builtin));
    }

    #[test]
    fn set_model_preserves_other_settings_and_order() {
        let path =
            std::env::temp_dir().join(format!("cockpit-settings-{}.json", std::process::id()));
        std::fs::write(
            &path,
            "{\n  \"theme\": \"dark\",\n  \"subagents\": {\n    \"agents\": {\n      \"zed\": {\n        \"model\": \"a\",\n        \"effortLevel\": \"high\",\n        \"modelPolicy\": \"preferred\"\n      }\n    }\n  },\n  \"allowedUrls\": [\"x\"]\n}\n",
        )
        .unwrap();
        set_model(&path, "zed", "b").unwrap();
        set_model(&path, "explore", "c").unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        let v: Value = serde_json::from_str(&text).unwrap();
        let z = &v["subagents"]["agents"]["zed"];
        assert_eq!(
            (
                z["model"].as_str(),
                z["effortLevel"].as_str(),
                z["modelPolicy"].as_str()
            ),
            (Some("b"), Some("high"), Some("required"))
        );
        assert_eq!(
            v["subagents"]["agents"]["explore"]["effortLevel"],
            "inherit"
        );
        assert_eq!(v["allowedUrls"][0], "x");
        assert!(text.find("theme") < text.find("subagents") && text.ends_with("}\n"));
        assert_eq!(load_models(&path)["explore"], "c");
        std::fs::write(&path, "{ // comment\n}").unwrap();
        assert!(set_model(&path, "a", "b").is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{ // comment\n}");
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn set_model_creates_missing_parent_directory() {
        let dir = std::env::temp_dir().join(format!("cockpit-newdir-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("nested/settings.json");
        set_model(&path, "explore", "m1").unwrap();
        assert_eq!(load_models(&path)["explore"], "m1");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
