//! UI settings (theme, language): process-wide state, persistence and the settings menu.
use super::theme::*;
use super::widgets::{block, centered};
use ratatui::Frame;
use ratatui::crossterm::event::KeyCode;
use ratatui::layout::Rect;
use ratatui::text::Line;
use ratatui::widgets::{Clear, Padding, Paragraph};
use serde::Deserialize;
use std::cell::Cell;
use std::path::PathBuf;

// Thread-local: all drawing happens on the UI thread, and tests stay isolated.
thread_local! {
    static LIGHT: Cell<bool> = const { Cell::new(false) };
    static EN: Cell<bool> = const { Cell::new(false) };
}

pub fn light() -> bool {
    LIGHT.get()
}

pub fn is_en() -> bool {
    EN.get()
}

/// Spanish / English literal by current language.
pub fn tr(es: &'static str, en: &'static str) -> &'static str {
    if is_en() { en } else { es }
}

#[derive(Deserialize, Default)]
struct Saved {
    theme: Option<String>,
    lang: Option<String>,
}

fn path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join(".config/copilot-hive/settings.toml"))
}

pub fn load() {
    let saved: Saved = path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|t| toml::from_str(&t).ok())
        .unwrap_or_default();
    LIGHT.set(saved.theme.as_deref() == Some("light"));
    EN.set(saved.lang.as_deref() == Some("en"));
}

fn save() -> std::io::Result<()> {
    let Some(p) = path().filter(|_| !cfg!(test)) else {
        return Ok(());
    };
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d)?;
    }
    let theme = if light() { "light" } else { "dark" };
    let lang = if is_en() { "en" } else { "es" };
    std::fs::write(p, format!("theme = \"{theme}\"\nlang = \"{lang}\"\n"))
}

const ROWS: usize = 2;

pub(super) struct Menu {
    sel: usize,
    /// Last save error.
    error: Option<String>,
}

impl Menu {
    pub(super) fn new() -> Self {
        Menu {
            sel: 0,
            error: None,
        }
    }

    /// Returns true to close the menu.
    pub(super) fn on_key(&mut self, code: KeyCode) -> bool {
        match code {
            KeyCode::Esc | KeyCode::Char('q' | 'S') => return true,
            KeyCode::Up | KeyCode::Char('k') => self.sel = self.sel.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => self.sel = (self.sel + 1).min(ROWS - 1),
            KeyCode::Enter | KeyCode::Left | KeyCode::Right | KeyCode::Char(' ') => {
                if self.sel == 0 {
                    LIGHT.set(!light());
                } else {
                    EN.set(!is_en());
                }
                self.error = save().err().map(|e| e.to_string());
            }
            _ => {}
        }
        false
    }

    pub(super) fn draw(&self, f: &mut Frame, area: Rect) {
        let value = |s: &'static str| format!("‹ {s} ›");
        let rows = [
            (
                tr("Tema", "Theme"),
                value(if light() {
                    tr("Claro", "Light")
                } else {
                    tr("Oscuro", "Dark")
                }),
            ),
            (
                tr("Idioma", "Language"),
                value(if is_en() { "English" } else { "Español" }),
            ),
        ];
        let mut lines: Vec<Line<'static>> = rows
            .iter()
            .enumerate()
            .map(|(i, (k, v))| {
                let text = format!("{k:<10}{v}");
                if i == self.sel {
                    Line::styled(format!("▶ {text}"), bold(c_bg()).bg(c_accent()))
                } else {
                    Line::styled(format!("  {text}"), fg(c_fg()))
                }
            })
            .collect();
        lines.push(Line::default());
        lines.push(Line::styled(
            tr(
                "↑↓ elegir · Enter/←→ cambiar · Esc cerrar",
                "↑↓ select · Enter/←→ change · Esc close",
            ),
            fg(c_dim()),
        ));
        if let Some(e) = &self.error {
            lines.push(Line::styled(format!("✖ {e}"), fg(c_error())));
        }
        let r = centered(area, 52, lines.len() as u16 + 2);
        f.render_widget(Clear, r);
        f.render_widget(
            Paragraph::new(lines).block(
                block(
                    Line::styled(format!(" {} ", tr("Ajustes", "Settings")), bold(c_accent())),
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
    fn rows_toggle_theme_and_language() {
        let mut m = Menu::new();
        let (l, e) = (light(), is_en());
        m.on_key(KeyCode::Enter);
        assert_eq!((light(), is_en()), (!l, e));
        m.on_key(KeyCode::Down);
        m.on_key(KeyCode::Right);
        assert_eq!((light(), is_en()), (!l, !e));
        m.on_key(KeyCode::Enter);
        m.on_key(KeyCode::Up);
        m.on_key(KeyCode::Enter);
        assert_eq!((light(), is_en()), (l, e));
        assert!(m.on_key(KeyCode::Esc));
    }
}
