//! Locally saved reusable prompts (`~/.config/copilot-hive/prompts.toml`).
use super::input::Editor;
use super::markdown::wrap;
use super::settings::tr;
use super::theme::*;
use super::widgets::{block, centered, trunc};
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::Line;
use ratatui::widgets::{Clear, Padding, Paragraph};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(super) struct Prompt {
    pub name: String,
    pub body: String,
}

#[derive(Default, Serialize, Deserialize)]
struct Store {
    #[serde(default)]
    prompts: Vec<Prompt>,
}

fn default_path() -> Option<PathBuf> {
    if cfg!(test) {
        return None;
    }
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join(".config/copilot-hive/prompts.toml"))
}

fn load_from(path: &Path) -> Result<Vec<Prompt>, String> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(e) => return Err(e.to_string()),
    };
    let store: Store = toml::from_str(&text).map_err(|e| e.message().to_string())?;
    Ok(store.prompts)
}

fn error_line(e: &str, w: usize) -> Line<'static> {
    let clean = e.split_whitespace().collect::<Vec<_>>().join(" ");
    Line::styled(trunc(&format!("✖ {clean}"), w), fg(c_error()))
}

fn save_to(path: &Path, prompts: &[Prompt]) -> Result<(), String> {
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
    }
    let text = toml::to_string_pretty(&Store {
        prompts: prompts.to_vec(),
    })
    .map_err(|e| e.to_string())?;
    std::fs::write(path, text).map_err(|e| e.to_string())
}

fn editor_with(s: &str) -> Editor {
    let mut e = Editor::default();
    s.chars().for_each(|c| e.insert(c));
    e
}

pub(super) enum Action {
    None,
    Close,
    /// Insert the selected prompt into the current tab's editor.
    Apply(String),
}

enum Mode {
    List,
    Form {
        editing: Option<usize>,
        in_body: bool,
        name: Editor,
        body: Editor,
    },
}

pub(super) struct Menu {
    path: Option<PathBuf>,
    prompts: Vec<Prompt>,
    sel: usize,
    draft: String,
    mode: Mode,
    error: Option<String>,
    /// Prevents overwriting a file that failed to load/parse.
    load_failed: bool,
}

impl Menu {
    pub(super) fn open(draft: String) -> Self {
        Self::with_path(default_path(), draft, false)
    }

    pub(super) fn save_draft(draft: String) -> Self {
        Self::with_path(default_path(), draft, true)
    }

    fn with_path(path: Option<PathBuf>, draft: String, start_form: bool) -> Self {
        let (prompts, error, load_failed) = match path.as_deref().map(load_from) {
            Some(Ok(p)) => (p, None, false),
            Some(Err(e)) => (vec![], Some(e), true),
            None => (vec![], None, false),
        };
        let mode = if start_form && !load_failed {
            Mode::Form {
                editing: None,
                in_body: false,
                name: Editor::default(),
                body: editor_with(&draft),
            }
        } else {
            Mode::List
        };
        Menu {
            path,
            prompts,
            sel: 0,
            draft,
            mode,
            error,
            load_failed,
        }
    }

    fn persist(&mut self) -> Result<(), String> {
        if self.load_failed {
            return Err(self.error.clone().unwrap_or_else(|| {
                tr(
                    "error al cargar prompts.toml",
                    "failed to load prompts.toml",
                )
                .into()
            }));
        }
        if let Some(p) = &self.path {
            save_to(p, &self.prompts)?;
        }
        Ok(())
    }

    pub(super) fn on_key(&mut self, k: KeyEvent) -> Action {
        match &self.mode {
            Mode::List => self.on_list_key(k.code),
            Mode::Form { .. } => {
                self.on_form_key(k);
                Action::None
            }
        }
    }

