//! Minimal ACP client: JSON-RPC over NDJSON against `copilot --acp --stdio`.
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::{mpsc::UnboundedReceiver, oneshot};

pub struct Client {
    _child: Child,
    stdin: ChildStdin,
    stdout: tokio::io::Lines<BufReader<ChildStdout>>,
    next_id: u64,
    /// Commands from the UI, serviced while a request is pending or the client is idle.
    ctl: Option<(UnboundedReceiver<Ctl>, String)>,
}

/// Picks the option to auto-approve a `session/request_permission` request.
pub fn approve_option(params: &Value) -> Option<&str> {
    let options = params.get("options")?.as_array()?;
    let pick = options
        .iter()
        .find(|o| o["kind"] == "allow_once")
        .or_else(|| options.first())?;
    pick["optionId"].as_str()
}

/// Picks the option to reject a `session/request_permission` request, if one is offered.
pub fn reject_option(params: &Value) -> Option<&str> {
    let o = params
        .get("options")?
        .as_array()?
        .iter()
        .find(|o| o["kind"] == "reject_once")?;
    o["optionId"].as_str()
}

/// Picks the "always allow" option of a `session/request_permission` request, if offered.
pub fn always_option(params: &Value) -> Option<&str> {
    let o = params
        .get("options")?
        .as_array()?
        .iter()
        .find(|o| o["kind"] == "allow_always")?;
    o["optionId"].as_str()
}

/// A select-type session config option (`model`, `reasoning_effort`, `allow_all`...).
#[derive(Debug, PartialEq, Clone)]
pub struct ConfigOpt {
    pub id: String,
    pub current: String,
    /// (value, display name)
    pub options: Vec<(String, String)>,
}

pub fn parse_config_options(v: &Value) -> Vec<ConfigOpt> {
    let str_of = |v: &Value, k: &str| v[k].as_str().map(str::to_string);
    v.as_array()
        .into_iter()
        .flatten()
        .filter_map(|o| {
            let options = o["options"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|x| {
                    let value = str_of(x, "value")?;
                    let name = str_of(x, "name").unwrap_or_else(|| value.clone());
                    Some((value, name))
                })
                .collect();
            Some(ConfigOpt {
                id: str_of(o, "id")?,
                current: str_of(o, "currentValue").unwrap_or_default(),
                options,
            })
        })
        .collect()
}

/// Commands the UI can send to a running session, even while a prompt is in flight.
#[derive(Debug)]
pub enum Ctl {
    Cancel,
    SetConfig { id: String, value: String },
}

/// What the UI cares about from a `session/update` notification.
#[derive(Debug, PartialEq)]
pub enum Update {
    Text(String),
    /// Only emitted while replaying a loaded session.
    User(String),
    /// New config option state (`config_option_update`).
    Config(Vec<ConfigOpt>),
    /// Context window usage.
    Usage {
        used: u64,
        size: u64,
    },
    /// `tool_call` (creation) or `tool_call_update` (partial: absent fields are `None`).
    Tool {
        id: String,
        title: Option<String>,
        kind: Option<String>,
        status: Option<String>,
    },
}

pub fn parse_update(line: &str) -> Option<Update> {
    let msg: Value = serde_json::from_str(line).ok()?;
    if msg["method"] != "session/update" {
        return None;
    }
    let u = &msg["params"]["update"];
    let s = |k: &str| u[k].as_str().map(str::to_string);
    match u["sessionUpdate"].as_str()? {
        "agent_message_chunk" => Some(Update::Text(
            u["content"]["text"].as_str().map(str::to_string)?,
        )),
        "user_message_chunk" => Some(Update::User(
            u["content"]["text"].as_str().map(str::to_string)?,
        )),
        "config_option_update" => Some(Update::Config(parse_config_options(&u["configOptions"]))),
        "usage_update" => Some(Update::Usage {
            used: u["used"].as_u64()?,
            size: u["size"].as_u64()?,
        }),
        "tool_call" | "tool_call_update" => Some(Update::Tool {
            id: s("toolCallId")?,
            title: s("title"),
            kind: s("kind"),
            status: s("status"),
        }),
        _ => None,
    }
}

