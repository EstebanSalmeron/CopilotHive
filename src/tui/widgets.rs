//! Drawing: tabs, info bar, transcript, agent sidebar, prompt box, status bar and overlays.
use super::markdown::{render, wrap};
use super::settings::tr;
use super::theme::*;
use super::{App, Cache, Confirm, Dash, Item, Life, Selector, Tab, ToolState, dashboard};
use crate::acp;
use crate::events::Status;
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Paragraph};
use std::borrow::Cow;
use std::time::Duration;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

const MIN_W: u16 = 60;
const MIN_H: u16 = 18;
const SIDEBAR_W: u16 = 50;
const SIDEBAR_MIN_TOTAL: u16 = 100;

pub(super) fn block(title: impl Into<Line<'static>>, border: Color) -> Block<'static> {
    Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(fg(border))
        .title(title)
        .style(Style::new().bg(c_bg()))
}

pub(super) fn trunc(s: &str, w: usize) -> String {
    if s.width() <= w {
        return s.into();
    }
    let (mut out, mut cw) = (String::new(), 0);
    for c in s.chars() {
        let x = c.width().unwrap_or(0);
        if cw + x + 1 > w {
            break;
        }
        out.push(c);
        cw += x;
    }
    out.push('…');
    out
}

pub(super) fn fmt_dur(d: Duration) -> String {
    let s = d.as_secs();
    if s >= 60 {
        format!("{}m{:02}s", s / 60, s % 60)
    } else {
        format!("{:.1}s", d.as_secs_f64())
    }
}

pub(super) fn hints(items: &[(&str, &str)]) -> Line<'static> {
    let mut spans = vec![];
    for (k, d) in items {
        spans.push(Span::styled(format!(" {k} "), bold(c_bg()).bg(c_accent())));
        spans.push(Span::styled(format!(" {d}  "), fg(c_dim())));
    }
    Line::from(spans)
}

pub(super) fn centered(area: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(area.width);
    let h = h.min(area.height);
    Rect::new(
        area.x + (area.width - w) / 2,
        area.y + (area.height - h) / 2,
        w,
        h,
    )
}

fn status_icon(t: &Tab, tick: u64) -> Span<'static> {
    if matches!(t.life, Life::Failed(_)) {
        Span::styled("✖", bold(c_error()))
    } else if t.perm.is_some() {
        let c = if blink_on(tick) {
            c_attention()
        } else {
            c_dim()
        };
        Span::styled("⚠", bold(c))
    } else if t.allow_all {
        Span::styled(
            "⚡",
            bold(if blink_on(tick) {
                c_error()
            } else {
                c_attention()
            }),
        )
    } else if matches!(t.life, Life::Starting) {
        Span::styled(spinner(tick), fg(c_dim()))
    } else if t.working() {
        Span::styled(spinner(tick), bold(c_running()))
    } else {
        Span::styled("●", fg(c_ok()))
    }
}

pub fn draw(app: &mut App, f: &mut Frame) {
    let area = f.area();
    f.render_widget(Block::new().style(Style::new().bg(c_bg()).fg(c_fg())), area);
    if area.width < MIN_W || area.height < MIN_H {
        let msg = tr_fmt!(
            "Terminal demasiado pequeña ({}x{})\nSe necesitan al menos {MIN_W}x{MIN_H}",
            "Terminal too small ({}x{})\nAt least {MIN_W}x{MIN_H} needed",
            area.width,
            area.height
        );
        f.render_widget(
            Paragraph::new(msg)
                .alignment(Alignment::Center)
                .style(bold(c_attention())),
            centered(area, area.width, 2),
        );
        return;
    }
    let tick = app.tick;
    let others_pending = app
        .tabs
        .iter()
        .enumerate()
        .filter(|(i, t)| *i != app.current && t.perm.is_some())
        .count();
    let (dash, help) = (app.dash, app.help);
    draw_tabs(app, f, tick, area);
    let side_lines = (dash == Dash::Side && area.width >= SIDEBAR_MIN_TOTAL)
        .then(|| sidebar_lines(app, SIDEBAR_W as usize - 2, tick));
    let t = &mut app.tabs[app.current];
    let in_w = area.width.saturating_sub(2) as usize;
    let in_rows = t.editor.layout(in_w).0.len().clamp(1, 6) as u16;
    let [_, _, info, body, input, status] = Layout::vertical([
        Constraint::Length(5),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(3),
        Constraint::Length(in_rows + 2),
        Constraint::Length(1),
    ])
    .areas(area);
    draw_info(t, f, info, tick);
    let (main, side) = if dash == Dash::Side && area.width >= SIDEBAR_MIN_TOTAL {
        let [a, b] =
            Layout::horizontal([Constraint::Min(30), Constraint::Length(SIDEBAR_W)]).areas(body);
        (a, Some(b))
    } else {
        (body, None)
    };
    let mut cursor = None;
    if dash == Dash::Full {
        dashboard::draw(t, f, body.union(input), tick);
    } else {
        draw_transcript(t, f, main, tick);
        if let Some(s) = side {
            draw_sidebar(side_lines.unwrap_or_default(), f, s);
        }
        cursor = draw_input(t, f, input);
    }
    draw_status(
        t,
        f,
        status,
        app.confirm.as_ref(),
        dash == Dash::Full,
        others_pending,
    );
    let modal = app.confirm.is_some() || app.selector.is_some();
    if app.confirm == Some(Confirm::AllowAll) {
        draw_allow_all_confirm(f, area);
    } else if let Some(s) = &app.selector {
        draw_selector(s, f, area);
    } else if t.perm.is_some() {
        draw_perm(t, f, area, tick, others_pending);
    } else if let Some(pos) = cursor.filter(|_| t.typing && !help && !modal) {
        f.set_cursor_position(pos);
    }
    if let Some(h) = &mut app.history {
        h.draw(
            f,
            Rect::new(area.x, area.y + 5, area.width, area.height - 5),
        );
    }
    if let Some(m) = &app.settings {
        m.draw(f, area);
    }
    if help {
        draw_help(f, area);
    }
}

