//! Tabbed terminal UI: one ACP session per project, live agent monitoring, token usage.
/// `format!` choosing the Spanish or English template by current UI language.
macro_rules! tr_fmt {
    ($es:literal, $en:literal $(, $a:expr)* $(,)?) => {
        if crate::tui::settings::is_en() { format!($en $(, $a)*) } else { format!($es $(, $a)*) }
    };
}

mod clipboard;
mod dashboard;
mod history;
mod input;
mod markdown;
mod mcp;
mod modals;
mod open;
mod prompts;
mod session;
mod settings;
mod theme;
mod widgets;

use crate::acp::{self, ConfigOpt, Ctl, Perm, Update};
use crate::agents::{self, AgentDef};
use crate::config::Project;
use crate::events::{Status, Tree};
use anyhow::{Context, Result, bail};
use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{
    self, Event as Term, KeyCode, KeyEvent, KeyEventKind, KeyModifiers,
};
use ratatui::text::Line;
use session::spawn_tab;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

type Tx = UnboundedSender<(usize, Msg)>;

enum Msg {
    Update(Update),
    Perm(Perm),
    Lines(Vec<String>),
    Ready,
    Session(String),
    Done(String),
    Warn(String),
    Failed(String),
    History(history::Loaded),
    Open(open::Loaded),
}

#[derive(PartialEq, Clone, Copy, Debug)]
enum ToolState {
    Running,
    Ok,
    Failed,
}

enum Item {
    User(String),
    /// Markdown; consecutive chunks are appended.
    Agent(String),
    Tool {
        id: String,
        title: String,
        kind: String,
        state: ToolState,
    },
    Note(String),
}

enum Life {
    Starting,
    Ready,
    Failed(String),
}

/// Scroll position as rows above the bottom; 0 follows new output.
#[derive(Default)]
struct Scroll {
    off: usize,
    unseen: bool,
    last_total: usize,
    height: usize,
}

impl Scroll {
    /// Called on every draw with the current row count and viewport height.
    fn sync(&mut self, total: usize, height: usize) {
        if self.off > 0 && total > self.last_total {
            self.off += total - self.last_total;
            self.unseen = true;
        }
        self.last_total = total;
        self.height = height;
        self.off = self.off.min(total.saturating_sub(height));
        if self.off == 0 {
            self.unseen = false;
        }
    }

    fn up(&mut self, n: usize) {
        self.off = self.off.saturating_add(n);
    }

    fn down(&mut self, n: usize) {
        self.off = self.off.saturating_sub(n);
    }

    fn page(&self) -> usize {
        self.height.saturating_sub(1).max(1)
    }
}

struct Cache {
    width: usize,
    rev: u64,
    light: bool,
    lines: Vec<Line<'static>>,
}

struct Tab {
    /// Stable id: messages from tasks carry it, tab positions shift on close.
    id: usize,
    /// Aborted on drop so closing the tab kills the session task and its copilot child.
    task: Option<tokio::task::AbortHandle>,
    name: String,
    cwd: String,
    sid: Option<String>,
    items: Vec<Item>,
    rev: u64,
    cache: Option<Cache>,
    tree: Tree,
    perm: Option<Perm>,
    /// Context window usage from ACP `usage_update`: (used, size).
    ctx: Option<(u64, u64)>,
    life: Life,
    busy: bool,
    editor: input::Editor,
    typing: bool,
    scroll: Scroll,
    prompts: UnboundedSender<(String, Vec<String>)>,
    /// Base64 PNGs pasted into the draft, sent with the next prompt.
    images: Vec<String>,
    ctl: UnboundedSender<Ctl>,
    /// ACP session config options (model, reasoning_effort, allow_all...).
    config: Vec<ConfigOpt>,
    /// Auto-approve every permission request (mirrors the ACP `allow_all` option).
    allow_all: bool,
    defs: Vec<AgentDef>,
    /// Mandatory models from the Copilot user settings (`subagents.agents.<name>.model`).
    assigned: HashMap<String, String>,
    /// Selected row of the full dashboard, by agent name so it survives re-sorting.
    dash_sel: Option<String>,
    /// MCP servers the session was started with, and whether each is enabled.
    mcp: Vec<(mcp::Server, bool)>,
}

impl Drop for Tab {
    fn drop(&mut self) {
        if let Some(t) = &self.task {
            t.abort();
        }
    }
}

impl Tab {
    fn new(
        name: String,
        prompts: UnboundedSender<(String, Vec<String>)>,
        ctl: UnboundedSender<Ctl>,
        defs: Vec<AgentDef>,
        assigned: HashMap<String, String>,
    ) -> Self {
        Tab {
            id: 0,
            task: None,
            name,
            cwd: String::new(),
            sid: None,
            items: vec![],
            rev: 0,
            cache: None,
            tree: Tree::default(),
            perm: None,
            ctx: None,
            life: Life::Starting,
            busy: false,
            editor: input::Editor::default(),
            typing: false,
            scroll: Scroll::default(),
            prompts,
            images: vec![],
            ctl,
            config: vec![],
            allow_all: false,
            defs,
            assigned,
            dash_sel: None,
            mcp: vec![],
        }
    }

    fn config_opt(&self, id: &str) -> Option<&ConfigOpt> {
        self.config.iter().find(|o| o.id == id)
    }

