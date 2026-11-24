//! Read-only browser of past sessions of the current project (`H`).
use super::markdown::wrap;
use super::settings::{self, tr};
use super::theme::*;
use super::widgets::{block, build_lines, hints, trunc};
use super::{Cache, Item, Scroll, ToolState};
use crate::history::{Entry, SessionInfo};
use ratatui::Frame;
use ratatui::crossterm::event::KeyCode;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_REQ: AtomicU64 = AtomicU64::new(1);

fn next_req() -> u64 {
    NEXT_REQ.fetch_add(1, Ordering::Relaxed)
}

struct Viewer {
    title: String,
    items: Vec<Item>,
    scroll: Scroll,
    cache: Option<Cache>,
}

pub(super) enum Loaded {
    List(u64, Vec<SessionInfo>),
    Session(u64, String, Result<Vec<Entry>, String>),
}

pub(super) enum Action {
    None,
    Close,
    /// Session id and title.
    Resume(String, String),
    /// Load `events.jsonl` in a blocking task: `(req_id, title, events_path)`.
    Load(u64, String, PathBuf),
}

pub(super) struct History {
    project: String,
    sessions: Vec<SessionInfo>,
    sel: usize,
    viewer: Option<Viewer>,
    loading_list: Option<u64>,
    loading_session: Option<u64>,
    /// Last load error, shown in the list.
    error: Option<String>,
}

impl History {
    pub(super) fn open(project: &str) -> (Self, u64) {
        let req = next_req();
        (
            History {
                project: project.into(),
                sessions: vec![],
                sel: 0,
                viewer: None,
                loading_list: Some(req),
                loading_session: None,
                error: None,
            },
            req,
        )
    }

    pub(super) fn on_loaded(&mut self, ev: Loaded) {
        match ev {
            Loaded::List(req, sessions) if self.loading_list == Some(req) => {
                self.loading_list = None;
                self.sessions = sessions;
                self.sel = 0;
            }
            Loaded::Session(req, title, res) if self.loading_session == Some(req) => {
                self.loading_session = None;
                match res {
                    Ok(entries) => {
                        let items = entries
                            .into_iter()
                            .map(|e| match e {
                                Entry::User(s) => Item::User(s),
                                Entry::Agent(s) => Item::Agent(s),
                                Entry::Tool { id, title, ok } => Item::Tool {
                                    id,
                                    title,
                                    kind: String::new(),
                                    state: match ok {
                                        Some(true) => ToolState::Ok,
                                        Some(false) => ToolState::Failed,
                                        None => ToolState::Running,
                                    },
                                },
                            })
                            .collect();
                        // Start at the top: `off` is clamped on the first draw.
                        let scroll = Scroll {
                            off: usize::MAX / 2,
                            ..Scroll::default()
                        };
                        self.error = None;
                        self.viewer = Some(Viewer {
                            title,
                            items,
                            scroll,
                            cache: None,
                        });
                    }
                    Err(e) => self.error = Some(e),
                }
            }
            // Stale result from a cancelled or superseded request.
            _ => {}
        }
    }