fn draw_tabs(app: &App, f: &mut Frame, tick: u64, area: Rect) {
    let mut spans = vec![];
    for (i, t) in app.tabs.iter().enumerate() {
        let active = i == app.current;
        let base = if active {
            bold(c_fg()).bg(c_surface())
        } else {
            fg(c_dim())
        };
        let mut label = vec![
            Span::styled(format!(" {} ", i + 1), fg(c_dim()).patch(base)),
            status_icon(t, tick).patch_style(Style::new().bg(if active {
                c_surface()
            } else {
                c_bg()
            })),
            Span::styled(format!(" {} ", t.name), base),
        ];
        let subs = t.tree.active_subagents();
        if subs > 0 {
            label.push(Span::styled(
                format!("◈{subs} "),
                bold(c_purple()).patch(Style::new().bg(if active { c_surface() } else { c_bg() })),
            ));
        }
        spans.extend(label);
        spans.push(Span::styled("│", fg(c_border())));
    }
    let working = app.tabs.iter().filter(|t| t.working()).count();
    let subs: usize = app.tabs.iter().map(|t| t.tree.active_subagents()).sum();
    let credits: f64 = app
        .tabs
        .iter()
        .map(|t| t.tree.total_tokens().credits())
        .sum();
    let blk = block(Line::default(), c_border());
    let inner = blk.inner(area);
    f.render_widget(blk, area);
    let [left, right] =
        Layout::horizontal([Constraint::Length(11), Constraint::Min(10)]).areas(inner);
    f.render_widget(Paragraph::new(robot(tick, working > 0)), left);
    let summary = Line::from(vec![
        Span::styled(
            tr_fmt!("{} proyectos", "{} projects", app.tabs.len()),
            fg(c_fg()),
        ),
        Span::styled(" · ", fg(c_border())),
        Span::styled(
            tr_fmt!("{working} trabajando", "{working} working"),
            fg(if working > 0 { c_running() } else { c_dim() }),
        ),
        Span::styled(" · ", fg(c_border())),
        Span::styled(
            tr_fmt!("◈{subs} subagentes", "◈{subs} subagents"),
            fg(c_purple()),
        ),
        Span::styled(" · ", fg(c_border())),
        Span::styled(cost(credits), fg(c_orange())),
    ]);
    let title = Line::from(vec![
        Span::styled("◆ COPILOT COCKPIT", bold(c_accent())),
        Span::styled("  centro de mando", fg(c_dim())),
    ]);
    f.render_widget(
        Paragraph::new(vec![title, summary, Line::from(spans)]),
        right,
    );
}

/// Eyes are closed for 2 ticks every 40 (idle) or 15 (working).
pub(super) fn eyes_closed(tick: u64, working: bool) -> bool {
    tick % if working { 15 } else { 40 } < 2
}

/// 3-row robot: fixed eyes that blink; working = color + blinking antenna.
fn robot(tick: u64, working: bool) -> Vec<Line<'static>> {
    let eyes = if eyes_closed(tick, working) {
        "- -"
    } else {
        "◉ ◉"
    };
    let ant = if working && (tick / 2).is_multiple_of(2) {
        "*"
    } else {
        "⌁"
    };
    let c = if working { c_running() } else { c_ok() };
    vec![
        Line::from(vec![Span::raw("   "), Span::styled(ant, fg(c_attention()))]),
        Line::from(vec![
            Span::styled(" ▐", fg(c)),
            Span::styled(format!("{eyes:^5}"), bold(c)),
            Span::styled("▌", fg(c)),
        ]),
        Line::styled(" ╰─▄▄▄─╯", fg(c)),
    ]
}

fn gauge(used: u64, size: u64) -> Vec<Span<'static>> {
    let pct = (used * 100).checked_div(size).unwrap_or(0).min(100);
    let filled = (pct as usize * 10).div_ceil(100);
    let c = gauge_color(pct);
    vec![
        Span::styled("▰".repeat(filled), fg(c)),
        Span::styled("▱".repeat(10 - filled), fg(c_border())),
        Span::styled(format!(" {pct}% {}/{}", human(used), human(size)), fg(c)),
    ]
}