    /// Observed model id, falling back to the ACP config value.
    fn model(&self) -> Option<&str> {
        self.tree
            .model()
            .or_else(|| self.config_opt("model").map(|o| o.current.as_str()))
    }

    fn set_config(&self, id: &str, value: &str) {
        let _ = self.ctl.send(Ctl::SetConfig {
            id: id.into(),
            value: value.into(),
        });
    }

    fn set_allow_all(&mut self, on: bool) {
        self.allow_all = on;
        self.set_config("allow_all", if on { "on" } else { "off" });
        if on {
            self.answer(acp::approve_option);
        }
    }

    fn working(&self) -> bool {
        !matches!(self.life, Life::Failed(_))
            && (self.busy
                || self
                    .tree
                    .agents
                    .first()
                    .is_some_and(|a| a.id.is_none() && a.status == Status::Running))
    }

    fn push(&mut self, item: Item) {
        self.items.push(item);
        self.rev += 1;
    }

    fn on_update(&mut self, u: Update) {
        match u {
            Update::Text(s) => {
                self.rev += 1;
                match self.items.last_mut() {
                    Some(Item::Agent(t)) => t.push_str(&s),
                    _ => self.items.push(Item::Agent(s)),
                }
            }
            Update::User(s) => {
                self.rev += 1;
                match self.items.last_mut() {
                    Some(Item::User(t)) => t.push_str(&s),
                    _ => self.items.push(Item::User(s)),
                }
            }
            Update::Usage { used, size } => self.ctx = Some((used, size)),
            Update::Config(opts) => {
                if let Some(o) = opts.iter().find(|o| o.id == "allow_all") {
                    self.allow_all = o.current == "on";
                }
                self.config = opts;
            }
            Update::Tool {
                id,
                title,
                kind,
                status,
            } => {
                let state = match status.as_deref() {
                    Some("completed") => Some(ToolState::Ok),
                    Some("failed") => Some(ToolState::Failed),
                    Some(_) => Some(ToolState::Running),
                    None => None,
                };
                self.rev += 1;
                let found = self.items.iter_mut().find_map(|i| match i {
                    Item::Tool {
                        id: i,
                        title,
                        kind,
                        state,
                    } if *i == id => Some((title, kind, state)),
                    _ => None,
                });
                match found {
                    Some((t, k, s)) => {
                        if let Some(x) = title {
                            *t = x;
                        }
                        if let Some(x) = kind {
                            *k = x;
                        }
                        *s = state.unwrap_or(*s);
                    }
                    None => self.items.push(Item::Tool {
                        title: title.unwrap_or_else(|| "tool".into()),
                        kind: kind.unwrap_or_default(),
                        state: state.unwrap_or(ToolState::Running),
                        id,
                    }),
                }
            }
        }
    }

    fn send_prompt(&mut self) {
        let text = self.editor.text.trim().to_string();
        if text.is_empty() && self.images.is_empty() {
            return;
        }
        let n = self.images.len();
        if self
            .prompts
            .send((text.clone(), self.images.clone()))
            .is_ok()
        {
            self.editor.take();
            self.images.clear();
            let shown = match (text.is_empty(), n) {
                (_, 0) => text,
                (true, n) => format!("📎 ×{n}"),
                (false, n) => format!("📎 ×{n} {text}"),
            };
            self.push(Item::User(shown));
            self.busy = true;
            self.scroll.off = 0;
        } else {
            self.push(Item::Note(
                settings::tr(
                    "la sesión no está disponible; el borrador se conserva",
                    "session unavailable; draft kept",
                )
                .into(),
            ));
        }
    }

    fn cancel_turn(&mut self) {
        if self.working() {
            // Dropping the pending permission replies "cancelled" so the request loop can continue.
            self.perm = None;
            let _ = self.ctl.send(Ctl::Cancel);
        }
    }

