//! Drawing: tabs, info bar, transcript, agent sidebar, prompt box and status bar.
use super::markdown::{render, wrap};
use super::modals;
use super::settings::{self, tr};
use super::theme::*;
use super::{App, Cache, Confirm, Dash, Item, Life, Pane, SideScroll, Tab, ToolState, dashboard};
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
            "↯",
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
        app.panes.clear();
        app.focus = Pane::Transcript;
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
    let modal_open = app.has_modal();
    let side_lines = (dash == Dash::Side && area.width >= SIDEBAR_MIN_TOTAL).then(|| {
        let w = SIDEBAR_W as usize - 2;
        (
            sidebar_lines(app, w, tick),
            subagent_lines(&app.tabs[app.current], w),
            mcp_lines(&app.tabs[app.current], w),
        )
    });
    let in_w = area.width.saturating_sub(2) as usize;
    let in_rows = app.tabs[app.current]
        .editor
        .layout(in_w)
        .0
        .len()
        .clamp(1, 6) as u16;
    let [header, _, info, _, body, input, status] = Layout::vertical([
        Constraint::Length(7),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(3),
        Constraint::Length(in_rows + 2),
        Constraint::Length(1),
    ])
    .areas(area);
    draw_tabs(app, f, tick, header);
    draw_info(&app.tabs[app.current], f, info, tick);
    let (main, side) = if dash == Dash::Side && area.width >= SIDEBAR_MIN_TOTAL {
        let [a, b] =
            Layout::horizontal([Constraint::Min(30), Constraint::Length(SIDEBAR_W)]).areas(body);
        (a, Some(b))
    } else {
        (body, None)
    };
    let mut cursor = None;
    if dash == Dash::Full {
        app.panes.clear();
        app.focus = Pane::Transcript;
        dashboard::draw(&app.tabs[app.current], f, body.union(input), tick);
    } else {
        app.panes.clear();
        app.panes.push((Pane::Transcript, main));
        let side_parts = side.map(|s| {
            let (l, sub, m) = side_lines.unwrap_or_default();
            let (top, mid, bottom, hs, hm) = sidebar_layout(&sub, &m, s);
            app.panes.push((Pane::Dashboard, top));
            if hs > 0 {
                app.panes.push((Pane::Subagents, mid));
            }
            if hm > 0 {
                app.panes.push((Pane::Mcp, bottom));
            }
            (l, sub, m, top, mid, bottom, hs, hm)
        });
        if !app.panes.iter().any(|(p, _)| *p == app.focus) {
            app.focus = Pane::Transcript;
        }
        let active = !app.tabs[app.current].typing && !modal_open;
        let focus = app.focus;
        draw_transcript(
            &mut app.tabs[app.current],
            f,
            main,
            tick,
            active && focus == Pane::Transcript,
        );
        if let Some((l, sub, m, top, mid, bottom, hs, hm)) = side_parts {
            let t = &mut app.tabs[app.current];
            draw_sidebar(
                (l, sub, m),
                (top, mid, bottom, hs, hm),
                (&mut app.dash_scroll, &mut t.sub_scroll, &mut t.mcp_scroll),
                active.then_some(focus),
                f,
            );
        }
        cursor = draw_input(&app.tabs[app.current], f, input);
    }
    let t = &app.tabs[app.current];
    draw_status(
        t,
        f,
        status,
        app.confirm.as_ref(),
        dash == Dash::Full,
        others_pending,
        app.groups().len() > 1,
    );
    if app.confirm == Some(Confirm::AllowAll) {
        modals::draw_confirm(f, area);
    } else if let Some(s) = &app.selector {
        modals::draw_selector(s, f, area);
    } else if t.perm.is_some() {
        modals::draw_perm(t, f, area, tick, others_pending);
    } else if let Some(pos) = cursor.filter(|_| t.typing && !modal_open) {
        f.set_cursor_position(pos);
    }
    if app.custom_empty() {
        let r = Rect::new(area.x, info.y, area.width, info.height + 1 + body.height);
        f.render_widget(Clear, r);
        f.render_widget(
            Paragraph::new(vec![
                Line::styled(
                    tr_fmt!(
                        "«{}» está vacío",
                        "“{}” is empty",
                        crate::tui::multi::GROUP
                    ),
                    bold(c_accent()),
                ),
                Line::default(),
                Line::styled(
                    tr(
                        "g crear una sesión multiproyecto · G ver/borrar las guardadas · { } cambiar de grupo",
                        "g create a multi-project session · G see/delete saved ones · { } switch group",
                    ),
                    fg(c_dim()),
                ),
            ])
            .alignment(Alignment::Center),
            centered(r, r.width, 3),
        );
    }
    if let Some(h) = &mut app.history {
        h.draw(
            f,
            Rect::new(area.x, area.y + 5, area.width, area.height - 5),
        );
    }
    if let Some(o) = &app.open {
        o.draw(f, area);
    }
    if let Some(m) = &app.settings {
        m.draw(f, area);
    }
    if let Some(m) = &app.mcp {
        m.draw(f, area);
    }
    if let Some(m) = &app.multi {
        m.draw(f, area);
    }
    if let Some(p) = &app.prompts {
        p.draw(f, area);
    }
    if let Some(t) = &app.tour {
        t.draw(f, area, app.tick);
    }
    if help {
        modals::draw_help(f, area);
    }
}

/// Joins `items` into one line at most `width` wide, dropping leading items so `active` stays visible.
fn windowed(items: Vec<Vec<Span<'_>>>, active: usize, width: usize) -> Line<'_> {
    let w = |it: &Vec<Span>| it.iter().map(Span::width).sum::<usize>();
    let mut start = 0;
    while start < active && items[start..=active].iter().map(w).sum::<usize>() > width {
        start += 1;
    }
    Line::from(items.into_iter().skip(start).flatten().collect::<Vec<_>>())
}

fn draw_tabs(app: &App, f: &mut Frame, tick: u64, area: Rect) {
    let groups = app.groups();
    let cur_group = app.cur_group();
    let group_items: Vec<Vec<Span>> = if groups.len() > 1 {
        groups
            .iter()
            .map(|g| {
                let members = app.tabs.iter().filter(|t| t.group.as_deref() == *g);
                let (mut n, mut work, mut perm) = (0, false, false);
                for t in members {
                    n += 1;
                    work |= t.working();
                    perm |= t.perm.is_some();
                }
                let sel = *g == cur_group;
                let style = if sel {
                    bold(c_bg()).bg(c_accent())
                } else {
                    fg(c_dim())
                };
                let count = if *g == Some(crate::tui::multi::GROUP) {
                    format!("{n}/{}", app.saved_n)
                } else {
                    n.to_string()
                };
                let mut v = vec![Span::styled(
                    format!(" {} {count} ", g.unwrap_or("—")),
                    style,
                )];
                if perm {
                    v.push(Span::styled("!", bold(c_error())));
                } else if work {
                    v.push(Span::styled("●", fg(c_running())));
                }
                v.push(Span::styled(" ", fg(c_border())));
                v
            })
            .collect()
    } else {
        vec![]
    };
    let group_active = groups.iter().position(|g| *g == cur_group).unwrap_or(0);
    let mut tab_active = 0;
    let mut tab_items: Vec<Vec<Span>> = vec![];
    for (n, i) in app.group_tabs().into_iter().enumerate() {
        let t = &app.tabs[i];
        let active = i == app.current;
        if active {
            tab_active = n;
        }
        let base = if active {
            bold(c_fg()).bg(c_surface())
        } else {
            fg(c_dim())
        };
        let mut label = vec![
            Span::styled(format!(" {} ", n + 1), fg(c_dim()).patch(base)),
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
        label.push(Span::styled("│", fg(c_border())));
        tab_items.push(label);
    }
    let working = app.tabs.iter().filter(|t| t.working()).count();
    let subs: usize = app.tabs.iter().map(|t| t.tree.active_subagents()).sum();
    let credits: f64 = app
        .tabs
        .iter()
        .map(|t| t.tree.total_tokens().credits())
        .sum();
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
            tr_fmt!("{subs} subagentes", "{subs} subagents"),
            fg(c_purple()),
        ),
        Span::styled(" · ", fg(c_border())),
        Span::styled(cost(credits), fg(c_orange())),
    ]);
    let quip_line = match quip(tick) {
        Some(q) => Line::from(vec![
            Span::styled("◀ ", fg(c_attention())),
            Span::styled(format!("“{q}”"), Style::new().fg(c_attention()).italic()),
        ]),
        None => Line::default(),
    };
    let content_w = quip_line.width().max(summary.width());

    const MONTH_W: u16 = 24;
    const PATROL_BOX_W: u16 = 28;
    let logo_lines = logo();
    let logo_box_w = logo_lines.iter().map(Line::width).max().unwrap_or(0) as u16 + 4;
    let month_w = if area.width >= 13 + 26 + MONTH_W {
        MONTH_W
    } else {
        0
    };
    let logo_w = if area.width >= 13 + 50 + month_w + logo_box_w {
        logo_box_w
    } else {
        0
    };
    let cmd_min = 13 + content_w.max(36);
    let patrol_box_w = if (area.width as usize)
        >= cmd_min + month_w as usize + logo_w as usize + PATROL_BOX_W as usize
    {
        PATROL_BOX_W
    } else {
        0
    };
    let [cmd_a, patrol_box_a, month_a, logo_a] = Layout::horizontal([
        Constraint::Min(24),
        Constraint::Length(patrol_box_w),
        Constraint::Length(month_w),
        Constraint::Length(logo_w),
    ])
    .areas(area);

    let cmd_blk = block(
        Line::styled(
            tr(" ◆ centro de mando ", " ◆ command center "),
            bold(c_accent()),
        ),
        c_border(),
    );
    let cmd_inner = cmd_blk.inner(cmd_a);
    f.render_widget(cmd_blk, cmd_a);
    let [left, right] =
        Layout::horizontal([Constraint::Length(11), Constraint::Min(10)]).areas(cmd_inner);
    f.render_widget(Paragraph::new(robot(tick, working > 0)), left);
    f.render_widget(
        Paragraph::new(vec![
            quip_line,
            Line::default(),
            summary,
            windowed(group_items, group_active, right.width as usize),
            windowed(tab_items, tab_active, right.width as usize),
        ]),
        right,
    );

    if patrol_box_w > 0 {
        let pending_perm = app.tabs.iter().any(|t| t.perm.is_some());
        let (title, border) = if pending_perm {
            let b = if blink_on(tick) {
                c_attention()
            } else {
                c_orange()
            };
            (
                Line::styled(tr(" ⚠ alerta ", " ⚠ alert "), bold(c_attention())),
                b,
            )
        } else if working > 0 {
            (
                Line::styled(tr(" ✦ colmena ", " ✦ hive "), bold(c_running())),
                c_border(),
            )
        } else {
            (
                Line::styled(tr(" ☾ reposo ", " ☾ idle "), fg(c_dim())),
                c_border(),
            )
        };
        let p_blk = block(title, border);
        let p_inner = p_blk.inner(patrol_box_a);
        f.render_widget(
            Paragraph::new(patrol(
                tick,
                working > 0,
                subs,
                pending_perm,
                p_inner.width as usize,
            ))
            .block(p_blk),
            patrol_box_a,
        );
    }

    if month_w > 0 {
        let sub = Line::styled(tr("todos los proyectos", "all projects"), fg(c_dim()));
        let rows = if !app.month_loaded && app.month_sessions.is_empty() {
            vec![
                Line::default(),
                Line::styled(tr("calculando…", "calculating…"), fg(c_dim())),
                Line::default(),
                sub,
                Line::default(),
            ]
        } else {
            let (cr, usd) = cost_parts(app.month_credits());
            vec![
                Line::default(),
                Line::styled(cr, bold(c_orange())),
                Line::styled(usd, bold(c_attention())),
                Line::default(),
                sub,
            ]
        };
        let month_blk = block(
            Line::styled(
                tr_fmt!(" mes {} ", " month {} ", app.month_label),
                bold(c_accent()),
            ),
            c_border(),
        );
        f.render_widget(
            Paragraph::new(rows)
                .alignment(Alignment::Center)
                .block(month_blk),
            month_a,
        );
    }
    if logo_w > 0 {
        f.render_widget(
            Paragraph::new(logo_lines)
                .alignment(Alignment::Center)
                .block(block(Line::default(), c_border())),
            logo_a,
        );
    }
}