/// A pending permission request; answer through `reply` with the chosen optionId
/// (`None` or a dropped sender cancels).
pub struct Perm {
    pub params: Value,
    pub reply: oneshot::Sender<Option<String>>,
}

impl Client {
    pub fn spawn(cwd: &str) -> Result<Self> {
        let mut child = Command::new("copilot")
            .args(["--acp", "--stdio"])
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .context("failed to spawn `copilot --acp --stdio`")?;
        let stdin = child.stdin.take().context("no child stdin")?;
        let stdout = child.stdout.take().context("no child stdout")?;
        Ok(Self {
            _child: child,
            stdin,
            stdout: BufReader::new(stdout).lines(),
            next_id: 0,
            ctl: None,
        })
    }

    /// Services `rx` commands for `session_id` (see [`Ctl`]) from `request` and `idle`.
    pub fn on_ctl(&mut self, rx: UnboundedReceiver<Ctl>, session_id: String) {
        self.ctl = Some((rx, session_id));
    }

    async fn handle_ctl(&mut self, ctl: Ctl) -> Result<()> {
        let sid = self.ctl.as_ref().map(|c| c.1.clone()).unwrap_or_default();
        let msg = match ctl {
            Ctl::Cancel => {
                json!({"jsonrpc":"2.0","method":"session/cancel","params":{"sessionId":sid}})
            }
            Ctl::SetConfig { id, value } => {
                // Fire and forget: the result arrives as a `config_option_update` notification.
                self.next_id += 1;
                json!({"jsonrpc":"2.0","id":self.next_id,"method":"session/set_config_option",
                    "params":{"sessionId":sid,"configId":id,"value":value}})
            }
        };
        self.send(&msg).await
    }

    /// Waits for one event while no request is pending: a line from copilot (passed to
    /// `dump`) or a UI command. Cancel-safe, so it can sit in a `select!`.
    pub async fn idle(&mut self, dump: &mut impl FnMut(&str) -> Result<()>) -> Result<()> {
        let ctl = async {
            match &mut self.ctl {
                Some((rx, _)) => rx.recv().await,
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            l = self.stdout.next_line() => match l? {
                Some(l) => dump(&l),
                None => bail!("copilot closed stdout"),
            },
            Some(c) = ctl => self.handle_ctl(c).await,
        }
    }

    async fn send(&mut self, msg: &Value) -> Result<()> {
        let mut line = serde_json::to_vec(msg)?;
        line.push(b'\n');
        self.stdin.write_all(&line).await?;
        self.stdin.flush().await?;
        Ok(())
    }

    /// Sends a request and returns its result. Every raw line received meanwhile
    /// is passed to `dump`; permission requests go to `on_perm` and the reply is
    /// awaited. Fails if no line arrives for `idle` (time spent in `on_perm` excluded).
    pub async fn request(
        &mut self,
        method: &str,
        params: Value,
        idle: Duration,
        dump: &mut impl FnMut(&str) -> Result<()>,
        on_perm: &mut impl FnMut(Perm),
    ) -> Result<Value> {
        self.next_id += 1;
        let id = self.next_id;
        self.send(&json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}))
            .await?;
        loop {
            let ctl = async {
                match &mut self.ctl {
                    Some((rx, _)) => rx.recv().await,
                    None => std::future::pending().await,
                }
            };
            let line = tokio::select! {
                r = tokio::time::timeout(idle, self.stdout.next_line()) => {
                    match r.with_context(|| format!("timed out waiting for {method}"))?? {
                        Some(l) => l,
                        None => break,
                    }
                }
                Some(c) = ctl => {
                    self.handle_ctl(c).await?;
                    continue;
                }
            };
            dump(&line)?;
            let Ok(msg) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            match (msg.get("method").and_then(Value::as_str), msg.get("id")) {
                (Some(m), Some(rid)) => {
                    let reply = if m == "session/request_permission" {
                        let (reply, rx) = oneshot::channel();
                        on_perm(Perm {
                            params: msg["params"].clone(),
                            reply,
                        });
                        match rx.await {
                            Ok(Some(opt)) => {
                                json!({"outcome":{"outcome":"selected","optionId":opt}})
                            }
                            _ => json!({"outcome":{"outcome":"cancelled"}}),
                        }
                    } else {
                        // Unsupported client capability (fs, terminal...).
                        let err = json!({"code":-32601,"message":"method not supported"});
                        self.send(&json!({"jsonrpc":"2.0","id":rid,"error":err}))
                            .await?;
                        continue;
                    };
                    self.send(&json!({"jsonrpc":"2.0","id":rid,"result":reply}))
                        .await?;
                }
                (None, Some(rid)) if *rid == json!(id) => {
                    if let Some(err) = msg.get("error") {
                        bail!("{method} failed: {err}");
                    }
                    return Ok(msg["result"].clone());
                }
                _ => {}
            }
        }
        bail!("copilot closed stdout while waiting for {method}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefers_allow_once() {
        let p = json!({"options":[
            {"optionId":"a","kind":"allow_always"},
            {"optionId":"b","kind":"allow_once"}]});
        assert_eq!(approve_option(&p), Some("b"));
        assert_eq!(approve_option(&json!({"options":[]})), None);
    }