    fn answer(&mut self, pick: for<'a> fn(&'a serde_json::Value) -> Option<&'a str>) {
        if let Some(p) = self.perm.take() {
            let _ = p.reply.send(pick(&p.params).map(str::to_string));
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Dash {
    Side,
    Full,
    Hidden,
}

#[derive(PartialEq)]
enum Confirm {
    Quit,
    AllowAll,
    CloseTab,
}

enum SelTarget {
    Config(String),
    AgentModel(String),
}

struct Selector {
    title: String,
    target: SelTarget,
    /// (value, label)
    items: Vec<(String, String)>,
    sel: usize,
}

pub struct App {
    tabs: Vec<Tab>,
    next_id: usize,
    current: usize,
    dash: Dash,
    help: bool,
    confirm: Option<Confirm>,
    selector: Option<Selector>,
    history: Option<history::History>,
    open: Option<open::Browser>,
    settings: Option<settings::Menu>,
    mcp: Option<mcp::Menu>,
    prompts: Option<prompts::Menu>,
    tx: Tx,
    tick: u64,
}

impl App {
    fn on_msg(&mut self, id: usize, m: Msg) {
        if let Msg::History(ev) = m {
            if let Some(h) = &mut self.history {
                h.on_loaded(ev);
            }
            return;
        }
        if let Msg::Open(ev) = m {
            if let Some(o) = &mut self.open {
                o.on_loaded(ev);
            }
            return;
        }
        // Late messages from a closed tab find no id and are dropped.
        let Some(t) = self.tabs.iter_mut().find(|t| t.id == id) else {
            return;
        };
        match m {
            Msg::Update(u) => t.on_update(u),
            Msg::Perm(p) if t.allow_all => {
                let title = p.params["toolCall"]["title"].as_str().unwrap_or("tool");
                let note = tr_fmt!(
                    "↯ auto-aprobado (allow all): {title}",
                    "↯ auto-approved (allow all): {title}"
                );
                let _ = p
                    .reply
                    .send(acp::approve_option(&p.params).map(str::to_string));
                t.push(Item::Note(note));
            }
            Msg::Perm(p) => t.perm = Some(p),
            Msg::Lines(ls) => ls.iter().for_each(|l| t.tree.apply(l)),
            Msg::Ready => t.life = Life::Ready,
            Msg::Session(s) => t.sid = Some(s),
            Msg::Done(reason) => {
                t.busy = false;
                t.perm = None;
                if reason == "cancelled" {
                    t.push(Item::Note(
                        settings::tr("turno cancelado", "turn cancelled").into(),
                    ));
                }
            }
            Msg::Warn(w) => t.push(Item::Note(w)),
            Msg::Failed(e) => {
                t.life = Life::Failed(e);
                t.busy = false;
                t.perm = None;
            }
            Msg::History(_) | Msg::Open(_) => {}
        }
    }

    fn animating(&self) -> bool {
        self.tabs
            .iter()
            .any(|t| t.perm.is_some() || t.working() || matches!(t.life, Life::Starting))
    }

    fn switch(&mut self, to: usize) {
        if to < self.tabs.len() {
            self.current = to;
        }
    }

    /// Returns true to quit.
    fn on_key(&mut self, k: KeyEvent) -> bool {
        let (code, mods) = (k.code, k.modifiers);
        let n = self.tabs.len();
        if self.help {
            self.help = false;
            return false;
        }
        if let Some(c) = self.confirm.take() {
            if code != KeyCode::Char('y') {
                return false;
            }
            match c {
                Confirm::Quit => return true,
                Confirm::CloseTab => self.close_current(),
                Confirm::AllowAll => self.tabs[self.current].set_allow_all(true),
            }
            return false;
        }
        if self.selector.is_some() {
            self.on_selector_key(code);
            return false;
        }
        if let Some(m) = &mut self.settings {
            if m.on_key(code) {
                self.settings = None;
            }
            return false;
        }
        if let Some(m) = &mut self.mcp {
            match m.on_key(code) {
                mcp::Action::None => {}
                mcp::Action::Close => self.mcp = None,
                mcp::Action::Apply => {
                    self.mcp = None;
                    self.restart_current();
                }
            }
            return false;
        }
        if let Some(p) = &mut self.prompts {
            match p.on_key(k) {
                prompts::Action::None => {}
                prompts::Action::Close => self.prompts = None,
                prompts::Action::Apply(text) => {
                    self.prompts = None;
                    let t = &mut self.tabs[self.current];
                    text.chars().for_each(|c| t.editor.insert(c));
                    t.typing = true;
                    if self.dash == Dash::Full {
                        self.dash = Dash::Side;
                    }
                }
            }
            return false;
        }
        if let Some(o) = &mut self.open {
            match o.on_key(code) {
                open::Action::None => {}
                open::Action::Close => self.open = None,
                open::Action::Load(req, dir, hidden) => {
                    spawn_read_dir(req, dir, hidden, &self.tx);
                }
                open::Action::Open(path) => {
                    self.open = None;
                    self.open_dir(path);
                }
            }
            return false;
        }
        if let Some(h) = &mut self.history {
            match h.on_key(code) {
                history::Action::None => {}
                history::Action::Close => self.history = None,
                history::Action::Resume(sid, title) => {
                    self.history = None;
                    self.resume(sid, &title);
                }
                history::Action::Load(req, title, path) => {
                    let tx = self.tx.clone();
                    tokio::task::spawn_blocking(move || {
                        let res = crate::history::load(&path).map_err(|e| format!("{e:#}"));
                        let _ =
                            tx.send((0, Msg::History(history::Loaded::Session(req, title, res))));
                    });
                }
            }
            return false;
        }
        let ctrl = mods.contains(KeyModifiers::CONTROL);
        let alt = mods.contains(KeyModifiers::ALT);
        let t = &mut self.tabs[self.current];
        match code {
            KeyCode::Tab => return self.switch_by(1),
            KeyCode::BackTab => return self.switch_by(n - 1),
            KeyCode::Char('c') if ctrl => {
                t.cancel_turn();
                return false;
            }
            KeyCode::Char(d @ '1'..='9') if alt => {
                self.switch(d as usize - '1' as usize);
                return false;
            }
            _ => {}
        }
        if t.perm.is_some() {
            match code {
                KeyCode::Char('y') => t.answer(acp::approve_option),
                KeyCode::Char('n') | KeyCode::Esc => t.answer(acp::reject_option),
                KeyCode::Char('a') => {
                    t.answer(|p| acp::always_option(p).or(acp::approve_option(p)))
                }
                KeyCode::Char('A') => self.confirm = Some(Confirm::AllowAll),
                KeyCode::Char('!') => return self.jump_attention(),
                _ => {}
            }
            return false;
        }
        match code {
            KeyCode::Up | KeyCode::Down if self.dash == Dash::Full && !t.typing => {
                self.move_dash_sel(if code == KeyCode::Up { -1 } else { 1 });
            }
            KeyCode::PageUp => t.scroll.up(t.scroll.page()),
            KeyCode::PageDown => t.scroll.down(t.scroll.page()),
            KeyCode::Up => t.scroll.up(1),
            KeyCode::Down => t.scroll.down(1),
            _ if t.typing => match code {
                KeyCode::Esc => t.typing = false,
                KeyCode::Enter if alt || mods.contains(KeyModifiers::SHIFT) => {
                    t.editor.insert('\n')
                }
                KeyCode::Enter => t.send_prompt(),
                KeyCode::Char('j') if ctrl => t.editor.insert('\n'),
                KeyCode::Char('s') if ctrl => {
                    if t.editor.text.trim().is_empty() {
                        t.push(Item::Note(
                            settings::tr("el borrador está vacío", "draft is empty").into(),
                        ));
                    } else {
                        let draft = t.editor.text.clone();
                        self.prompts = Some(prompts::Menu::save_draft(draft));
                    }
                }
                KeyCode::Char('v') if ctrl => match clipboard::read_png_b64() {
                    Ok(b) => t.images.push(b),
                    Err(e) => t.push(Item::Note(format!("📎 {e:#}"))),
                },
                KeyCode::Backspace if t.editor.is_empty() && !t.images.is_empty() => {
                    t.images.pop();
                }
                KeyCode::Char('w') if ctrl => t.editor.kill_word(),
                KeyCode::Char('u') if ctrl => t.editor.kill_line(),
                KeyCode::Char(c) if !ctrl && !alt => t.editor.insert(c),
                KeyCode::Backspace => t.editor.backspace(),
                KeyCode::Delete => t.editor.delete(),
                KeyCode::Left => t.editor.left(),
                KeyCode::Right => t.editor.right(),
                KeyCode::Home => t.editor.home(),
                KeyCode::End => t.editor.end(),
                _ => {}
            },
            KeyCode::Home => t.scroll.up(usize::MAX / 2),
            KeyCode::End => t.scroll.off = 0,
            KeyCode::Char('k') => t.scroll.up(1),
            KeyCode::Char('j') => t.scroll.down(1),
            KeyCode::Char('q') => {
                if self.tabs.iter().any(Tab::working) {
                    self.confirm = Some(Confirm::Quit);
                } else {
                    return true;
                }
            }
            KeyCode::Char('x') if n < 2 => t.push(Item::Note(
                settings::tr(
                    "no se puede cerrar la última pestaña",
                    "cannot close the last tab",
                )
                .into(),
            )),
            KeyCode::Char('x') if t.working() => self.confirm = Some(Confirm::CloseTab),
            KeyCode::Char('x') => self.close_current(),
            KeyCode::Char('d') => {
                self.dash = match self.dash {
                    Dash::Side | Dash::Full => Dash::Hidden,
                    Dash::Hidden => Dash::Side,
                }
            }
            KeyCode::Char('s') => {
                self.dash = if self.dash == Dash::Full {
                    Dash::Side
                } else {
                    Dash::Full
                }
            }
            KeyCode::Char('A') if t.allow_all => t.set_allow_all(false),
            KeyCode::Char('A') => self.confirm = Some(Confirm::AllowAll),
            KeyCode::Char('m') => self.open_selector(true),
            KeyCode::Char('e') => self.open_selector(false),
            KeyCode::Char('M') => self.mcp = Some(mcp::Menu::new(&t.name, &t.cwd)),
            KeyCode::Char('p') => {
                let draft = t.editor.text.clone();
                self.prompts = Some(prompts::Menu::open(draft));
            }
            KeyCode::Char('S') => self.settings = Some(settings::Menu::new()),
            KeyCode::Char('o') => {
                let dir = start_dir(&t.cwd);
                let (b, req) = open::Browser::open(dir.clone());
                self.open = Some(b);
                spawn_read_dir(req, dir, false, &self.tx);
            }
            KeyCode::Char('H') => {
                let (h, req) = history::History::open(&t.name);
                let cwd = t.cwd.clone();
                let tx = self.tx.clone();
                self.history = Some(h);
                tokio::task::spawn_blocking(move || {
                    let sessions = crate::history::root()
                        .map(|r| crate::history::list(&r, &cwd))
                        .unwrap_or_default();
                    let _ = tx.send((0, Msg::History(history::Loaded::List(req, sessions))));
                });
            }
            KeyCode::Char('?') => self.help = true,
            KeyCode::Char('!') => return self.jump_attention(),
            KeyCode::Enter | KeyCode::Char('i') => {
                if code == KeyCode::Enter && self.dash == Dash::Full && t.editor.is_empty() {
                    let rows = dashboard::rows(t);
                    let sel = t.dash_sel.as_ref();
                    let row = rows.iter().find(|r| Some(&r.name) == sel || sel.is_none());
                    if let Some(r) = row.filter(|r| r.name != "main") {
                        subagent_prompt(&r.name)
                            .chars()
                            .for_each(|c| t.editor.insert(c));
                    }
                }
                t.typing = true;
                if self.dash == Dash::Full {
                    self.dash = Dash::Side;
                }
            }
            KeyCode::Char('f') => {
                if t.editor.is_empty() {
                    "/fleet ".chars().for_each(|c| t.editor.insert(c));
                }
                t.typing = true;
                if self.dash == Dash::Full {
                    self.dash = Dash::Side;
                }
            }
            KeyCode::Esc => t.cancel_turn(),
            _ => {}
        }
        false
    }

    fn close_current(&mut self) {
        self.tabs.remove(self.current);
        self.current = self.current.min(self.tabs.len() - 1);
    }

    /// Replaces the current tab with a fresh ACP process that reloads its session,
    /// so a changed MCP server list takes effect.
    fn restart_current(&mut self) {
        let old = &self.tabs[self.current];
        let (name, cwd, sid) = (old.name.clone(), old.cwd.clone(), old.sid.clone());
        let id = self.next_id;
        self.next_id += 1;
        let mut tab = spawn_tab(id, name, cwd, sid, &self.tx);
        tab.push(Item::Note(
            settings::tr(
                "MCP actualizados: sesión reiniciada (allow all y modelo se restablecen)",
                "MCP updated: session restarted (allow all and model are reset)",
            )
            .into(),
        ));
        self.tabs[self.current] = tab;
    }

    /// Opens a past session in a new tab (`session/load`), or focuses the tab that already has it.
    fn resume(&mut self, sid: String, title: &str) {
        if let Some(i) = self
            .tabs
            .iter()
            .position(|t| t.sid.as_deref() == Some(&sid))
        {
            self.current = i;
            return;
        }
        let cur = &self.tabs[self.current];
        let (name, cwd) = (format!("{} ↺", cur.name), cur.cwd.clone());
        let id = self.next_id;
        self.next_id += 1;
        let mut tab = spawn_tab(id, name, cwd, Some(sid), &self.tx);
        tab.push(Item::Note(tr_fmt!(
            "sesión reanudada: {title}",
            "session resumed: {title}"
        )));
        self.tabs.push(tab);
        self.current = self.tabs.len() - 1;
    }

    /// Opens `path` as a new tab with a fresh ACP session and focuses it.
    fn open_dir(&mut self, path: PathBuf) {
        let path = std::fs::canonicalize(&path).unwrap_or(path);
        let cwd = path.to_string_lossy().into_owned();
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| cwd.clone());
        let id = self.next_id;
        self.next_id += 1;
        let tab = spawn_tab(id, name, cwd, None, &self.tx);
        self.tabs.push(tab);
        self.current = self.tabs.len() - 1;
    }

    fn move_dash_sel(&mut self, delta: isize) {
        let t = &mut self.tabs[self.current];
        let rows = dashboard::rows(t);
        let i = rows
            .iter()
            .position(|r| Some(&r.name) == t.dash_sel.as_ref())
            .unwrap_or(0);
        let j = i
            .saturating_add_signed(delta)
            .min(rows.len().saturating_sub(1));
        t.dash_sel = rows.get(j).map(|r| r.name.clone());
    }

    /// `m`: model of the session (or of the selected agent in the full dashboard); `e`: effort.
    fn open_selector(&mut self, model: bool) {
        let t = &mut self.tabs[self.current];
        let agent = (model && self.dash == Dash::Full)
            .then(|| dashboard::rows(t))
            .and_then(|rows| {
                let sel = t.dash_sel.as_ref();
                rows.into_iter()
                    .find(|r| Some(&r.name) == sel || sel.is_none())
            })
            .filter(|r| r.name != "main")
            .map(|r| r.name);
        let (opt_id, title) = if model {
            ("model", settings::tr("Modelo", "Model"))
        } else {
            ("reasoning_effort", "Reasoning effort")
        };
        let Some(opt) = t.config_opt(opt_id).filter(|o| !o.options.is_empty()) else {
            t.push(Item::Note(tr_fmt!(
                "{title}: opción no disponible en esta sesión",
                "{title}: option not available in this session"
            )));
            return;
        };
        let items = opt.options.clone();
        let (title, target, current) = match agent {
            Some(a) => (
                tr_fmt!("🔒 Modelo obligatorio de {a}", "🔒 Required model of {a}"),
                SelTarget::AgentModel(a.clone()),
                t.assigned.get(&a).cloned().unwrap_or_default(),
            ),
            None => (
                title.to_string(),
                SelTarget::Config(opt_id.into()),
                opt.current.clone(),
            ),
        };
        let sel = items.iter().position(|(v, _)| *v == current).unwrap_or(0);
        self.selector = Some(Selector {
            title,
            target,
            items,
            sel,
        });
    }

    fn on_selector_key(&mut self, code: KeyCode) {
        let Some(s) = &mut self.selector else { return };
        match code {
            KeyCode::Up | KeyCode::Char('k') => s.sel = s.sel.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => {
                s.sel = (s.sel + 1).min(s.items.len().saturating_sub(1))
            }
            KeyCode::Enter => {
                let Some(s) = self.selector.take() else {
                    return;
                };
                let Some((value, _)) = s.items.get(s.sel) else {
                    return;
                };
                let t = &mut self.tabs[self.current];
                match s.target {
                    SelTarget::Config(id) => t.set_config(&id, value),
                    SelTarget::AgentModel(a) => assign_model(t, &a, value, agents::settings_path()),
                }
            }
            KeyCode::Esc => self.selector = None,
            _ => {}
        }
    }

    fn switch_by(&mut self, delta: usize) -> bool {
        self.current = (self.current + delta) % self.tabs.len();
        false
    }

    fn jump_attention(&mut self) -> bool {
        let n = self.tabs.len();
        if let Some(i) = (1..=n)
            .map(|d| (self.current + d) % n)
            .find(|&i| self.tabs[i].perm.is_some())
        {
            self.current = i;
        }
        false
    }
}

/// Starting directory for the Open browser: tab `cwd`, else `$HOME`, else `.`.
fn start_dir(cwd: &str) -> PathBuf {
    let p = PathBuf::from(cwd);
    let chosen = if !cwd.is_empty() && p.is_dir() {
        p
    } else {
        std::env::var_os("HOME")
            .map(PathBuf::from)
            .filter(|h| h.is_dir())
            .unwrap_or_else(|| PathBuf::from("."))
    };
    std::fs::canonicalize(&chosen).unwrap_or(chosen)
}

fn spawn_read_dir(req: u64, dir: PathBuf, show_hidden: bool, tx: &Tx) {
    let tx = tx.clone();
    tokio::task::spawn_blocking(move || {
        let entries = open::read_dir(&dir, show_hidden);
        let _ = tx.send((0, Msg::Open(open::Loaded { req, entries })));
    });
}

/// Draft that asks Copilot to run the given subagent.
fn subagent_prompt(name: &str) -> String {
    format!(
        "{} {name} {} ",
        settings::tr("Usa el subagente", "Use the subagent"),
        settings::tr("para:", "for:")
    )
}

/// Persists a mandatory subagent model in the Copilot user settings and records it.
fn assign_model(t: &mut Tab, agent: &str, model: &str, path: Option<PathBuf>) {
    let r = path
        .context("HOME is not set")
        .and_then(|p| agents::set_model(&p, agent, model));
    let note = match r {
        Ok(()) => {
            t.assigned.insert(agent.into(), model.into());
            tr_fmt!(
                "🔒 {agent} → {model} (settings.json, modelPolicy required; vale para subagentes nuevos)",
                "🔒 {agent} → {model} (settings.json, modelPolicy required; applies to new subagents)"
            )
        }
        Err(e) => tr_fmt!(
            "no se pudo fijar el modelo de {agent}: {e:#}",
            "could not set the model of {agent}: {e:#}"
        ),
    };
    t.push(Item::Note(note));
}

pub async fn run(projects: Vec<Project>) -> Result<()> {
    if projects.is_empty() {
        bail!("no projects configured (add [[projects]] entries to the config file)");
    }
    settings::load();
    let (tx, mut rx) = unbounded_channel();
    let mut tabs = vec![];
    for (i, p) in projects.iter().enumerate() {
        let cwd = std::fs::canonicalize(&p.path)
            .with_context(|| format!("bad project path {}", p.path.display()))?
            .to_string_lossy()
            .into_owned();
        let mut tab = spawn_tab(i, p.name.clone(), cwd, None, &tx);
        let settings = agents::settings_path();
        for (agent, model) in &p.agent_models {
            if tab.assigned.get(agent) != Some(model) {
                assign_model(&mut tab, agent, model, settings.clone());
            }
        }
        if p.allow_all {
            tab.allow_all = true;
            tab.push(Item::Note(
                settings::tr(
                    "↯ allow_all=true en la config: se aprobarán todos los permisos",
                    "↯ allow_all=true in config: every permission will be approved",
                )
                .into(),
            ));
            tab.set_config("allow_all", "on");
        }
        if let Some(m) = &p.model {
            tab.set_config("model", m);
        }
        tabs.push(tab);
    }
    let mut app = App {
        next_id: tabs.len(),
        tabs,
        current: 0,
        dash: Dash::Side,
        help: false,
        confirm: None,
        selector: None,
        history: None,
        open: None,
        settings: None,
        mcp: None,
        prompts: None,
        tx,
        tick: 0,
    };
    // ratatui::init installs a panic hook that restores the terminal.
    let mut term = ratatui::init();
    let r = event_loop(&mut term, &mut app, &mut rx).await;
    ratatui::restore();
    r
}

const TICK: Duration = Duration::from_millis(100);

/// Redraws only on messages, input, resize, or an animation tick while something is animating.
async fn event_loop(
    term: &mut DefaultTerminal,
    app: &mut App,
    rx: &mut UnboundedReceiver<(usize, Msg)>,
) -> Result<()> {
    let (mut dirty, mut last_tick) = (true, Instant::now());
    loop {
        while let Ok((i, m)) = rx.try_recv() {
            app.on_msg(i, m);
            dirty = true;
        }
        while event::poll(Duration::ZERO)? {
            match event::read()? {
                Term::Key(k) if k.kind != KeyEventKind::Release => {
                    dirty = true;
                    if app.on_key(k) {
                        return Ok(());
                    }
                }
                Term::Resize(..) => dirty = true,
                _ => {}
            }
        }
        if last_tick.elapsed() >= TICK {
            last_tick = Instant::now();
            app.tick += 1;
            // Idle blink: redraw when the eyes close and when they reopen.
            dirty |= app.animating()
                || widgets::eyes_closed(app.tick, false)
                || widgets::eyes_closed(app.tick - 1, false)
                || widgets::quip(app.tick).is_some() != widgets::quip(app.tick - 1).is_some();
        }
        if std::mem::take(&mut dirty) {
            term.draw(|f| widgets::draw(app, f))?;
        }
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(super) fn tab(name: &str) -> Tab {
        let (prompts, _) = unbounded_channel();
        let (ctl, _) = unbounded_channel();
        let mut t = Tab::new(name.into(), prompts, ctl, vec![], HashMap::new());
        t.life = Life::Ready;
        t
    }

    pub(super) fn app(mut tabs: Vec<Tab>) -> App {
        tabs.iter_mut().enumerate().for_each(|(i, t)| t.id = i);
        App {
            next_id: tabs.len(),
            tabs,
            current: 0,
            dash: Dash::Side,
            help: false,
            confirm: None,
            selector: None,
            history: None,
            open: None,
            settings: None,
            mcp: None,
            prompts: None,
            tx: unbounded_channel().0,
            tick: 0,
        }
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn scroll_pauses_and_flags_new_output() {
        let mut s = Scroll::default();
        s.sync(50, 10);
        assert_eq!((s.off, s.unseen), (0, false));
        s.up(5);
        s.sync(50, 10);
        assert_eq!(s.off, 5);
        s.sync(53, 10); // new output while scrolled up: view stays put
        assert_eq!((s.off, s.unseen), (8, true));
        s.up(1000);
        s.sync(53, 10);
        assert_eq!(s.off, 43); // clamped to the top
        s.down(1000);
        s.sync(53, 10);
        assert_eq!((s.off, s.unseen), (0, false));
    }

    #[test]
    fn keys_draft_per_tab_and_quit_rules() {
        let mut a = app(vec![tab("a"), tab("b")]);
        assert!(!a.on_key(key(KeyCode::Char('i'))));
        a.on_key(key(KeyCode::Char('q'))); // typed, not quit
        assert_eq!(a.tabs[0].editor.text, "q");
        a.on_key(key(KeyCode::Tab));
        assert_eq!(a.current, 1);
        assert!(a.tabs[1].editor.is_empty());
        a.on_key(key(KeyCode::BackTab));
        assert_eq!(a.tabs[0].editor.text, "q");
        a.on_key(key(KeyCode::Esc));
        assert!(!a.tabs[0].typing);
        a.tabs[1].busy = true;
        assert!(!a.on_key(key(KeyCode::Char('q'))));
        assert!(a.confirm == Some(Confirm::Quit));
        assert!(a.on_key(key(KeyCode::Char('y'))));
        a.tabs[1].busy = false;
        assert!(a.on_key(key(KeyCode::Char('q'))));
    }

    #[test]
    fn close_tab_rules_and_late_messages() {
        let mut a = app(vec![tab("a"), tab("b"), tab("c")]);
        a.current = 1;
        a.on_key(key(KeyCode::Char('x')));
        assert_eq!((a.tabs.len(), a.current), (2, 1));
        assert_eq!(a.tabs[1].name, "c");
        a.on_msg(1, Msg::Warn("late".into())); // id 1 was closed
        assert!(a.tabs.iter().all(|t| t.items.is_empty()));
        a.on_msg(2, Msg::Warn("hi".into())); // id 2 now sits at position 1
        assert_eq!(a.tabs[1].items.len(), 1);
        a.tabs[1].busy = true;
        a.on_key(key(KeyCode::Char('x')));
        assert!(a.confirm == Some(Confirm::CloseTab) && a.tabs.len() == 2);
        a.on_key(key(KeyCode::Char('y')));
        assert_eq!((a.tabs.len(), a.current), (1, 0));
        a.on_key(key(KeyCode::Char('x')));
        assert_eq!(a.tabs.len(), 1);
    }

    #[test]
    fn enter_in_full_dashboard_drafts_subagent_prompt() {
        let def = |n: &str| crate::agents::AgentDef {
            name: n.into(),
            description: String::new(),
            model: None,
            builtin: true,
        };
        let mut a = app(vec![tab("a")]);
        a.tabs[0].defs = vec![def("explore")].into();
        a.dash = Dash::Full;
        a.tabs[0].dash_sel = Some("explore".into());
        a.on_key(key(KeyCode::Enter));
        assert_eq!(a.tabs[0].editor.text, subagent_prompt("explore"));
        assert!(a.tabs[0].typing && a.dash == Dash::Side);
        // "main" and an existing draft are left untouched
        for (sel, draft) in [("main", ""), ("explore", "draft")] {
            let mut a = app(vec![tab("a")]);
            a.tabs[0].defs = vec![def("explore")].into();
            a.dash = Dash::Full;
            a.tabs[0].dash_sel = Some(sel.into());
            draft.chars().for_each(|c| a.tabs[0].editor.insert(c));
            a.on_key(key(KeyCode::Enter));
            assert_eq!(a.tabs[0].editor.text, draft);
        }
    }

    #[test]
    fn allow_all_needs_confirmation_and_approves_pending_permission() {
        let mut a = app(vec![tab("a")]);
        let (reply, mut rx) = tokio::sync::oneshot::channel();
        a.tabs[0].perm = Some(Perm {
            params: serde_json::json!({"options":[{"optionId":"ok","kind":"allow_once"}]}),
            reply,
        });
        a.on_key(key(KeyCode::Char('A')));
        assert!(a.confirm == Some(Confirm::AllowAll) && !a.tabs[0].allow_all);
        a.on_key(key(KeyCode::Char('y')));
        assert!(a.tabs[0].allow_all && a.tabs[0].perm.is_none());
        assert_eq!(rx.try_recv().ok().flatten().as_deref(), Some("ok"));
        // later requests are approved without showing the modal
        let (reply, mut rx) = tokio::sync::oneshot::channel();
        let p = Perm {
            params: serde_json::json!({"options":[{"optionId":"ok","kind":"allow_once"}]}),
            reply,
        };
        a.on_msg(0, Msg::Perm(p));
        assert!(a.tabs[0].perm.is_none());
        assert_eq!(rx.try_recv().ok().flatten().as_deref(), Some("ok"));
        // ACP says it is off: the UI follows
        let off = acp::ConfigOpt {
            id: "allow_all".into(),
            current: "off".into(),
            options: vec![],
        };
        a.on_msg(0, Msg::Update(Update::Config(vec![off])));
        assert!(!a.tabs[0].allow_all);
    }

    #[test]
    fn tool_updates_merge_by_id() {
        let mut t = tab("a");
        t.on_update(Update::Tool {
            id: "1".into(),
            title: Some("ls".into()),
            kind: Some("execute".into()),
            status: Some("in_progress".into()),
        });
        t.on_update(Update::Tool {
            id: "1".into(),
            title: None,
            kind: None,
            status: Some("completed".into()),
        });
        assert_eq!(t.items.len(), 1);
        assert!(
            matches!(&t.items[0], Item::Tool { title, state: ToolState::Ok, .. } if title == "ls")
        );
    }

    #[tokio::test]
    async fn open_modal_creates_and_focuses_new_tab() {
        let dir =
            std::env::temp_dir().join(format!("copilot-hive-open-tab-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let canonical = std::fs::canonicalize(&dir).unwrap();

        let mut a = app(vec![tab("initial")]);
        a.tabs[0].cwd = canonical.to_string_lossy().into_owned();
        a.on_key(key(KeyCode::Char('o')));
        assert!(a.open.is_some());

        a.on_key(key(KeyCode::Char('o')));
        assert!(a.open.is_none());
        assert_eq!(a.tabs.len(), 2);
        assert_eq!(a.current, 1);
        assert_eq!(
            a.tabs[1].name,
            canonical.file_name().unwrap().to_string_lossy()
        );
        assert_eq!(a.tabs[1].cwd, canonical.to_string_lossy());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn prompts_modal_saves_draft_and_applies_to_editor() {
        let mut a = app(vec![tab("a"), tab("b")]);
        // Type a draft in tab a and press Ctrl+S
        a.on_key(key(KeyCode::Char('i')));
        for c in "revisa el diff".chars() {
            a.on_key(key(KeyCode::Char(c)));
        }
        a.on_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL));
        assert!(a.prompts.is_some());
        for c in "review".chars() {
            a.on_key(key(KeyCode::Char(c)));
        }
        a.on_key(key(KeyCode::Enter)); // saves and returns to list
        a.tabs[0].editor.take();
        a.on_key(key(KeyCode::Enter)); // applies selected prompt to editor
        assert!(a.prompts.is_none());
        assert!(a.tabs[0].typing);
        assert_eq!(a.tabs[0].editor.text, "revisa el diff");
    }

    #[test]
    fn prompts_empty_draft_and_full_dashboard_apply() {
        let mut a = app(vec![tab("a")]);
        // Ctrl+S with whitespace-only draft pushes a note instead of opening modal
        a.on_key(key(KeyCode::Char('i')));
        a.on_key(key(KeyCode::Char(' ')));
        a.on_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL));
        assert!(a.prompts.is_none());
        assert!(
            a.tabs[0]
                .items
                .iter()
                .any(|it| matches!(it, Item::Note(s) if !s.is_empty()))
        );

        // Leave typing, switch to Dash::Full, open prompts with 'p', create and apply
        a.tabs[0].editor.take();
        a.on_key(key(KeyCode::Esc));
        a.on_key(key(KeyCode::Char('s'))); // Dash::Full
        assert!(a.dash == Dash::Full);
        a.on_key(key(KeyCode::Char('p')));
        assert!(a.prompts.is_some());
        a.on_key(key(KeyCode::Char('n')));
        for c in "audit".chars() {
            a.on_key(key(KeyCode::Char(c)));
        }
        a.on_key(key(KeyCode::Tab));
        for c in "haz audit".chars() {
            a.on_key(key(KeyCode::Char(c)));
        }
        a.on_key(key(KeyCode::Enter)); // save
        a.on_key(key(KeyCode::Enter)); // apply
        assert!(a.prompts.is_none());
        assert!(a.dash == Dash::Side);
        assert!(a.tabs[0].typing);
        assert_eq!(a.tabs[0].editor.text, "haz audit");
    }
}
