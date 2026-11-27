//! Multi-project session picker: choose several open projects and start one session that can
//! access all of them (the first pick is the `cwd`, the rest go through `copilot --add-dir`).
use super::settings::tr;
use super::theme::*;
use super::widgets::{block, centered, trunc};
use ratatui::Frame;
use ratatui::crossterm::event::KeyCode;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Padding, Paragraph};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone)]
pub(super) struct Item {
    pub name: String,
    pub cwd: String,
    pub group: Option<String>,
}

/// A multi-project session remembered in `~/.config/copilot-hive/sessions.toml`.
#[derive(Clone, Serialize, Deserialize, PartialEq, Debug)]
pub(super) struct Saved {
    pub name: String,
    /// Folders; the first is the session `cwd`, the rest go through `--add-dir`.
    pub dirs: Vec<String>,
    #[serde(default)]
    pub group: Option<String>,
}

impl Saved {
    pub(super) fn from_items(items: &[Item]) -> Self {
        Saved {
            name: title(items),
            dirs: items.iter().map(|i| i.cwd.clone()).collect(),
            group: items.first().and_then(|i| i.group.clone()),
        }
    }
}

#[derive(Default, Serialize, Deserialize)]
struct File {
    #[serde(default)]
    sessions: Vec<Saved>,
}

fn path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join(".config/copilot-hive/sessions.toml"))
}

pub(super) fn load() -> Vec<Saved> {
    path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|t| toml::from_str::<File>(&t).ok())
        .unwrap_or_default()
        .sessions
}

fn store(sessions: &[Saved]) -> std::io::Result<()> {
    let Some(p) = path().filter(|_| !cfg!(test)) else {
        return Ok(());
    };
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d)?;
    }
    let text = toml::to_string(&File {
        sessions: sessions.to_vec(),
    })
    .map_err(std::io::Error::other)?;
    std::fs::write(p, text)
}

/// Adds `s` unless a saved session already has the same folders (the newest wins the name).
fn upsert(list: &mut Vec<Saved>, s: Saved) {
    list.retain(|o| o.dirs != s.dirs);
    list.push(s);
}

/// Saves a started session so it is offered again after a restart.
pub(super) fn remember(s: &Saved) -> std::io::Result<()> {
    let mut all = load();
    upsert(&mut all, s.clone());
    store(&all)
}

pub(super) enum Action {
    None,
    Close,
    /// Picked projects in selection order (at least two); the caller also remembers them.
    Start(Vec<Item>),
    /// A remembered session chosen from the saved list.
    Open(Saved),
}

#[derive(PartialEq)]
enum View {
    Pick,
    Saved,
}

pub(super) struct Picker {
    items: Vec<Item>,
    /// Indices into `items`, in the order they were picked.
    picked: Vec<usize>,
    sel: usize,
    error: bool,
    view: View,
    saved: Vec<Saved>,
    saved_sel: usize,
}

impl Picker {
    pub(super) fn new(items: Vec<Item>, sel: usize, saved: Vec<Saved>) -> Self {
        Picker {
            view: View::Pick,
            saved,
            saved_sel: 0,
            sel: sel.min(items.len().saturating_sub(1)),
            items,
            picked: vec![],
            error: false,
        }
    }

    fn toggle(&mut self, i: usize) {
        match self.picked.iter().position(|&p| p == i) {
            Some(at) => drop(self.picked.remove(at)),
            None => self.picked.push(i),
        }
    }

    /// Picks every project of the highlighted one's group, or drops them all if already picked.
    fn toggle_group(&mut self) {
        let Some(cur) = self.items.get(self.sel) else {
            return;
        };
        let members: Vec<usize> = (0..self.items.len())
            .filter(|&i| self.items[i].group == cur.group)
            .collect();
        if members.iter().all(|m| self.picked.contains(m)) {
            self.picked.retain(|p| !members.contains(p));
        } else {
            let missing: Vec<usize> = members
                .into_iter()
                .filter(|m| !self.picked.contains(m))
                .collect();
            self.picked.extend(missing);
        }
    }

    fn on_saved_key(&mut self, code: KeyCode) -> Action {
        let last = self.saved.len().saturating_sub(1);
        match code {
            KeyCode::Esc | KeyCode::Char('q') => return Action::Close,
            KeyCode::Tab | KeyCode::BackTab => self.view = View::Pick,
            KeyCode::Up | KeyCode::Char('k') => self.saved_sel = self.saved_sel.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => self.saved_sel = (self.saved_sel + 1).min(last),
            KeyCode::Enter => {
                if let Some(s) = self.saved.get(self.saved_sel) {
                    return Action::Open(s.clone());
                }
            }
            KeyCode::Char('d') if self.saved_sel < self.saved.len() => {
                self.saved.remove(self.saved_sel);
                self.saved_sel = self.saved_sel.min(self.saved.len().saturating_sub(1));
                self.error = store(&self.saved).is_err();
            }
            _ => {}
        }
        Action::None
    }