    pub(super) fn on_key(&mut self, code: KeyCode) -> Action {
        if code == KeyCode::Char('r')
            && let Some(s) = self.sessions.get(self.sel)
        {
            return Action::Resume(s.id.clone(), s.name.clone());
        }
        if let Some(v) = &mut self.viewer {
            let s = &mut v.scroll;
            match code {
                KeyCode::Esc | KeyCode::Char('q') | KeyCode::Backspace => self.viewer = None,
                KeyCode::Up | KeyCode::Char('k') => s.up(1),
                KeyCode::Down | KeyCode::Char('j') => s.down(1),
                KeyCode::PageUp => s.up(s.page()),
                KeyCode::PageDown | KeyCode::Char(' ') => s.down(s.page()),
                KeyCode::Home | KeyCode::Char('g') => s.up(usize::MAX / 2),
                KeyCode::End | KeyCode::Char('G') => s.off = 0,
                _ => {}
            }
            return Action::None;
        }
        let prev = self.sel;
        let last = self.sessions.len().saturating_sub(1);
        match code {
            KeyCode::Esc | KeyCode::Char('q' | 'H') => return Action::Close,
            KeyCode::Up | KeyCode::Char('k') => self.sel = self.sel.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => self.sel = (self.sel + 1).min(last),
            KeyCode::PageUp => self.sel = self.sel.saturating_sub(10),
            KeyCode::PageDown => self.sel = (self.sel + 10).min(last),
            KeyCode::Home => self.sel = 0,
            KeyCode::End => self.sel = last,
            KeyCode::Enter => {
                if let Some(s) = self.sessions.get(self.sel) {
                    let req = next_req();
                    self.loading_session = Some(req);
                    self.error = None;
                    return Action::Load(req, s.name.clone(), s.events.clone());
                }
            }
            _ => {}
        }
        if self.sel != prev {
            self.loading_session = None;
        }
        Action::None
    }

    pub(super) fn draw(&mut self, f: &mut Frame, area: Rect) {
        f.render_widget(Clear, area);
        let [body, status] = ratatui::layout::Layout::vertical([
            ratatui::layout::Constraint::Min(3),
            ratatui::layout::Constraint::Length(1),
        ])
        .areas(area);
        match &mut self.viewer {
            Some(v) => {
                draw_viewer(v, f, body);
                let h = hints(&[
                    ("↑↓ PgUp/Dn", "scroll"),
                    ("Home/End", tr("inicio/final", "top/bottom")),
                    ("r", tr("reanudar", "resume")),
                    ("Esc", tr("volver a la lista", "back to list")),
                ]);
                f.render_widget(Paragraph::new(h), status);
            }
            None => {
                self.draw_list(f, body);
                let h = hints(&[
                    ("↑↓", tr("elegir", "select")),
                    ("Enter", tr("abrir", "open")),
                    ("r", tr("reanudar", "resume")),
                    ("Esc", tr("cerrar", "close")),
                ]);
                f.render_widget(Paragraph::new(h), status);
            }
        }
    }

    fn draw_list(&self, f: &mut Frame, area: Rect) {
        let title = Line::styled(
            tr_fmt!(
                " Historial · {} ({}) ",
                " History · {} ({}) ",
                self.project,
                self.sessions.len()
            ),
            bold(c_accent()),
        );
        let mut blk = block(title, c_accent());
        let inner = blk.inner(area);
        let rows = inner.height as usize;
        let top = (self.sel + 1).saturating_sub(rows);
        let w = inner.width as usize;
        let mut lines: Vec<Line<'static>> = self
            .sessions
            .iter()
            .enumerate()
            .skip(top)
            .take(rows)
            .map(|(i, s)| {
                let date = s
                    .updated_at
                    .get(..16)
                    .unwrap_or(&s.updated_at)
                    .replace('T', " ");
                let text = format!(" {date}  {}", trunc(&s.name, w.saturating_sub(20)));
                if i == self.sel {
                    Line::styled(format!("▶{text}"), bold(c_bg()).bg(c_accent()))
                } else {
                    Line::styled(format!(" {text}"), fg(c_fg()))
                }
            })
            .collect();
        if self.loading_list.is_some() {
            lines.push(Line::styled(
                tr("  Cargando historial…", "  Loading history…"),
                fg(c_running()),
            ));
        } else if lines.is_empty() {
            lines.push(Line::styled(
                tr(
                    "  No hay conversaciones anteriores para este proyecto.",
                    "  No past conversations for this project.",
                ),
                fg(c_dim()),
            ));
        }
        if let Some(e) = &self.error {
            lines.push(Line::styled(format!("  ✖ {e}"), fg(c_error())));
        }
        if self.loading_session.is_some() {
            blk = blk.title_bottom(
                Line::styled(
                    tr(" Cargando conversación… ", " Loading conversation… "),
                    fg(c_running()),
                )
                .right_aligned(),
            );
        }
        f.render_widget(Paragraph::new(lines).block(blk), area);
    }
}