const QUIPS: [(&str, &str); 8] = [
    (
        "Yo no compilo, yo solo opino.",
        "I don't compile, I just opine.",
    ),
    (
        "Otro café y refactorizo el mundo.",
        "One more coffee and I'll refactor the world.",
    ),
    (
        "En mi máquina funcionaba. Soy la máquina.",
        "Works on my machine. I am the machine.",
    ),
    (
        "Bip bup. Eso era un bug o una feature?",
        "Beep boop. Was that a bug or a feature?",
    ),
    (
        "Los tokens no crecen en los árboles...",
        "Tokens don't grow on trees...",
    ),
    (
        "Veo subagentes por todas partes.",
        "I see subagents everywhere.",
    ),
    (
        "Prometo no borrar producción. Hoy.",
        "I promise not to delete prod. Today.",
    ),
    (
        "Tranqui, yo vigilo los permisos.",
        "Relax, I'm watching the permissions.",
    ),
];
const QUIP_PERIOD: u64 = 400;
const QUIP_SHOWN: u64 = 100;

/// Joke shown in the robot's bubble for 10 s every 40 s (ticks are 100 ms).
pub(super) fn quip(tick: u64) -> Option<&'static str> {
    if tick % QUIP_PERIOD >= QUIP_SHOWN {
        return None;
    }
    let (es, en) = QUIPS[(tick / QUIP_PERIOD) as usize % QUIPS.len()];
    Some(tr(es, en))
}

/// Right-aligned header badge: big "HIVE" with the product name between honeycomb strips.
fn logo() -> Vec<Line<'static>> {
    const HIVE: [&str; 5] = [
        "█ █  ███  █ █  ███",
        "█ █   █   █ █  █  ",
        "███   █   █ █  ███",
        "█ █   █   █ █  █  ",
        "█ █  ███   █   ███",
    ];
    const SIDE: [&str; 5] = ["⬢⬡⬢⬡ ", "⬡⬢⬡⬢ ", "⬢⬡⬢⬡ ", "⬡⬢⬡⬢ ", "⬢⬡⬢⬡ "];
    let tag = ["", "C O P I L O T", "─────────────", "multi-agent", ""];
    (0..5)
        .map(|i| {
            Line::from(vec![
                Span::styled(SIDE[i], fg(c_orange())),
                Span::styled(HIVE[i], bold(c_accent())),
                Span::styled(format!("   {:<13}", tag[i]), fg(c_orange())),
                Span::styled(format!(" {}", SIDE[4 - i].trim_end()), fg(c_orange())),
            ])
        })
        .collect()
}

/// Eyes are closed for 2 ticks every 40 (idle) or 15 (working).
pub(super) fn eyes_closed(tick: u64, working: bool) -> bool {
    tick % if working { 15 } else { 40 } < 2
}

/// True on ticks where the idle patrol's `z`/`zZ` toggles.
pub(super) fn patrol_idle_tick(tick: u64) -> bool {
    tick.is_multiple_of(20)
}

/// 5-row mini-bot patrol scene rendered inside its own header box when space allows.
fn patrol(
    tick: u64,
    working: bool,
    subs: usize,
    pending_perm: bool,
    w: usize,
) -> Vec<Line<'static>> {
    if w < 7 {
        return vec![
            Line::default(),
            Line::default(),
            Line::default(),
            Line::default(),
            Line::styled("·".repeat(w), fg(c_border())),
        ];
    }
    let mut grid = [
        vec![(' ', Style::default()); w],
        vec![(' ', Style::default()); w],
        vec![(' ', Style::default()); w],
        vec![(' ', Style::default()); w],
        vec![(' ', Style::default()); w],
    ];
    let put = |row: &mut [(char, Style)], col: usize, s: &str, st: Style| {
        for (i, ch) in s.chars().enumerate() {
            if let Some(cell) = row.get_mut(col + i) {
                *cell = (ch, st);
            }
        }
    };

    if pending_perm {
        let x = (w - 7) / 2;
        let on = blink_on(tick);
        let c = c_attention();
        let warn_st = if on { bold(c) } else { fg(c_orange()) };
        let sp_st = if on { bold(c_orange()) } else { fg(c_dim()) };
        let (sp, alt) = if on { ("✦", "✧") } else { ("✧", "✦") };
        put(&mut grid[0], x, sp, sp_st);
        put(&mut grid[0], x + 3, "⚠", warn_st);
        put(&mut grid[0], x + 6, sp, sp_st);
        put(&mut grid[1], x, "\\▄███▄/", fg(c));
        if x >= 2 {
            put(&mut grid[2], x - 2, alt, sp_st);
            put(&mut grid[2], x + 8, alt, sp_st);
        }
        put(&mut grid[2], x + 1, "█", fg(c));
        put(&mut grid[2], x + 2, "◉ ◉", bold(c_orange()));
        put(&mut grid[2], x + 5, "█", fg(c));
        put(&mut grid[3], x + 1, "▀███▀", fg(c));
        for cell in &mut grid[4] {
            *cell = ('═', fg(c_border()));
        }
        if w >= 12 {
            grid[4][0] = ('╠', fg(c_dim()));
            grid[4][w - 1] = ('╣', fg(c_dim()));
        }
        put(&mut grid[4], x + 2, "┴ ┴", fg(c));
    } else if working {
        let span = w - 6;
        let step = (tick / 2) as usize;
        let pos = step % (span * 2);
        let (x, right) = if pos < span {
            (pos, true)
        } else {
            (span * 2 - pos, false)
        };
        let c = c_running();
        let body_x = if right { x } else { x + 1 };

        // Twinkling sky stars (antenna draws over them when passing).
        for (i, sx) in (2..w).step_by(6).enumerate() {
            let (ch, st) = match ((tick / 4) as usize + i) % 4 {
                0 => ('✦', fg(c_attention())),
                1 => ('✧', fg(c_dim())),
                2 => ('⋆', fg(c)),
                _ => ('·', fg(c_border())),
            };
            grid[0][sx] = (ch, st);
        }

        // Animated conveyor belt moving in the bot's travel direction.
        let arrow = if right { '»' } else { '«' };
        let shift = step % 4;
        for (col, cell) in grid[4].iter_mut().enumerate() {
            let phase = if right {
                (col + 4 - shift) % 4
            } else {
                (col + shift) % 4
            };
            *cell = match phase {
                0 => (arrow, fg(c_dim())),
                2 => ('·', fg(c_border())),
                _ => ('═', fg(c_border())),
            };
        }
        if w >= 12 {
            grid[4][0] = ('╠', fg(c_dim()));
            grid[4][w - 1] = ('╣', fg(c_dim()));
        }

        let ant = if step.is_multiple_of(2) { "*" } else { "⌁" };
        let feet = if step.is_multiple_of(2) {
            "╯ ╰"
        } else {
            "╰ ╯"
        };
        let spark = if step.is_multiple_of(2) { "✦" } else { "✧" };
        put(&mut grid[0], body_x + 2, ant, bold(c_attention()));
        put(&mut grid[1], body_x, "▄███▄", fg(c));
        put(&mut grid[2], body_x, "█", fg(c));
        put(&mut grid[2], body_x + 1, "◉ ◉", bold(c_attention()));
        put(&mut grid[2], body_x + 4, "█", fg(c));
        put(
            &mut grid[2],
            if right { x + 5 } else { x },
            "▓",
            fg(c_orange()),
        );
        put(&mut grid[3], body_x, "▀███▀", fg(c));
        if right && x >= 1 {
            put(&mut grid[3], x - 1, spark, fg(c_attention()));
        } else if !right && x + 6 < w {
            put(&mut grid[3], x + 6, spark, fg(c_attention()));
        }
        put(&mut grid[4], body_x + 1, feet, fg(c));

        if subs > 0 {
            let dx = if right {
                (x >= 4).then(|| x.saturating_sub(5))
            } else if x + 11 <= w {
                Some(x + 8)
            } else if x + 10 <= w {
                Some(x + 7)
            } else {
                None
            };
            if let Some(dx) = dx {
                let d_even = (tick / 3).is_multiple_of(2);
                let row = if d_even { 1 } else { 2 };
                put(&mut grid[row], dx, "[•]", bold(c_purple()));
                let glint = if d_even { "✦" } else { "·" };
                if right && dx >= 1 {
                    put(&mut grid[row], dx - 1, glint, fg(c_purple()));
                } else if !right && dx + 3 < w {
                    put(&mut grid[row], dx + 3, glint, fg(c_purple()));
                }
            }
        }
    } else {
        let x = (w - 7) / 2;
        let c = c_ok();
        let phase = (tick / 20).is_multiple_of(2);
        if w >= 12 {
            grid[0][1] = ('☾', fg(c_attention()));
        }
        if w >= 16 {
            grid[0][5] = (if phase { '⋆' } else { '·' }, fg(c_dim()));
            grid[0][w - 3] = (if phase { '·' } else { '⋆' }, fg(c_dim()));
        }
        if w >= 20 {
            grid[1][3] = (if phase { '·' } else { '⋆' }, fg(c_border()));
            grid[1][w - 4] = (if phase { '⋆' } else { '·' }, fg(c_border()));
        }
        put(
            &mut grid[0],
            x + 5,
            if phase { "z " } else { "zZ" },
            fg(c_dim()),
        );
        put(&mut grid[1], x, "▄███▄", fg(c));
        put(&mut grid[2], x, "█", fg(c));
        put(&mut grid[2], x + 1, "- -", fg(c_dim()));
        put(&mut grid[2], x + 4, "█", fg(c));
        put(&mut grid[3], x, "▀███▀", fg(c));
        for (col, cell) in grid[4].iter_mut().enumerate() {
            let ch = match col % 4 {
                0 => '─',
                2 => '·',
                _ => ' ',
            };
            *cell = (ch, fg(c_border()));
        }
        put(&mut grid[4], x + 1, "┴ ┴", fg(c));
    }

    grid.into_iter()
        .map(|row| {
            let mut spans = vec![];
            let mut i = 0;
            while i < row.len() {
                let st = row[i].1;
                let mut s = String::new();
                while i < row.len() && row[i].1 == st {
                    s.push(row[i].0);
                    i += 1;
                }
                spans.push(Span::styled(s, st));
            }
            Line::from(spans)
        })
        .collect()
}

