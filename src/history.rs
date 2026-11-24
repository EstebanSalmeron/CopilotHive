//! Past sessions read from Copilot's `session-state/<id>/{workspace.yaml,events.jsonl}`.
//! Internal, undocumented format (see docs/spike-acp.md); keep it isolated here.
use anyhow::{Context, Result};
use serde_json::Value;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

pub struct SessionInfo {
    pub id: String,
    pub name: String,
    pub updated_at: String,
    pub events: PathBuf,
}

pub enum Entry {
    User(String),
    Agent(String),
    Tool {
        id: String,
        title: String,
        ok: Option<bool>,
    },
}

pub fn root() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".copilot/session-state"))
}

/// Sessions whose `cwd` is `cwd`, newest first. Sessions without events are skipped.
pub fn list(root: &Path, cwd: &str) -> Vec<SessionInfo> {
    let mut out: Vec<SessionInfo> = std::fs::read_dir(root)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|d| {
            let dir = d.path();
            let events = dir.join("events.jsonl");
            if !events.is_file() {
                return None;
            }
            let yaml = std::fs::read_to_string(dir.join("workspace.yaml")).ok()?;
            let field = |k: &str| {
                yaml.lines()
                    .find_map(|l| l.strip_prefix(k)?.strip_prefix(':'))
                    .map(|v| v.trim().trim_matches(['"', '\'']).to_string())
            };
            (field("cwd")? == cwd).then(|| SessionInfo {
                id: field("id").unwrap_or_else(|| d.file_name().to_string_lossy().into()),
                name: field("name")
                    .or_else(|| field("summary"))
                    .filter(|n| !n.is_empty())
                    .unwrap_or_else(|| "(sin título)".into()),
                updated_at: field("updated_at").unwrap_or_default(),
                events,
            })
        })
        .collect();
    out.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    out
}

/// Root-agent conversation: user prompts, assistant text and tool calls (subagent events are skipped).
pub fn load(events: &Path) -> Result<Vec<Entry>> {
    let f = std::fs::File::open(events).with_context(|| format!("open {}", events.display()))?;
    let mut out: Vec<Entry> = vec![];
    for line in BufReader::new(f).lines() {
        let line = line?;
        // Cheap pre-filter: snapshot events can be megabytes and are not needed.
        let wanted = [
            "\"type\":\"user.message\"",
            "\"type\":\"assistant.message\"",
            "\"type\":\"tool.execution_",
            "\"type\": \"user.message\"",
            "\"type\": \"assistant.message\"",
            "\"type\": \"tool.execution_",
        ]
        .iter()
        .any(|p| line.contains(p));
        if !wanted {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if v.get("agentId").is_some() {
            continue;
        }
        let d = &v["data"];
        let text = d["content"]
            .as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty());
        match v["type"].as_str() {
            Some("user.message") => out.extend(text.map(|s| Entry::User(s.into()))),
            Some("assistant.message") => out.extend(text.map(|s| Entry::Agent(s.into()))),
            Some("tool.execution_start") => {
                let id = d["toolCallId"].as_str().unwrap_or_default().to_string();
                let title = d["toolTitle"]
                    .as_str()
                    .or(d["toolName"].as_str())
                    .unwrap_or("tool")
                    .to_string();
                out.push(Entry::Tool {
                    id,
                    title,
                    ok: None,
                });
            }
            Some("tool.execution_complete") => {
                let cid = d["toolCallId"].as_str();
                let done = out.iter_mut().rev().find_map(|e| match e {
                    Entry::Tool { id, ok, .. } if Some(id.as_str()) == cid => Some(ok),
                    _ => None,
                });
                if let Some(ok) = done {
                    *ok = d["success"].as_bool();
                }
            }
            _ => {}
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn lists_by_cwd_and_loads_root_conversation() {
        let root = std::env::temp_dir().join(format!("hive-hist-{}", std::process::id()));
        let mk = |id: &str, cwd: &str, upd: &str, ev: &str| {
            let d = root.join(id);
            fs::create_dir_all(&d).unwrap();
            fs::write(
                d.join("workspace.yaml"),
                format!("id: {id}\ncwd: {cwd}\nname: S {id}\nupdated_at: {upd}\n"),
            )
            .unwrap();
            fs::write(d.join("events.jsonl"), ev).unwrap();
        };
        let ev = concat!(
            r#"{"type":"user.message","data":{"content":"hola"}}"#,
            "\n",
            r#"{"type":"assistant.message","data":{"content":""}}"#,
            "\n",
            r#"{"type":"tool.execution_start","data":{"toolCallId":"1","toolTitle":"ls"}}"#,
            "\n",
            r#"{"type":"tool.execution_start","agentId":"x","data":{"toolCallId":"2","toolName":"sub"}}"#,
            "\n",
            r#"{"type":"tool.execution_complete","data":{"toolCallId":"1","success":true}}"#,
            "\n",
            r#"{"type":"assistant.message","data":{"content":"listo"}}"#,
            "\n",
        );
        mk("a", "/p", "2026-01-01", ev);
        mk("b", "/p", "2026-02-01", "");
        mk("c", "/other", "2026-03-01", ev);
        let l = list(&root, "/p");
        assert_eq!(
            l.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
            ["S b", "S a"]
        );
        let e = load(&l[1].events).unwrap();
        assert!(matches!(&e[..], [
            Entry::User(u),
            Entry::Tool { ok: Some(true), .. },
            Entry::Agent(a),
        ] if u == "hola" && a == "listo"));
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn loads_when_type_is_not_first_key_and_strips_single_quotes() {
        let root = std::env::temp_dir().join(format!("hive-hist-ord-{}", std::process::id()));
        let d = root.join("s1");
        fs::create_dir_all(&d).unwrap();
        fs::write(
            d.join("workspace.yaml"),
            "id: 's1'\ncwd: '/p'\nname: 'Mi sesion'\nupdated_at: '2026-01-01'\n",
        )
        .unwrap();
        let ev = concat!(
            r#"{"id":"1","type":"user.message","data":{"content":"hola"}}"#,
            "\n",
            r#"{"id": "2", "type": "assistant.message", "data": {"content": "ok"}}"#,
            "\n",
        );
        fs::write(d.join("events.jsonl"), ev).unwrap();
        let l = list(&root, "/p");
        assert_eq!(l.len(), 1);
        assert_eq!(l[0].name, "Mi sesion");
        let e = load(&l[0].events).unwrap();
        assert!(matches!(&e[..], [
            Entry::User(u),
            Entry::Agent(a),
        ] if u == "hola" && a == "ok"));
        fs::remove_dir_all(&root).unwrap();
    }
}
