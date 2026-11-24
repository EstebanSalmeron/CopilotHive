//! Full-screen dashboard: every subagent available in the project, with assigned vs used model.
use super::Tab;
use super::settings::tr;
use super::theme::*;
use super::widgets::{block, fmt_dur, trunc};
use crate::events::{KindStats, Status};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Cell, Paragraph, Row, Table, TableState};
use std::time::Duration;

#[derive(Clone, Copy, PartialEq)]
enum State {
    Running,
    Done,
    Cancelled,
    Available,
}

pub struct DashRow {
    pub name: String,
    state: State,
    description: String,
    source: &'static str,
    /// Frontmatter default.
    default_model: Option<String>,
    assigned: Option<String>,
    used: Option<String>,
    stats: KindStats,
}

impl DashRow {
    /// Mandatory model differs from the one actually observed.
    fn mismatch(&self) -> bool {
        matches!((&self.assigned, &self.used), (Some(a), Some(u)) if a != u)
    }
}

/// Union of discovered definitions and observed agent types, running first.
pub fn rows(t: &Tab) -> Vec<DashRow> {
    let mut stats = t.tree.stats();
    let mut out: Vec<DashRow> = vec![];
    let mut push = |name: &str, desc: &str, source, default_model: Option<String>| {
        let i = stats.iter().position(|k| k.kind == name);
        let s = i.map(|i| stats.remove(i)).unwrap_or_default();
        out.push(row(t, name, desc, source, default_model, s));
    };
    push(
        "main",
        tr("Agente principal de la sesión", "Main session agent"),
        tr("sesión", "session"),
        None,
    );
    for d in t.defs.iter() {
        let source = if d.builtin { "built-in" } else { "custom" };
        push(&d.name, &d.description, source, d.model.clone());
    }
    for k in std::mem::take(&mut stats) {
        out.push(row(
            t,
            &k.kind.clone(),
            "",
            tr("observado", "observed"),
            None,
            k,
        ));
    }
    out.sort_by_key(|r| {
        (
            r.name != "main",
            r.state != State::Running,
            r.stats.runs == 0,
            r.name.clone(),
        )
    });
    out
}

fn row(
    t: &Tab,
    name: &str,
    desc: &str,
    source: &'static str,
    default_model: Option<String>,
    stats: KindStats,
) -> DashRow {
    let state = match (stats.running, stats.last_status) {
        (n, _) if n > 0 => State::Running,
        (_, Some(Status::Cancelled)) => State::Cancelled,
        (_, Some(Status::Completed)) => State::Done,
        _ => State::Available,
    };
    DashRow {
        name: name.into(),
        state,
        description: desc.into(),
        source,
        default_model,
        assigned: t.assigned.get(name).cloned(),
        used: stats.last_model.clone(),
        stats: KindStats {
            kind: name.into(),
            ..stats
        },
    }
}

fn tok(k: &KindStats) -> String {
    let t = k.tokens;
    format!(
        "{}/{}/{}",
        human(t.input),
        human(t.output),
        human(t.cache_read + t.cache_write)
    )
}

