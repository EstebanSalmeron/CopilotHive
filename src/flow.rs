//! Animated flowchart of every project and its agents (project → main → subagents).
use crate::events::{Agent, Status, Tree};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};

const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const EDGE: usize = 8;

pub struct ProjectView<'a> {
    pub name: &'a str,
    pub status: &'a str,
    pub waiting_perm: bool,
    pub tree: &'a Tree,
}

/// Edge of fixed width; while `active` a dot travels along it, driven by `tick`.
fn edge(active: bool, tick: usize, color: Color) -> Span<'static> {
    let mut s: Vec<char> = vec!['─'; EDGE - 1];
    if active {
        s[tick % (EDGE - 1)] = '●';
    }
    s.push('▶');
    Span::styled(s.into_iter().collect::<String>(), Style::new().fg(color))
}

fn agent_box(a: &Agent, tick: usize) -> (Span<'static>, Color, bool) {
    let running = a.status == Status::Running;
    let (icon, color) = if running {
        (SPINNER[tick % SPINNER.len()], Color::Yellow)
    } else {
        ("✔", Color::Green)
    };
    let dur = a
        .duration_ms
        .map_or(String::new(), |ms| format!(" {:.1}s", ms as f64 / 1000.0));
    let text = format!("[{icon} {} · {} tools{dur}]", a.name, a.tool_calls);
    (Span::styled(text, Style::new().fg(color)), color, running)
}

pub fn render(projects: &[ProjectView], tick: usize) -> Vec<Line<'static>> {
    let mut out = vec![];
    for p in projects {
        let head_color = if p.waiting_perm {
            Color::Red
        } else {
            Color::Cyan
        };
        let mut head = vec![Span::styled(
            format!("┏ {} ", p.name),
            Style::new().fg(head_color),
        )];
        let main = p.tree.agents.first().filter(|a| a.id.is_none());
        let subs: Vec<&Agent> = p.tree.agents.iter().filter(|a| a.id.is_some()).collect();
        let pad = p.name.chars().count() + 3;
        match main {
            Some(m) => {
                let (b, color, running) = agent_box(m, tick);
                head.push(edge(running, tick, color));
                head.push(b);
            }
            None => head.push(Span::styled("(sin actividad)", Style::new().fg(Color::DarkGray))),
        }
        if p.waiting_perm {
            head.push(Span::styled(
                "  ⚠ permiso pendiente",
                Style::new().fg(Color::Red),
            ));
        }
        out.push(Line::from(head));
        // Subagents hang off the main box, aligned under it.
        let indent = " ".repeat(pad + EDGE);
        for (i, a) in subs.iter().enumerate() {
            let (b, color, running) = agent_box(a, tick);
            let branch = if i + 1 == subs.len() { "└" } else { "├" };
            out.push(Line::from(vec![
                Span::raw(indent.clone()),
                Span::styled(branch.to_string(), Style::new().fg(color)),
                edge(running, tick, color),
                b,
            ]));
        }
        out.push(Line::from(Span::styled(
            format!("┗ {}", p.status),
            Style::new().fg(Color::DarkGray),
        )));
        out.push(Line::raw(""));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dot_moves_only_while_running() {
        let text = |s: Span| s.content.to_string();
        assert_eq!(text(edge(true, 0, Color::Red)), "●──────▶");
        assert_eq!(text(edge(true, 2, Color::Red)), "──●────▶");
        assert_eq!(text(edge(false, 2, Color::Red)), "───────▶");
    }

    #[test]
    fn renders_subagents_under_project() {
        let mut t = Tree::default();
        for l in [
            r#"{"type":"assistant.turn_start","data":{}}"#,
            r#"{"type":"subagent.started","agentId":"a1","data":{"agentType":"explore"}}"#,
        ] {
            t.apply(l);
        }
        let v = [ProjectView {
            name: "p",
            status: "ok",
            waiting_perm: false,
            tree: &t,
        }];
        // project line + 1 subagent + status + blank
        assert_eq!(render(&v, 0).len(), 4);
    }
}