    // Builds a Client whose "copilot" is a shell script.
    fn fake(script: &str) -> Client {
        let mut child = Command::new("sh")
            .args(["-c", script])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        Client {
            _child: child,
            stdin,
            stdout: BufReader::new(stdout).lines(),
            next_id: 0,
            ctl: None,
        }
    }

    async fn run(script: &str) -> (Result<Value>, Vec<String>) {
        let mut seen = vec![];
        let r = fake(script)
            .request(
                "m",
                json!({}),
                Duration::from_secs(5),
                &mut |l| {
                    seen.push(l.to_string());
                    Ok(())
                },
                &mut |p| {
                    let _ = p.reply.send(approve_option(&p.params).map(str::to_string));
                },
            )
            .await;
        (r, seen)
    }

    #[tokio::test]
    async fn skips_malformed_and_unrelated_lines() {
        let (r, seen) = run(
            r#"printf '%s\n' 'not json' '{"method":"session/update","params":{}}' '{"id":7,"result":1}' '{"id":1,"result":{"ok":true}}'"#,
        )
        .await;
        assert_eq!(r.unwrap(), json!({"ok":true}));
        assert_eq!(seen.len(), 4); // every raw line is dumped
    }

    #[tokio::test]
    async fn partial_last_line_or_exit_is_an_error() {
        let (r, _) = run(r#"printf '{"id":1,"res'"#).await;
        assert!(r.unwrap_err().to_string().contains("closed stdout"));
        let (r, _) = run("exit 0").await;
        assert!(r.is_err());
    }

    #[tokio::test]
    async fn idle_timeout_is_an_error() {
        let r = fake("sleep 5")
            .request(
                "m",
                json!({}),
                Duration::from_millis(50),
                &mut |_| Ok(()),
                &mut |_| {},
            )
            .await;
        assert!(r.unwrap_err().to_string().contains("timed out"));
    }

    #[tokio::test]
    async fn idle_timeout_resets_on_each_line() {
        // Total time (~0.6s) exceeds the idle limit (0.4s), but no gap does.
        let script =
            r#"sleep 0.2; echo '{}'; sleep 0.2; echo '{}'; sleep 0.2; echo '{"id":1,"result":1}'"#;
        let r = fake(script)
            .request(
                "m",
                json!({}),
                Duration::from_millis(400),
                &mut |_| Ok(()),
                &mut |_| {},
            )
            .await;
        assert_eq!(r.unwrap(), json!(1));
    }

    #[test]
    fn parses_updates_and_reject() {
        let up = |u: &str| {
            parse_update(&format!(
                r#"{{"method":"session/update","params":{{"update":{u}}}}}"#
            ))
        };
        assert_eq!(
            up(r#"{"sessionUpdate":"agent_message_chunk","content":{"text":"hi"}}"#),
            Some(Update::Text("hi".into()))
        );
        assert_eq!(
            up(r#"{"sessionUpdate":"usage_update","used":5,"size":200}"#),
            Some(Update::Usage { used: 5, size: 200 })
        );
        assert_eq!(
            up(r#"{"sessionUpdate":"tool_call_update","toolCallId":"t","status":"completed"}"#),
            Some(Update::Tool {
                id: "t".into(),
                title: None,
                kind: None,
                status: Some("completed".into())
            })
        );
        assert_eq!(up(r#"{"sessionUpdate":"plan"}"#), None);
        assert_eq!(parse_update(r#"{"method":"other"}"#), None);
        let p = json!({"options":[{"optionId":"r","kind":"reject_once"},{"optionId":"x","kind":"allow_always"}]});
        assert_eq!(reject_option(&p), Some("r"));
        assert_eq!(always_option(&p), Some("x"));
    }

    async fn first_ctl_line(ctl: Ctl) -> Value {
        // The child echoes the first line the client sends after the request.
        let mut c = fake(r#"read req; read c; echo "{\"id\":1,\"result\":$c}""#);
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        c.on_ctl(rx, "s1".into());
        tx.send(ctl).unwrap();
        c.request(
            "m",
            json!({}),
            Duration::from_secs(5),
            &mut |_| Ok(()),
            &mut |_| {},
        )
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn ctl_sends_cancel_and_set_config() {
        let r = first_ctl_line(Ctl::Cancel).await;
        assert_eq!(
            (r["method"].as_str(), r["params"]["sessionId"].as_str()),
            (Some("session/cancel"), Some("s1"))
        );
        let r = first_ctl_line(Ctl::SetConfig {
            id: "allow_all".into(),
            value: "on".into(),
        })
        .await;
        assert_eq!(r["method"], "session/set_config_option");
        assert_eq!(
            (
                r["params"]["configId"].as_str(),
                r["params"]["value"].as_str()
            ),
            (Some("allow_all"), Some("on"))
        );
    }

    #[tokio::test]
    async fn idle_forwards_lines_and_reports_exit() {
        let mut c = fake("echo '{\"a\":1}'");
        let mut seen = vec![];
        c.idle(&mut |l| {
            seen.push(l.to_string());
            Ok(())
        })
        .await
        .unwrap();
        assert_eq!(seen, [r#"{"a":1}"#]);
        assert!(c.idle(&mut |_| Ok(())).await.is_err());
    }

    #[test]
    fn parses_config_options_and_updates() {
        let opts = json!([{"id":"model","currentValue":"a","options":[{"value":"a","name":"A"},{"value":"b"}]},{"nope":1}]);
        let v = parse_config_options(&opts);
        assert_eq!(v.len(), 1);
        assert_eq!(
            v[0].options,
            [("a".into(), "A".into()), ("b".into(), "b".into())]
        );
        let l = json!({"method":"session/update","params":{"update":{"sessionUpdate":"config_option_update","configOptions":opts}}});
        assert_eq!(parse_update(&l.to_string()), Some(Update::Config(v)));
    }

    #[tokio::test]
    async fn jsonrpc_error_is_reported() {
        let (r, _) = run(r#"echo '{"id":1,"error":{"code":-1}}'"#).await;
        assert!(r.unwrap_err().to_string().contains("m failed"));
    }

    #[tokio::test]
    async fn auto_approves_permission_requests() {
        // The child echoes the client's reply back as the final result.
        let script = r#"read req
echo '{"id":9,"method":"session/request_permission","params":{"options":[{"optionId":"x","kind":"allow_once"}]}}'
read reply
echo "{\"id\":1,\"result\":$reply}""#;
        let (r, _) = run(script).await;
        let reply = r.unwrap();
        assert_eq!(reply["id"], 9);
        assert_eq!(reply["result"]["outcome"]["optionId"], "x");
    }

    #[tokio::test]
    async fn no_options_cancels_and_unknown_method_errors() {
        let script = r#"read req
echo '{"id":9,"method":"session/request_permission","params":{}}'
read r1
echo '{"id":10,"method":"fs/read_text_file","params":{}}'
read r2
echo "{\"id\":1,\"result\":[$r1,$r2]}""#;
        let (r, _) = run(script).await;
        let v = r.unwrap();
        assert_eq!(v[0]["result"]["outcome"]["outcome"], "cancelled");
        assert_eq!(v[1]["error"]["code"], -32601);
    }
}
