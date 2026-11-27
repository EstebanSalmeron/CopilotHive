//! Async session lifecycle: ACP client task, `events.jsonl` polling, and tab spawning.
use super::{Msg, Tab, Tx};
use crate::acp::{self, Ctl, Perm, Update};
use crate::agents;
use crate::events;
use anyhow::{Context, Result};
use serde_json::json;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};

struct AbortOnDrop(tokio::task::JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_project(
    idx: usize,
    cwd: String,
    extra_dirs: Vec<String>,
    resume: Option<String>,
    disabled_mcp: Vec<String>,
    mut prompts: UnboundedReceiver<(String, Vec<String>)>,
    ctl: UnboundedReceiver<Ctl>,
    tx: Tx,
) -> Result<()> {
    let short = Duration::from_secs(30);
    let long = Duration::from_secs(300);
    let mut dump = |_: &str| Ok(());
    let mut on_perm = |p: Perm| {
        let _ = tx.send((idx, Msg::Perm(p)));
    };
    let mut c = acp::Client::spawn(&cwd, &extra_dirs, &disabled_mcp)?;
    let tx_warn = tx.clone();
    c.on_warn(move |w| {
        let _ = tx_warn.send((idx, Msg::Warn(w)));
    });
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
    // Aborting run_project does not stop its children; the guard aborts watch on drop.
    let _watch = AbortOnDrop(tokio::spawn(watch(idx, sid.clone(), tx.clone())));
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
                let Some((text, images)) = text else { return Ok(()) };
                let mut blocks = images
                    .iter()
                    .map(|d| json!({"type":"image","mimeType":"image/png","data":d}))
                    .collect::<Vec<_>>();
                if !text.is_empty() {
                    blocks.push(json!({"type":"text","text":text}));
                }
                let r = c
                    .request(
                        "session/prompt",
                        json!({"sessionId":sid,"prompt":blocks}),
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

/// Polls events.jsonl once a second via `spawn_blocking` to keep the single-threaded runtime free.
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
        let p = path.clone();
        let off = offset;
        let res = tokio::task::spawn_blocking(move || {
            let Ok(mut f) = std::fs::File::open(&p) else {
                return Ok((vec![], off));
            };
            let mut o = off;
            events::read_new(&mut f, &mut o).map(|lines| (lines, o))
        })
        .await;
        match res {
            Ok(Ok((lines, new_off))) => {
                offset = new_off;
                if !lines.is_empty() {
                    let _ = tx.send((idx, Msg::Lines(lines)));
                }
            }
            Ok(Err(e)) => {
                let _ = tx.send((idx, Msg::Warn(format!("events.jsonl: {e:#}"))));
            }
            Err(_) => {}
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

/// Creates a tab and starts its ACP session (`session/new`, or `session/load` when resuming).
pub(super) fn spawn_tab(
    i: usize,
    name: String,
    cwd: String,
    extra_dirs: Vec<String>,
    resume: Option<String>,
    tx: &Tx,
) -> Tab {
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
    tab.extra_dirs = extra_dirs.clone();
    let tx2 = tx.clone();
    let disabled_mcp = super::mcp::disabled(&tab.cwd);
    tab.mcp = super::mcp::discover(Path::new(&tab.cwd), home.as_deref())
        .into_iter()
        .map(|s| {
            let on = !disabled_mcp.contains(&s.name);
            (s, on)
        })
        .collect();
    let task = tokio::spawn(async move {
        if let Err(e) = run_project(
            i,
            cwd,
            extra_dirs,
            resume,
            disabled_mcp,
            prx,
            crx,
            tx2.clone(),
        )
        .await
        {
            let _ = tx2.send((i, Msg::Failed(format!("{e:#}"))));
        }
    });
    tab.id = i;
    tab.task = Some(task.abort_handle());
    tab
}
