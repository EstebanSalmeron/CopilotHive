//! Modal overlays: permission request, allow-all confirmation, config selector and help.
use super::markdown::wrap;
use super::settings::tr;
use super::theme::*;
use super::widgets::{block, centered, trunc};
use super::{Selector, Tab};
use crate::acp;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Padding, Paragraph};

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

pub(super) fn draw_perm(t: &Tab, f: &mut Frame, area: Rect, tick: u64, others: usize) {
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
            .padding(Padding::horizontal(1)),
        ),
        r,
    );
}

pub(super) fn draw_confirm(f: &mut Frame, area: Rect) {
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
            block(Line::styled(" ↯ ALLOW ALL ", bold(c_error())), c_error())
                .padding(Padding::horizontal(1)),
        ),
        r,
    );
}

pub(super) fn draw_selector(s: &Selector, f: &mut Frame, area: Rect) {
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
            .padding(Padding::horizontal(1)),
        ),
        r,
    );
}

pub(super) fn draw_help(f: &mut Frame, area: Rect) {
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
        (
            "f",
            tr(
                "/fleet: subagentes en paralelo",
                "/fleet: parallel subagents",
            ),
        ),
        ("Enter", tr("enviar (en el prompt)", "send (in the prompt)")),
        ("Alt+Enter / Ctrl+J", tr("nueva línea", "newline")),
        (
            "Ctrl+S",
            tr(
                "guardar el borrador como prompt reutilizable",
                "save the draft as a reusable prompt",
            ),
        ),
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
            tr("dashboard: mostrar / ocultar", "dashboard: show / hide"),
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
            "o",
            tr(
                "abrir: explorar carpetas y abrir proyecto en nueva pestaña",
                "open: browse folders and open project in a new tab",
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
            "p",
            tr(
                "prompts guardados: aplicar, crear, editar o borrar",
                "saved prompts: apply, create, edit or delete",
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