fn draw_info(t: &Tab, f: &mut Frame, area: Rect, tick: u64) {
    let sep = || Span::styled("  │  ", fg(c_border()));
    let mut spans = vec![Span::styled(" ", fg(c_dim()))];
    if t.allow_all {
        let c = if blink_on(tick) {
            c_error()
        } else {
            c_attention()
        };
        spans.push(Span::styled(" ⚡ ALLOW ALL ", bold(c_bg()).bg(c)));
        spans.push(Span::raw(" "));
    }
    match t.model() {
        Some(m) => {
            spans.push(Span::styled(m.to_string(), bold(c_purple())));
            let effort = t
                .tree
                .reasoning_effort
                .clone()
                .or_else(|| t.config_opt("reasoning_effort").map(|o| o.current.clone()));
            if let Some(e) = &effort {
                spans.push(Span::styled(format!(" ({e})"), fg(c_purple())));
            }
        }
        None => spans.push(Span::styled(tr("modelo: —", "model: —"), fg(c_dim()))),
    }
    let tk = t.tree.total_tokens();
    spans.push(sep());
    spans.push(Span::styled(
        format!(
            "in {} · out {} · cache {}",
            human(tk.input),
            human(tk.output),
            human(tk.cache_read + tk.cache_write)
        ),
        fg(c_fg()),
    ));
    spans.push(sep());
    spans.push(Span::styled(cost(tk.credits()), fg(c_orange())));
    if let Some((used, size)) = t.ctx {
        spans.push(sep());
        spans.push(Span::styled("ctx ", fg(c_dim())));
        spans.extend(gauge(used, size));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn tool_icon(kind: &str) -> &'static str {
    match kind {
        "read" => "≡",
        "edit" | "move" => "✎",
        "delete" => "✂",
        "execute" => "▶",
        "search" => "⌕",
        "fetch" => "⇩",
        "think" | "agent" => "◈",
        _ => "·",
    }
}

pub(super) fn build_lines(items: &[Item], width: usize) -> Vec<Line<'static>> {
    let mut out: Vec<Line<'static>> = vec![];
    for (n, item) in items.iter().enumerate() {
        match item {
            Item::User(s) => {
                if n > 0 {
                    out.push(Line::default());
                }
                for (i, l) in s.split('\n').enumerate() {
                    let prefix = if i == 0 { "❯ " } else { "  " };
                    let line = Line::from(vec![
                        Span::styled(prefix, bold(c_purple())),
                        Span::styled(l.to_string(), bold(c_fg())),
                    ]);
                    out.extend(wrap(&line, width));
                }
                out.push(Line::default());
            }
            Item::Agent(md) => {
                for l in render(md) {
                    out.extend(wrap(&l, width));
                }
            }
            Item::Tool {
                title, kind, state, ..
            } => {
                let (glyph, c) = match state {
                    ToolState::Running => ("…", c_running()),
                    ToolState::Ok => ("✓", c_ok()),
                    ToolState::Failed => ("✗", c_error()),
                };
                let line = Line::from(vec![
                    Span::styled(format!("  {} {title} ", tool_icon(kind)), fg(c_dim())),
                    Span::styled(glyph, fg(c)),
                ]);
                out.extend(wrap(&line, width));
            }
            Item::Note(s) => {
                out.extend(wrap(&Line::styled(format!("  ℹ {s}"), fg(c_dim())), width))
            }
        }
    }
    out
}

fn welcome() -> Vec<Line<'static>> {
    let k = |k: &'static str, d: &'static str| {
        Line::from(vec![
            Span::styled(format!("  {k:<14}"), bold(c_accent())),
            Span::styled(d, fg(c_dim())),
        ])
    };
    vec![
        Line::styled(
            tr(
                "  Bienvenido a copilot-cockpit",
                "  Welcome to copilot-cockpit",
            ),
            bold(c_purple()),
        ),
        Line::default(),
        k("Enter / i", tr("escribir un prompt", "write a prompt")),
        k(
            "f",
            tr(
                "/fleet: subagentes en paralelo (escribe el objetivo)",
                "/fleet: parallel subagents (type the objective)",
            ),
        ),
        k(
            "Alt+Enter",
            tr("nueva línea en el prompt", "newline in the prompt"),
        ),
        k(
            "Tab / Alt+1..9",
            tr("cambiar de proyecto", "switch project"),
        ),
        k("d", tr("mostrar/ocultar dashboard", "show/hide dashboard")),
        k(
            "Esc / Ctrl+C",
            tr("cancelar el turno en curso", "cancel the running turn"),
        ),
        k("?", tr("ayuda completa", "full help")),
        k(
            "S",
            tr("ajustes: tema e idioma", "settings: theme and language"),
        ),
    ]
}