    pub(super) fn on_key(&mut self, code: KeyCode) -> Action {
        if self.view == View::Saved {
            return self.on_saved_key(code);
        }
        self.error = false;
        let last = self.items.len().saturating_sub(1);
        match code {
            KeyCode::Esc | KeyCode::Char('q') => return Action::Close,
            KeyCode::Tab | KeyCode::BackTab => self.view = View::Saved,
            KeyCode::Up | KeyCode::Char('k') => self.sel = self.sel.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => self.sel = (self.sel + 1).min(last),
            KeyCode::PageUp => self.sel = self.sel.saturating_sub(10),
            KeyCode::PageDown => self.sel = (self.sel + 10).min(last),
            KeyCode::Home => self.sel = 0,
            KeyCode::End => self.sel = last,
            KeyCode::Char(' ') if !self.items.is_empty() => self.toggle(self.sel),
            KeyCode::Char('g') => self.toggle_group(),
            KeyCode::Char('c') => self.picked.clear(),
            KeyCode::Enter => {
                if self.picked.len() < 2 {
                    self.error = true;
                } else {
                    let items = self.picked.iter().map(|&i| self.items[i].clone()).collect();
                    return Action::Start(items);
                }
            }
            _ => {}
        }
        Action::None
    }

    fn draw_saved(&self, f: &mut Frame, area: Rect) {
        let rows = self.saved.len().max(1);
        let h = (rows as u16 + 4).min(area.height.saturating_sub(2));
        let r = centered(area, area.width.min(78), h);
        let visible = h.saturating_sub(4) as usize;
        let off = self
            .saved_sel
            .saturating_sub(visible.saturating_sub(1))
            .min(rows.saturating_sub(visible));
        let w = r.width.saturating_sub(4) as usize;
        let mut lines: Vec<Line<'static>> = if self.saved.is_empty() {
            vec![Line::styled(
                tr(
                    "(aún no hay sesiones guardadas: se guardan al iniciarlas)",
                    "(no saved sessions yet: they are saved when started)",
                ),
                fg(c_dim()),
            )]
        } else {
            self.saved
                .iter()
                .enumerate()
                .skip(off)
                .take(visible)
                .map(|(i, s)| {
                    let n = s.dirs.len();
                    let text = trunc(&format!("{}  ({n})", s.name), w.saturating_sub(2));
                    if i == self.saved_sel {
                        Line::styled(format!("▶ {text}"), bold(c_bg()).bg(c_accent()))
                    } else {
                        Line::styled(format!("  {text}"), fg(c_fg()))
                    }
                })
                .collect()
        };
        lines.push(Line::default());
        lines.push(if self.error {
            Line::styled(
                tr("✖ no se pudo guardar", "✖ could not save"),
                fg(c_error()),
            )
        } else {
            Line::styled(
                tr(
                    "↑↓ · Enter abrir · d borrar · Tab elegir proyectos · Esc",
                    "↑↓ · Enter open · d delete · Tab pick projects · Esc",
                ),
                fg(c_dim()),
            )
        });
        f.render_widget(Clear, r);
        f.render_widget(
            Paragraph::new(lines).block(
                block(
                    Line::styled(
                        tr_fmt!(
                            " Sesiones guardadas · {} ",
                            " Saved sessions · {} ",
                            self.saved.len()
                        ),
                        bold(c_accent()),
                    ),
                    c_accent(),
                )
                .padding(Padding::horizontal(1)),
            ),
            r,
        );
    }

    pub(super) fn draw(&self, f: &mut Frame, area: Rect) {
        if self.view == View::Saved {
            return self.draw_saved(f, area);
        }
        let h = (self.items.len() as u16 + 4).min(area.height.saturating_sub(2));
        let rows = h.saturating_sub(4) as usize;
        let r = centered(area, area.width.min(78), h);
        let off = self
            .sel
            .saturating_sub(rows.saturating_sub(1))
            .min(self.items.len().saturating_sub(rows));
        let w = r.width.saturating_sub(4) as usize;
        let mut lines: Vec<Line<'static>> = self
            .items
            .iter()
            .enumerate()
            .skip(off)
            .take(rows)
            .map(|(i, it)| {
                let mark = match self.picked.iter().position(|&p| p == i) {
                    Some(n) => format!("[{}]", n + 1),
                    None => "[ ]".into(),
                };
                let group = it.group.as_deref().unwrap_or("");
                let name_w = w.saturating_sub(7 + group.chars().count() + 1).max(8);
                let text = format!(
                    "{mark:<4}{:<name_w$} {group}",
                    trunc(&it.name, name_w),
                    name_w = name_w
                );
                if i == self.sel {
                    Line::styled(format!("▶ {text}"), bold(c_bg()).bg(c_accent()))
                } else {
                    let c = if self.picked.contains(&i) {
                        c_fg()
                    } else {
                        c_dim()
                    };
                    Line::styled(format!("  {text}"), fg(c))
                }
            })
            .collect();
        lines.push(Line::default());
        lines.push(if self.error {
            Line::from(Span::styled(
                tr(
                    "✖ elige al menos 2 proyectos",
                    "✖ pick at least 2 projects",
                ),
                fg(c_error()),
            ))
        } else {
            Line::styled(
                tr(
                    "↑↓ · Espacio elegir (1º = cwd) · g grupo · c limpiar · Enter iniciar · Tab guardadas",
                    "↑↓ · Space pick (1st = cwd) · g group · c clear · Enter start · Tab saved",
                ),
                fg(c_dim()),
            )
        });
        f.render_widget(Clear, r);
        f.render_widget(
            Paragraph::new(lines).block(
                block(
                    Line::styled(
                        tr_fmt!(
                            " Sesión multiproyecto · {} elegidos ",
                            " Multi-project session · {} picked ",
                            self.picked.len()
                        ),
                        bold(c_accent()),
                    ),
                    c_accent(),
                )
                .padding(Padding::horizontal(1)),
            ),
            r,
        );
    }
}