    fn on_list_key(&mut self, code: KeyCode) -> Action {
        match code {
            KeyCode::Esc | KeyCode::Char('q' | 'p') => return Action::Close,
            KeyCode::Up | KeyCode::Char('k') => self.sel = self.sel.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => {
                self.sel = (self.sel + 1).min(self.prompts.len().saturating_sub(1));
            }
            KeyCode::Enter => {
                if let Some(p) = self.prompts.get(self.sel) {
                    return Action::Apply(p.body.clone());
                }
            }
            KeyCode::Char('n') if !self.load_failed => {
                self.error = None;
                self.mode = Mode::Form {
                    editing: None,
                    in_body: false,
                    name: Editor::default(),
                    body: Editor::default(),
                };
            }
            KeyCode::Char('s') if !self.load_failed => {
                if self.draft.trim().is_empty() {
                    self.error = Some(tr("el borrador está vacío", "draft is empty").into());
                } else {
                    self.error = None;
                    self.mode = Mode::Form {
                        editing: None,
                        in_body: false,
                        name: Editor::default(),
                        body: editor_with(&self.draft),
                    };
                }
            }
            KeyCode::Char('e') => {
                if let Some(p) = self.prompts.get(self.sel) {
                    self.error = None;
                    self.mode = Mode::Form {
                        editing: Some(self.sel),
                        in_body: false,
                        name: editor_with(&p.name),
                        body: editor_with(&p.body),
                    };
                }
            }
            KeyCode::Char('d') | KeyCode::Delete if self.sel < self.prompts.len() => {
                let removed = self.prompts.remove(self.sel);
                match self.persist() {
                    Ok(()) => {
                        self.error = None;
                        self.sel = self.sel.min(self.prompts.len().saturating_sub(1));
                    }
                    Err(e) => {
                        self.prompts.insert(self.sel, removed);
                        self.error = Some(e);
                    }
                }
            }
            _ => {}
        }
        Action::None
    }

    fn on_form_key(&mut self, k: KeyEvent) {
        let (code, mods) = (k.code, k.modifiers);
        let ctrl = mods.contains(KeyModifiers::CONTROL);
        let alt = mods.contains(KeyModifiers::ALT);
        let Mode::Form {
            editing,
            in_body,
            name,
            body,
        } = &mut self.mode
        else {
            return;
        };
        match code {
            KeyCode::Esc => {
                self.error = None;
                self.mode = Mode::List;
            }
            KeyCode::Tab | KeyCode::BackTab => *in_body = !*in_body,
            KeyCode::Enter if *in_body && (alt || mods.contains(KeyModifiers::SHIFT)) => {
                body.insert('\n');
            }
            KeyCode::Char('j') if *in_body && ctrl => body.insert('\n'),
            KeyCode::Enter if !*in_body && body.text.trim().is_empty() => *in_body = true,
            KeyCode::Enter => {
                let title = name.text.trim().to_string();
                let text = body.text.trim().to_string();
                if title.is_empty() {
                    self.error = Some(tr("escribe un nombre", "enter a name").into());
                    *in_body = false;
                    return;
                }
                if text.is_empty() {
                    self.error =
                        Some(tr("escribe el texto del prompt", "enter the prompt text").into());
                    *in_body = true;
                    return;
                }
                let idx = *editing;
                let entry = Prompt {
                    name: title,
                    body: text,
                };
                let prev = match idx {
                    Some(i) if i < self.prompts.len() => {
                        Some((i, std::mem::replace(&mut self.prompts[i], entry)))
                    }
                    _ => {
                        self.prompts.push(entry);
                        None
                    }
                };
                match self.persist() {
                    Ok(()) => {
                        self.sel = prev.map_or(self.prompts.len() - 1, |(i, _)| i);
                        self.error = None;
                        self.mode = Mode::List;
                    }
                    Err(e) => {
                        match prev {
                            Some((i, old)) => self.prompts[i] = old,
                            None => {
                                self.prompts.pop();
                            }
                        }
                        self.error = Some(e);
                    }
                }
            }
            _ => {
                let ed = if *in_body { body } else { name };
                match code {
                    KeyCode::Char('w') if ctrl => ed.kill_word(),
                    KeyCode::Char('u') if ctrl => ed.kill_line(),
                    KeyCode::Char(c) if !ctrl && !alt => ed.insert(c),
                    KeyCode::Backspace => ed.backspace(),
                    KeyCode::Delete => ed.delete(),
                    KeyCode::Left => ed.left(),
                    KeyCode::Right => ed.right(),
                    KeyCode::Home => ed.home(),
                    KeyCode::End => ed.end(),
                    _ => {}
                }
            }
        }
    }