fn draw_transcript(t: &mut Tab, f: &mut Frame, area: Rect, tick: u64) {
    let busy = t.working();
    let (state, c) = match &t.life {
        Life::Failed(_) => ("error".to_string(), c_error()),
        Life::Starting => (tr("iniciando…", "starting…").into(), c_dim()),
        _ if busy => (
            format!("{} {}", spinner(tick), tr("trabajando", "working")),
            c_running(),
        ),
        _ => (tr("listo", "ready").into(), c_ok()),
    };
    let border = if t.perm.is_some() {
        c_attention()
    } else {
        c_accent()
    };
    let title = Line::from(vec![
        Span::styled(format!(" {} ", t.name), bold(c_fg())),
        Span::styled(format!("· {state} "), fg(c)),
    ]);
    let blk = block(title, if t.typing { c_border() } else { border });
    let inner = blk.inner(area);
    let width = inner.width.max(1) as usize;
    let lines: Cow<[Line<'static>]> = match &t.life {
        Life::Failed(e) => {
            let mut l = vec![
                Line::styled(
                    tr(
                        "  ✖ No se pudo iniciar la sesión",
                        "  ✖ Could not start the session",
                    ),
                    bold(c_error()),
                ),
                Line::default(),
            ];
            l.extend(
                e.lines()
                    .flat_map(|x| wrap(&Line::styled(format!("  {x}"), fg(c_fg())), width)),
            );
            l.extend([
                Line::default(),
                Line::styled(tr("  Sugerencia: comprueba que `copilot` está en el PATH,", "  Tip: check that `copilot` is in your PATH,"), fg(c_attention())),
                Line::styled(tr("  que has iniciado sesión (`copilot login`) y que la ruta del proyecto existe.", "  that you are logged in (`copilot login`) and that the project path exists."), fg(c_attention())),
            ]);
            l.into()
        }
        Life::Starting if t.items.is_empty() => vec![Line::styled(
            format!("  {} starting…", spinner(tick)),
            fg(c_running()),
        )]
        .into(),
        _ if t.items.is_empty() => welcome().into(),
        _ => {
            if t.cache
                .as_ref()
                .is_none_or(|c| c.width != width || c.rev != t.rev)
            {
                t.cache = Some(Cache {
                    width,
                    rev: t.rev,
                    lines: build_lines(&t.items, width),
                });
            }
            t.cache
                .as_ref()
                .map_or_else(Cow::default, |c| Cow::Borrowed(c.lines.as_slice()))
        }
    };
    let h = inner.height as usize;
    t.scroll.sync(lines.len(), h);
    let end = lines.len() - t.scroll.off;
    let start = end.saturating_sub(h);
    let mut blk = blk;
    if t.scroll.unseen {
        let c = if blink_on(tick) {
            c_attention()
        } else {
            c_orange()
        };
        blk = blk.title_bottom(Line::styled(" ↓ nuevo (End) ", bold(c)).right_aligned());
    } else if t.scroll.off > 0 {
        blk = blk.title_bottom(
            Line::styled(
                format!(" ↑ {} filas arriba (End) ", t.scroll.off),
                fg(c_dim()),
            )
            .right_aligned(),
        );
    }
    f.render_widget(Paragraph::new(lines[start..end].to_vec()).block(blk), area);
}

/// Tree of every project: session → agent → subagents, with live tool and cost.
fn sidebar_lines(app: &App, w: usize, tick: u64) -> Vec<Line<'static>> {
    let mut out = vec![];
    for (i, t) in app.tabs.iter().enumerate() {
        let cur = i == app.current;
        let total = t.tree.total_tokens().credits();
        let mut head = vec![
            status_icon(t, tick),
            Span::styled(
                format!(" {}", trunc(&t.name, w.saturating_sub(14))),
                if cur { bold(c_accent()) } else { bold(c_fg()) },
            ),
        ];
        if total > 0.0 {
            head.push(Span::styled(format!("  {total:.1} cr"), fg(c_orange())));
        }
        out.push(Line::from(head));
        if t.perm.is_some() {
            out.push(Line::styled(
                tr("  ⚠ permiso pendiente", "  ⚠ permission pending"),
                bold(c_attention()),
            ));
        }
        let n = t.tree.agents.len();
        for (j, a) in t.tree.agents.iter().enumerate() {
            let sub = a.id.is_some();
            let (icon, c) = match a.status {
                Status::Running => (spinner(tick), c_running()),
                Status::Completed => ("✔", c_ok()),
                Status::Cancelled => ("✖", c_error()),
            };
            let branch = if j + 1 == n { "└" } else { "├" };
            let pad = if sub { "   " } else { "" };
            let mut text = format!(" {branch}{pad} {icon} {}", a.name);
            if a.tokens.nano_aiu > 0 {
                text.push_str(&format!(" {:.1} cr", a.tokens.credits()));
            }
            out.push(Line::styled(trunc(&text, w), fg(c)));
            if let (Some(tool), Status::Running) = (&a.current_tool, a.status) {
                let rail = if j + 1 == n { " " } else { "│" };
                out.push(Line::styled(
                    trunc(&format!(" {rail}{pad}   ↳ {tool}"), w),
                    fg(c_dim()),
                ));
            }
        }
        out.push(Line::default());
    }
    out
}

