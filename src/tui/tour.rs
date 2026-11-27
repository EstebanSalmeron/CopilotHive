//! Interactive tutorial (`t`): paged slides, each with a small animated demo.
use super::settings::tr;
use super::theme::*;
use super::widgets::{block, centered};
use ratatui::Frame;
use ratatui::crossterm::event::KeyCode;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Padding, Paragraph};

const PAGES: usize = 7;

pub struct Tour {
    page: usize,
    /// Tick at which the current page was shown: demos animate relative to it.
    t0: u64,
}

impl Tour {
    pub fn new(tick: u64) -> Self {
        Self { page: 0, t0: tick }
    }

    /// Returns true when the tour should close.
    pub fn on_key(&mut self, code: KeyCode, tick: u64) -> bool {
        let page = match code {
            KeyCode::Esc | KeyCode::Char('q' | 't') => return true,
            KeyCode::Right | KeyCode::Char('l' | ' ') | KeyCode::Enter => {
                if self.page + 1 == PAGES {
                    return true;
                }
                self.page + 1
            }
            KeyCode::Left | KeyCode::Char('h') => self.page.saturating_sub(1),
            KeyCode::Char(c @ '1'..='7') => c as usize - '1' as usize,
            _ => self.page,
        };
        if page != self.page {
            self.page = page;
            self.t0 = tick;
        }
        false
    }

    pub fn draw(&self, f: &mut Frame, area: Rect, tick: u64) {
        let local = tick.saturating_sub(self.t0);
        let (title, bullets, demo) = slide(self.page, local);
        let mut lines: Vec<Line<'static>> = vec![Line::styled(format!(" {title}"), bold(c_fg()))];
        lines.push(Line::default());
        for (k, d) in bullets {
            lines.push(Line::from(vec![
                Span::styled(format!(" {k:<14}"), bold(c_accent())),
                Span::styled(d, fg(c_fg())),
            ]));
        }
        lines.push(Line::default());
        lines.push(Line::styled(format!(" {:─<68}", "─ demo "), fg(c_border())));
        lines.extend(demo);
        let r = centered(area, 76, 22);
        f.render_widget(Clear, r);
        let dots: String = (0..PAGES)
            .map(|i| if i == self.page { '●' } else { '○' })
            .collect();
        f.render_widget(
            Paragraph::new(lines).block(
                block(
                    Line::styled(
                        format!(" {} {}/{PAGES} ", tr("Tutorial", "Tour"), self.page + 1),
                        bold(c_accent()),
                    ),
                    c_accent(),
                )
                .title_bottom(Line::styled(
                    format!(
                        " {dots}  ←→ {}  1-{PAGES} {}  Esc {} ",
                        tr("navegar", "navigate"),
                        tr("ir a", "go to"),
                        tr("cerrar", "close")
                    ),
                    fg(c_dim()),
                ))
                .padding(Padding::horizontal(1)),
            ),
            r,
        );
    }
}

type Slide = (
    &'static str,
    Vec<(&'static str, &'static str)>,
    Vec<Line<'static>>,
);

