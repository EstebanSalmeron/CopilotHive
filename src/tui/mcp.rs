//! Per-project MCP servers: discovery, persisted disabled list and the toggle menu.
//! Copilot cannot toggle MCP servers in a live session, so applying restarts the ACP process
//! with `--disable-mcp-server <name>` and reloads the session.
use super::settings::tr;
use super::theme::*;
use super::widgets::{block, centered};
use ratatui::Frame;
use ratatui::crossterm::event::KeyCode;
use ratatui::layout::Rect;
use ratatui::text::Line;
use ratatui::widgets::{Clear, Padding, Paragraph};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub(super) struct Server {
    pub name: String,
    pub kind: &'static str,
}

fn state_path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join(".config/copilot-hive/mcp.toml"))
}

/// Servers declared in the user config and in the project's `.mcp.json` / `.github/mcp.json`.
pub(super) fn discover(cwd: &Path, home: Option<&Path>) -> Vec<Server> {
    let files = home
        .map(|h| h.join(".copilot/mcp-config.json"))
        .into_iter()
        .chain([cwd.join(".mcp.json"), cwd.join(".github/mcp.json")]);
    let mut out: Vec<Server> = vec![];
    for f in files {
        let Ok(v) = std::fs::read_to_string(f)
            .map_err(drop)
            .and_then(|t| serde_json::from_str::<Value>(&t).map_err(drop))
        else {
            continue;
        };
        for key in ["mcpServers", "servers"] {
            let Some(m) = v[key].as_object() else {
                continue;
            };
            for (name, s) in m {
                if out.iter().all(|o| &o.name != name) {
                    let kind = if s["url"].is_string() {
                        "http"
                    } else {
                        "local"
                    };
                    out.push(Server {
                        name: name.clone(),
                        kind,
                    });
                }
            }
        }
    }
    out
}

fn load_all() -> BTreeMap<String, Vec<String>> {
    state_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|t| toml::from_str(&t).ok())
        .unwrap_or_default()
}

/// Servers the user disabled for the project at `cwd`.
pub(super) fn disabled(cwd: &str) -> Vec<String> {
    load_all().remove(cwd).unwrap_or_default()
}

fn save(cwd: &str, names: Vec<String>) -> std::io::Result<()> {
    let Some(p) = state_path().filter(|_| !cfg!(test)) else {
        return Ok(());
    };
    let mut all = load_all();
    if names.is_empty() {
        all.remove(cwd);
    } else {
        all.insert(cwd.to_string(), names);
    }
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d)?;
    }
    let text = toml::to_string(&all).map_err(std::io::Error::other)?;
    std::fs::write(p, text)
}

pub(super) enum Action {
    None,
    Close,
    /// Disabled list changed and was saved: restart the session.
    Apply,
}

pub(super) struct Menu {
    cwd: String,
    project: String,
    servers: Vec<Server>,
    off: Vec<String>,
    initial: Vec<String>,
    sel: usize,
    error: Option<String>,
}

impl Menu {
    pub(super) fn new(project: &str, cwd: &str) -> Self {
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let off = disabled(cwd);
        Menu {
            cwd: cwd.into(),
            project: project.into(),
            servers: discover(Path::new(cwd), home.as_deref()),
            initial: off.clone(),
            off,
            sel: 0,
            error: None,
        }
    }

    pub(super) fn on_key(&mut self, code: KeyCode) -> Action {
        match code {
            KeyCode::Esc | KeyCode::Char('q' | 'M') => return Action::Close,
            KeyCode::Up | KeyCode::Char('k') => self.sel = self.sel.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => {
                self.sel = (self.sel + 1).min(self.servers.len().saturating_sub(1));
            }
            KeyCode::Char(' ') => {
                if let Some(s) = self.servers.get(self.sel) {
                    match self.off.iter().position(|n| *n == s.name) {
                        Some(i) => drop(self.off.remove(i)),
                        None => self.off.push(s.name.clone()),
                    }
                }
            }
            KeyCode::Enter => {
                let (mut a, mut b) = (self.off.clone(), self.initial.clone());
                a.sort();
                b.sort();
                if a == b {
                    return Action::Close;
                }
                match save(&self.cwd, self.off.clone()) {
                    Ok(()) => return Action::Apply,
                    Err(e) => self.error = Some(e.to_string()),
                }
            }
            _ => {}
        }
        Action::None
    }

    pub(super) fn draw(&self, f: &mut Frame, area: Rect) {
        let mut lines: Vec<Line<'static>> = if self.servers.is_empty() {
            vec![Line::styled(
                tr(
                    "(sin servidores MCP configurados)",
                    "(no MCP servers configured)",
                ),
                fg(c_dim()),
            )]
        } else {
            self.servers
                .iter()
                .enumerate()
                .map(|(i, s)| {
                    let on = !self.off.contains(&s.name);
                    let text = format!("[{}] {:<28}{}", if on { "x" } else { " " }, s.name, s.kind);
                    if i == self.sel {
                        Line::styled(format!("▶ {text}"), bold(c_bg()).bg(c_accent()))
                    } else {
                        let c = if on { c_fg() } else { c_dim() };
                        Line::styled(format!("  {text}"), fg(c))
                    }
                })
                .collect()
        };
        lines.push(Line::default());
        lines.push(Line::styled(
            tr(
                "↑↓ elegir · Espacio alternar · Enter aplicar (reinicia la sesión) · Esc cancelar",
                "↑↓ select · Space toggle · Enter apply (restarts the session) · Esc cancel",
            ),
            fg(c_dim()),
        ));
        if let Some(e) = &self.error {
            lines.push(Line::styled(format!("✖ {e}"), fg(c_error())));
        }
        let r = centered(area, 78, lines.len() as u16 + 2);
        f.render_widget(Clear, r);
        f.render_widget(
            Paragraph::new(lines).block(
                block(
                    Line::styled(format!(" MCP · {} ", self.project), bold(c_accent())),
                    c_accent(),
                )
                .padding(Padding::horizontal(1)),
            ),
            r,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovers_user_and_project_servers_without_duplicates() {
        let d = std::env::temp_dir().join(format!("hive-mcp-{}", std::process::id()));
        let (home, cwd) = (d.join("h"), d.join("p"));
        std::fs::create_dir_all(home.join(".copilot")).unwrap();
        std::fs::create_dir_all(&cwd).unwrap();
        std::fs::write(
            home.join(".copilot/mcp-config.json"),
            r#"{"mcpServers":{"a":{"command":"x"},"b":{"url":"http://x"}}}"#,
        )
        .unwrap();
        std::fs::write(cwd.join(".mcp.json"), r#"{"mcpServers":{"a":{},"c":{}}}"#).unwrap();
        let s = discover(&cwd, Some(&home));
        let got: Vec<_> = s.iter().map(|s| (s.name.as_str(), s.kind)).collect();
        assert_eq!(got, [("a", "local"), ("b", "http"), ("c", "local")]);
        std::fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn space_toggles_and_enter_applies_only_on_change() {
        let mut m = Menu {
            cwd: "/x".into(),
            project: "p".into(),
            servers: vec![Server {
                name: "a".into(),
                kind: "local",
            }],
            off: vec![],
            initial: vec![],
            sel: 0,
            error: None,
        };
        assert!(matches!(m.on_key(KeyCode::Enter), Action::Close));
        m.on_key(KeyCode::Char(' '));
        assert_eq!(m.off, ["a"]);
        assert!(matches!(m.on_key(KeyCode::Enter), Action::Apply));
        m.on_key(KeyCode::Char(' '));
        assert!(m.off.is_empty());
    }
}