fn draw_sidebar(lines: Vec<Line<'static>>, f: &mut Frame, area: Rect) {
    let title = Line::styled(tr(" dashboard ", " dashboard "), bold(c_accent()));
    f.render_widget(Paragraph::new(lines).block(block(title, c_border())), area);
}

/// Draws the prompt box; returns the screen position of the cursor.
fn draw_input(t: &Tab, f: &mut Frame, area: Rect) -> Option<(u16, u16)> {
    let title = if t.typing {
        " prompt ".to_string()
    } else if t.editor.is_empty() {
        tr(" prompt (Enter para escribir) ", " prompt (Enter to type) ").into()
    } else {
        tr(" prompt ✎ borrador ", " prompt ✎ draft ").into()
    };
    let blk = block(
        Line::styled(title, bold(if t.typing { c_ok() } else { c_dim() })),
        if t.typing { c_ok() } else { c_border() },
    );
    let inner = blk.inner(area);
    let (rows, (cr, cc)) = t.editor.layout(inner.width as usize);
    let h = inner.height as usize;
    let top = (cr + 1).saturating_sub(h);
    let lines: Vec<Line<'static>> = if t.editor.is_empty() {
        vec![Line::styled(
            tr(
                "Escribe tu prompt… (Enter envía · Alt+Enter nueva línea · Esc sale)",
                "Type your prompt… (Enter sends · Alt+Enter newline · Esc exits)",
            ),
            fg(c_dim()),
        )]
    } else {
        rows.into_iter().skip(top).take(h).map(Line::raw).collect()
    };
    f.render_widget(Paragraph::new(lines).block(blk), area);
    Some((inner.x + cc as u16, inner.y + (cr - top) as u16))
}

fn draw_status(
    t: &Tab,
    f: &mut Frame,
    area: Rect,
    confirm: Option<&Confirm>,
    full: bool,
    others: usize,
) {
    let line = if confirm == Some(&Confirm::AllowAll) {
        Line::styled(
            tr(
                " ⚠ ¿Activar ALLOW ALL? y: activar · otra tecla: cancelar",
                " ⚠ Enable ALLOW ALL? y: enable · any other key: cancel",
            ),
            bold(c_error()),
        )
    } else if confirm == Some(&Confirm::CloseTab) {
        Line::styled(
            tr(
                " ¿Cerrar pestaña? Está trabajando.  y: cerrar · otra tecla: volver",
                " Close tab? It is working.  y: close · any other key: back",
            ),
            bold(c_attention()),
        )
    } else if confirm.is_some() {
        Line::styled(
            tr(
                " ¿Salir? Hay agentes trabajando.  y: salir · otra tecla: volver",
                " Quit? Agents are working.  y: quit · any other key: back",
            ),
            bold(c_attention()),
        )
    } else if t.perm.is_some() {
        let mut h = vec![
            ("y", tr("aprobar", "approve")),
            ("n", tr("rechazar", "reject")),
            ("a", tr("siempre", "always")),
            ("A", "allow all"),
        ];
        if others > 0 {
            h.push(("!", tr("siguiente pendiente", "next pending")));
        }
        h.push(("Tab", tr("proyecto", "project")));
        hints(&h)
    } else if t.typing {
        hints(&[
            ("Enter", tr("enviar", "send")),
            ("Alt+Enter", tr("nueva línea", "newline")),
            ("Esc", tr("salir del prompt", "leave prompt")),
            ("Ctrl+W/U", tr("borrar", "delete")),
            ("PgUp/Dn", "scroll"),
        ])
    } else if full {
        hints(&[
            ("↑↓", tr("elegir", "select")),
            ("m", tr("modelo 🔒", "model 🔒")),
            ("d", tr("vista", "view")),
            ("Tab", tr("proyecto", "project")),
            ("?", tr("ayuda", "help")),
        ])
    } else {
        let mut h = vec![
            ("Enter", tr("escribir", "type")),
            ("↑↓ PgUp/Dn", "scroll"),
            ("d", "dashboard"),
            ("S", tr("ajustes", "settings")),
            ("H", tr("historial", "history")),
            ("m", tr("modelo", "model")),
            (
                "A",
                if t.allow_all {
                    "allow all OFF"
                } else {
                    "allow all"
                },
            ),
            ("Tab", tr("proyecto", "project")),
        ];
        if t.working() {
            h.push(("Esc", tr("cancelar", "cancel")));
        }
        if others > 0 {
            h.push(("!", tr("pendiente", "pending")));
        }
        h.extend([("?", tr("ayuda", "help")), ("q", tr("salir", "quit"))]);
        hints(&h)
    };
    f.render_widget(Paragraph::new(line), area);
}