    pub(super) fn draw(&self, f: &mut Frame, area: Rect) {
        match &self.mode {
            Mode::List => self.draw_list(f, area),
            Mode::Form {
                editing,
                in_body,
                name,
                body,
            } => self.draw_form(f, area, editing.is_some(), *in_body, name, body),
        }
    }

    fn draw_list(&self, f: &mut Frame, area: Rect) {
        let w = area.width.min(78);
        let inner_w = w.saturating_sub(4) as usize;
        let preview: Vec<Line<'static>> = self
            .prompts
            .get(self.sel)
            .map(|p| {
                p.body
                    .lines()
                    .flat_map(|l| wrap(&Line::styled(l.to_string(), fg(c_dim())), inner_w))
                    .take(4)
                    .collect()
            })
            .unwrap_or_default();
        let footer = wrap(
            &Line::styled(
                tr(
                    "↑↓ elegir · Enter aplicar · n nuevo · s borrador · e editar · d borrar · Esc cerrar",
                    "↑↓ select · Enter apply · n new · s draft · e edit · d delete · Esc close",
                ),
                fg(c_dim()),
            ),
            inner_w,
        );
        let overhead = 4
            + usize::from(!preview.is_empty())
            + preview.len()
            + 1
            + footer.len()
            + usize::from(self.error.is_some());
        let list_max = (area.height as usize).saturating_sub(overhead).clamp(1, 10);
        let list_rows = self.prompts.len().clamp(1, list_max);
        let top = (self.sel + 1).saturating_sub(list_rows);
        let mut lines: Vec<Line<'static>> = if self.prompts.is_empty() {
            vec![Line::styled(
                tr(
                    "(sin prompts guardados — pulsa n para crear)",
                    "(no saved prompts — press n to create)",
                ),
                fg(c_dim()),
            )]
        } else {
            self.prompts
                .iter()
                .enumerate()
                .skip(top)
                .take(list_rows)
                .map(|(i, p)| {
                    let first = p.body.lines().next().unwrap_or("");
                    let row = trunc(&format!("{} — {first}", p.name), inner_w.saturating_sub(2));
                    if i == self.sel {
                        Line::styled(format!("▶ {row}"), bold(c_bg()).bg(c_accent()))
                    } else {
                        Line::styled(format!("  {row}"), fg(c_fg()))
                    }
                })
                .collect()
        };
        if !preview.is_empty() {
            lines.push(Line::styled("─".repeat(inner_w), fg(c_border())));
            lines.extend(preview);
        }
        lines.push(Line::default());
        lines.extend(footer);
        if let Some(e) = &self.error {
            lines.push(error_line(e, inner_w));
        }
        let h = (lines.len() as u16 + 2).min(area.height.saturating_sub(2));
        let r = centered(area, w, h);
        f.render_widget(Clear, r);
        f.render_widget(
            Paragraph::new(lines).block(
                block(
                    Line::styled(
                        format!(" {} ", tr("Prompts guardados", "Saved prompts")),
                        bold(c_accent()),
                    ),
                    c_accent(),
                )
                .padding(Padding::horizontal(1)),
            ),
            r,
        );
    }

    fn draw_form(
        &self,
        f: &mut Frame,
        area: Rect,
        editing: bool,
        in_body: bool,
        name: &Editor,
        body: &Editor,
    ) {
        let w = area.width.min(78);
        let err_h = u16::from(self.error.is_some());
        let h = (14 + err_h).min(area.height.saturating_sub(2));
        let r = centered(area, w, h);
        f.render_widget(Clear, r);
        let title = if editing {
            tr(" Editar prompt ", " Edit prompt ")
        } else {
            tr(" Guardar prompt ", " Save prompt ")
        };
        let outer = block(Line::styled(title, bold(c_accent())), c_accent())
            .padding(Padding::horizontal(1));
        let inner = outer.inner(r);
        f.render_widget(outer, r);

        let [name_a, body_a, hint_a, err_a] = Layout::vertical([
            Constraint::Length(3),
            Constraint::Min(3),
            Constraint::Length(1),
            Constraint::Length(err_h),
        ])
        .areas(inner);

        let name_cur = draw_field(
            f,
            name_a,
            tr(" Nombre ", " Name "),
            tr("Nombre corto…", "Short name…"),
            name,
            !in_body,
        );
        let body_cur = draw_field(
            f,
            body_a,
            " Prompt ",
            tr("Texto del prompt…", "Prompt text…"),
            body,
            in_body,
        );
        f.render_widget(
            Paragraph::new(Line::styled(
                tr(
                    "Tab campo · Enter guardar · Alt+Enter nueva línea · Esc volver",
                    "Tab field · Enter save · Alt+Enter newline · Esc back",
                ),
                fg(c_dim()),
            )),
            hint_a,
        );
        if let Some(e) = &self.error {
            f.render_widget(Paragraph::new(error_line(e, err_a.width as usize)), err_a);
        }
        if let Some(pos) = if in_body { body_cur } else { name_cur } {
            f.set_cursor_position(pos);
        }
    }
}

