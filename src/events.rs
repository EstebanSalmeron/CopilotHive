//! Agent/subagent tree built from Copilot's `session-state/<id>/events.jsonl`.
//! The format is internal and undocumented (see docs/spike-acp.md); keep it isolated here.
use anyhow::Result;
use serde_json::Value;
use std::io::{Read, Seek, SeekFrom};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Status {
    Running,
    Completed,
    Cancelled,
}

#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct Tokens {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    /// Billed usage in nano AI units (1e9 = 1 AI credit = $0.01).
    pub nano_aiu: u64,
}

impl Tokens {
    fn add(&mut self, o: Tokens) {
        self.input += o.input;
        self.output += o.output;
        self.cache_read += o.cache_read;
        self.cache_write += o.cache_write;
        self.nano_aiu += o.nano_aiu;
    }

    pub fn credits(&self) -> f64 {
        self.nano_aiu as f64 / 1e9
    }

    fn from_usage(usage: &Value) -> Tokens {
        let mut t = Tokens {
            nano_aiu: usage["copilotUsage"]["totalNanoAiu"].as_u64().unwrap_or(0),
            ..Tokens::default()
        };
        for d in usage["copilotUsage"]["tokenDetails"]
            .as_array()
            .into_iter()
            .flatten()
        {
            let n = d["tokenCount"].as_u64().unwrap_or(0);
            match d["tokenType"].as_str() {
                Some("input") => t.input += n,
                Some("output") => t.output += n,
                Some("cache_read") => t.cache_read += n,
                Some("cache_write") => t.cache_write += n,
                _ => {}
            }
        }
        t
    }
}

#[derive(Default, Debug)]
pub struct KindStats {
    pub kind: String,
    pub runs: usize,
    pub running: usize,
    pub tool_calls: u64,
    pub tokens: Tokens,
    pub time: Duration,
    pub last_model: Option<String>,
    pub last_status: Option<Status>,
}

#[derive(Debug)]
pub struct Agent {
    /// `None` for the root agent.
    pub id: Option<String>,
    pub name: String,
    /// Agent type (`explore`, `general-purpose`...); `main` for the root.
    pub kind: String,
    pub status: Status,
    /// Only known once completed (`subagent.completed.durationMs`).
    pub duration_ms: Option<u64>,
    pub tool_calls: u64,
    /// Model id actually observed (`subagent.started`/`usage_record`/`model_change`).
    pub model: Option<String>,
    pub tokens: Tokens,
    pub current_tool: Option<String>,
    /// When it started running in this process (events carry no live clock we rely on).
    pub started_at: Option<Instant>,
    tool_id: Option<String>,
}

impl Agent {
    fn new(id: Option<String>, name: &str, kind: &str, status: Status) -> Self {
        Agent {
            id,
            name: name.into(),
            kind: kind.into(),
            status,
            duration_ms: None,
            tool_calls: 0,
            model: None,
            tokens: Tokens::default(),
            current_tool: None,
            started_at: None,
            tool_id: None,
        }
    }

    /// Accumulated duration plus live time if currently running.
    pub fn elapsed(&self) -> Option<Duration> {
        match (self.duration_ms, self.started_at) {
            (None, None) => None,
            (ms, start) => Some(
                Duration::from_millis(ms.unwrap_or(0))
                    + start.map_or(Duration::ZERO, |t| t.elapsed()),
            ),
        }
    }
}

#[derive(Debug)]
pub struct Tree {
    pub agents: Vec<Agent>,
    pub reasoning_effort: Option<String>,
    month_prefix: String,
    pub month_nano_aiu: u64,
}

impl Default for Tree {
    fn default() -> Self {
        Self {
            agents: vec![],
            reasoning_effort: None,
            month_prefix: crate::history::current_month().prefix,
            month_nano_aiu: 0,
        }
    }
}

impl Tree {
    fn agent(&mut self, id: Option<&str>) -> Option<&mut Agent> {
        match id {
            None => Some(self.root()),
            Some(id) => self.agents.iter_mut().find(|a| a.id.as_deref() == Some(id)),
        }
    }

    pub fn model(&self) -> Option<&str> {
        self.agents.first()?.model.as_deref()
    }

    pub fn total_tokens(&self) -> Tokens {
        let mut t = Tokens::default();
        self.agents.iter().for_each(|a| t.add(a.tokens));
        t
    }