/// Short human description of what the tool wants to run.
fn perm_detail(params: &serde_json::Value) -> String {
    let raw = &params["toolCall"]["rawInput"];
    match raw {
        serde_json::Value::Null => String::new(),
        v => v["command"]
            .as_str()
            .map_or_else(|| v.to_string(), str::to_string),
    }
}

fn draw_perm(t: &Tab, f: &mut Frame, area: Rect, tick: u64, others: usize) {
    let Some(p) = &t.perm else { return };
    let c = if blink_on(tick) {
        c_attention()
    } else {
        c_orange()
    };
    let r = centered(area, area.width.min(80), 12);
    let tc = &p.params["toolCall"];
    let title = tc["title"]
        .as_str()
        .unwrap_or(tr("permiso solicitado", "permission requested"));
    let w = r.width.saturating_sub(4) as usize;
    let mut lines = vec![
        Line::styled(trunc(title, w), bold(c_fg())),
        Line::styled(
            tr_fmt!("tipo: {}", "kind: {}", tc["kind"].as_str().unwrap_or("?")),
            fg(c_dim()),
        ),
        Line::default(),
    ];
    let detail = perm_detail(&p.params);
    for l in detail.lines().take(4) {
        lines.extend(
            wrap(&Line::styled(l.to_string(), fg(c_orange())), w)
                .into_iter()
                .take(2),
        );
    }
    lines.push(Line::default());
    let mut btns = vec![
        Span::styled(
            tr(" [y] aprobar ", " [y] approve "),
            bold(c_bg()).bg(c_ok()),
        ),
        Span::raw("  "),
        Span::styled(
            tr(" [n] rechazar ", " [n] reject "),
            bold(c_bg()).bg(c_error()),
        ),
    ];
    if acp::always_option(&p.params).is_some() {
        btns.extend([
            Span::raw("  "),
            Span::styled(
                tr(" [a] siempre ", " [a] always "),
                bold(c_bg()).bg(c_attention()),
            ),
        ]);
    }
    lines.push(Line::from(btns));
    if others > 0 {
        lines.push(Line::styled(
            tr_fmt!(
                "{others} proyecto(s) más esperan permiso (!)",
                "{others} more project(s) waiting for permission (!)"
            ),
            fg(c_dim()),
        ));
    }
    f.render_widget(Clear, r);
    f.render_widget(
        Paragraph::new(lines).block(
            block(
                Line::styled(
                    tr_fmt!(
                        " ⚠ Permiso requerido · {} ",
                        " ⚠ Permission required · {} ",
                        t.name
                    ),
                    bold(c),
                ),
                c,
            )
            .padding(ratatui::widgets::Padding::horizontal(1)),
        ),
        r,
    );
}

fn draw_allow_all_confirm(f: &mut Frame, area: Rect) {
    let r = centered(area, area.width.min(64), 8);
    let lines = vec![
        Line::styled(
            tr(
                "Se aprobarán TODOS los permisos sin preguntar",
                "ALL permissions will be approved without asking",
            ),
            bold(c_error()),
        ),
        Line::styled(
            tr(
                "(comandos, escritura de ficheros, red) en esta sesión.",
                "(commands, file writes, network) in this session.",
            ),
            fg(c_fg()),
        ),
        Line::default(),
        Line::from(vec![
            Span::styled(
                tr(" [y] activar ", " [y] enable "),
                bold(c_bg()).bg(c_error()),
            ),
            Span::raw("  "),
            Span::styled(
                tr(
                    " cualquier otra tecla: cancelar ",
                    " any other key: cancel ",
                ),
                bold(c_bg()).bg(c_ok()),
            ),
        ]),
    ];
    f.render_widget(Clear, r);
    f.render_widget(
        Paragraph::new(lines).block(
            block(Line::styled(" ⚡ ALLOW ALL ", bold(c_error())), c_error())
                .padding(ratatui::widgets::Padding::horizontal(1)),
        ),
        r,
    );
}

fn draw_selector(s: &Selector, f: &mut Frame, area: Rect) {
    let h = (s.items.len() as u16 + 4).min(area.height.saturating_sub(2));
    let r = centered(area, area.width.min(56), h);
    let rows = h.saturating_sub(4) as usize;
    let top = (s.sel + 1).saturating_sub(rows);
    let mut lines: Vec<Line<'static>> = s
        .items
        .iter()
        .enumerate()
        .skip(top)
        .take(rows)
        .map(|(i, (v, label))| {
            let text = if label == v {
                v.clone()
            } else {
                format!("{label} ({v})")
            };
            if i == s.sel {
                Line::styled(format!("▶ {text}"), bold(c_bg()).bg(c_accent()))
            } else {
                Line::styled(format!("  {text}"), fg(c_fg()))
            }
        })
        .collect();
    lines.push(Line::default());
    lines.push(Line::styled(
        tr(
            "↑↓ elegir · Enter aplicar · Esc cerrar",
            "↑↓ select · Enter apply · Esc close",
        ),
        fg(c_dim()),
    ));
    f.render_widget(Clear, r);
    f.render_widget(
        Paragraph::new(lines).block(
            block(
                Line::styled(format!(" {} ", s.title), bold(c_accent())),
                c_accent(),
            )
            .padding(ratatui::widgets::Padding::horizontal(1)),
        ),
        r,
    );
}