/// 5-row robot: fixed eyes that blink; working = color + blinking antenna.
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
        Line::from(vec![
            Span::raw("     "),
            Span::styled(ant, fg(c_attention())),
        ]),
        Line::styled("  ▄█████▄", fg(c)),
        Line::from(vec![
            Span::styled("  █ ", fg(c)),
            Span::styled(eyes, bold(c_attention())),
            Span::styled(" █", fg(c)),
        ]),
        Line::styled("  █▄▄▄▄▄█", fg(c)),
        Line::styled("   ▀▀ ▀▀", fg(c)),
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
        spans.push(Span::styled(" ↯ ALLOW ALL ", bold(c_bg()).bg(c)));
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
            tr("  Bienvenido a copilot-hive", "  Welcome to copilot-hive"),
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
        k(
            "C / R",
            tr(
                "/clear: sesión nueva · /restart: recargar",
                "/clear: new session · /restart: reload",
            ),
        ),
        k("?", tr("ayuda completa", "full help")),
        k("t", tr("tutorial animado", "animated tour")),
        k(
            "S",
            tr("ajustes: tema e idioma", "settings: theme and language"),
        ),
        k(
            "M",
            tr("servidores MCP del proyecto", "project MCP servers"),
        ),
        k("p", tr("prompts guardados", "saved prompts")),
    ]
}

fn draw_transcript(t: &mut Tab, f: &mut Frame, area: Rect, tick: u64, focused: bool) {
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
    } else if focused {
        c_accent()
    } else {
        c_border()
    };
    let title = Line::from(vec![
        Span::styled(format!(" {} ", t.name), bold(c_fg())),
        Span::styled(format!("· {state} "), fg(c)),
    ]);
    let blk = block(title, border);
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
            tr_fmt!("  {} iniciando…", "  {} starting…", spinner(tick)),
            fg(c_running()),
        )]
        .into(),
        _ if t.items.is_empty() => welcome().into(),
        _ => {
            let light = settings::light();
            if t.cache
                .as_ref()
                .is_none_or(|c| c.width != width || c.rev != t.rev || c.light != light)
            {
                t.cache = Some(Cache {
                    width,
                    rev: t.rev,
                    light,
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
        blk = blk.title_bottom(
            Line::styled(tr(" ↓ nuevo (End) ", " ↓ new (End) "), bold(c)).right_aligned(),
        );
    } else if t.scroll.off > 0 {
        blk = blk.title_bottom(
            Line::styled(
                tr_fmt!(
                    " ↑ {} filas arriba (End) ",
                    " ↑ {} lines up (End) ",
                    t.scroll.off
                ),
                fg(c_dim()),
            )
            .right_aligned(),
        );
    }
    f.render_widget(Paragraph::new(lines[start..end].to_vec()).block(blk), area);
}

fn align_row(
    mut left: Vec<Span<'static>>,
    right: Vec<Span<'static>>,
    w: usize,
    bg: Option<Color>,
) -> Vec<Line<'static>> {
    let w = w.max(1);
    let paint = |mut line: Line<'static>| {
        if let Some(c) = bg {
            let st = Style::new().bg(c);
            for s in &mut line.spans {
                s.style = st.patch(s.style);
            }
            let lw = line.width();
            if lw < w {
                line.spans.push(Span::styled(" ".repeat(w - lw), st));
            }
            line.style = st;
        }
        line
    };
    let lw: usize = left.iter().map(Span::width).sum();
    let rw: usize = right.iter().map(Span::width).sum();
    if right.is_empty() {
        return if lw <= w {
            vec![paint(Line::from(left))]
        } else {
            wrap(&Line::from(left), w).into_iter().map(paint).collect()
        };
    }
    if lw + 1 + rw <= w {
        left.push(Span::raw(" ".repeat(w - lw - rw)));
        left.extend(right);
        vec![paint(Line::from(left))]
    } else {
        left.push(Span::raw(" "));
        left.extend(right);
        wrap(&Line::from(left), w).into_iter().map(paint).collect()
    }
}

/// Tree of every project: active agents first, then completed grouped by kind with spend share.
fn sidebar_lines(app: &App, w: usize, tick: u64) -> Vec<Line<'static>> {
    let mut out = vec![];
    for (i, t) in app.tabs.iter().enumerate() {
        if i > 0 {
            out.push(Line::styled(
                format!(" {}", "─".repeat(w.saturating_sub(2))),
                fg(c_border()),
            ));
        }
        let cur = i == app.current;
        let total_tk = t.tree.total_tokens();
        let total_nano = total_tk.nano_aiu;
        let total = total_tk.credits();
        let head_left = vec![
            status_icon(t, tick),
            Span::styled(
                format!(" {}", t.name),
                if cur { bold(c_accent()) } else { bold(c_fg()) },
            ),
        ];
        let head_right = if total_nano > 0 {
            vec![Span::styled(
                format!("{total:.1} cr · ${:.2}", total * 0.01),
                fg(c_orange()),
            )]
        } else {
            vec![Span::styled(tr("sin uso", "idle"), fg(c_dim()))]
        };
        out.extend(align_row(head_left, head_right, w, cur.then(c_surface)));
        if t.perm.is_some() {
            out.extend(wrap(
                &Line::styled(
                    tr("  ⚠ permiso pendiente", "  ⚠ permission pending"),
                    bold(c_attention()),
                ),
                w,
            ));
        }

        let running: Vec<_> = t
            .tree
            .agents
            .iter()
            .filter(|a| a.status == Status::Running)
            .collect();
        let mut by_kind: std::collections::BTreeMap<&str, (usize, u64, bool)> =
            std::collections::BTreeMap::new();
        for a in t.tree.agents.iter().filter(|a| a.status != Status::Running) {
            let e = by_kind.entry(a.kind.as_str()).or_insert((0, 0, true));
            e.0 += 1;
            e.1 = e.1.saturating_add(a.tokens.nano_aiu);
            e.2 &= a.status == Status::Cancelled;
        }
        let mut groups: Vec<(&str, usize, u64, bool)> = by_kind
            .into_iter()
            .map(|(k, (n, nano, canc))| (k, n, nano, canc))
            .collect();
        groups.sort_by_key(|&(kind, _, nano, _)| (kind != "main", std::cmp::Reverse(nano), kind));

        let total_rows = running.len() + groups.len();
        for (j, a) in running.iter().enumerate() {
            let last = j + 1 == total_rows;
            let left = vec![
                Span::styled(if last { " └ " } else { " ├ " }, fg(c_border())),
                Span::styled(spinner(tick), bold(c_running())),
                Span::styled(format!(" {}", a.name), bold(c_fg())),
            ];
            let right = if a.tokens.nano_aiu > 0 {
                vec![Span::styled(
                    format!("{:.1} cr", a.tokens.credits()),
                    fg(c_orange()),
                )]
            } else {
                vec![]
            };
            out.extend(align_row(left, right, w, None));
            if let Some(tool) = &a.current_tool {
                let rail = if last { "   " } else { " │ " };
                out.extend(wrap(
                    &Line::from(vec![
                        Span::styled(rail, fg(c_border())),
                        Span::styled(format!("  ↳ {tool}"), fg(c_dim())),
                    ]),
                    w,
                ));
            }
        }

        for (k, (kind, count, group_nano, all_cancelled)) in groups.iter().enumerate() {
            let last = k + 1 == groups.len();
            let (icon, ic) = if *all_cancelled {
                ("✖", c_error())
            } else {
                ("✔", c_ok())
            };
            let mut left = vec![
                Span::styled(if last { " └ " } else { " ├ " }, fg(c_border())),
                Span::styled(icon, fg(ic)),
                Span::styled(format!(" {kind}"), fg(c_fg())),
            ];
            if *count > 1 {
                left.push(Span::styled(format!(" ×{count}"), fg(c_dim())));
            }
            let right = if *group_nano > 0 {
                let cost_str = format!("{:.1} cr", *group_nano as f64 / 1e9);
                let lw: usize = left.iter().map(Span::width).sum();
                let mut r = vec![];
                if total_nano > 0 && lw + 7 + cost_str.width() <= w {
                    let filled = ((*group_nano as u128 * 5 + total_nano as u128 / 2)
                        / total_nano as u128)
                        .clamp(1, 5) as usize;
                    r.push(Span::styled("▰".repeat(filled), fg(c_orange())));
                    if filled < 5 {
                        r.push(Span::styled("▱".repeat(5 - filled), fg(c_border())));
                    }
                    r.push(Span::raw(" "));
                }
                r.push(Span::styled(cost_str, fg(c_orange())));
                r
            } else {
                vec![]
            };
            out.extend(align_row(left, right, w, None));
        }
    }
    out
}

fn subagent_lines(t: &Tab, w: usize) -> Vec<Line<'static>> {
    t.defs
        .iter()
        .filter(|d| !d.builtin)
        .flat_map(|d| {
            let text = if let Some(m) = t.assigned.get(&d.name).filter(|m| !m.is_empty()) {
                format!(" ◈ {} · 🔒 {m}", d.name)
            } else if let Some(m) = d.model.as_deref().filter(|m| !m.is_empty()) {
                format!(" ◈ {} · {m}", d.name)
            } else {
                format!(" ◈ {}", d.name)
            };
            wrap(&Line::styled(text, fg(c_fg())), w)
        })
        .collect()
}

fn mcp_lines(t: &Tab, w: usize) -> Vec<Line<'static>> {
    if t.mcp.is_empty() {
        return vec![];
    }
    let mut out = vec![];
    for (s, on) in &t.mcp {
        let (icon, c) = if *on {
            ("●", c_ok())
        } else {
            ("○", c_dim())
        };
        out.extend(wrap(
            &Line::styled(format!(" {icon} {} · {}", s.name, s.kind), fg(c)),
            w,
        ));
    }
    out
}

fn sidebar_layout(subs: &[Line], mcp: &[Line], area: Rect) -> (Rect, Rect, Rect, u16, u16) {
    // Subagents and MCP sit below dashboard, together never taking more than half the sidebar.
    let budget = area.height / 2;
    let want = |v: &[Line]| {
        if v.is_empty() {
            0
        } else {
            (v.len() as u16).saturating_add(2)
        }
    };
    let (ws, wm, half) = (want(subs), want(mcp), budget / 2);
    let (hs, hm) = if ws <= half {
        (ws, wm.min(budget - ws))
    } else if wm <= half {
        (ws.min(budget - wm), wm)
    } else {
        (half, budget - half)
    };
    let [top, mid, bottom] = Layout::vertical([
        Constraint::Min(0),
        Constraint::Length(hs),
        Constraint::Length(hm),
    ])
    .areas(area);
    (top, mid, bottom, hs, hm)
}

fn draw_side_box(
    title: Line<'static>,
    lines: Vec<Line<'static>>,
    scroll: &mut SideScroll,
    focused: bool,
    f: &mut Frame,
    area: Rect,
) {
    let mut blk = block(title, if focused { c_accent() } else { c_border() });
    let inner_h = blk.inner(area).height as usize;
    let total = lines.len();
    scroll.sync(total, inner_h);
    let start = scroll.off;
    let end = (start + inner_h).min(total);
    if total > inner_h && inner_h > 0 {
        blk = blk.title_bottom(
            Line::styled(
                format!(" ↑↓ {}-{end}/{total} ", start + 1),
                fg(if focused { c_accent() } else { c_dim() }),
            )
            .right_aligned(),
        );
    }
    f.render_widget(Paragraph::new(lines[start..end].to_vec()).block(blk), area);
}