/// Tab title: all names when short, otherwise the first two plus a count.
pub(super) fn title(items: &[Item]) -> String {
    let names: Vec<&str> = items.iter().map(|i| i.name.as_str()).collect();
    if names.len() <= 3 {
        names.join(" + ")
    } else {
        format!("{} + {} +{}", names[0], names[1], names.len() - 2)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(n: &str, g: &str) -> Item {
        Item {
            name: n.into(),
            cwd: format!("/{n}"),
            group: Some(g.into()),
        }
    }

    fn picker() -> Picker {
        Picker::new(
            vec![
                item("a", "X"),
                item("b", "Y"),
                item("c", "X"),
                item("d", "Y"),
            ],
            0,
            vec![],
        )
    }

    #[test]
    fn needs_two_picks_and_keeps_pick_order() {
        let mut p = picker();
        p.on_key(KeyCode::Char(' '));
        assert!(matches!(p.on_key(KeyCode::Enter), Action::None));
        assert!(p.error);
        p.on_key(KeyCode::End);
        p.on_key(KeyCode::Char(' '));
        let Action::Start(items) = p.on_key(KeyCode::Enter) else {
            panic!("expected start")
        };
        let names: Vec<_> = items.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, ["a", "d"]);
        p.on_key(KeyCode::Char(' '));
        assert_eq!(p.picked, [0]);
    }

    #[test]
    fn g_toggles_whole_group_and_c_clears() {
        let mut p = picker();
        p.on_key(KeyCode::Char('g'));
        assert_eq!(p.picked, [0, 2]);
        p.on_key(KeyCode::Char('g'));
        assert!(p.picked.is_empty());
        p.on_key(KeyCode::Char('g'));
        p.on_key(KeyCode::Char('c'));
        assert!(p.picked.is_empty());
        assert!(matches!(p.on_key(KeyCode::Esc), Action::Close));
    }

    #[test]
    fn upsert_replaces_same_folders_and_tab_switches_view() {
        let s = |n: &str, d: &[&str]| Saved {
            name: n.into(),
            dirs: d.iter().map(|x| x.to_string()).collect(),
            group: None,
        };
        let mut l = vec![s("old", &["/a", "/b"]), s("other", &["/c", "/d"])];
        upsert(&mut l, s("new", &["/a", "/b"]));
        assert_eq!(
            l.iter().map(|x| x.name.as_str()).collect::<Vec<_>>(),
            ["other", "new"]
        );
        let text = toml::to_string(&File {
            sessions: l.clone(),
        })
        .unwrap();
        assert_eq!(toml::from_str::<File>(&text).unwrap().sessions, l);

        let mut p = Picker::new(vec![item("a", "X")], 0, l);
        p.on_key(KeyCode::Tab);
        assert!(matches!(p.on_key(KeyCode::Down), Action::None));
        let Action::Open(o) = p.on_key(KeyCode::Enter) else {
            panic!("expected open")
        };
        assert_eq!(o.name, "new");
        p.on_key(KeyCode::Char('d'));
        assert_eq!(p.saved.len(), 1);
        p.on_key(KeyCode::Tab);
        assert!(p.view == View::Pick);
    }

    #[test]
    fn title_abbreviates_long_selections() {
        let it = |n| item(n, "X");
        assert_eq!(title(&[it("a"), it("b")]), "a + b");
        assert_eq!(title(&[it("a"), it("b"), it("c"), it("d")]), "a + b +2");
    }
}