fn draw_help(f: &mut Frame, area: Rect) {
    let rows = [
        (
            "Tab / Shift+Tab",
            tr("siguiente / anterior proyecto", "next / previous project"),
        ),
        ("Alt+1..9", tr("ir al proyecto n", "go to project n")),
        (
            "!",
            tr(
                "saltar al proyecto con permiso pendiente",
                "jump to the project with a pending permission",
            ),
        ),
        (
            "Enter / i",
            tr(
                "escribir (el borrador se guarda por proyecto)",
                "type (the draft is kept per project)",
            ),
        ),
        ("Enter", tr("enviar (en el prompt)", "send (in the prompt)")),
        ("Alt+Enter / Ctrl+J", tr("nueva línea", "newline")),
        ("← → Home End", tr("mover el cursor", "move the cursor")),
        (
            "Ctrl+W / Ctrl+U",
            tr(
                "borrar palabra / hasta inicio de línea",
                "delete word / to start of line",
            ),
        ),
        (
            "Esc",
            tr(
                "salir del prompt; en modo normal cancela el turno",
                "leave the prompt; in normal mode cancels the turn",
            ),
        ),
        (
            "Ctrl+C",
            tr("cancelar el turno en curso", "cancel the running turn"),
        ),
        (
            "↑ ↓ PgUp PgDn",
            tr("scroll de la transcripción", "scroll the transcript"),
        ),
        (
            "Home / End",
            tr(
                "inicio / final (End reanuda el autoscroll)",
                "top / bottom (End resumes autoscroll)",
            ),
        ),
        (
            "d",
            tr(
                "dashboard lateral: mostrar / ocultar",
                "side dashboard: show / hide",
            ),
        ),
        (
            "s",
            tr(
                "vista completa de subagentes: abrir / cerrar",
                "full subagent view: open / close",
            ),
        ),
        (
            "H",
            tr(
                "historial: conversaciones anteriores del proyecto",
                "history: past conversations of the project",
            ),
        ),
        (
            "S",
            tr(
                "ajustes: tema (oscuro/claro) e idioma",
                "settings: theme (dark/light) and language",
            ),
        ),
        (
            "Enter (subagentes)",
            tr(
                "abre el prompt para pedir a Copilot que use el agente elegido",
                "opens the prompt asking Copilot to use the selected agent",
            ),
        ),
        (
            "↑ ↓ (subagentes)",
            tr(
                "elegir agente; m fija su modelo obligatorio 🔒",
                "pick agent; m sets its required model 🔒",
            ),
        ),
        (
            "m / e",
            tr(
                "elegir modelo / reasoning effort de la sesión",
                "pick the session model / reasoning effort",
            ),
        ),
        (
            "A",
            tr(
                "allow all (aprobar todos los permisos) on/off",
                "allow all (approve every permission) on/off",
            ),
        ),
        (
            "y / n / a / A",
            tr(
                "permiso: aprobar / rechazar / siempre / allow all",
                "permission: approve / reject / always / allow all",
            ),
        ),
        (
            "x",
            tr(
                "cerrar pestaña (confirma si trabaja)",
                "close tab (asks if it is working)",
            ),
        ),
        (
            "q",
            tr(
                "salir (pide confirmación si hay trabajo)",
                "quit (asks for confirmation if work is running)",
            ),
        ),
    ];
    let mut lines: Vec<Line<'static>> = rows
        .iter()
        .map(|(k, d)| {
            Line::from(vec![
                Span::styled(format!(" {k:<19}"), bold(c_accent())),
                Span::styled(*d, fg(c_fg())),
            ])
        })
        .collect();
    lines.push(Line::default());
    lines.push(Line::styled(
        tr(
            " Pulsa cualquier tecla para cerrar",
            " Press any key to close",
        ),
        fg(c_dim()),
    ));
    let r = centered(area, 78, lines.len() as u16 + 2);
    f.render_widget(Clear, r);
    f.render_widget(
        Paragraph::new(lines).block(block(
            Line::styled(tr(" Ayuda ", " Help "), bold(c_accent())),
            c_accent(),
        )),
        r,
    );
}

#[cfg(test)]
mod tests {
    use super::super::tests::{app, tab};
    use super::super::{Item, Life};
    use super::*;
    use crate::acp::Perm;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use serde_json::json;

    #[test]
    fn blink_is_brief_and_faster_when_working() {
        let closed = |w| (0..120).filter(|&t| eyes_closed(t, w)).count();
        assert_eq!(closed(false), 6);
        assert_eq!(closed(true), 16);
    }