fn draw_sidebar(
    (lines, subs, mcp): (Vec<Line<'static>>, Vec<Line<'static>>, Vec<Line<'static>>),
    (top, mid, bottom, hs, hm): (Rect, Rect, Rect, u16, u16),
    (dash_s, sub_s, mcp_s): (&mut SideScroll, &mut SideScroll, &mut SideScroll),
    focus: Option<Pane>,
    f: &mut Frame,
) {
    draw_side_box(
        Line::styled(" dashboard ", bold(c_accent())),
        lines,
        dash_s,
        focus == Some(Pane::Dashboard),
        f,
        top,
    );
    if hs > 0 {
        draw_side_box(
            Line::styled(tr(" Subagentes ", " Subagents "), bold(c_accent())),
            subs,
            sub_s,
            focus == Some(Pane::Subagents),
            f,
            mid,
        );
    }
    if hm > 0 {
        draw_side_box(
            Line::styled(" MCP ", bold(c_accent())),
            mcp,
            mcp_s,
            focus == Some(Pane::Mcp),
            f,
            bottom,
        );
    }
}

/// Draws the prompt box; returns the screen position of the cursor.
fn draw_input(t: &Tab, f: &mut Frame, area: Rect) -> Option<(u16, u16)> {
    let pics = match t.images.len() {
        0 => String::new(),
        n => format!("📎 ×{n} "),
    };
    let title = if t.typing {
        format!(" prompt {pics}")
    } else if t.editor.is_empty() {
        format!(
            "{}{pics}",
            tr(" prompt (Enter para escribir) ", " prompt (Enter to type) ")
        )
    } else {
        format!("{}{pics}", tr(" prompt ✎ borrador ", " prompt ✎ draft "))
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
                "Escribe tu prompt… (Enter envía · Ctrl+V pega imagen · Alt+Enter nueva línea · Esc sale)",
                "Type your prompt… (Enter sends · Ctrl+V paste image · Alt+Enter newline · Esc exits)",
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
    grouped: bool,
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
    } else if matches!(confirm, Some(Confirm::Clear | Confirm::Restart)) {
        let clear = confirm == Some(&Confirm::Clear);
        Line::styled(
            match (clear, t.working()) {
                (true, false) => tr(
                    " ¿Sesión nueva (/clear)? Se pierde el contexto.  y: sí · otra tecla: volver",
                    " New session (/clear)? Context is lost.  y: yes · any other key: back",
                ),
                (true, true) => tr(
                    " ¿Sesión nueva (/clear)? ESTÁ TRABAJANDO.  y: sí · otra tecla: volver",
                    " New session (/clear)? IT IS WORKING.  y: yes · any other key: back",
                ),
                (false, false) => tr(
                    " ¿Reiniciar (/restart)? Recarga agentes y MCP.  y: sí · otra tecla: volver",
                    " Restart (/restart)? Reloads agents and MCP.  y: yes · any other key: back",
                ),
                (false, true) => tr(
                    " ¿Reiniciar (/restart)? ESTÁ TRABAJANDO.  y: sí · otra tecla: volver",
                    " Restart (/restart)? IT IS WORKING.  y: yes · any other key: back",
                ),
            },
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
            ("Ctrl+S", tr("guardar", "save")),
            ("Esc", tr("salir del prompt", "leave prompt")),
            ("Ctrl+W/U", tr("borrar", "delete")),
            ("PgUp/Dn", "scroll"),
        ])
    } else if full {
        hints(&[
            ("↑↓", tr("elegir", "select")),
            ("m", tr("modelo 🔒", "model 🔒")),
            ("s", tr("vista lateral", "side view")),
            ("d", tr("ocultar", "hide")),
            ("Tab", tr("proyecto", "project")),
            ("?", tr("ayuda", "help")),
        ])
    } else {
        let mut h = vec![
            ("Enter", tr("escribir", "type")),
            ("↑↓ PgUp/Dn", "scroll"),
            ("Tab", tr("proyecto", "project")),
        ];
        if grouped {
            h.push(("{ }", tr("grupo", "group")));
        }
        h.extend([
            ("g", tr("multi-sesión", "multi-session")),
            ("d", "dashboard"),
            ("o", tr("abrir", "open")),
            ("S", tr("ajustes", "settings")),
            ("M", "MCP"),
            ("p", "prompts"),
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
        ]);
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

    #[test]
    fn group_row_lists_groups_and_tab_bar_follows_active_tab() {
        let g = |n: String, g: &str| {
            let mut t = tab(&n);
            t.group = Some(g.into());
            t
        };
        let mut ts: Vec<Tab> = (0..26).map(|i| g(format!("svc-{i:02}"), "Core")).collect();
        ts.push(g("ui-app".into(), "UI"));
        let mut a = app(ts);
        let s = screen(&mut a, 160, 20);
        assert!(s.contains("Core 26") && s.contains("UI 1"), "{s}");
        a.current = 24;
        let s = screen(&mut a, 160, 20);
        assert!(s.contains("25 ") && s.contains("svc-24"), "{s}");
        assert!(!s.contains("1 ● svc-00"), "{s}");
    }

    #[test]
    fn status_bar_shows_group_and_multi_session_keys() {
        let mut a = app(vec![tab("a")]);
        let s = screen(&mut a, 200, 20);
        assert!(s.contains(" g "), "{s}");
        let mut b = tab("b");
        b.group = Some("X".into());
        a.tabs[0].group = Some("Y".into());
        a.tabs.push(b);
        let s = screen(&mut a, 200, 20);
        assert!(s.contains(" { } ") && s.contains("multi-sesión"), "{s}");
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
        let srv = |n: &str| super::super::mcp::Server {
            name: n.into(),
            kind: "local",
        };
        a.tabs[0].mcp = vec![(srv("pg"), true), (srv("web"), false)];
        let s = screen(&mut a, 120, 35);
        let lines: Vec<&str> = s.lines().collect();
        // 120 cols: command center + monthly box (2 rounded boxes), logo hidden.
        assert_eq!(
            lines[0].matches('╭').count(),
            2,
            "header top:\n{}",
            lines[0]
        );
        assert_eq!(
            lines[6].matches('╰').count(),
            2,
            "header bot:\n{}",
            lines[6]
        );
        assert!(lines[7].trim().is_empty(), "spacer row:\n{}", lines[7]);
        for needle in [
            "● pg · local",
            "○ web · local",
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
    fn transcript_cache_invalidates_on_theme_change() {
        let mut t = tab("alpha");
        t.push(Item::Agent("unique_cache_probe".into()));
        let mut a = app(vec![t]);

        let fg_of_probe = |a: &mut App| -> Color {
            let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
            term.draw(|f| draw(a, f)).unwrap();
            let buf = term.backend().buffer();
            let probe = "unique_cache_probe";
            for y in 0..30 {
                let cells: Vec<&str> = (0..100).map(|x| buf[(x, y)].symbol()).collect();
                if let Some(x) = cells.windows(probe.len()).position(|w| w.concat() == probe) {
                    return buf[(x as u16, y)].fg;
                }
            }
            panic!("probe text not found in rendered buffer");
        };

        let dark_fg = fg_of_probe(&mut a);
        let mut m = super::super::settings::Menu::new();
        m.on_key(ratatui::crossterm::event::KeyCode::Enter); // dark -> light
        let light_fg = fg_of_probe(&mut a);
        m.on_key(ratatui::crossterm::event::KeyCode::Enter); // restore dark

        assert_ne!(dark_fg, light_fg);
        assert_eq!(dark_fg, Color::Rgb(0xc0, 0xca, 0xf5));
        assert_eq!(light_fg, Color::Rgb(0x2a, 0x4a, 0x96));
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
    fn sidebar_shows_only_custom_subagents_and_hides_when_empty() {
        let def = |name: &str, model: Option<&str>, builtin: bool| crate::agents::AgentDef {
            name: name.into(),
            description: String::new(),
            model: model.map(Into::into),
            builtin,
        };
        let mut t = tab("alpha");
        t.defs = vec![
            def("explore", None, true),
            def("developer", Some("sonnet"), false),
            def("tester", Some("haiku"), false),
            def("reviewer", None, false),
        ]
        .into();
        t.assigned.insert("developer".into(), "opus".into());
        let mut a = app(vec![t, tab("empty")]);
        a.tabs[1].defs = vec![def("explore", None, true)].into();

        let s = screen(&mut a, 120, 35);
        assert!(
            s.contains("Subagentes") || s.contains("Subagents"),
            "missing title\n{s}"
        );
        for needle in ["◈ developer · 🔒  opus", "◈ tester · haiku", "◈ reviewer"] {
            assert!(s.contains(needle), "missing {needle}\n{s}");
        }
        assert!(!s.contains("◈ explore"), "builtin leaked into box\n{s}");

        // Coexisting with MCP at MIN_H does not panic and keeps dashboard visible.
        a.tabs[0].mcp = vec![
            (
                super::super::mcp::Server {
                    name: "pg".into(),
                    kind: "local",
                },
                true,
            ),
            (
                super::super::mcp::Server {
                    name: "web".into(),
                    kind: "http",
                },
                false,
            ),
        ];
        let small = screen(&mut a, 100, MIN_H);
        assert!(small.contains("dashboard"), "{small}");

        // Tab with only built-in defs hides the Subagentes box.
        a.current = 1;
        let empty = screen(&mut a, 120, 35);
        assert!(
            !empty.contains("Subagentes") && !empty.contains("Subagents"),
            "box should be hidden\n{empty}"
        );
    }

    #[test]
    fn sidebar_subagents_handles_budget_utf8_and_i18n() {
        let def = |name: &str, model: Option<&str>| crate::agents::AgentDef {
            name: name.into(),
            description: String::new(),
            model: model.map(Into::into),
            builtin: false,
        };
        let mut t = tab("alpha");
        let long_name = "diseñador-arquitectura-módulos-súper-largo";
        let mut defs = vec![def(long_name, Some("claude-sonnet-4.5"))];
        defs.extend((1..=10).map(|i| def(&format!("agent-{i}"), Some("haiku"))));
        t.defs = defs.into();
        t.assigned
            .insert(long_name.into(), "claude-opus-4.6-fast".into());
        t.mcp = (1..=10)
            .map(|i| {
                (
                    super::super::mcp::Server {
                        name: format!("srv-{i}"),
                        kind: "local",
                    },
                    true,
                )
            })
            .collect();
        let mut a = app(vec![t]);

        // Both boxes overloaded: dashboard, Subagentes and MCP all stay visible,
        // and the long UTF-8 entry wraps onto the next line instead of truncating.
        let es = screen(&mut a, 120, 35);
        assert!(es.contains("╭ dashboard "), "missing dashboard\n{es}");
        assert!(es.contains("╭ Subagentes "), "missing ES title\n{es}");
        assert!(es.contains("╭ MCP "), "missing MCP\n{es}");
        assert!(
            es.contains("◈ diseñador-arquitectura-módulos-súper-largo")
                && es.contains("claude-opus-4.6-fast"),
            "expected UTF-8 wrap without truncation\n{es}"
        );

        // Switching language updates the box title to English.
        let mut m = super::super::settings::Menu::new();
        m.on_key(ratatui::crossterm::event::KeyCode::Down);
        m.on_key(ratatui::crossterm::event::KeyCode::Enter); // ES -> EN
        let en = screen(&mut a, 120, 35);
        m.on_key(ratatui::crossterm::event::KeyCode::Enter); // restore ES
        assert!(en.contains("╭ Subagents "), "missing EN title\n{en}");
        assert!(!en.contains("╭ Subagentes "), "unexpected ES title\n{en}");

        // Only Subagentes (many items, no MCP): takes up to half without starving dashboard.
        a.tabs[0].mcp.clear();
        let subs_only = screen(&mut a, 120, 35);
        assert!(subs_only.contains("╭ dashboard "), "{subs_only}");
        assert!(subs_only.contains("◈ agent-6"), "{subs_only}");
        assert!(subs_only.contains("↑↓ 1-8/12"), "{subs_only}");
        assert!(!subs_only.contains("╭ MCP "), "{subs_only}");
    }

    #[test]
    fn sidebar_wraps_long_content_without_truncation() {
        let mut t = tab("proyecto-con-nombre-muy-largo-para-probar-wrap");
        t.tree.apply(r#"{"type":"assistant.turn_start","data":{}}"#);
        t.tree.apply(
            r#"{"type":"subagent.started","agentId":"a1","data":{"agentType":"explore","agentDisplayName":"investigador-super-detallado-del-repositorio"}}"#,
        );
        t.tree.apply(
            r#"{"type":"tool.execution_start","agentId":"a1","data":{"toolCallId":"c1","toolName":"buscar_patrones_en_muchos_archivos_del_proyecto"}}"#,
        );
        t.defs = vec![crate::agents::AgentDef {
            name: "arquitecto-de-sistemas-distribuidos".into(),
            description: String::new(),
            model: Some("claude-sonnet-4.5".into()),
            builtin: false,
        }];
        t.mcp = vec![(
            super::super::mcp::Server {
                name: "servidor-mcp-con-identificador-larguisimo".into(),
                kind: "local",
            },
            true,
        )];
        let a = app(vec![t]);
        let join = |ls: &[Line]| -> String {
            ls.iter()
                .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
                .collect()
        };

        let dash = sidebar_lines(&a, 24, 0);
        assert!(dash.len() > 4, "expected wrapped dashboard lines");
        assert!(dash.iter().all(|l| l.width() <= 24));
        let dash_txt = join(&dash);
        assert!(dash_txt.contains("probar-wrap") && !dash_txt.contains('…'));
        assert!(dash_txt.contains("del-repositorio"));
        assert!(dash_txt.contains("del_proyecto"));

        let subs = subagent_lines(&a.tabs[0], 24);
        assert!(subs.len() > 1);
        let subs_txt = join(&subs);
        assert!(subs_txt.contains("distribuidos") && subs_txt.contains("claude-sonnet-4.5"));
        assert!(!subs_txt.contains('…'));

        let mcps = mcp_lines(&a.tabs[0], 24);
        assert!(mcps.len() > 1);
        let mcps_txt = join(&mcps);
        assert!(mcps_txt.contains("larguisimo") && mcps_txt.contains("local"));
        assert!(!mcps_txt.contains('…'));
    }

    #[test]
    fn sidebar_groups_completed_keeps_running_top_and_separates_projects() {
        let mut t1 = tab("alpha");
        for l in [
            r#"{"type":"assistant.turn_start","data":{}}"#,
            r#"{"type":"session.usage_record","data":{"usage":{"copilotUsage":{"totalNanoAiu":1000000000}}}}"#,
            r#"{"type":"assistant.turn_end","data":{}}"#,
            r#"{"type":"subagent.started","agentId":"d1","data":{"agentType":"developer","agentDisplayName":"dev-1"}}"#,
            r#"{"type":"session.usage_record","agentId":"d1","data":{"usage":{"copilotUsage":{"totalNanoAiu":2000000000}}}}"#,
            r#"{"type":"subagent.completed","agentId":"d1","data":{"durationMs":100}}"#,
            r#"{"type":"subagent.started","agentId":"d2","data":{"agentType":"developer","agentDisplayName":"dev-2"}}"#,
            r#"{"type":"session.usage_record","agentId":"d2","data":{"usage":{"copilotUsage":{"totalNanoAiu":2000000000}}}}"#,
            r#"{"type":"subagent.completed","agentId":"d2","data":{"durationMs":100}}"#,
            r#"{"type":"subagent.started","agentId":"d3","data":{"agentType":"developer","agentDisplayName":"dev-3"}}"#,
            r#"{"type":"session.usage_record","agentId":"d3","data":{"usage":{"copilotUsage":{"totalNanoAiu":2000000000}}}}"#,
            r#"{"type":"subagent.completed","agentId":"d3","data":{"durationMs":100}}"#,
            r#"{"type":"subagent.started","agentId":"t1","data":{"agentType":"tester","agentDisplayName":"test-1"}}"#,
            r#"{"type":"session.usage_record","agentId":"t1","data":{"usage":{"copilotUsage":{"totalNanoAiu":1500000000}}}}"#,
            r#"{"type":"subagent.completed","agentId":"t1","data":{"durationMs":100}}"#,
            r#"{"type":"subagent.started","agentId":"t2","data":{"agentType":"tester","agentDisplayName":"test-2"}}"#,
            r#"{"type":"session.usage_record","agentId":"t2","data":{"usage":{"copilotUsage":{"totalNanoAiu":500000000}}}}"#,
            r#"{"type":"subagent.completed","agentId":"t2","data":{"cancelled":true}}"#,
            r#"{"type":"subagent.started","agentId":"c1","data":{"agentType":"reviewer","agentDisplayName":"rev-1"}}"#,
            r#"{"type":"subagent.completed","agentId":"c1","data":{"cancelled":true}}"#,
            r#"{"type":"subagent.started","agentId":"r1","data":{"agentType":"explore","agentDisplayName":"live-explorer"}}"#,
            r#"{"type":"session.usage_record","agentId":"r1","data":{"usage":{"copilotUsage":{"totalNanoAiu":1000000000}}}}"#,
            r#"{"type":"tool.execution_start","agentId":"r1","data":{"toolCallId":"tc1","toolName":"grep"}}"#,
        ] {
            t1.tree.apply(l);
        }
        let a = app(vec![t1, tab("beta")]);
        let text = |l: &Line| -> String { l.spans.iter().map(|s| s.content.as_ref()).collect() };

        let lines = sidebar_lines(&a, 48, 0);
        assert!(lines.iter().all(|l| l.width() <= 48));
        let rows: Vec<String> = lines.iter().map(text).collect();

        // Active tab header has surface background and right-aligned total cost.
        assert_eq!(lines[0].style.bg, Some(c_surface()));
        assert!(rows[0].contains("alpha") && rows[0].ends_with("10.0 cr · $0.10"));

        // Running agent appears first with its live tool and rail, before completed groups.
        let run_idx = rows
            .iter()
            .position(|r| r.contains("live-explorer"))
            .unwrap();
        let tool_idx = rows.iter().position(|r| r.contains("↳ grep")).unwrap();
        let main_idx = rows.iter().position(|r| r.contains("✔ main")).unwrap();
        let dev_idx = rows
            .iter()
            .position(|r| r.contains("developer ×3"))
            .unwrap();
        let test_idx = rows.iter().position(|r| r.contains("tester ×2")).unwrap();
        let rev_idx = rows.iter().position(|r| r.contains("✖ reviewer")).unwrap();
        assert!(run_idx < tool_idx && tool_idx < main_idx);
        assert!(main_idx < dev_idx && dev_idx < test_idx && test_idx < rev_idx);
        assert!(rows[run_idx].contains("1.0 cr"));
        assert!(rows[tool_idx].starts_with(" │ "));
        assert!(rows[dev_idx].contains('▰') && rows[dev_idx].ends_with("6.0 cr"));
        assert!(
            rows[test_idx].contains('✔')
                && rows[test_idx].contains('▰')
                && rows[test_idx].ends_with("2.0 cr")
        );

        // Separator between projects and idle label for second project without spend.
        let sep_idx = rows.iter().position(|r| r.contains('─')).unwrap();
        let beta_idx = rows.iter().position(|r| r.contains("beta")).unwrap();
        assert_eq!(sep_idx + 1, beta_idx);
        assert_eq!(lines[beta_idx].style.bg, None);
        assert!(rows[beta_idx].contains("sin uso") || rows[beta_idx].contains("idle"));

        // Narrow widths never panic and never overflow w (for w >= 1).
        for w in [0, 1, 2, 12, 24] {
            let narrow = sidebar_lines(&a, w, 0);
            assert!(!narrow.is_empty());
            if w >= 1 {
                assert!(
                    narrow.iter().all(|l| l.width() <= w),
                    "overflow at w={w}: {:?}",
                    narrow.iter().map(text).collect::<Vec<_>>()
                );
            }
        }
    }

    #[test]
    fn align_row_handles_alignment_surface_bg_wrap_and_wide_utf8() {
        let text = |l: &Line| -> String { l.spans.iter().map(|s| s.content.as_ref()).collect() };

        // Exact fit (lw + 1 + rw == w): single line, right-aligned, surface bg everywhere.
        let exact = align_row(
            vec![Span::styled("abc", bold(c_accent()))],
            vec![Span::styled("123", fg(c_orange()))],
            7,
            Some(c_surface()),
        );
        assert_eq!(exact.len(), 1);
        assert_eq!(text(&exact[0]), "abc 123");
        assert_eq!(exact[0].style.bg, Some(c_surface()));
        assert!(
            exact[0]
                .spans
                .iter()
                .all(|s| s.style.bg == Some(c_surface()))
        );
        assert_eq!(exact[0].spans[0].style.fg, Some(c_accent()));

        // No room for 1-space gap (lw + rw == w): wraps and pads every wrapped line to w.
        let wrapped = align_row(
            vec![Span::styled("abcd", fg(c_fg()))],
            vec![Span::styled("12", fg(c_orange()))],
            6,
            Some(c_surface()),
        );
        assert!(wrapped.len() >= 2);
        for l in &wrapped {
            assert_eq!(l.width(), 6);
            assert_eq!(l.style.bg, Some(c_surface()));
            assert!(l.spans.iter().all(|s| s.style.bg == Some(c_surface())));
        }

        // Empty right pads single line to w with surface bg.
        let solo = align_row(vec![Span::raw("hi")], vec![], 6, Some(c_surface()));
        assert_eq!(solo.len(), 1);
        assert_eq!(solo[0].width(), 6);
        assert_eq!(solo[0].style.bg, Some(c_surface()));

        // Extreme widths with wide UTF-8 never panic and never overflow w (for w >= 2).
        for w in [0, 1, 2, 10, 48] {
            let rows = align_row(
                vec![Span::styled("● 日本語🚀", fg(c_fg()))],
                vec![Span::styled("費用 1.0", fg(c_orange()))],
                w,
                Some(c_surface()),
            );
            assert!(!rows.is_empty());
            if w >= 2 {
                assert!(
                    rows.iter().all(|l| l.width() <= w),
                    "wide UTF-8 overflow at w={w}: {:?}",
                    rows.iter().map(text).collect::<Vec<_>>()
                );
            }
        }
    }

    #[test]
    fn sidebar_edge_cases_last_running_rail_ties_bar_overflow_and_dash_scroll() {
        use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let text = |l: &Line| -> String { l.spans.iter().map(|s| s.content.as_ref()).collect() };

        // 1) Running agent as the only/last item uses └ and blank rail (no │) for its tool.
        let mut only_run = tab("solo");
        for l in [
            r#"{"type":"subagent.started","agentId":"r1","data":{"agentType":"explore","agentDisplayName":"only-live"}}"#,
            r#"{"type":"tool.execution_start","agentId":"r1","data":{"toolCallId":"t1","toolName":"view"}}"#,
        ] {
            only_run.tree.apply(l);
        }
        let solo_rows: Vec<String> = sidebar_lines(&app(vec![only_run]), 40, 0)
            .iter()
            .map(text)
            .collect();
        let run_row = solo_rows.iter().find(|r| r.contains("only-live")).unwrap();
        let tool_row = solo_rows.iter().find(|r| r.contains("↳ view")).unwrap();
        assert!(run_row.starts_with(" └ "));
        assert!(tool_row.starts_with("     ↳ view") && !tool_row.contains('│'));

        // 2) Alphabetical tie-breaker on equal nano_aiu, large u64 rounding without overflow,
        //    tiny non-zero share clamps to 1 block, and bar hides when width is too narrow.
        let mut big = tab("scale");
        let half = u64::MAX / 2;
        let quarter = u64::MAX / 4;
        for l in [
            r#"{"type":"assistant.turn_start","data":{}}"#,
            r#"{"type":"session.usage_record","data":{"usage":{"copilotUsage":{"totalNanoAiu":1}}}}"#,
            r#"{"type":"assistant.turn_end","data":{}}"#,
            r#"{"type":"subagent.started","agentId":"z1","data":{"agentType":"zeta","agentDisplayName":"z1"}}"#,
            &format!(
                r#"{{"type":"session.usage_record","agentId":"z1","data":{{"usage":{{"copilotUsage":{{"totalNanoAiu":{quarter}}}}}}}}}"#
            ),
            r#"{"type":"subagent.completed","agentId":"z1","data":{"durationMs":10}}"#,
            r#"{"type":"subagent.started","agentId":"a1","data":{"agentType":"alpha","agentDisplayName":"a1"}}"#,
            &format!(
                r#"{{"type":"session.usage_record","agentId":"a1","data":{{"usage":{{"copilotUsage":{{"totalNanoAiu":{quarter}}}}}}}}}"#
            ),
            r#"{"type":"subagent.completed","agentId":"a1","data":{"durationMs":10}}"#,
            r#"{"type":"subagent.started","agentId":"h1","data":{"agentType":"heavy","agentDisplayName":"h1"}}"#,
            &format!(
                r#"{{"type":"session.usage_record","agentId":"h1","data":{{"usage":{{"copilotUsage":{{"totalNanoAiu":{half}}}}}}}}}"#
            ),
            r#"{"type":"subagent.completed","agentId":"h1","data":{"durationMs":10}}"#,
        ] {
            big.tree.apply(l);
        }
        let big_rows: Vec<String> = sidebar_lines(&app(vec![big]), 48, 0)
            .iter()
            .map(text)
            .collect();
        let main_i = big_rows.iter().position(|r| r.contains("✔ main")).unwrap();
        let heavy_i = big_rows.iter().position(|r| r.contains("✔ heavy")).unwrap();
        let alpha_i = big_rows.iter().position(|r| r.contains("✔ alpha")).unwrap();
        let zeta_i = big_rows.iter().position(|r| r.contains("✔ zeta")).unwrap();
        assert!(main_i < heavy_i && heavy_i < alpha_i && alpha_i < zeta_i);
        assert!(big_rows[main_i].contains("▰▱▱▱▱"));
        assert!(big_rows[heavy_i].contains("▰▰▰▱▱"));
        assert!(big_rows[alpha_i].contains("▰▱▱▱▱"));

        // 3) Bar is omitted when it would not fit in w, keeping single-line aligned cost.
        let mut fit = tab("fit");
        for l in [
            r#"{"type":"subagent.started","agentId":"d1","data":{"agentType":"developer","agentDisplayName":"d1"}}"#,
            r#"{"type":"session.usage_record","agentId":"d1","data":{"usage":{"copilotUsage":{"totalNanoAiu":1000000000}}}}"#,
            r#"{"type":"subagent.completed","agentId":"d1","data":{"durationMs":10}}"#,
        ] {
            fit.tree.apply(l);
        }
        let fit_app = app(vec![fit]);
        // " └ ✔ developer" (14) + 7 + "1.0 cr" (6) = 27 -> fits with full 5-block bar.
        let with_bar: Vec<String> = sidebar_lines(&fit_app, 27, 0).iter().map(text).collect();
        assert!(with_bar.iter().any(|r| r.contains("▰▰▰▰▰ 1.0 cr")));
        // At w=26 bar does not fit, so bar is omitted while cost stays right-aligned on 1 line.
        let no_bar: Vec<String> = sidebar_lines(&fit_app, 26, 0).iter().map(text).collect();
        let dev_row = no_bar.iter().find(|r| r.contains("developer")).unwrap();
        assert!(!dev_row.contains('▰') && dev_row.ends_with("1.0 cr"));
        assert_eq!(dev_row.width(), 26);

        // 4) Idle label respects tr("sin uso", "idle") and dashboard scrolls across many projects.
        let mut many = app((1..=14).map(|i| tab(&format!("proj-{i:02}"))).collect());
        let idle_lbl = tr("sin uso", "idle");
        let lines = sidebar_lines(&many, 48, 0);
        assert!(lines.iter().map(text).any(|r| r.contains(idle_lbl)));
        assert_eq!(lines.iter().filter(|l| text(l).contains('─')).count(), 13);

        let _ = screen(&mut many, 120, 22);
        many.focus = Pane::Dashboard;
        many.on_key(KeyEvent::new(KeyCode::End, KeyModifiers::NONE));
        assert!(many.dash_scroll.max > 0);
        assert_eq!(many.dash_scroll.off, many.dash_scroll.max);
        let bottom = screen(&mut many, 120, 22);
        assert!(
            bottom.contains("proj-14"),
            "expected last project\n{bottom}"
        );
        many.on_key(KeyEvent::new(KeyCode::Home, KeyModifiers::NONE));
        assert_eq!(many.dash_scroll.off, 0);
    }

    #[test]
    fn sidebar_focus_scroll_keyboard_mouse_and_fallback() {
        use ratatui::crossterm::event::{
            KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
        };
        let key = |c: KeyCode| KeyEvent::new(c, KeyModifiers::NONE);
        let mouse = |kind: MouseEventKind, col: u16, row: u16| MouseEvent {
            kind,
            column: col,
            row,
            modifiers: KeyModifiers::NONE,
        };

        let mut t = tab("alpha");
        t.defs = (1..=12)
            .map(|i| crate::agents::AgentDef {
                name: format!("sub-{i:02}"),
                description: String::new(),
                model: None,
                builtin: false,
            })
            .collect();
        t.mcp = (1..=12)
            .map(|i| {
                (
                    super::super::mcp::Server {
                        name: format!("mcp-{i:02}"),
                        kind: "local",
                    },
                    true,
                )
            })
            .collect();
        let mut a = app(vec![t, tab("empty")]);
        let _ = screen(&mut a, 120, 35);
        assert_eq!(a.focus, Pane::Transcript);
        assert_eq!(a.panes.len(), 4);

        // Cycle focus with ']' and '[' across all visible panes.
        a.on_key(key(KeyCode::Char(']')));
        assert_eq!(a.focus, Pane::Dashboard);
        a.on_key(key(KeyCode::Char(']')));
        assert_eq!(a.focus, Pane::Subagents);
        a.on_key(key(KeyCode::Char(']')));
        assert_eq!(a.focus, Pane::Mcp);
        a.on_key(key(KeyCode::Char('[')));
        assert_eq!(a.focus, Pane::Subagents);

        // Keyboard scroll moves only Subagents and clamps at max.
        a.on_key(key(KeyCode::Char('j')));
        a.on_key(key(KeyCode::Down));
        assert_eq!(a.tabs[0].sub_scroll.off, 2);
        assert_eq!(a.tabs[0].mcp_scroll.off, 0);
        assert_eq!(a.tabs[0].scroll.off, 0);
        a.on_key(key(KeyCode::End));
        let sub_max = a.tabs[0].sub_scroll.max;
        assert!(sub_max > 0);
        assert_eq!(a.tabs[0].sub_scroll.off, sub_max);
        a.on_key(key(KeyCode::Down));
        assert_eq!(a.tabs[0].sub_scroll.off, sub_max);
        let s = screen(&mut a, 120, 35);
        assert!(s.contains("sub-12"), "expected bottom subagent\n{s}");
        a.on_key(key(KeyCode::Home));
        assert_eq!(a.tabs[0].sub_scroll.off, 0);

        // Left click on MCP while typing exits typing, focuses MCP, and routes keyboard scroll.
        let mcp_rect = a
            .panes
            .iter()
            .find(|(p, _)| *p == Pane::Mcp)
            .map(|(_, r)| *r)
            .unwrap();
        a.tabs[0].typing = true;
        assert!(a.on_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            mcp_rect.x + 1,
            mcp_rect.y + 1
        )));
        assert_eq!(a.focus, Pane::Mcp);
        assert!(!a.tabs[0].typing);
        a.on_key(key(KeyCode::Down));
        assert_eq!(a.tabs[0].mcp_scroll.off, 1);

        // Mouse wheel scrolls hovered panel directly; mouse move does not mark dirty.
        assert!(!a.on_mouse(mouse(MouseEventKind::Moved, mcp_rect.x + 1, mcp_rect.y + 1)));
        assert!(a.on_mouse(mouse(
            MouseEventKind::ScrollDown,
            mcp_rect.x + 1,
            mcp_rect.y + 1
        )));
        assert_eq!(a.tabs[0].mcp_scroll.off, 4.min(a.tabs[0].mcp_scroll.max));
        assert!(a.on_mouse(mouse(
            MouseEventKind::ScrollUp,
            mcp_rect.x + 1,
            mcp_rect.y + 1
        )));
        assert_eq!(
            a.tabs[0].mcp_scroll.off,
            4.min(a.tabs[0].mcp_scroll.max) - 3
        );

        // Mouse is ignored when a modal is open.
        a.help = true;
        assert!(!a.on_mouse(mouse(
            MouseEventKind::ScrollDown,
            mcp_rect.x + 1,
            mcp_rect.y + 1
        )));
        a.help = false;

        // If focused box disappears (tab without MCP), focus falls back to Transcript.
        a.current = 1;
        let _ = screen(&mut a, 120, 35);
        assert_eq!(a.focus, Pane::Transcript);

        // Narrow terminal (< SIDEBAR_MIN_TOTAL) also resets focus to Transcript.
        a.current = 0;
        let _ = screen(&mut a, 120, 35);
        a.focus = Pane::Dashboard;
        let _ = screen(&mut a, 80, 35);
        assert_eq!(a.focus, Pane::Transcript);
    }

    #[test]
    fn sidebar_scroll_paging_clamping_tiny_height_and_narrow_wrap() {
        use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let key = |c: KeyCode| KeyEvent::new(c, KeyModifiers::NONE);

        let mut t = tab("日本語🚀");
        t.defs = (1..=14)
            .map(|i| crate::agents::AgentDef {
                name: format!("sub-{i:02}"),
                description: String::new(),
                model: None,
                builtin: false,
            })
            .collect();
        t.mcp = vec![(
            super::super::mcp::Server {
                name: "m1".into(),
                kind: "local",
            },
            true,
        )];
        let mut a = app(vec![t]);

        // Narrow width wrap with wide UTF-8 / emoji never panics and keeps styles.
        let narrow = sidebar_lines(&a, 1, 0);
        assert!(
            narrow
                .iter()
                .any(|l| l.spans.iter().any(|s| s.style.fg == Some(c_accent())))
        );

        // PgDn / PgUp step by inner_h - 1 on focused Subagents pane.
        let _ = screen(&mut a, 120, 30);
        a.focus = Pane::Subagents;
        let step = a.tabs[0].sub_scroll.page();
        a.on_key(key(KeyCode::PageDown));
        assert_eq!(a.tabs[0].sub_scroll.off, step.min(a.tabs[0].sub_scroll.max));
        a.on_key(key(KeyCode::PageUp));
        assert_eq!(a.tabs[0].sub_scroll.off, 0);

        // Shrinking line count clamps offset and hides ↑↓ when lines fit.
        a.on_key(key(KeyCode::End));
        assert!(a.tabs[0].sub_scroll.off > 0);
        a.tabs[0].defs.truncate(2);
        let s = screen(&mut a, 120, 30);
        assert_eq!(a.tabs[0].sub_scroll.off, 0);
        assert!(
            !s.contains("↑↓ 1-"),
            "indicator should hide when lines fit\n{s}"
        );

        // Tiny sidebar height (inner_h == 0) renders safely without ↑↓ indicator.
        a.tabs[0].defs = (1..=10)
            .map(|i| crate::agents::AgentDef {
                name: format!("sub-{i:02}"),
                description: String::new(),
                model: None,
                builtin: false,
            })
            .collect();
        "1\n2\n3\n4\n5\n6"
            .chars()
            .for_each(|c| a.tabs[0].editor.insert(c));
        let tiny = screen(&mut a, 100, 18);
        assert!(
            !tiny.contains("↑↓ 1-"),
            "indicator should hide at inner_h=0\n{tiny}"
        );
    }

    #[test]
    fn mouse_click_transcript_outside_full_dash_and_perm_modal() {
        use ratatui::crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
        let mouse = |kind: MouseEventKind, col: u16, row: u16| MouseEvent {
            kind,
            column: col,
            row,
            modifiers: KeyModifiers::NONE,
        };

        let mut a = app(vec![tab("alpha")]);
        let _ = screen(&mut a, 120, 35);
        let tr_rect = a
            .panes
            .iter()
            .find(|(p, _)| *p == Pane::Transcript)
            .map(|(_, r)| *r)
            .unwrap();

        // Click on Transcript from sidebar focuses Transcript; repeating returns false.
        a.focus = Pane::Dashboard;
        assert!(a.on_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            tr_rect.x + 1,
            tr_rect.y + 1
        )));
        assert_eq!(a.focus, Pane::Transcript);
        assert!(!a.on_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            tr_rect.x + 1,
            tr_rect.y + 1
        )));

        // Click on Transcript while typing keeps typing active.
        a.tabs[0].typing = true;
        assert!(!a.on_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            tr_rect.x + 1,
            tr_rect.y + 1
        )));
        assert!(a.tabs[0].typing);
        a.tabs[0].typing = false;

        // Click outside panes (row 0 tab bar) is ignored.
        a.focus = Pane::Dashboard;
        assert!(!a.on_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 5, 0)));
        assert_eq!(a.focus, Pane::Dashboard);

        // Mouse is ignored in Dash::Full and when a permission modal is open.
        a.dash = Dash::Full;
        assert!(!a.on_mouse(mouse(
            MouseEventKind::ScrollDown,
            tr_rect.x + 1,
            tr_rect.y + 1
        )));
        a.dash = Dash::Side;
        let (reply, _) = tokio::sync::oneshot::channel();
        a.tabs[0].perm = Some(super::super::Perm {
            params: serde_json::json!({}),
            reply,
        });
        assert!(!a.on_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            tr_rect.x + 1,
            tr_rect.y + 1
        )));
    }

    #[test]
    fn sidebar_scroll_is_per_tab_and_prompt_apply_resets_focus() {
        use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let key = |c: KeyCode| KeyEvent::new(c, KeyModifiers::NONE);
        let make_tab = |name: &str| {
            let mut t = tab(name);
            t.defs = (1..=12)
                .map(|i| crate::agents::AgentDef {
                    name: format!("{name}-sub-{i:02}"),
                    description: String::new(),
                    model: None,
                    builtin: false,
                })
                .collect();
            t.mcp = (1..=12)
                .map(|i| {
                    (
                        super::super::mcp::Server {
                            name: format!("{name}-mcp-{i:02}"),
                            kind: "local",
                        },
                        true,
                    )
                })
                .collect();
            t
        };

        let mut a = app(vec![make_tab("a"), make_tab("b")]);
        let _ = screen(&mut a, 120, 35);

        // Scroll Subagents and MCP in tab 0.
        a.focus = Pane::Subagents;
        a.on_key(key(KeyCode::Down));
        a.on_key(key(KeyCode::Down));
        a.focus = Pane::Mcp;
        a.on_key(key(KeyCode::Down));
        assert_eq!((a.tabs[0].sub_scroll.off, a.tabs[0].mcp_scroll.off), (2, 1));

        // Switching to tab 1 keeps tab 1 at 0 and does not leak tab 0's scroll.
        a.on_key(key(KeyCode::Tab));
        let _ = screen(&mut a, 120, 35);
        assert_eq!((a.tabs[1].sub_scroll.off, a.tabs[1].mcp_scroll.off), (0, 0));
        a.on_key(key(KeyCode::Down));
        a.on_key(key(KeyCode::Down));
        assert_eq!(a.tabs[1].mcp_scroll.off, 2);

        // Switching back preserves tab 0's independent offsets.
        a.on_key(key(KeyCode::BackTab));
        let _ = screen(&mut a, 120, 35);
        assert_eq!((a.tabs[0].sub_scroll.off, a.tabs[0].mcp_scroll.off), (2, 1));

        // Applying a saved prompt while focused on a sidebar pane resets focus to Transcript.
        a.focus = Pane::Mcp;
        a.on_key(key(KeyCode::Char('p')));
        a.on_key(key(KeyCode::Char('n')));
        a.on_key(key(KeyCode::Char('t')));
        a.on_key(key(KeyCode::Tab));
        a.on_key(key(KeyCode::Char('x')));
        a.on_key(key(KeyCode::Enter)); // save form -> back to list
        a.on_key(key(KeyCode::Enter)); // apply prompt
        assert_eq!(a.focus, Pane::Transcript);
        assert!(a.tabs[0].typing);
        assert_eq!(a.tabs[0].editor.text, "x");
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

    #[test]
    fn renders_month_cost_in_header_wide_and_narrow() {
        let mut a = app(vec![tab("alpha")]);
        a.month_label = "2026-11".into();

        // Loading state before MonthUsage arrives.
        let loading = screen(&mut a, 100, 25);
        assert!(
            loading.contains("2026-11")
                && (loading.contains("calculando…") || loading.contains("calculating…")),
            "{loading}"
        );

        // Loaded state with usage: credits on row 2, USD on row 3.
        a.month_loaded = true;
        a.month_sessions.insert("s1".into(), 12_500_000_000);

        // Wide terminal (145x35): 3 independent rounded boxes (command center, month, logo).
        let wide = screen(&mut a, 145, 35);
        let wide_lines: Vec<&str> = wide.lines().collect();
        assert_eq!(wide_lines[0].matches('╭').count(), 3, "{wide}");
        assert_eq!(wide_lines[6].matches('╰').count(), 3, "{wide}");
        assert!(
            wide_lines[0].contains("mes 2026-11") || wide_lines[0].contains("month 2026-11"),
            "{}",
            wide_lines[0]
        );
        assert!(wide_lines[2].contains("12.50 cr"), "{}", wide_lines[2]);
        assert!(wide_lines[3].contains("$0.125"), "{}", wide_lines[3]);
        assert!(wide.contains("C O P I L O T"), "{wide}");

        // Narrow terminal (90x25): hides logo first, keeps 2 rounded boxes (command center + month).
        let narrow = screen(&mut a, 90, 25);
        let narrow_lines: Vec<&str> = narrow.lines().collect();
        assert_eq!(narrow_lines[0].matches('╭').count(), 2, "{narrow}");
        assert_eq!(narrow_lines[6].matches('╰').count(), 2, "{narrow}");
        assert!(narrow.contains("2026-11"), "{narrow}");
        assert!(narrow_lines[2].contains("12.50 cr"), "{}", narrow_lines[2]);
        assert!(narrow_lines[3].contains("$0.125"), "{}", narrow_lines[3]);
        assert!(!narrow.contains("C O P I L O T"), "{narrow}");

        // Minimum terminal (60x18): hides monthly block cleanly, leaving 1 rounded box.
        let min = screen(&mut a, MIN_W, MIN_H);
        let min_lines: Vec<&str> = min.lines().collect();
        assert_eq!(min_lines[0].matches('╭').count(), 1, "{min}");
        assert_eq!(min_lines[6].matches('╰').count(), 1, "{min}");
        assert!(!min.contains("2026-11"), "{min}");
        assert!(!min.contains("12.50 cr"), "{min}");

        // Loaded with large usage (27045.79 cr / $270.46): fits on 2 centered lines.
        a.month_sessions.insert("s1".into(), 27_045_790_000_000);
        let big = screen(&mut a, 90, 25);
        let big_lines: Vec<&str> = big.lines().collect();
        assert!(big_lines[2].contains("27045.79 cr"), "{}", big_lines[2]);
        assert!(big_lines[3].contains("$270.46"), "{}", big_lines[3]);

        // Loaded with zero usage shows 0.00 cr / $0.000 on 2 lines (not loading).
        a.month_sessions.clear();
        let zero = screen(&mut a, 90, 25);
        let zero_lines: Vec<&str> = zero.lines().collect();
        assert!(zero.contains("2026-11"), "{zero}");
        assert!(zero_lines[2].contains("0.00 cr"), "{}", zero_lines[2]);
        assert!(zero_lines[3].contains("$0.000"), "{}", zero_lines[3]);
        assert!(
            zero.contains("todos los proyectos") || zero.contains("all projects"),
            "{zero}"
        );
        assert!(
            !zero.contains("calculando…") && !zero.contains("calculating…"),
            "{zero}"
        );
    }

    #[test]
    fn header_boxes_cutoffs_centering_and_i18n() {
        let mut a = app(vec![tab("alpha")]);
        a.month_label = "2026-11".into();
        a.month_loaded = true;
        a.month_sessions.insert("s1".into(), 12_500_000_000);

        // Width cutoffs: 4 boxes (>=173), 3 boxes (125..=172: patrol+month or month+logo),
        // 2 boxes (63..=124), 1 box (60..=62).
        for (w, boxes, has_patrol, has_month, has_logo) in [
            (180, 4, true, true, true),
            (173, 4, true, true, true),
            (172, 3, false, true, true),
            (165, 3, false, true, true),
            (155, 3, false, true, true),
            (145, 3, false, true, true),
            (135, 3, false, true, true),
            (134, 3, true, true, false),
            (125, 3, true, true, false),
            (124, 2, false, true, false),
            (120, 2, false, true, false),
            (110, 2, false, true, false),
            (90, 2, false, true, false),
            (63, 2, false, true, false),
            (62, 1, false, false, false),
            (60, 1, false, false, false),
        ] {
            let s = screen(&mut a, w, 25);
            let ls: Vec<&str> = s.lines().collect();
            assert_eq!(ls[0].matches('╭').count(), boxes, "w={w}\n{s}");
            assert_eq!(ls[0].matches('╮').count(), boxes, "w={w}\n{s}");
            assert_eq!(ls[6].matches('╰').count(), boxes, "w={w}\n{s}");
            assert_eq!(ls[6].matches('╯').count(), boxes, "w={w}\n{s}");
            assert!(ls[7].trim().is_empty(), "w={w} separator clobbered\n{s}");
            assert!(ls[8].contains("in 0 · out 0"), "w={w} info bar\n{s}");
            assert!(ls[5].contains("alpha"), "w={w} tab clipped\n{s}");
            if has_patrol || matches!(w, 110 | 120 | 124 | 145 | 155 | 165 | 172) {
                assert!(
                    ls[3].contains("0.00 cr · $0.000"),
                    "w={w} summary clipped\n{s}"
                );
            }
            assert_eq!(
                ls[0].contains("reposo") || ls[0].contains("idle"),
                has_patrol,
                "w={w}\n{s}"
            );
            assert_eq!(
                ls[2].contains("12.50 cr") && ls[3].contains("$0.125"),
                has_month,
                "w={w}\n{s}"
            );
            assert_eq!(s.contains("C O P I L O T"), has_logo, "w={w}\n{s}");
        }

        // Centering in month box (rows 2, 3, 5) and logo box (rows 1..=5) at exact 3-box cutoff.
        let s135 = screen(&mut a, 135, 25);
        let ls: Vec<&str> = s135.lines().collect();
        let centered_ok = |cell: &str| {
            let lead = cell.chars().take_while(|c| *c == ' ').count();
            let trail = cell.chars().rev().take_while(|c| *c == ' ').count();
            lead.abs_diff(trail) <= 1
        };
        for row in [2, 3, 5] {
            let month_inner: String = ls[row].chars().skip(64).take(22).collect();
            assert!(centered_ok(&month_inner), "row {row}: `{month_inner}`");
        }
        for (row, line) in ls.iter().enumerate().take(6).skip(1) {
            let logo_inner: String = line.chars().skip(88).take(46).collect();
            assert!(centered_ok(&logo_inner), "logo row {row}: `{logo_inner}`");
            assert!(
                logo_inner.starts_with(' ') && logo_inner.ends_with(' '),
                "logo row {row}: `{logo_inner}`"
            );
        }

        // i18n: Spanish default and English toggle (loading, loaded, and patrol box title).
        a.month_loaded = false;
        a.month_sessions.clear();
        let es = screen(&mut a, 180, 25);
        for needle in [
            "◆ centro de mando",
            "☾ reposo",
            "mes 2026-11",
            "calculando…",
            "todos los proyectos",
        ] {
            assert!(es.contains(needle), "ES missing `{needle}`\n{es}");
        }
        let mut m = super::super::settings::Menu::new();
        m.on_key(ratatui::crossterm::event::KeyCode::Down);
        m.on_key(ratatui::crossterm::event::KeyCode::Enter); // ES -> EN
        let en = screen(&mut a, 180, 25);
        m.on_key(ratatui::crossterm::event::KeyCode::Enter); // restore ES
        for needle in [
            "◆ command center",
            "☾ idle",
            "month 2026-11",
            "calculating…",
            "all projects",
        ] {
            assert!(en.contains(needle), "EN missing `{needle}`\n{en}");
        }
    }

    #[test]
    fn patrol_states_dimensions_and_motion() {
        let txt = |ls: &[Line]| -> String {
            ls.iter()
                .map(|l| {
                    l.spans
                        .iter()
                        .map(|s| s.content.as_ref())
                        .collect::<String>()
                })
                .collect::<Vec<_>>()
                .join("\n")
        };
        let col_of = |line: &Line, needle: &str| -> Option<usize> {
            let s: String = line.spans.iter().map(|sp| sp.content.as_ref()).collect();
            s.find(needle).map(|b| s[..b].chars().count())
        };

        // Never panics, underflows, or exceeds w across 0..=30, ticks 0..200, all state combos.
        for w in 0..=30 {
            for working in [false, true] {
                for subs in [0, 2] {
                    for perm in [false, true] {
                        for tick in 0..200 {
                            let ls = patrol(tick, working, subs, perm, w);
                            assert_eq!(ls.len(), 5, "w={w} tick={tick}");
                            for (r, l) in ls.iter().enumerate() {
                                assert!(
                                    l.width() <= w,
                                    "row {r} exceeded w={w}: width={} `{}`",
                                    l.width(),
                                    txt(&ls)
                                );
                            }
                        }
                    }
                }
            }
        }

        // Priority: pending_perm > working > idle (even with subs > 0).
        let perm = txt(&patrol(0, true, 2, true, 26));
        assert!(
            perm.contains('⚠')
                && perm.contains("\\▄███▄/")
                && perm.contains('═')
                && !perm.contains('▓'),
            "perm should override working:\n{perm}"
        );
        let idle_with_subs = txt(&patrol(0, false, 2, false, 26));
        assert!(
            idle_with_subs.contains("- -") && !idle_with_subs.contains("[•]"),
            "idle should ignore subs:\n{idle_with_subs}"
        );

        // Idle: moon, resting eyes, calm track, and z / zZ toggle every 20 ticks.
        let idle0 = txt(&patrol(0, false, 0, false, 26));
        let idle20 = txt(&patrol(20, false, 0, false, 26));
        assert!(
            idle0.contains('☾')
                && idle0.contains("- -")
                && idle0.contains('z')
                && idle0.contains('─')
                && idle0.contains('·'),
            "{idle0}"
        );
        assert!(idle20.contains("zZ"), "{idle20}");
        assert!(patrol_idle_tick(0) && !patrol_idle_tick(1) && patrol_idle_tick(20));

        // Working ping-pong on w=22 (span=16): moves right then left and returns to start.
        let mut xs = Vec::new();
        for step in 0..=32u64 {
            let ls = patrol(step * 2, true, 0, false, 22);
            let s = txt(&ls);
            assert!(!s.contains("[•]"));
            assert!(s.contains('╠') && s.contains('╣'));
            let box_x = col_of(&ls[2], "█")
                .unwrap()
                .min(col_of(&ls[2], "▓").unwrap());
            xs.push(box_x);
        }
        assert_eq!(xs[0], 0);
        assert_eq!(xs[16], 16);
        assert_eq!(xs[32], 0);
        assert!(xs[..=16].windows(2).all(|p| p[1] == p[0] + 1));
        assert!(xs[16..].windows(2).all(|p| p[0] == p[1] + 1));

        // Working with subagents: mini-drone [•] trails behind in both directions.
        let right_ls = patrol(10, true, 2, false, 22);
        let r_drone = col_of(&right_ls[2], "[•]").expect("missing drone going right");
        let r_bot = col_of(&right_ls[2], "█").unwrap();
        assert!(r_drone + 3 < r_bot, "drone not behind when going right");

        let left_ls = patrol(46, true, 2, false, 22);
        let l_drone = col_of(&left_ls[2], "[•]").expect("missing drone going left");
        let l_bot = col_of(&left_ls[2], "█").unwrap();
        assert!(l_drone > l_bot + 4, "drone not behind when going left");
    }

    #[test]
    fn patrol_renders_in_wide_header_and_hides_in_narrow() {
        let mut a = app(vec![busy_tab()]);
        a.tick = 10; // quip is active on tick 10
        let q = quip(a.tick).unwrap();

        // Wide terminal (180x35): 4 independent boxes (cmd, patrol, month, logo), no clipping.
        let wide = screen(&mut a, 180, 35);
        let header_wide = wide.lines().take(7).collect::<Vec<_>>().join("\n");
        assert_eq!(wide.lines().next().unwrap().matches('╭').count(), 4);
        assert!(
            header_wide.contains("✦ colmena")
                && header_wide.contains('▓')
                && header_wide.contains("[•]"),
            "patrol box missing in wide header:\n{header_wide}"
        );
        assert!(
            header_wide.contains(q)
                && header_wide.contains("1 proyectos")
                && header_wide.contains("alpha"),
            "wide header clipped content:\n{header_wide}"
        );

        // Medium terminal (130x25): logo hides, patrol box fits alongside cmd + month (3 boxes).
        let med = screen(&mut a, 130, 25);
        let header_med = med.lines().take(7).collect::<Vec<_>>().join("\n");
        assert_eq!(med.lines().next().unwrap().matches('╭').count(), 3);
        assert!(
            header_med.contains("✦ colmena") && header_med.contains('▓'),
            "patrol box missing in medium header:\n{header_med}"
        );
        assert!(!header_med.contains("C O P I L O T"));

        // Tight width where quip + summary fit, but patrol would clip: patrol hides cleanly (2 boxes).
        let tight = screen(&mut a, 92, 20);
        let header_tight = tight.lines().take(7).collect::<Vec<_>>().join("\n");
        assert_eq!(tight.lines().next().unwrap().matches('╭').count(), 2);
        assert!(
            !header_tight.contains('▓') && !header_tight.contains("[•]"),
            "patrol should hide when space is tight:\n{header_tight}"
        );
        assert!(
            header_tight.contains(q)
                && header_tight.contains("1 proyectos")
                && header_tight.contains("alpha"),
            "tight header clipped content:\n{header_tight}"
        );

        // Narrow terminal (60x18): patrol hides cleanly without breaking tabs (1 box).
        let narrow = screen(&mut a, 60, 18);
        let header_narrow = narrow.lines().take(7).collect::<Vec<_>>().join("\n");
        assert_eq!(narrow.lines().next().unwrap().matches('╭').count(), 1);
        assert!(
            !header_narrow.contains('▓') && !header_narrow.contains("[•]"),
            "patrol should hide in narrow header:\n{header_narrow}"
        );
        assert!(
            header_narrow.contains("alpha"),
            "tab broken in narrow header:\n{header_narrow}"
        );

        // Pending permission switches the patrol box title to alert and shows the alert bot.
        let (reply, _rx) = tokio::sync::oneshot::channel();
        a.tabs[0].perm = Some(Perm {
            params: json!({}),
            reply,
        });
        let alert = screen(&mut a, 180, 35);
        let header_alert = alert.lines().take(7).collect::<Vec<_>>().join("\n");
        assert!(
            header_alert.contains("⚠ alerta") && header_alert.contains("\\▄███▄/"),
            "alert patrol box missing:\n{header_alert}"
        );
    }
}