fn draw_field(
    f: &mut Frame,
    area: Rect,
    title: &'static str,
    placeholder: &'static str,
    ed: &Editor,
    focused: bool,
) -> Option<(u16, u16)> {
    let blk = block(
        Line::styled(title, bold(if focused { c_ok() } else { c_dim() })),
        if focused { c_ok() } else { c_border() },
    );
    let inner = blk.inner(area);
    let (rows, (cr, cc)) = ed.layout(inner.width as usize);
    let h = inner.height as usize;
    let top = (cr + 1).saturating_sub(h);
    let lines: Vec<Line<'static>> = if ed.is_empty() {
        vec![Line::styled(placeholder, fg(c_dim()))]
    } else {
        rows.into_iter().skip(top).take(h).map(Line::raw).collect()
    };
    f.render_widget(Paragraph::new(lines).block(blk), area);
    (inner.width > 0 && inner.height > 0)
        .then(|| (inner.x + cc as u16, inner.y + (cr - top) as u16))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn type_str(m: &mut Menu, s: &str) {
        for c in s.chars() {
            m.on_key(key(KeyCode::Char(c)));
        }
    }

    #[test]
    fn create_persist_apply_edit_and_delete() {
        let dir = std::env::temp_dir().join(format!("hive-prompts-{}", std::process::id()));
        let file = dir.join("prompts.toml");
        let _ = std::fs::remove_dir_all(&dir);

        // Create and persist
        let mut m = Menu::with_path(Some(file.clone()), String::new(), false);
        m.on_key(key(KeyCode::Char('n')));
        type_str(&mut m, "Review");
        m.on_key(key(KeyCode::Enter)); // empty body -> moves focus to body
        type_str(&mut m, "Check diff");
        m.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT));
        type_str(&mut m, "find bugs");
        m.on_key(key(KeyCode::Enter)); // saves
        assert_eq!(m.prompts.len(), 1);
        assert_eq!(m.prompts[0].body, "Check diff\nfind bugs");

        // Reload in a fresh menu (simulates another project/session) and apply
        let mut m2 = Menu::with_path(Some(file.clone()), String::new(), false);
        assert_eq!(m2.prompts, m.prompts);
        match m2.on_key(key(KeyCode::Enter)) {
            Action::Apply(b) => assert_eq!(b, "Check diff\nfind bugs"),
            _ => panic!("expected Apply"),
        }

        // Edit existing
        m2.on_key(key(KeyCode::Char('e')));
        m2.on_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
        type_str(&mut m2, "PR Review");
        m2.on_key(key(KeyCode::Enter));
        assert_eq!(m2.prompts[0].name, "PR Review");

        // Delete and verify persisted
        m2.on_key(key(KeyCode::Char('d')));
        assert!(m2.prompts.is_empty());
        assert!(load_from(&file).unwrap().is_empty());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_draft_prefills_body_and_saves_on_enter() {
        let mut m = Menu::with_path(None, "Explain this module".into(), true);
        type_str(&mut m, "Explain");
        m.on_key(key(KeyCode::Enter));
        assert_eq!(m.prompts.len(), 1);
        assert_eq!(m.prompts[0].name, "Explain");
        assert_eq!(m.prompts[0].body, "Explain this module");
    }

    #[test]
    fn corrupt_file_is_not_overwritten() {
        let dir = std::env::temp_dir().join(format!("hive-prompts-bad-{}", std::process::id()));
        let file = dir.join("prompts.toml");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&file, "not valid toml [[[").unwrap();

        let mut m = Menu::with_path(Some(file.clone()), "draft".into(), false);
        assert!(m.load_failed);
        let err = m.error.as_deref().unwrap();
        assert!(!err.contains('\n'));

        // n and s stay in List mode and keep the error visible
        m.on_key(key(KeyCode::Char('n')));
        assert!(matches!(m.mode, Mode::List) && m.error.is_some());
        m.on_key(key(KeyCode::Char('s')));
        assert!(matches!(m.mode, Mode::List) && m.error.is_some());
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "not valid toml [[["
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn form_validation_utf8_and_render_smoke() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let render = |m: &Menu, w: u16, h: u16| -> String {
            let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
            term.draw(|f| m.draw(f, f.area())).unwrap();
            let buf = term.backend().buffer();
            (0..h)
                .map(|y| (0..w).map(|x| buf[(x, y)].symbol()).collect::<String>())
                .collect::<Vec<_>>()
                .join("\n")
        };

        let mut m = Menu::with_path(None, String::new(), false);
        // s with empty draft sets error and stays in list
        m.on_key(key(KeyCode::Char('s')));
        assert!(m.error.is_some());
        assert!(matches!(m.mode, Mode::List));

        // n -> empty name blocked, Esc cancels without saving
        m.on_key(key(KeyCode::Char('n')));
        m.on_key(key(KeyCode::Tab));
        type_str(&mut m, "cuerpo ñáé 日本語");
        m.on_key(key(KeyCode::Enter));
        assert!(m.error.is_some());
        assert!(matches!(m.mode, Mode::Form { .. }));
        let form_screen = render(&m, 60, 18);
        assert!(form_screen.contains("cuerpo ñáé"));

        m.on_key(key(KeyCode::Esc));
        assert!(matches!(m.mode, Mode::List));
        assert!(m.prompts.is_empty());

        // Populate multiple UTF-8 multiline prompts and render at 100x30 and 60x18
        for i in 0..12 {
            m.on_key(key(KeyCode::Char('n')));
            type_str(&mut m, &format!("Reseña {i} 🚀"));
            m.on_key(key(KeyCode::Tab));
            type_str(&mut m, "línea uno\nlínea dos\nlínea tres\nlínea cuatro");
            m.on_key(key(KeyCode::Enter));
        }
        assert_eq!(m.prompts.len(), 12);
        for _ in 0..15 {
            m.on_key(key(KeyCode::Down));
        }
        assert_eq!(m.sel, 11);
        let list_wide = render(&m, 100, 30);
        assert!(list_wide.contains("Reseña 11"));
        let list_min = render(&m, 60, 18);
        assert!(list_min.contains("Reseña 11"));
    }

    #[test]
    fn list_60x18_keeps_wrapped_footer_and_error_and_tiny_form_does_not_panic() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let render = |m: &Menu, w: u16, h: u16| -> String {
            let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
            term.draw(|f| m.draw(f, f.area())).unwrap();
            let buf = term.backend().buffer();
            (0..h)
                .map(|y| (0..w).map(|x| buf[(x, y)].symbol()).collect::<String>())
                .collect::<Vec<_>>()
                .join("\n")
        };

        let mut m = Menu::with_path(None, String::new(), false);
        for i in 0..10 {
            m.prompts.push(Prompt {
                name: format!("Prompt {i}"),
                body: "line 1\nline 2\nline 3\nline 4".into(),
            });
        }
        m.error = Some("line one\nline two".into());
        let screen = render(&m, 60, 18);
        assert!(screen.contains("Esc"), "footer clipped:\n{screen}");
        assert!(
            screen.contains("✖ line one line two"),
            "error clipped:\n{screen}"
        );

        // Empty name while focused on body returns focus to name
        m.error = None;
        m.on_key(key(KeyCode::Char('n')));
        m.on_key(key(KeyCode::Tab));
        type_str(&mut m, "body");
        m.on_key(key(KeyCode::Enter));
        assert!(matches!(m.mode, Mode::Form { in_body: false, .. }));

        // Tiny terminal (inner.height == 0 in draw_field) must not panic
        let _ = render(&m, 20, 4);
    }
}