pub fn draw(t: &Tab, f: &mut Frame, area: Rect, tick: u64) {
    let rows = rows(t);
    let sel = rows
        .iter()
        .position(|r| Some(&r.name) == t.dash_sel.as_ref())
        .unwrap_or(0);
    let [table_a, detail_a, foot_a] = Layout::vertical([
        Constraint::Min(5),
        Constraint::Length(5),
        Constraint::Length(1),
    ])
    .areas(area);

    let header = Row::new(
        [
            tr("Agente", "Agent"),
            tr("Estado", "Status"),
            tr("Asignado", "Assigned"),
            tr("Usado", "Used"),
            tr("Ej.", "Runs"),
            "Tools",
            "Tokens in/out/cache",
            tr("Créditos", "Credits"),
            tr("Tiempo", "Time"),
        ]
        .map(|h| Cell::from(h).style(bold(c_accent()))),
    );
    let body: Vec<Row> = rows
        .iter()
        .map(|r| {
            let (icon, label, c) = match r.state {
                State::Running => (spinner(tick), tr("ejecutando", "running"), c_running()),
                State::Done => ("✔", tr("completado", "done"), c_ok()),
                State::Cancelled => ("✖", tr("cancelado", "cancelled"), c_error()),
                State::Available => ("○", tr("disponible", "available"), c_dim()),
            };
            let assigned = r
                .assigned
                .as_ref()
                .map_or(Span::styled("—", fg(c_dim())), |a| {
                    Span::styled(
                        format!("🔒 {a}"),
                        fg(if r.mismatch() { c_attention() } else { c_ok() }),
                    )
                });
            let used = r.used.clone().unwrap_or_else(|| "—".into());
            let used_c = if r.mismatch() { c_attention() } else { c_fg() };
            Row::new(vec![
                Cell::from(Span::styled(format!("{icon} {}", r.name), fg(c))),
                Cell::from(Span::styled(label, fg(c))),
                Cell::from(assigned),
                Cell::from(Span::styled(
                    if r.mismatch() {
                        format!("⚠ {used}")
                    } else {
                        used
                    },
                    fg(used_c),
                )),
                Cell::from(r.stats.runs.to_string()),
                Cell::from(r.stats.tool_calls.to_string()),
                Cell::from(tok(&r.stats)),
                Cell::from(Span::styled(cost(r.stats.tokens.credits()), fg(c_orange()))),
                Cell::from(fmt_dur(r.stats.time)),
            ])
        })
        .collect();
    let widths = [
        Constraint::Length(20),
        Constraint::Length(11),
        Constraint::Min(18),
        Constraint::Min(18),
        Constraint::Length(4),
        Constraint::Length(6),
        Constraint::Length(20),
        Constraint::Length(18),
        Constraint::Length(8),
    ];
    let title = Line::styled(
        tr_fmt!(
            " agentes del proyecto ({}) · ↑↓ elegir · m modelo 🔒 · d cambiar vista ",
            " project agents ({}) · ↑↓ select · m model 🔒 · d switch view ",
            rows.len()
        ),
        bold(c_accent()),
    );
    let table = Table::new(body, widths)
        .header(header)
        .row_highlight_style(
            Style::new()
                .bg(c_surface())
                .add_modifier(ratatui::style::Modifier::BOLD),
        )
        .highlight_symbol("▶ ")
        .block(block(title, c_accent()));
    let mut state = TableState::default().with_selected(Some(sel));
    f.render_stateful_widget(table, table_a, &mut state);

    let w = detail_a.width.saturating_sub(4) as usize;
    let detail: Vec<Line> = match rows.get(sel) {
        Some(r) => {
            let model = |m: &Option<String>| m.clone().unwrap_or_else(|| "—".into());
            vec![
                Line::from(vec![
                    Span::styled(r.name.clone(), bold(c_fg())),
                    Span::styled(format!("  [{}]", r.source), fg(c_dim())),
                ]),
                Line::styled(trunc(&r.description, w), fg(c_dim())),
                Line::styled(
                    trunc(
                        &tr_fmt!(
                            "default (frontmatter): {} · asignado 🔒: {} · usado: {}",
                            "default (frontmatter): {} · assigned 🔒: {} · used: {}",
                            model(&r.default_model),
                            model(&r.assigned),
                            model(&r.used)
                        ),
                        w,
                    ),
                    fg(if r.mismatch() { c_attention() } else { c_fg() }),
                ),
            ]
        }
        None => vec![],
    };
    f.render_widget(
        Paragraph::new(detail).block(block(tr(" detalle ", " detail "), c_border())),
        detail_a,
    );

    let all = t.tree.total_tokens();
    let (runs, tools, time) = rows.iter().fold((0, 0, Duration::ZERO), |(r, c, d), x| {
        (r + x.stats.runs, c + x.stats.tool_calls, d + x.stats.time)
    });
    f.render_widget(
        Paragraph::new(Line::styled(
            format!("{} · {}", tr_fmt!(
                " Total: {} agentes · {runs} ejecuciones · {tools} tools · in {} · out {} · cache {} · {} · {}", " Total: {} agents · {runs} runs · {tools} tools · in {} · out {} · cache {} · {} · {}",
                rows.len(),
                human(all.input),
                human(all.output),
                human(all.cache_read + all.cache_write),
                cost(all.credits()),
                fmt_dur(time)
            ), tr("Enter: usar subagente", "Enter: use subagent")),
            bold(c_fg()),
        )),
        foot_a,
    );
}

#[cfg(test)]
mod tests {
    use super::super::tests::tab;
    use super::*;

    fn t() -> Tab {
        let mut t = tab("p");
        for l in [
            r#"{"type":"session.model_change","data":{"newModel":"main-m"}}"#,
            r#"{"type":"subagent.started","agentId":"a","data":{"agentType":"explore","model":"haiku"}}"#,
            r#"{"type":"session.usage_record","agentId":"a","data":{"usage":{"model":"haiku","copilotUsage":{"tokenDetails":[{"tokenType":"input","tokenCount":1000}]}}}}"#,
            r#"{"type":"subagent.completed","agentId":"a","data":{"model":"haiku","durationMs":2000,"totalToolCalls":3}}"#,
            r#"{"type":"subagent.started","agentId":"b","data":{"agentType":"explore","model":"haiku"}}"#,
            r#"{"type":"subagent.started","agentId":"c","data":{"agentType":"custom-x","model":"m"}}"#,
        ] {
            t.tree.apply(l);
        }
        t.defs = vec![
            crate::agents::AgentDef {
                name: "explore".into(),
                description: "d".into(),
                model: None,
                builtin: true,
            },
            crate::agents::AgentDef {
                name: "task".into(),
                description: "d".into(),
                model: None,
                builtin: true,
            },
        ];
        t.assigned.insert("explore".into(), "sonnet".into());
        t
    }

    #[test]
    fn rows_group_by_type_and_flag_model_mismatch() {
        let r = rows(&t());
        assert_eq!(r[0].name, "main");
        let e = r.iter().find(|r| r.name == "explore").unwrap();
        assert_eq!((e.stats.runs, e.stats.tokens.input), (2, 1000));
        assert!(e.mismatch());
        // observed-only type is listed; unused built-in is "available" and sorted last
        assert!(
            r.iter()
                .any(|r| r.name == "custom-x" && r.state == State::Running)
        );
        assert!(r.last().is_some_and(|r| r.state == State::Available));
    }
}