fn slide(page: usize, t: u64) -> Slide {
    match page {
        0 => (
            tr(
                "Un proyecto, una pestaña, una sesión de Copilot",
                "One project, one tab, one Copilot session",
            ),
            vec![
                (
                    "Tab / Alt+1..9",
                    tr(
                        "cambiar de proyecto del grupo (el borrador se conserva)",
                        "switch project within the group (drafts are kept)",
                    ),
                ),
                (
                    "o",
                    tr(
                        "abrir otra carpeta en una pestaña nueva",
                        "open another folder in a new tab",
                    ),
                ),
                ("x", tr("cerrar la pestaña actual", "close the current tab")),
                (
                    "!",
                    tr(
                        "saltar al proyecto con un permiso pendiente",
                        "jump to the project with a pending permission",
                    ),
                ),
            ],
            demo_tabs(t),
        ),
        1 => (
            tr(
                "Grupos y sesiones multiproyecto",
                "Groups and multi-project sessions",
            ),
            vec![
                (
                    "{ / }",
                    tr(
                        "cambiar de grupo (`group` en projects.toml)",
                        "switch group (`group` in projects.toml)",
                    ),
                ),
                (
                    "g",
                    tr(
                        "una sesión con varios proyectos (--add-dir)",
                        "one session across several projects (--add-dir)",
                    ),
                ),
                (
                    "Espacio a g n",
                    tr(
                        "elegir · todos · grupo · alias; Enter inicia",
                        "pick · all · group · alias; Enter starts",
                    ),
                ),
                (
                    "Tab",
                    tr(
                        "en el selector: sesiones guardadas (sessions.toml)",
                        "in the picker: saved sessions (sessions.toml)",
                    ),
                ),
            ],
            demo_groups(t),
        ),
        2 => (
            tr("Escribe prompts", "Write prompts"),
            vec![
                (
                    "Enter / i",
                    tr(
                        "empezar a escribir; Enter envía",
                        "start typing; Enter sends",
                    ),
                ),
                (
                    "Alt+Enter",
                    tr("nueva línea (también Ctrl+J)", "newline (also Ctrl+J)"),
                ),
                (
                    "Ctrl+V",
                    tr(
                        "pegar una imagen del portapapeles (macOS)",
                        "paste a clipboard image (macOS)",
                    ),
                ),
                (
                    "p / Ctrl+S",
                    tr(
                        "prompts guardados: reutiliza y guarda borradores",
                        "saved prompts: reuse and save drafts",
                    ),
                ),
                (
                    "Esc / Ctrl+C",
                    tr("cancelar el turno en curso", "cancel the running turn"),
                ),
            ],
            demo_typing(t),
        ),
        3 => (
            tr(
                "Tú decides qué puede hacer Copilot",
                "You decide what Copilot may do",
            ),
            vec![
                (
                    "y / n",
                    tr(
                        "aprobar / rechazar el permiso pedido",
                        "approve / reject the requested permission",
                    ),
                ),
                (
                    "a",
                    tr(
                        "aprobar siempre este tipo de acción",
                        "always approve this kind of action",
                    ),
                ),
                (
                    "A",
                    tr(
                        "allow all: aprueba todo (pide confirmación; peligroso)",
                        "allow all: approve everything (asks first; dangerous)",
                    ),
                ),
            ],
            demo_perm(t),
        ),
        4 => (
            tr(
                "Agentes y subagentes en tiempo real",
                "Agents and subagents in real time",
            ),
            vec![
                (
                    "f",
                    tr(
                        "/fleet: Copilot reparte el trabajo en subagentes paralelos",
                        "/fleet: Copilot splits the work across parallel subagents",
                    ),
                ),
                (
                    "d",
                    tr(
                        "dashboard: estado, tool calls, plan y créditos",
                        "dashboard: status, tool calls, plan and credits",
                    ),
                ),
                (
                    "s",
                    tr(
                        "vista completa de subagentes; m fija su modelo 🔒",
                        "full subagent view; m pins its model 🔒",
                    ),
                ),
                (
                    "] / [",
                    tr(
                        "cambiar el foco entre paneles (o usa el ratón)",
                        "move focus between panes (or use the mouse)",
                    ),
                ),
            ],
            demo_fleet(t),
        ),
        5 => (
            tr("Ajusta la sesión a tu medida", "Tune the session"),
            vec![
                (
                    "m / e",
                    tr("modelo / reasoning effort", "model / reasoning effort"),
                ),
                (
                    "M",
                    tr(
                        "activar o desactivar servidores MCP del proyecto",
                        "enable or disable the project's MCP servers",
                    ),
                ),
                (
                    "H",
                    tr(
                        "historial: ver o reanudar sesiones anteriores",
                        "history: browse or resume past sessions",
                    ),
                ),
                (
                    "S",
                    tr(
                        "ajustes: tema oscuro/claro e idioma",
                        "settings: dark/light theme and language",
                    ),
                ),
            ],
            demo_models(t),
        ),
        _ => (
            tr("Todo listo", "All set"),
            vec![
                ("?", tr("ayuda con todas las teclas", "help with every key")),
                ("t", tr("volver a ver este tutorial", "replay this tour")),
                (
                    "q",
                    tr(
                        "salir (confirma si hay agentes trabajando)",
                        "quit (asks if agents are working)",
                    ),
                ),
            ],
            demo_end(t),
        ),
    }
}