fn draw_viewer(v: &mut Viewer, f: &mut Frame, area: Rect) {
    let title = Line::from(vec![
        Span::styled(tr(" Historial · ", " History · "), bold(c_accent())),
        Span::styled(format!("{} ", v.title), bold(c_fg())),
    ]);
    let blk = block(title, c_accent());
    let inner = blk.inner(area);
    let width = inner.width.max(1) as usize;
    let light = settings::light();
    if v.cache
        .as_ref()
        .is_none_or(|c| c.width != width || c.light != light)
    {
        let mut lines = build_lines(&v.items, width);
        if lines.is_empty() {
            lines = wrap(
                &Line::styled(
                    tr("  (conversación vacía)", "  (empty conversation)"),
                    fg(c_dim()),
                ),
                width,
            );
        }
        v.cache = Some(Cache {
            width,
            rev: 0,
            light,
            lines,
        });
    }
    let Some(c) = &v.cache else { return };
    let h = inner.height as usize;
    v.scroll.sync(c.lines.len(), h);
    let end = c.lines.len() - v.scroll.off;
    let start = end.saturating_sub(h);
    let blk = blk.title_bottom(
        Line::styled(format!(" {}/{} ", end, c.lines.len()), fg(c_dim())).right_aligned(),
    );
    f.render_widget(
        Paragraph::new(c.lines[start..end].to_vec()).block(blk),
        area,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(n: &str) -> SessionInfo {
        SessionInfo {
            id: n.into(),
            name: n.into(),
            updated_at: String::new(),
            events: "/nonexistent".into(),
        }
    }

    #[test]
    fn list_navigation_clamps_and_esc_closes() {
        let mut h = History {
            project: "p".into(),
            sessions: vec![info("a"), info("b")],
            sel: 0,
            viewer: None,
            loading_list: None,
            loading_session: None,
            error: None,
        };
        h.on_key(KeyCode::Down);
        h.on_key(KeyCode::Down);
        assert_eq!(h.sel, 1);
        let Action::Load(req, title, _) = h.on_key(KeyCode::Enter) else {
            panic!("expected Action::Load");
        };
        assert_eq!(title, "b");
        h.on_loaded(Loaded::Session(req, title, Err("boom".into())));
        assert!(h.viewer.is_none() && h.error.as_deref() == Some("boom"));
        assert!(matches!(h.on_key(KeyCode::Esc), Action::Close));
        assert!(matches!(h.on_key(KeyCode::Char('r')), Action::Resume(id, _) if id == "b"));
    }

    #[test]
    fn async_load_discards_stale_list_and_session_results() {
        let (mut h, list_req) = History::open("p");
        assert!(h.loading_list.is_some());
        // Stale list response is ignored.
        h.on_loaded(Loaded::List(list_req + 999, vec![info("stale")]));
        assert!(h.loading_list.is_some() && h.sessions.is_empty());
        // Matching list response populates sessions.
        h.on_loaded(Loaded::List(list_req, vec![info("a"), info("b")]));
        assert!(h.loading_list.is_none() && h.sessions.len() == 2);

        // Start loading session "a", then move selection to "b": old result must be discarded.
        let Action::Load(req_a, title_a, _) = h.on_key(KeyCode::Enter) else {
            panic!("expected Action::Load for a");
        };
        h.on_key(KeyCode::Down);
        h.on_loaded(Loaded::Session(
            req_a,
            title_a,
            Ok(vec![Entry::User("hi".into())]),
        ));
        assert!(h.viewer.is_none());

        // Loading session "b" and delivering its matching result opens the viewer.
        let Action::Load(req_b, title_b, _) = h.on_key(KeyCode::Enter) else {
            panic!("expected Action::Load for b");
        };
        h.on_loaded(Loaded::Session(
            req_b,
            title_b,
            Ok(vec![Entry::User("hello".into())]),
        ));
        assert!(h.viewer.as_ref().is_some_and(|v| v.title == "b"));
    }
}
