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
    /// User-chosen label shown instead of `name`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alias: Option<String>,
    /// Folders; the first is the session `cwd`, the rest go through `--add-dir`.
    pub dirs: Vec<String>,
}

/// Group every multi-project session lives in.
pub(super) const GROUP: &str = "Custom Grouping";

impl Saved {
    /// Tab title: the alias when set, otherwise the generated name.
    pub(super) fn label(&self) -> &str {
        self.alias.as_deref().unwrap_or(&self.name)
    }

    pub(super) fn from_items(items: &[Item], alias: Option<String>) -> Self {
        Saved {
            name: title(items),
            alias,
            dirs: items.iter().map(|i| i.cwd.clone()).collect(),
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

/// Adds `s`, replacing a saved session with the same folders; an existing alias is kept
/// when `s` has none.
fn upsert(list: &mut Vec<Saved>, mut s: Saved) -> Saved {
    if let Some(old) = list.iter().find(|o| o.dirs == s.dirs) {
        s.alias = s.alias.or_else(|| old.alias.clone());
    }
    list.retain(|o| o.dirs != s.dirs);
    list.push(s.clone());
    s
}

/// Saves a started session so it is offered again after a restart.
/// Returns the stored entry, which keeps a previously set alias.
pub(super) fn remember(s: Saved) -> (Saved, Option<std::io::Error>) {
    let mut all = load();
    let kept = upsert(&mut all, s);
    (kept, store(&all).err())
}

pub(super) enum Action {
    None,
    Close,
    /// Picked projects in selection order (at least two); the caller also remembers them.
    Start(Vec<Item>, Option<String>),
    /// A remembered session chosen from the saved list.
    Open(Saved),
}

/// What the alias text box is editing.
enum Edit {
    /// The alias of the session about to start.
    Pending,
    /// The alias of the saved session at this index.
    Saved(usize),
    /// Asked after picking, right before the session starts.
    Start,
}

#[derive(PartialEq)]
enum View {
    Pick,
    Saved,
}

/// Fuzzy score of `query` as a case-insensitive subsequence of `text`; `None` if it doesn't match.
/// Consecutive characters and word starts score higher, gaps cost a little.
fn fuzzy(query: &str, text: &str) -> Option<i32> {
    let hay: Vec<char> = text.to_lowercase().chars().collect();
    let (mut at, mut score, mut prev) = (0, 0, None::<usize>);
    for q in query.to_lowercase().chars().filter(|c| !c.is_whitespace()) {
        let found = (at..hay.len()).find(|&i| hay[i] == q)?;
        score += 10;
        if prev == Some(found.wrapping_sub(1)) {
            score += 15;
        }
        if found == 0 || matches!(hay[found - 1], '-' | '_' | ' ' | '/' | '.') {
            score += 10;
        }
        score -= (found - at) as i32;
        prev = Some(found);
        at = found + 1;
    }
    Some(score)
}

pub(super) struct Picker {
    items: Vec<Item>,
    /// Indices into `items`, in the order they were picked.
    picked: Vec<usize>,
    /// Indices into `items` that match `filter`, best match first.
    vis: Vec<usize>,
    filter: String,
    /// The filter box has the keyboard (letters are typed instead of acting as shortcuts).
    typing: bool,
    /// Highlighted row in `vis`.
    sel: usize,
    error: bool,
    view: View,
    saved: Vec<Saved>,
    saved_sel: usize,
    alias: String,
    edit: Option<(Edit, String)>,
    /// `d` was pressed once on a saved session; a second press deletes it.
    confirm_del: bool,
}

impl Picker {
    pub(super) fn new(items: Vec<Item>, sel: usize, saved: Vec<Saved>) -> Self {
        Picker {
            view: View::Pick,
            saved,
            saved_sel: 0,
            alias: String::new(),
            edit: None,
            confirm_del: false,
            vis: (0..items.len()).collect(),
            filter: String::new(),
            typing: false,
            sel: sel.min(items.len().saturating_sub(1)),
            items,
            picked: vec![],
            error: false,
        }
    }

    /// Index into `items` of the highlighted row.
    fn cur(&self) -> Option<usize> {
        self.vis.get(self.sel).copied()
    }

    /// Recomputes `vis` after the filter changed, keeping the highlighted project when possible.
    fn refilter(&mut self) {
        let keep = self.cur();
        let mut scored: Vec<(i32, usize)> = self
            .items
            .iter()
            .enumerate()
            .filter_map(|(i, it)| {
                let hay = format!("{} {}", it.name, it.group.as_deref().unwrap_or(""));
                fuzzy(&self.filter, &hay).map(|sc| (sc, i))
            })
            .collect();
        if !self.filter.is_empty() {
            scored.sort_by_key(|&(sc, i)| (std::cmp::Reverse(sc), i));
        }
        self.vis = scored.into_iter().map(|(_, i)| i).collect();
        self.sel = keep
            .and_then(|k| self.vis.iter().position(|&v| v == k))
            .unwrap_or(0);
    }

    fn on_filter_key(&mut self, code: KeyCode) {
        match code {
            KeyCode::Esc | KeyCode::Enter => self.typing = false,
            KeyCode::Backspace => {
                self.filter.pop();
                self.refilter();
            }
            KeyCode::Char(c) if self.filter.chars().count() < 40 => {
                self.filter.push(c);
                self.refilter();
                self.sel = 0;
            }
            KeyCode::Up => self.sel = self.sel.saturating_sub(1),
            KeyCode::Down => self.sel = (self.sel + 1).min(self.vis.len().saturating_sub(1)),
            _ => {}
        }
    }

    fn toggle(&mut self, i: usize) {
        match self.picked.iter().position(|&p| p == i) {
            Some(at) => drop(self.picked.remove(at)),
            None => self.picked.push(i),
        }
    }

    /// Picks every listed project (all, or the filter's matches), or drops them if all are picked.
    fn toggle_all(&mut self) {
        if self.vis.iter().all(|v| self.picked.contains(v)) {
            self.picked.retain(|p| !self.vis.contains(p));
        } else {
            let missing: Vec<usize> = self
                .vis
                .iter()
                .copied()
                .filter(|i| !self.picked.contains(i))
                .collect();
            self.picked.extend(missing);
        }
    }

    /// Picks the listed projects of the highlighted one's group, or drops them if all are picked.
    fn toggle_group(&mut self) {
        let Some(cur) = self.cur().map(|i| &self.items[i]) else {
            return;
        };
        let members: Vec<usize> = self
            .vis
            .iter()
            .copied()
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
        let armed = std::mem::take(&mut self.confirm_del);
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
            KeyCode::Char('r') if self.saved_sel < self.saved.len() => {
                let cur = self.saved[self.saved_sel].alias.clone().unwrap_or_default();
                self.edit = Some((Edit::Saved(self.saved_sel), cur));
            }
            KeyCode::Char('d') if self.saved_sel < self.saved.len() && !armed => {
                self.confirm_del = true;
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

    fn on_edit_key(&mut self, code: KeyCode) -> Action {
        let Some((target, buf)) = &mut self.edit else {
            return Action::None;
        };
        match code {
            KeyCode::Esc => self.edit = None,
            KeyCode::Backspace => drop(buf.pop()),
            KeyCode::Char(c) if buf.chars().count() < 40 => buf.push(c),
            KeyCode::Enter => {
                let text = buf.trim().to_string();
                let alias = (!text.is_empty()).then_some(text);
                match target {
                    Edit::Pending => self.alias = alias.unwrap_or_default(),
                    Edit::Start => {
                        self.alias = alias.unwrap_or_default();
                        self.edit = None;
                        return self.start();
                    }
                    Edit::Saved(i) => {
                        if let Some(s) = self.saved.get_mut(*i) {
                            s.alias = alias;
                            self.error = store(&self.saved).is_err();
                        }
                    }
                }
                self.edit = None;
            }
            _ => {}
        }
        Action::None
    }

    fn start(&self) -> Action {
        let items = self.picked.iter().map(|&i| self.items[i].clone()).collect();
        let alias = (!self.alias.is_empty()).then(|| self.alias.clone());
        Action::Start(items, alias)
    }

    /// Opens the picker on the saved-sessions list.
    pub(super) fn show_saved(mut self) -> Self {
        self.view = View::Saved;
        self
    }

    pub(super) fn on_key(&mut self, code: KeyCode) -> Action {
        if self.edit.is_some() {
            return self.on_edit_key(code);
        }
        if self.view == View::Saved {
            return self.on_saved_key(code);
        }
        if self.typing {
            self.on_filter_key(code);
            return Action::None;
        }
        self.error = false;
        let last = self.vis.len().saturating_sub(1);
        match code {
            KeyCode::Esc if !self.filter.is_empty() => {
                self.filter.clear();
                self.refilter();
            }
            KeyCode::Esc | KeyCode::Char('q') => return Action::Close,
            KeyCode::Char('/') => self.typing = true,
            KeyCode::Tab | KeyCode::BackTab => self.view = View::Saved,
            KeyCode::Up | KeyCode::Char('k') => self.sel = self.sel.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => self.sel = (self.sel + 1).min(last),
            KeyCode::PageUp => self.sel = self.sel.saturating_sub(10),
            KeyCode::PageDown => self.sel = (self.sel + 10).min(last),
            KeyCode::Home => self.sel = 0,
            KeyCode::End => self.sel = last,
            KeyCode::Char(' ') => {
                if let Some(i) = self.cur() {
                    self.toggle(i);
                }
            }
            KeyCode::Char('g') => self.toggle_group(),
            KeyCode::Char('a') => self.toggle_all(),
            KeyCode::Char('c') => self.picked.clear(),
            KeyCode::Char('n') => self.edit = Some((Edit::Pending, self.alias.clone())),
            KeyCode::Enter => {
                if self.picked.len() < 2 {
                    self.error = true;
                } else {
                    self.edit = Some((Edit::Start, self.alias.clone()));
                }
            }
            _ => {}
        }
        Action::None
    }

    fn edit_line(&self) -> Option<Line<'static>> {
        let (target, buf) = self.edit.as_ref()?;
        let hint = match target {
            Edit::Start => tr(
                "  Enter iniciar (vacío = sin alias) · Esc volver",
                "  Enter start (empty = no alias) · Esc back",
            ),
            _ => tr("  Enter ok · Esc cancelar", "  Enter ok · Esc cancel"),
        };
        Some(Line::from(vec![
            Span::styled(tr("alias: ", "alias: "), bold(c_accent())),
            Span::styled(format!("{buf}▏"), fg(c_fg())),
            Span::styled(hint, fg(c_dim())),
        ]))
    }

    fn draw_saved(&self, f: &mut Frame, area: Rect) {
        let rows = self.saved.len().max(1);
        let h = (rows as u16 + 4).min(area.height.saturating_sub(2));
        let r = centered(area, area.width.min(92), h);
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
                    let text = trunc(&format!("{}  ({n})", s.label()), w.saturating_sub(2));
                    if i == self.saved_sel {
                        Line::styled(format!("▶ {text}"), bold(c_bg()).bg(c_accent()))
                    } else {
                        Line::styled(format!("  {text}"), fg(c_fg()))
                    }
                })
                .collect()
        };
        lines.push(Line::default());
        lines.push(if let Some(l) = self.edit_line() {
            l
        } else if self.error {
            Line::styled(
                tr("✖ no se pudo guardar", "✖ could not save"),
                fg(c_error()),
            )
        } else if self.confirm_del {
            Line::styled(
                tr(
                    "¿borrar esta sesión? pulsa d otra vez · otra tecla cancela",
                    "delete this session? press d again · any other key cancels",
                ),
                bold(c_attention()),
            )
        } else {
            Line::styled(
                tr(
                    "↑↓ · Enter abrir · r alias · d borrar · Tab elegir · Esc",
                    "↑↓ · Enter open · r alias · d delete · Tab pick · Esc",
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
        let h = (self.items.len() as u16 + 5).min(area.height.saturating_sub(2));
        let rows = h.saturating_sub(5) as usize;
        let r = centered(area, area.width.min(92), h);
        let off = self
            .sel
            .saturating_sub(rows.saturating_sub(1))
            .min(self.vis.len().saturating_sub(rows));
        let w = r.width.saturating_sub(4) as usize;
        let mut lines: Vec<Line<'static>> = self
            .vis
            .iter()
            .enumerate()
            .skip(off)
            .take(rows)
            .map(|(row, &i)| {
                let it = &self.items[i];
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
                if row == self.sel {
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
        if self.vis.is_empty() {
            lines.push(Line::styled(
                tr("(sin coincidencias)", "(no matches)"),
                fg(c_dim()),
            ));
        }
        while lines.len() < rows.max(1) {
            lines.push(Line::default());
        }
        lines.push(if let Some(l) = self.edit_line() {
            l
        } else if self.typing || !self.filter.is_empty() {
            let cursor = if self.typing { "▏" } else { "" };
            Line::from(vec![
                Span::styled(tr("buscar: ", "search: "), bold(c_accent())),
                Span::styled(format!("{}{cursor}", self.filter), fg(c_fg())),
                Span::styled(
                    if self.typing {
                        tr(
                            "  Enter ok · Esc salir · ↑↓ mover",
                            "  Enter ok · Esc stop · ↑↓ move",
                        )
                    } else {
                        tr("  / editar · Esc limpiar", "  / edit · Esc clear")
                    },
                    fg(c_dim()),
                ),
            ])
        } else if self.error {
            Line::from(Span::styled(
                tr("✖ elige al menos 2 proyectos", "✖ pick at least 2 projects"),
                fg(c_error()),
            ))
        } else {
            Line::styled(
                tr(
                    "Espacio elegir · a todos · g grupo · c limpiar · n alias",
                    "Space pick · a all · g group · c clear · n alias",
                ),
                fg(c_dim()),
            )
        });
        lines.push(Line::styled(
            tr(
                "/ buscar · Enter iniciar · Tab guardadas · Esc cerrar",
                "/ search · Enter start · Tab saved · Esc close",
            ),
            fg(c_dim()),
        ));
        f.render_widget(Clear, r);
        f.render_widget(
            Paragraph::new(lines).block(
                block(
                    Line::styled(
                        {
                            let alias = if self.alias.is_empty() {
                                String::new()
                            } else {
                                format!("· {} ", self.alias)
                            };
                            tr_fmt!(
                                " Sesión multiproyecto · {} elegidos {alias}",
                                " Multi-project session · {} picked {alias}",
                                self.picked.len()
                            )
                        },
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
        assert!(matches!(p.on_key(KeyCode::Enter), Action::None));
        assert!(p.edit.is_some()); // the alias is always asked before starting
        let Action::Start(items, _) = p.on_key(KeyCode::Enter) else {
            panic!("expected start")
        };
        let names: Vec<_> = items.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, ["a", "d"]);
        p.on_key(KeyCode::Char(' '));
        assert_eq!(p.picked, [0]);
    }

    #[test]
    fn a_picks_all_keeping_existing_order_then_clears() {
        let mut p = picker();
        p.on_key(KeyCode::End);
        p.on_key(KeyCode::Char(' '));
        p.on_key(KeyCode::Char('a'));
        assert_eq!(p.picked, [3, 0, 1, 2]);
        p.on_key(KeyCode::Char('a'));
        assert!(p.picked.is_empty());
    }

    #[test]
    fn fuzzy_filter_ranks_matches_and_scopes_select_all() {
        let mut p = Picker::new(
            vec![
                item("sales-core-order", "Core"),
                item("sales-ui-order", "UI"),
                item("customer-core", "Core"),
            ],
            0,
            vec![],
        );
        assert_eq!(fuzzy("zzz", "sales"), None);
        p.on_key(KeyCode::Char('/'));
        "scor"
            .chars()
            .for_each(|c| drop(p.on_key(KeyCode::Char(c))));
        assert_eq!(p.vis, [0, 2]); // "sales-ui-order" has no "c" after the "s"; the tighter match ranks first
        p.on_key(KeyCode::Backspace);
        p.on_key(KeyCode::Backspace);
        p.on_key(KeyCode::Backspace);
        "order"
            .chars()
            .for_each(|c| drop(p.on_key(KeyCode::Char(c))));
        assert_eq!(p.vis.len(), 2);
        p.on_key(KeyCode::Enter); // leave the box, keep the filter
        assert!(!p.typing && !p.filter.is_empty());
        p.on_key(KeyCode::Char('a')); // shortcut again, applied to matches only
        assert_eq!(p.picked.len(), 2);
        assert!(!p.picked.contains(&2));
        assert!(matches!(p.on_key(KeyCode::Esc), Action::None)); // clears filter first
        assert_eq!(p.vis.len(), 3);
        assert!(matches!(p.on_key(KeyCode::Esc), Action::Close));
    }

    #[test]
    fn renders_search_box_and_no_matches() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let mut p = picker();
        p.on_key(KeyCode::Char('/'));
        p.on_key(KeyCode::Char('z'));
        let mut term = Terminal::new(TestBackend::new(110, 30)).unwrap();
        term.draw(|f| p.draw(f, f.area())).unwrap();
        let buf = term.backend().buffer();
        let text: String = (0..30)
            .flat_map(|y| (0..110).map(move |x| (x, y)))
            .map(|(x, y)| buf[(x, y)].symbol().to_string())
            .collect();
        assert!(text.contains("buscar: z") && text.contains("sin coincidencias"));
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
            alias: None,
            dirs: d.iter().map(|x| x.to_string()).collect(),
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
        p.on_key(KeyCode::Char('r'));
        "my".chars().for_each(|c| drop(p.on_key(KeyCode::Char(c))));
        p.on_key(KeyCode::Enter);
        assert_eq!(p.saved[1].alias.as_deref(), Some("my"));
        assert_eq!(p.saved[1].label(), "my");
        p.on_key(KeyCode::Char('d'));
        assert_eq!(p.saved.len(), 2); // needs a second press
        p.on_key(KeyCode::Char('j')); // any other key cancels
        p.on_key(KeyCode::Char('d'));
        assert_eq!(p.saved.len(), 2);
        p.on_key(KeyCode::Char('d'));
        assert_eq!(p.saved.len(), 1);
        p.on_key(KeyCode::Tab);
        assert!(p.view == View::Pick);
    }

    #[test]
    fn pending_alias_goes_with_start_and_survives_upsert() {
        let mut p = picker();
        p.on_key(KeyCode::Char('g'));
        p.on_key(KeyCode::Char('n'));
        p.on_key(KeyCode::Char('q')); // typed into the box, not a close
        p.on_key(KeyCode::Char('x'));
        p.on_key(KeyCode::Backspace);
        p.on_key(KeyCode::Enter);
        p.on_key(KeyCode::Enter); // asks for the alias again, prefilled
        assert_eq!(p.edit.as_ref().map(|e| e.1.as_str()), Some("q"));
        let Action::Start(items, alias) = p.on_key(KeyCode::Enter) else {
            panic!("expected start")
        };
        assert_eq!(alias.as_deref(), Some("q"));
        let first = Saved::from_items(&items, alias);
        let mut l = vec![first.clone()];
        upsert(&mut l, Saved::from_items(&items, None));
        assert_eq!(l[0].alias.as_deref(), Some("q"));
        let text = toml::to_string(&File { sessions: l }).unwrap();
        assert!(text.contains("alias = \"q\""));
    }

    #[test]
    fn title_abbreviates_long_selections() {
        let it = |n| item(n, "X");
        assert_eq!(title(&[it("a"), it("b")]), "a + b");
        assert_eq!(title(&[it("a"), it("b"), it("c"), it("d")]), "a + b +2");
    }
}