    /// Per-type totals across every run (grouped by agent type, not by agent id).
    pub fn stats(&self) -> Vec<KindStats> {
        let mut out: Vec<KindStats> = vec![];
        for a in &self.agents {
            let i = match out.iter().position(|k| k.kind == a.kind) {
                Some(i) => i,
                None => {
                    out.push(KindStats {
                        kind: a.kind.clone(),
                        ..Default::default()
                    });
                    out.len() - 1
                }
            };
            let k = &mut out[i];
            k.runs += 1;
            k.running += (a.status == Status::Running) as usize;
            k.tool_calls += a.tool_calls;
            k.tokens.add(a.tokens);
            k.time += a.elapsed().unwrap_or_default();
            k.last_status = Some(a.status);
            if a.model.is_some() {
                k.last_model.clone_from(&a.model);
            }
        }
        out
    }

    pub fn active_subagents(&self) -> usize {
        self.agents
            .iter()
            .filter(|a| a.id.is_some() && a.status == Status::Running)
            .count()
    }

    pub fn apply(&mut self, line: &str) {
        let Ok(ev) = serde_json::from_str::<Value>(line) else {
            return;
        };
        let agent_id = ev["agentId"].as_str();
        let data = &ev["data"];
        match ev["type"].as_str() {
            Some("assistant.turn_start") if agent_id.is_none() => {
                let r = self.root();
                r.status = Status::Running;
                r.started_at = Some(Instant::now());
            }
            Some("assistant.turn_end") if agent_id.is_none() => {
                let r = self.root();
                r.status = Status::Completed;
                r.current_tool = None;
                if let Some(t) = r.started_at.take() {
                    let add = t.elapsed().as_millis() as u64;
                    r.duration_ms = Some(r.duration_ms.unwrap_or(0) + add);
                }
            }
            Some("tool.execution_start") => {
                if let Some(a) = self.agent(agent_id) {
                    a.tool_calls += 1;
                    a.current_tool = data["toolName"].as_str().map(str::to_string);
                    a.tool_id = data["toolCallId"].as_str().map(str::to_string);
                }
            }
            Some("tool.execution_complete") => {
                if let Some(a) = self.agent(agent_id)
                    && a.tool_id.as_deref() == data["toolCallId"].as_str()
                {
                    a.current_tool = None;
                    a.tool_id = None;
                }
            }
            Some("session.model_change") => {
                if let Some(m) = data["newModel"].as_str() {
                    self.root().model = Some(m.to_string());
                }
                self.reasoning_effort = data["reasoningEffort"].as_str().map(str::to_string);
            }
            Some("session.usage_record") => {
                let usage = &data["usage"];
                let tokens = Tokens::from_usage(usage);
                let model = usage["model"].as_str();
                if ev["timestamp"]
                    .as_str()
                    .is_none_or(|ts| ts.starts_with(&self.month_prefix))
                {
                    self.month_nano_aiu = self.month_nano_aiu.saturating_add(tokens.nano_aiu);
                }
                if let Some(a) = self.agent(agent_id) {
                    a.tokens.add(tokens);
                    if let Some(m) = model {
                        a.model = Some(m.to_string());
                    }
                }
            }
            Some("subagent.started") => {
                let Some(id) = agent_id else { return };
                if self.agent(Some(id)).is_some() {
                    return;
                }
                let kind = data["agentType"].as_str().unwrap_or("subagent");
                let name = ["agentDisplayName", "agentName", "agentType"]
                    .iter()
                    .find_map(|k| data[k].as_str())
                    .unwrap_or("subagent");
                let mut a = Agent::new(Some(id.to_string()), name, kind, Status::Running);
                a.model = data["model"].as_str().map(str::to_string);
                a.started_at = Some(Instant::now());
                self.agents.push(a);
            }
            Some("subagent.completed") => {
                if let Some(a) = self.agent(agent_id).filter(|a| a.id.is_some()) {
                    a.status = if data["cancelled"].as_bool() == Some(true) {
                        Status::Cancelled
                    } else {
                        Status::Completed
                    };
                    a.current_tool = None;
                    if let Some(m) = data["model"].as_str() {
                        a.model = Some(m.to_string());
                    }
                    let live = a.started_at.take().map(|t| t.elapsed().as_millis() as u64);
                    a.duration_ms = data["durationMs"].as_u64().or(live);
                    if let Some(n) = data["totalToolCalls"].as_u64() {
                        a.tool_calls = n;
                    }
                }
            }
            _ => {}
        }
    }

    fn root(&mut self) -> &mut Agent {
        if self.agents.first().is_none_or(|a| a.id.is_some()) {
            self.agents
                .insert(0, Agent::new(None, "main", "main", Status::Completed));
        }
        &mut self.agents[0]
    }
}