fn demo_tabs(t: u64) -> Vec<Line<'static>> {
    let names = ["backend", "web", "docs"];
    let on = (t / 15) as usize % names.len();
    let mut spans = vec![Span::raw(" ")];
    for (i, n) in names.iter().enumerate() {
        let (label, style) = if i == on {
            (format!(" {} {n} ", spinner(t)), bold(c_accent()))
        } else {
            (format!(" · {n} "), fg(c_dim()))
        };
        spans.push(Span::styled(label, style));
        spans.push(Span::raw(" "));
    }
    vec![
        Line::default(),
        Line::from(spans),
        Line::styled(format!("   cwd = ~/projects/{}", names[on]), fg(c_dim())),
    ]
}

fn demo_groups(t: u64) -> Vec<Line<'static>> {
    let groups = ["Core 26", "UI 5", "BFF 5", "Custom Grouping 1"];
    let on = (t / 15) as usize % groups.len();
    let mut row = vec![Span::raw(" ")];
    for (i, g) in groups.iter().enumerate() {
        let style = if i == on {
            bold(c_bg()).bg(c_accent())
        } else {
            fg(c_dim())
        };
        row.push(Span::styled(format!(" {g} "), style));
        row.push(Span::raw(" "));
    }
    let names = ["api", "web", "docs"];
    let picked = (t / 12) as usize % 4;
    let mut picks = vec![Span::raw(" ")];
    for (i, n) in names.iter().enumerate() {
        let (mark, style) = if i < picked {
            (format!("[{}]", i + 1), bold(c_fg()))
        } else {
            ("[ ]".into(), fg(c_dim()))
        };
        picks.push(Span::styled(format!(" {mark} {n} "), style));
    }
    let result = if picked >= 2 {
        let joined = names[..picked].join(" + ");
        Line::styled(
            format!("   → {joined}  (cwd = {}, --add-dir …)", names[0]),
            fg(c_ok()),
        )
    } else {
        Line::styled(
            tr(
                "   elige al menos 2 proyectos",
                "   pick at least 2 projects",
            ),
            fg(c_dim()),
        )
    };
    vec![Line::from(row), Line::from(picks), result]
}

fn typed(text: &'static str, t: u64) -> (String, bool) {
    let n = text.chars().count();
    let k = (t as usize / 2) % (n + 25);
    (text.chars().take(k).collect(), k >= n)
}

fn demo_typing(t: u64) -> Vec<Line<'static>> {
    let (s, done) = typed(
        tr(
            "Añade tests al módulo de config",
            "Add tests to the config module",
        ),
        t,
    );
    let cursor = if blink_on(t) { "▏" } else { " " };
    let mut v = vec![
        Line::default(),
        Line::from(vec![
            Span::styled(" ❯ ", bold(c_accent())),
            Span::styled(s, fg(c_fg())),
            Span::styled(cursor, fg(c_accent())),
        ]),
    ];
    if done {
        v.push(Line::from(vec![
            Span::styled(format!(" {} ", spinner(t)), fg(c_running())),
            Span::styled(
                tr("Copilot está trabajando…", "Copilot is working…"),
                fg(c_dim()),
            ),
        ]));
    }
    v
}

