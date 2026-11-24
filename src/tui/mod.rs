//! Tabbed terminal UI: one ACP session per project, live agent monitoring, token usage.
/// `format!` choosing the Spanish or English template by current UI language.
macro_rules! tr_fmt {
    ($es:literal, $en:literal $(, $a:expr)* $(,)?) => {
        if crate::tui::settings::is_en() { format!($en $(, $a)*) } else { format!($es $(, $a)*) }
    };
}

mod dashboard;
mod history;
mod input;
mod markdown;
mod settings;
mod theme;
mod widgets;

use crate::acp::{self, ConfigOpt, Ctl, Perm, Update};
use crate::agents::{self, AgentDef};
use crate::config::Project;
use crate::events::{self, Status, Tree};
use anyhow::{Context, Result, bail};
use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{
    self, Event as Term, KeyCode, KeyEvent, KeyEventKind, KeyModifiers,
};
use ratatui::text::Line;
use serde_json::json;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
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
    lines: Vec<Line<'static>>,
}

struct Tab {
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
    prompts: UnboundedSender<String>,
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
}

impl Tab {
    fn new(
        name: String,
        prompts: UnboundedSender<String>,
        ctl: UnboundedSender<Ctl>,
        defs: Vec<AgentDef>,
        assigned: HashMap<String, String>,
    ) -> Self {
        Tab {
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
            ctl,
            config: vec![],
            allow_all: false,
            defs,
            assigned,
            dash_sel: None,
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
        if text.is_empty() {
            return;
        }
        if self.prompts.send(text.clone()).is_ok() {
            self.editor.take();
            self.push(Item::User(text));
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
    current: usize,
    dash: Dash,
    help: bool,
    confirm: Option<Confirm>,
    selector: Option<Selector>,
    history: Option<history::History>,
    settings: Option<settings::Menu>,
    tx: Tx,
    tick: u64,
}

impl App {
    fn on_msg(&mut self, i: usize, m: Msg) {
        let Some(t) = self.tabs.get_mut(i) else {
            return;
        };
        match m {
            Msg::Update(u) => t.on_update(u),
            Msg::Perm(p) if t.allow_all => {
                let title = p.params["toolCall"]["title"].as_str().unwrap_or("tool");
                let note = tr_fmt!(
                    "⚡ auto-aprobado (allow all): {title}",
                    "⚡ auto-approved (allow all): {title}"
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
            if c == Confirm::Quit {
                return true;
            }
            self.tabs[self.current].set_allow_all(true);
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
        if let Some(h) = &mut self.history {
            match h.on_key(code) {
                history::Action::None => {}
                history::Action::Close => self.history = None,
                history::Action::Resume(sid, title) => {
                    self.history = None;
                    self.resume(sid, &title);
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
            KeyCode::Char('S') => self.settings = Some(settings::Menu::new()),
            KeyCode::Char('H') => {
                self.history = Some(history::History::open(&t.name, &t.cwd));
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
        let i = self.tabs.len();
        let mut tab = spawn_tab(i, name, cwd, Some(sid), &self.tx);
        tab.push(Item::Note(tr_fmt!(
            "sesión reanudada: {title}",
            "session resumed: {title}"
        )));
        self.tabs.push(tab);
        self.current = i;
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

async fn run_project(
    idx: usize,
    cwd: String,
    resume: Option<String>,
    mut prompts: UnboundedReceiver<String>,
    ctl: UnboundedReceiver<Ctl>,
    tx: Tx,
) -> Result<()> {
    let short = Duration::from_secs(30);
    let long = Duration::from_secs(300);
    let mut dump = |_: &str| Ok(());
    let mut on_perm = |p: Perm| {
        let _ = tx.send((idx, Msg::Perm(p)));
    };
    let mut c = acp::Client::spawn(&cwd)?;
    c.request(
        "initialize",
        json!({"protocolVersion":1,"clientCapabilities":{}}),
        short,
        &mut dump,
        &mut on_perm,
    )
    .await?;
    let (s, sid) = match resume {
        Some(sid) => {
            // The agent replays the whole conversation as updates before answering.
            let mut replay = |l: &str| {
                if let Some(u) = acp::parse_update(l) {
                    let _ = tx.send((idx, Msg::Update(u)));
                }
                Ok(())
            };
            let s = c
                .request(
                    "session/load",
                    json!({"sessionId":sid,"cwd":cwd,"mcpServers":[]}),
                    short,
                    &mut replay,
                    &mut on_perm,
                )
                .await?;
            (s, sid)
        }
        None => {
            let s = c
                .request(
                    "session/new",
                    json!({"cwd":cwd,"mcpServers":[]}),
                    short,
                    &mut dump,
                    &mut on_perm,
                )
                .await?;
            let sid = s["sessionId"].as_str().context("no sessionId")?.to_string();
            (s, sid)
        }
    };
    let _ = tx.send((idx, Msg::Session(sid.clone())));
    c.on_ctl(ctl, sid.clone());
    let opts = acp::parse_config_options(&s["configOptions"]);
    let _ = tx.send((idx, Msg::Update(Update::Config(opts))));
    tokio::spawn(watch(idx, sid.clone(), tx.clone()));
    let _ = tx.send((idx, Msg::Ready));
    let mut on_line = |l: &str| {
        if let Some(u) = acp::parse_update(l) {
            let _ = tx.send((idx, Msg::Update(u)));
        }
        Ok(())
    };
    loop {
        tokio::select! {
            text = prompts.recv() => {
                let Some(text) = text else { return Ok(()) };
                let r = c
                    .request(
                        "session/prompt",
                        json!({"sessionId":sid,"prompt":[{"type":"text","text":text}]}),
                        long,
                        &mut on_line,
                        &mut on_perm,
                    )
                    .await?;
                let reason = r["stopReason"].as_str().unwrap_or("?").to_string();
                let _ = tx.send((idx, Msg::Done(reason)));
            }
            r = c.idle(&mut on_line) => r?,
        }
    }
}

/// Polls events.jsonl once a second; polling avoids a file-watcher dependency.
async fn watch(idx: usize, sid: String, tx: Tx) {
    let Some(home) = std::env::var_os("HOME") else {
        return;
    };
    let path = PathBuf::from(home)
        .join(".copilot/session-state")
        .join(sid)
        .join("events.jsonl");
    let mut offset = 0;
    loop {
        if let Ok(mut f) = std::fs::File::open(&path) {
            match events::read_new(&mut f, &mut offset) {
                Ok(lines) if !lines.is_empty() => {
                    let _ = tx.send((idx, Msg::Lines(lines)));
                }
                Ok(_) => {}
                Err(e) => {
                    let _ = tx.send((idx, Msg::Warn(format!("events.jsonl: {e:#}"))));
                }
            }
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

/// Creates a tab and starts its ACP session (`session/new`, or `session/load` when resuming).
fn spawn_tab(i: usize, name: String, cwd: String, resume: Option<String>, tx: &Tx) -> Tab {
    let (ptx, prx) = unbounded_channel();
    let (ctl, crx) = unbounded_channel();
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let defs = agents::discover(Path::new(&cwd), home.as_deref());
    let loaded = agents::settings_path()
        .as_deref()
        .map(agents::load_models)
        .unwrap_or_default();
    let mut tab = Tab::new(name, ptx, ctl, defs, loaded);
    tab.cwd = cwd.clone();
    let tx2 = tx.clone();
    tokio::spawn(async move {
        if let Err(e) = run_project(i, cwd, resume, prx, crx, tx2.clone()).await {
            let _ = tx2.send((i, Msg::Failed(format!("{e:#}"))));
        }
    });
    tab
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
                    "⚡ allow_all=true en la config: se aprobarán todos los permisos",
                    "⚡ allow_all=true in config: every permission will be approved",
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
        tabs,
        current: 0,
        dash: Dash::Side,
        help: false,
        confirm: None,
        selector: None,
        history: None,
        settings: None,
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
                || widgets::eyes_closed(app.tick - 1, false);
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

    pub(super) fn app(tabs: Vec<Tab>) -> App {
        App {
            tabs,
            current: 0,
            dash: Dash::Side,
            help: false,
            confirm: None,
            selector: None,
            history: None,
            settings: None,
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
}