    fn screen(app: &mut App, w: u16, h: u16) -> String {
        let mut term = match Terminal::new(TestBackend::new(w, h)) {
            Ok(t) => t,
            Err(e) => panic!("{e}"),
        };
        term.draw(|f| draw(app, f)).unwrap();
        let buf = term.backend().buffer();
        (0..h)
            .map(|y| (0..w).map(|x| buf[(x, y)].symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn busy_tab() -> Tab {
        let mut t = tab("alpha");
        for l in [
            r#"{"type":"session.model_change","data":{"newModel":"claude-x","reasoningEffort":"high"}}"#,
            r#"{"type":"assistant.turn_start","data":{}}"#,
            r#"{"type":"session.usage_record","data":{"usage":{"model":"claude-x","modelDisplayName":"Claude X","copilotUsage":{"tokenDetails":[{"tokenType":"input","tokenCount":12345},{"tokenType":"output","tokenCount":800}]}}}}"#,
            r#"{"type":"subagent.started","agentId":"a1","data":{"agentType":"explore","agentDisplayName":"Explorer"}}"#,
            r#"{"type":"tool.execution_start","agentId":"a1","data":{"toolCallId":"c","toolName":"grep"}}"#,
        ] {
            t.tree.apply(l);
        }
        t.ctx = Some((84_000, 200_000));
        t.push(Item::User("hola".into()));
        t.push(Item::Agent(
            "# Título\nuna **respuesta** con `código`".into(),
        ));
        t
    }

    #[test]
    fn renders_dashboard_with_model_tokens_and_agents() {
        let mut a = app(vec![busy_tab(), tab("beta")]);
        let s = screen(&mut a, 120, 35);
        for needle in [
            "claude-x",
            "(high)",
            "12.3k",
            "42%",
            "84.0k/200.0k",
            "Explorer",
            "↳ grep",
            "Título",
            "alpha",
            "beta",
        ] {
            assert!(s.contains(needle), "missing {needle}\n{s}");
        }
    }

    #[test]
    fn english_and_light_theme_apply_from_settings_menu() {
        let mut m = super::super::settings::Menu::new();
        m.on_key(ratatui::crossterm::event::KeyCode::Enter); // theme -> light
        m.on_key(ratatui::crossterm::event::KeyCode::Down);
        m.on_key(ratatui::crossterm::event::KeyCode::Enter); // language -> English
        let mut a = app(vec![tab("a")]);
        assert!(screen(&mut a, 40, 10).contains("Terminal too small"));
        assert_eq!(c_bg(), Color::Rgb(0xd2, 0xd5, 0xe3));
    }

    #[test]
    fn renders_permission_modal_and_small_terminal_warning() {
        let mut t = busy_tab();
        let (reply, _rx) = tokio::sync::oneshot::channel();
        t.perm = Some(Perm {
            params: json!({"toolCall":{"title":"Run shell","kind":"execute","rawInput":{"command":"rm -rf build"}},
                "options":[{"optionId":"o","kind":"allow_always"}]}),
            reply,
        });
        let mut a = app(vec![t]);
        let s = screen(&mut a, 120, 35);
        for needle in [
            "Permiso requerido",
            "Run shell",
            "rm -rf build",
            "[y] aprobar",
            "[a] siempre",
        ] {
            assert!(s.contains(needle), "missing {needle}\n{s}");
        }
        assert!(screen(&mut a, 40, 10).contains("demasiado pequeña"));
    }

    #[test]
    fn renders_full_dashboard_selector_and_allow_all() {
        let mut t = busy_tab();
        t.assigned.insert("explore".into(), "other-model".into());
        t.config = vec![crate::acp::ConfigOpt {
            id: "model".into(),
            current: "claude-x".into(),
            options: vec![
                ("claude-x".into(), "Claude X".into()),
                ("m2".into(), "M2".into()),
            ],
        }];
        let mut a = app(vec![t]);
        a.dash = Dash::Full;
        let s = screen(&mut a, 120, 35);
        for needle in ["explore", "other-model", "Total:", "12.3k"] {
            assert!(s.contains(needle), "missing {needle}\n{s}");
        }
        a.on_key(ratatui::crossterm::event::KeyEvent::from(
            ratatui::crossterm::event::KeyCode::Char('m'),
        ));
        assert!(screen(&mut a, 120, 35).contains("Claude X (claude-x)"));
        a.on_key(ratatui::crossterm::event::KeyEvent::from(
            ratatui::crossterm::event::KeyCode::Esc,
        ));
        a.tabs[0].allow_all = true;
        a.confirm = Some(Confirm::AllowAll);
        let s = screen(&mut a, 120, 35);
        assert!(
            s.contains("TODOS los permisos") && s.contains("ALLOW ALL"),
            "{s}"
        );
    }

    #[test]
    fn renders_startup_error_and_welcome() {
        let mut t = tab("x");
        t.life = Life::Failed("failed to spawn".into());
        let mut a = app(vec![t, tab("y")]);
        let s = screen(&mut a, 100, 30);
        assert!(
            s.contains("failed to spawn") && s.contains("Sugerencia"),
            "{s}"
        );
        a.current = 1;
        assert!(screen(&mut a, 100, 30).contains("Bienvenido"));
    }
}