fn demo_perm(t: u64) -> Vec<Line<'static>> {
    let approved = (t / 20) % 2 == 1;
    let c = if blink_on(t) {
        c_attention()
    } else {
        c_orange()
    };
    if approved {
        return vec![
            Line::default(),
            Line::styled(" ✔ cargo test --all", fg(c_ok())),
            Line::styled(tr("   aprobado con y", "   approved with y"), fg(c_dim())),
        ];
    }
    vec![
        Line::default(),
        Line::styled(
            tr(
                " ⚠ Copilot quiere ejecutar: cargo test --all",
                " ⚠ Copilot wants to run: cargo test --all",
            ),
            bold(c),
        ),
        Line::from(vec![
            Span::styled("   [y] ", bold(c_ok())),
            Span::styled(tr("aprobar  ", "approve  "), fg(c_fg())),
            Span::styled("[n] ", bold(c_error())),
            Span::styled(tr("rechazar  ", "reject  "), fg(c_fg())),
            Span::styled("[a] ", bold(c_purple())),
            Span::styled(tr("siempre", "always"), fg(c_fg())),
        ]),
    ]
}

fn demo_fleet(t: u64) -> Vec<Line<'static>> {
    let (s, _) = typed("/fleet migrar a edición 2024", t);
    let t = t % 80;
    let mut v = vec![
        Line::default(),
        Line::from(vec![
            Span::styled(" ❯ ", bold(c_accent())),
            Span::styled(s, fg(c_fg())),
        ]),
    ];
    // (name, spawn tick, finish tick)
    for (name, a, b) in [
        ("explore", 30, 50),
        ("developer", 34, 62),
        ("tester", 38, 74),
    ] {
        if t < a {
            continue;
        }
        let (icon, st) = if t >= b {
            ("✔".to_string(), fg(c_ok()))
        } else {
            (spinner(t).to_string(), fg(c_running()))
        };
        v.push(Line::from(vec![
            Span::styled(format!("   {icon} "), st),
            Span::styled(name, fg(c_fg())),
        ]));
    }
    v
}

fn demo_models(t: u64) -> Vec<Line<'static>> {
    let models = ["claude-sonnet-4.5", "gpt-5.3-codex", "claude-haiku-4.5"];
    let on = (t / 12) as usize % models.len();
    let mut v = vec![Line::styled(
        tr(" Modelo de la sesión (m):", " Session model (m):"),
        fg(c_dim()),
    )];
    for (i, m) in models.iter().enumerate() {
        v.push(if i == on {
            Line::styled(format!("   ▶ {m}"), bold(c_accent()))
        } else {
            Line::styled(format!("     {m}"), fg(c_dim()))
        });
    }
    v
}

fn demo_end(t: u64) -> Vec<Line<'static>> {
    let c = if blink_on(t) { c_accent() } else { c_purple() };
    vec![
        Line::default(),
        Line::styled(
            tr(
                "   ✨ Pulsa Enter para empezar a trabajar",
                "   ✨ Press Enter to start working",
            ),
            bold(c),
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn navigates_and_closes() {
        let mut t = Tour::new(0);
        assert!(!t.on_key(KeyCode::Left, 1));
        assert_eq!(t.page, 0);
        assert!(!t.on_key(KeyCode::Char('4'), 5));
        assert_eq!((t.page, t.t0), (3, 5));
        assert!(!t.on_key(KeyCode::Char('7'), 5));
        assert_eq!(t.page, 6);
        assert!(!t.on_key(KeyCode::Char('6'), 5));
        assert!(!t.on_key(KeyCode::Right, 6));
        assert!(t.on_key(KeyCode::Enter, 7), "Enter on the last page closes");
        assert!(Tour::new(0).on_key(KeyCode::Esc, 0));
    }

    #[test]
    fn every_page_renders() {
        for p in 0..PAGES {
            for t in [0, 17, 45, 79, 400] {
                let (title, bullets, demo) = slide(p, t);
                assert!(!title.is_empty() && !bullets.is_empty() && !demo.is_empty());
            }
        }
    }
}
