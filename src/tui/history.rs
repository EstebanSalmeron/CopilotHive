//! Read-only browser of past sessions of the current project (`H`).
use super::markdown::wrap;
use super::settings::tr;
use super::theme::*;
use super::widgets::{block, build_lines, hints, trunc};
use super::{Cache, Item, Scroll, ToolState};
use crate::history::{self, Entry, SessionInfo};
use ratatui::Frame;
use ratatui::crossterm::event::KeyCode;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};

struct Viewer {
    title: String,
    items: Vec<Item>,
    scroll: Scroll,
    cache: Option<Cache>,
}

pub(super) enum Action {
    None,
    Close,
    /// Session id and title.
    Resume(String, String),
}

pub(super) struct History {
    project: String,
    sessions: Vec<SessionInfo>,
    sel: usize,
    viewer: Option<Viewer>,
    /// Last load error, shown in the list.
    error: Option<String>,
}

impl History {
    pub(super) fn open(project: &str, cwd: &str) -> Self {
        let sessions = history::root()
            .map(|r| history::list(&r, cwd))
            .unwrap_or_default();
        History {
            project: project.into(),
            sessions,
            sel: 0,
            viewer: None,
            error: None,
        }
    }

    fn open_selected(&mut self) {
        let Some(s) = self.sessions.get(self.sel) else {
            return;
        };
        match history::load(&s.events) {
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
                    title: s.name.clone(),
                    items,
                    scroll,
                    cache: None,
                });
            }
            Err(e) => self.error = Some(format!("{e:#}")),
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
        let last = self.sessions.len().saturating_sub(1);
        match code {
            KeyCode::Esc | KeyCode::Char('q' | 'H') => return Action::Close,
            KeyCode::Up | KeyCode::Char('k') => self.sel = self.sel.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => self.sel = (self.sel + 1).min(last),
            KeyCode::PageUp => self.sel = self.sel.saturating_sub(10),
            KeyCode::PageDown => self.sel = (self.sel + 10).min(last),
            KeyCode::Home => self.sel = 0,
            KeyCode::End => self.sel = last,
            KeyCode::Enter => self.open_selected(),
            _ => {}
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
        let blk = block(title, c_accent());
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
        if lines.is_empty() {
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
    if v.cache.as_ref().is_none_or(|c| c.width != width) {
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

    #[test]
    fn list_navigation_clamps_and_esc_closes() {
        let s = |n: &str| SessionInfo {
            id: n.into(),
            name: n.into(),
            updated_at: String::new(),
            events: "/nonexistent".into(),
        };
        let mut h = History {
            project: "p".into(),
            sessions: vec![s("a"), s("b")],
            sel: 0,
            viewer: None,
            error: None,
        };
        h.on_key(KeyCode::Down);
        h.on_key(KeyCode::Down);
        assert_eq!(h.sel, 1);
        h.on_key(KeyCode::Enter); // unreadable file: stays in the list with an error
        assert!(h.viewer.is_none() && h.error.is_some());
        assert!(matches!(h.on_key(KeyCode::Esc), Action::Close));
        assert!(matches!(h.on_key(KeyCode::Char('r')), Action::Resume(id, _) if id == "b"));
    }
}