/// Reads what was appended since `*offset`, returning only complete lines
/// (a partial last line stays unread until its newline arrives).
pub fn read_new(r: &mut (impl Read + Seek), offset: &mut u64) -> Result<Vec<String>> {
    r.seek(SeekFrom::Start(*offset))?;
    let mut buf = Vec::new();
    r.read_to_end(&mut buf)?;
    let Some(end) = buf.iter().rposition(|&b| b == b'\n') else {
        return Ok(vec![]);
    };
    *offset += end as u64 + 1;
    Ok(String::from_utf8_lossy(&buf[..end])
        .lines()
        .map(str::to_string)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    // Synthetic events shaped like the real ones; no real content.
    const FIXTURE: &str = r#"{"type":"assistant.turn_start","data":{}}
{"type":"tool.execution_start","data":{"toolCallId":"t1","toolName":"task"}}
{"type":"subagent.started","agentId":"a1","data":{"toolCallId":"t1","agentType":"explore","agentDisplayName":"Explore Agent"}}
{"type":"tool.execution_start","agentId":"a1","data":{"toolCallId":"t2","toolName":"bash"}}
{"type":"tool.execution_start","agentId":"a1","data":{"toolCallId":"t3","toolName":"view"}}
{"type":"subagent.started","agentId":"a2","data":{"toolCallId":"t4","agentType":"general-purpose"}}
{"type":"subagent.completed","agentId":"a1","data":{"toolCallId":"t1","durationMs":1500,"totalToolCalls":2}}
garbage
"#;

    #[test]
    fn builds_tree_from_events() {
        let mut t = Tree::default();
        FIXTURE.lines().for_each(|l| t.apply(l));
        let v: Vec<_> = t
            .agents
            .iter()
            .map(|a| (a.name.as_str(), a.status, a.duration_ms, a.tool_calls))
            .collect();
        assert_eq!(
            v,
            [
                ("main", Status::Running, None, 1),
                ("Explore Agent", Status::Completed, Some(1500), 2),
                ("general-purpose", Status::Running, None, 0),
            ]
        );
        t.apply(r#"{"type":"assistant.turn_end","data":{}}"#);
        assert_eq!(t.agents[0].status, Status::Completed);
    }

    #[test]
    fn ignores_unknown_events_and_orphan_completion() {
        let mut t = Tree::default();
        t.apply(r#"{"type":"future.event","agentId":"a9","data":{}}"#);
        t.apply(r#"{"type":"subagent.completed","agentId":"a9","data":{"durationMs":5}}"#);
        t.apply(r#"{"type":"tool.execution_start","agentId":"a9","data":{}}"#);
        t.apply(r#"{"type":"subagent.started","data":{}}"#);
        assert!(t.agents.is_empty());
    }

    #[test]
    fn tracks_model_tokens_and_current_tool() {
        let mut t = Tree::default();
        for l in [
            r#"{"type":"session.model_change","data":{"newModel":"gpt-x","reasoningEffort":"high"}}"#,
            r#"{"type":"session.usage_record","data":{"usage":{"model":"gpt-x","modelDisplayName":"GPT X","copilotUsage":{"totalNanoAiu":2500000000,"tokenDetails":[{"tokenType":"input","tokenCount":10},{"tokenType":"output","tokenCount":5},{"tokenType":"cache_read","tokenCount":7},{"tokenType":"cache_write","tokenCount":1}]}}}}"#,
            r#"{"type":"subagent.started","agentId":"a1","data":{"agentType":"explore"}}"#,
            r#"{"type":"session.usage_record","agentId":"a1","data":{"usage":{"model":"small","copilotUsage":{"tokenDetails":[{"tokenType":"input","tokenCount":3}]}}}}"#,
            r#"{"type":"tool.execution_start","agentId":"a1","data":{"toolCallId":"c1","toolName":"grep"}}"#,
        ] {
            t.apply(l);
        }
        assert_eq!(t.model(), Some("gpt-x"));
        assert_eq!(t.reasoning_effort.as_deref(), Some("high"));
        assert_eq!(
            t.agents[0].tokens,
            Tokens {
                input: 10,
                output: 5,
                cache_read: 7,
                cache_write: 1,
                nano_aiu: 2_500_000_000
            }
        );
        assert_eq!(t.agents[1].model.as_deref(), Some("small"));
        assert_eq!(t.agents[1].current_tool.as_deref(), Some("grep"));
        assert_eq!(t.total_tokens().input, 13);
        assert_eq!(t.active_subagents(), 1);
        t.apply(r#"{"type":"tool.execution_complete","agentId":"a1","data":{"toolCallId":"c1"}}"#);
        assert_eq!(t.agents[1].current_tool, None);
        assert!(t.agents[1].elapsed().is_some());
    }

    #[test]
    fn stats_accumulate_tokens_per_agent_type_across_runs() {
        let mut t = Tree::default();
        let usage = |id: &str, n: u32| {
            format!(
                r#"{{"type":"session.usage_record","agentId":"{id}","data":{{"usage":{{"model":"m-{id}","copilotUsage":{{"tokenDetails":[{{"tokenType":"output","tokenCount":{n}}}]}}}}}}}}"#
            )
        };
        for l in [
            r#"{"type":"subagent.started","agentId":"a1","data":{"agentType":"explore","model":"haiku"}}"#.to_string(),
            usage("a1", 10),
            r#"{"type":"subagent.completed","agentId":"a1","data":{"durationMs":1000,"totalToolCalls":3,"model":"haiku"}}"#.to_string(),
            r#"{"type":"subagent.started","agentId":"a2","data":{"agentType":"explore"}}"#.to_string(),
            usage("a2", 5),
            r#"{"type":"subagent.started","agentId":"a3","data":{"agentType":"task"}}"#.to_string(),
            r#"{"type":"subagent.completed","agentId":"a3","data":{"cancelled":true}}"#.to_string(),
        ] {
            t.apply(&l);
        }
        let s = t.stats();
        assert_eq!(s.len(), 2);
        let e = &s[0];
        assert_eq!(
            (
                e.kind.as_str(),
                e.runs,
                e.running,
                e.tool_calls,
                e.tokens.output
            ),
            ("explore", 2, 1, 3, 15)
        );
        assert_eq!(e.last_model.as_deref(), Some("m-a2"));
        assert!(e.time >= Duration::from_secs(1));
        assert_eq!(s[1].last_status, Some(Status::Cancelled));
    }

    #[test]
    fn main_elapsed_stops_on_turn_end_and_accumulates() {
        let mut t = Tree::default();
        t.apply(r#"{"type":"assistant.turn_start","data":{}}"#);
        std::thread::sleep(Duration::from_millis(15));
        t.apply(r#"{"type":"assistant.turn_end","data":{}}"#);
        let d1 = t.agents[0].elapsed().unwrap();
        assert!(d1 >= Duration::from_millis(10));
        std::thread::sleep(Duration::from_millis(20));
        assert_eq!(t.agents[0].elapsed(), Some(d1));

        t.apply(r#"{"type":"assistant.turn_start","data":{}}"#);
        std::thread::sleep(Duration::from_millis(15));
        t.apply(r#"{"type":"assistant.turn_end","data":{}}"#);
        let d2 = t.agents[0].elapsed().unwrap();
        assert!(d2 > d1);
    }

    #[test]
    fn read_new_returns_only_complete_lines_and_advances() {
        let mut off = 0;
        let mut c = Cursor::new(b"a\nb\nc".to_vec());
        assert_eq!(read_new(&mut c, &mut off).unwrap(), ["a", "b"]);
        assert_eq!(off, 4);
        c.get_mut().extend_from_slice(b"d\ne\n");
        assert_eq!(read_new(&mut c, &mut off).unwrap(), ["cd", "e"]);
        assert!(read_new(&mut c, &mut off).unwrap().is_empty());
    }

    #[test]
    fn month_nano_aiu_counts_current_month_and_skips_previous() {
        let mut t = Tree::default();
        t.month_prefix = "2026-11-".into();
        for l in [
            r#"{"type":"session.usage_record","timestamp":"2026-10-31T23:00:00Z","data":{"usage":{"copilotUsage":{"totalNanoAiu":9000}}}}"#,
            r#"{"type":"session.usage_record","timestamp":"2026-11-01T00:00:00Z","data":{"usage":{"copilotUsage":{"totalNanoAiu":1000}}}}"#,
            r#"{"type":"session.usage_record","timestamp":"2026-11-02T00:00:00Z","agentId":"orphan-sub","data":{"usage":{"copilotUsage":{"totalNanoAiu":250}}}}"#,
            r#"{"type":"session.usage_record","data":{"usage":{"copilotUsage":{"totalNanoAiu":500}}}}"#,
        ] {
            t.apply(l);
        }
        assert_eq!(t.month_nano_aiu, 1750);
        assert_eq!(t.total_tokens().nano_aiu, 10_500);
    }
}
