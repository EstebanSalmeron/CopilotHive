mod acp;
mod agents;
mod config;
mod events;
mod history;
mod tui;

use anyhow::{Context, Result, bail};
use serde_json::json;
use std::io::Write;
use std::time::Duration;

const DEFAULT_PROMPT: &str = "Use two explore subagents in parallel (task tool, agent_type explore): \
one lists the files in the top-level directory, the other lists the files under src/. \
Then summarize both results in one line.";

const TIMEOUT: Duration = Duration::from_secs(300);
const USAGE: &str = "usage: copilot-hive tui [--config FILE]\n       copilot-hive spike [--cwd DIR] [--out FILE] [PROMPT]";

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("tui") => {
            let path = match (args.next().as_deref(), args.next()) {
                (Some("--config"), Some(p)) => p.into(),
                (None, _) => config::default_path()?,
                _ => bail!(USAGE),
            };
            tui::run(config::load(&path)?).await
        }
        Some("spike") => spike(args).await,
        _ => bail!(USAGE),
    }
}

async fn spike(mut args: impl Iterator<Item = String>) -> Result<()> {
    let mut cwd = ".".to_string();
    let mut out = "spike-output.jsonl".to_string();
    let mut prompt = DEFAULT_PROMPT.to_string();
    while let Some(a) = args.next() {
        match a.as_str() {
            "--cwd" => cwd = args.next().context("--cwd needs a value")?,
            "--out" => out = args.next().context("--out needs a value")?,
            _ => prompt = a,
        }
    }
    let cwd = std::fs::canonicalize(&cwd)
        .with_context(|| format!("bad cwd {cwd}"))?
        .to_string_lossy()
        .into_owned();

    let mut file = std::fs::File::create(&out).with_context(|| format!("cannot create {out}"))?;
    let mut dump = |line: &str| -> Result<()> { Ok(writeln!(file, "{line}")?) };

    let mut approve = |p: acp::Perm| {
        let _ = p
            .reply
            .send(acp::approve_option(&p.params).map(str::to_string));
    };
    let mut c = acp::Client::spawn(&cwd, &[])?;
    c.request(
        "initialize",
        json!({"protocolVersion":1,"clientCapabilities":{}}),
        TIMEOUT,
        &mut dump,
        &mut approve,
    )
    .await?;
    let s = c
        .request(
            "session/new",
            json!({"cwd":cwd,"mcpServers":[]}),
            TIMEOUT,
            &mut dump,
            &mut approve,
        )
        .await?;
    let sid = s["sessionId"].as_str().context("no sessionId")?;
    let r = c
        .request(
            "session/prompt",
            json!({"sessionId":sid,"prompt":[{"type":"text","text":prompt}]}),
            TIMEOUT,
            &mut dump,
            &mut approve,
        )
        .await?;
    eprintln!("done: stopReason={}", r["stopReason"]);
    Ok(())
}
