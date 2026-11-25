//! Past sessions read from Copilot's `session-state/<id>/{workspace.yaml,events.jsonl}`.
//! Internal, undocumented format (see docs/spike-acp.md); keep it isolated here.
use anyhow::{Context, Result};
use serde_json::Value;
use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub struct CurrentMonth {
    /// `"YYYY-MM"` for display.
    pub label: String,
    /// `"YYYY-MM-"` for matching ISO timestamps.
    pub prefix: String,
    /// UTC `YYYY-MM-01T00:00:00Z`.
    pub start: SystemTime,
}

/// Howard Hinnant's civil-from-days algorithm (UTC, proleptic Gregorian).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (y + i64::from(m <= 2), m as u32, d as u32)
}

fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = y - i64::from(m <= 2);
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400) as u64;
    let (m, d) = (m as u64, d as u64);
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe as i64 - 719_468
}

fn month_from_unix_secs(secs: u64) -> CurrentMonth {
    let (y, m, _) = civil_from_days((secs / 86_400) as i64);
    let start_days = days_from_civil(y, m, 1).max(0) as u64;
    CurrentMonth {
        label: format!("{y:04}-{m:02}"),
        prefix: format!("{y:04}-{m:02}-"),
        start: UNIX_EPOCH + Duration::from_secs(start_days * 86_400),
    }
}

pub fn current_month() -> CurrentMonth {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    month_from_unix_secs(secs)
}

/// Nano AIU billed during the month matching `prefix` (`"YYYY-MM-"`), per session id,
/// across all projects and including subagents.
pub fn month_usage(
    root: &Path,
    prefix: &str,
    min_mtime: Option<SystemTime>,
) -> HashMap<String, u64> {
    let mut out = HashMap::new();
    for d in std::fs::read_dir(root).into_iter().flatten().flatten() {
        let events = d.path().join("events.jsonl");
        if !events.is_file() {
            continue;
        }
        if let Some(min) = min_mtime {
            let Ok(mtime) = std::fs::metadata(&events).and_then(|m| m.modified()) else {
                continue;
            };
            if mtime < min {
                continue;
            }
        }
        let Ok(f) = std::fs::File::open(&events) else {
            continue;
        };
        let mut total = 0u64;
        for line in BufReader::new(f).lines() {
            let Ok(line) = line else { continue };
            if !line.contains("session.usage_record") {
                continue;
            }
            let Ok(v) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            if v["type"].as_str() == Some("session.usage_record")
                && v["timestamp"]
                    .as_str()
                    .is_some_and(|ts| ts.starts_with(prefix))
            {
                let n = v["data"]["usage"]["copilotUsage"]["totalNanoAiu"]
                    .as_u64()
                    .unwrap_or(0);
                total = total.saturating_add(n);
            }
        }
        if total > 0 {
            out.insert(d.file_name().to_string_lossy().into_owned(), total);
        }
    }
    out
}

pub struct SessionInfo {
    pub id: String,
    pub name: String,
    pub updated_at: String,
    pub events: PathBuf,
}

pub enum Entry {
    User(String),
    Agent(String),
    Tool {
        id: String,
        title: String,
        ok: Option<bool>,
    },
}

pub fn root() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".copilot/session-state"))
}

/// Sessions whose `cwd` is `cwd`, newest first. Sessions without events are skipped.
pub fn list(root: &Path, cwd: &str) -> Vec<SessionInfo> {
    let mut out: Vec<SessionInfo> = std::fs::read_dir(root)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|d| {
            let dir = d.path();
            let events = dir.join("events.jsonl");
            if !events.is_file() {
                return None;
            }
            let yaml = std::fs::read_to_string(dir.join("workspace.yaml")).ok()?;
            let field = |k: &str| {
                yaml.lines()
                    .find_map(|l| l.strip_prefix(k)?.strip_prefix(':'))
                    .map(|v| v.trim().trim_matches(['"', '\'']).to_string())
            };
            (field("cwd")? == cwd).then(|| SessionInfo {
                id: field("id").unwrap_or_else(|| d.file_name().to_string_lossy().into()),
                name: field("name")
                    .or_else(|| field("summary"))
                    .filter(|n| !n.is_empty())
                    .unwrap_or_else(|| "(sin título)".into()),
                updated_at: field("updated_at").unwrap_or_default(),
                events,
            })
        })
        .collect();
    out.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    out
}

/// Root-agent conversation: user prompts, assistant text and tool calls (subagent events are skipped).
pub fn load(events: &Path) -> Result<Vec<Entry>> {
    let f = std::fs::File::open(events).with_context(|| format!("open {}", events.display()))?;
    let mut out: Vec<Entry> = vec![];
    for line in BufReader::new(f).lines() {
        let line = line?;
        // Cheap pre-filter: snapshot events can be megabytes and are not needed.
        let wanted = [
            "\"type\":\"user.message\"",
            "\"type\":\"assistant.message\"",
            "\"type\":\"tool.execution_",
            "\"type\": \"user.message\"",
            "\"type\": \"assistant.message\"",
            "\"type\": \"tool.execution_",
        ]
        .iter()
        .any(|p| line.contains(p));
        if !wanted {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if v.get("agentId").is_some() {
            continue;
        }
        let d = &v["data"];
        let text = d["content"]
            .as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty());
        match v["type"].as_str() {
            Some("user.message") => out.extend(text.map(|s| Entry::User(s.into()))),
            Some("assistant.message") => out.extend(text.map(|s| Entry::Agent(s.into()))),
            Some("tool.execution_start") => {
                let id = d["toolCallId"].as_str().unwrap_or_default().to_string();
                let title = d["toolTitle"]
                    .as_str()
                    .or(d["toolName"].as_str())
                    .unwrap_or("tool")
                    .to_string();
                out.push(Entry::Tool {
                    id,
                    title,
                    ok: None,
                });
            }
            Some("tool.execution_complete") => {
                let cid = d["toolCallId"].as_str();
                let done = out.iter_mut().rev().find_map(|e| match e {
                    Entry::Tool { id, ok, .. } if Some(id.as_str()) == cid => Some(ok),
                    _ => None,
                });
                if let Some(ok) = done {
                    *ok = d["success"].as_bool();
                }
            }
            _ => {}
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn lists_by_cwd_and_loads_root_conversation() {
        let root = std::env::temp_dir().join(format!("hive-hist-{}", std::process::id()));
        let mk = |id: &str, cwd: &str, upd: &str, ev: &str| {
            let d = root.join(id);
            fs::create_dir_all(&d).unwrap();
            fs::write(
                d.join("workspace.yaml"),
                format!("id: {id}\ncwd: {cwd}\nname: S {id}\nupdated_at: {upd}\n"),
            )
            .unwrap();
            fs::write(d.join("events.jsonl"), ev).unwrap();
        };
        let ev = concat!(
            r#"{"type":"user.message","data":{"content":"hola"}}"#,
            "\n",
            r#"{"type":"assistant.message","data":{"content":""}}"#,
            "\n",
            r#"{"type":"tool.execution_start","data":{"toolCallId":"1","toolTitle":"ls"}}"#,
            "\n",
            r#"{"type":"tool.execution_start","agentId":"x","data":{"toolCallId":"2","toolName":"sub"}}"#,
            "\n",
            r#"{"type":"tool.execution_complete","data":{"toolCallId":"1","success":true}}"#,
            "\n",
            r#"{"type":"assistant.message","data":{"content":"listo"}}"#,
            "\n",
        );
        mk("a", "/p", "2026-01-01", ev);
        mk("b", "/p", "2026-02-01", "");
        mk("c", "/other", "2026-03-01", ev);
        let l = list(&root, "/p");
        assert_eq!(
            l.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
            ["S b", "S a"]
        );
        let e = load(&l[1].events).unwrap();
        assert!(matches!(&e[..], [
            Entry::User(u),
            Entry::Tool { ok: Some(true), .. },
            Entry::Agent(a),
        ] if u == "hola" && a == "listo"));
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn loads_when_type_is_not_first_key_and_strips_single_quotes() {
        let root = std::env::temp_dir().join(format!("hive-hist-ord-{}", std::process::id()));
        let d = root.join("s1");
        fs::create_dir_all(&d).unwrap();
        fs::write(
            d.join("workspace.yaml"),
            "id: 's1'\ncwd: '/p'\nname: 'Mi sesion'\nupdated_at: '2026-01-01'\n",
        )
        .unwrap();
        let ev = concat!(
            r#"{"id":"1","type":"user.message","data":{"content":"hola"}}"#,
            "\n",
            r#"{"id": "2", "type": "assistant.message", "data": {"content": "ok"}}"#,
            "\n",
        );
        fs::write(d.join("events.jsonl"), ev).unwrap();
        let l = list(&root, "/p");
        assert_eq!(l.len(), 1);
        assert_eq!(l[0].name, "Mi sesion");
        let e = load(&l[0].events).unwrap();
        assert!(matches!(&e[..], [
            Entry::User(u),
            Entry::Agent(a),
        ] if u == "hola" && a == "ok"));
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn civil_date_and_month_bounds_handle_leap_years() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(days_from_civil(2000, 2, 29)), (2000, 2, 29));
        assert_eq!(civil_from_days(days_from_civil(2024, 2, 29)), (2024, 2, 29));
        assert_eq!(
            civil_from_days(days_from_civil(2024, 2, 29) + 1),
            (2024, 3, 1)
        );
        // Century non-leap year: 2100-02-28 + 1 day is 2100-03-01.
        assert_eq!(
            civil_from_days(days_from_civil(2100, 2, 28) + 1),
            (2100, 3, 1)
        );
        assert_eq!(
            civil_from_days(days_from_civil(2025, 12, 31)),
            (2025, 12, 31)
        );
        assert_eq!(
            civil_from_days(days_from_civil(2025, 12, 31) + 1),
            (2026, 1, 1)
        );
        // Epoch, Jan 1 00:00:00Z, Dec 31 23:59:59Z, and mid-month.
        assert_eq!(month_from_unix_secs(0).label, "1970-01");
        let jan1 = days_from_civil(2026, 1, 1) as u64 * 86_400;
        let m_jan = month_from_unix_secs(jan1);
        assert_eq!(
            (m_jan.label.as_str(), m_jan.prefix.as_str()),
            ("2026-01", "2026-01-")
        );
        assert_eq!(m_jan.start, UNIX_EPOCH + Duration::from_secs(jan1));
        let dec31_end = days_from_civil(2026, 12, 31) as u64 * 86_400 + 86_399;
        assert_eq!(month_from_unix_secs(dec31_end).label, "2026-12");
        assert_eq!(month_from_unix_secs(dec31_end + 1).label, "2027-01");
        // 2026-11-25T12:00:00Z
        let ts = days_from_civil(2026, 11, 25) as u64 * 86_400 + 43_200;
        let m = month_from_unix_secs(ts);
        assert_eq!(m.label, "2026-11");
        assert_eq!(m.prefix, "2026-11-");
        assert_eq!(
            m.start,
            UNIX_EPOCH + Duration::from_secs(days_from_civil(2026, 11, 1) as u64 * 86_400)
        );
    }

    #[test]
    fn month_usage_handles_missing_files_corrupt_lines_and_saturation() {
        let root = std::env::temp_dir().join(format!("hive-month-edge-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        assert!(month_usage(&root, "2026-11-", None).is_empty());

        // Directory without events.jsonl and a stray regular file in root are skipped.
        fs::create_dir_all(root.join("no-events")).unwrap();
        fs::write(root.join("stray-file"), "x").unwrap();

        // Session with invalid UTF-8, false-positive text, missing timestamp, and u64::MAX saturation.
        let d = root.join("s-edge");
        fs::create_dir_all(&d).unwrap();
        let mut raw = Vec::new();
        raw.extend_from_slice(b"\xff\xfe session.usage_record\n");
        raw.extend_from_slice(
            br#"{"type":"user.message","timestamp":"2026-11-01T00:00:00Z","data":{"content":"session.usage_record"}}"#,
        );
        raw.push(b'\n');
        raw.extend_from_slice(
            br#"{"type":"session.usage_record","data":{"usage":{"copilotUsage":{"totalNanoAiu":500}}}}"#,
        );
        raw.push(b'\n');
        raw.extend_from_slice(
            format!(
                r#"{{"type":"session.usage_record","timestamp":"2026-11-01T00:00:00Z","data":{{"usage":{{"copilotUsage":{{"totalNanoAiu":{}}}}}}}}}"#,
                u64::MAX - 5
            )
            .as_bytes(),
        );
        raw.push(b'\n');
        raw.extend_from_slice(
            br#"{"type":"session.usage_record","timestamp":"2026-11-01T00:01:00Z","agentId":"sub","data":{"usage":{"copilotUsage":{"totalNanoAiu":100}}}}"#,
        );
        raw.push(b'\n');
        fs::write(d.join("events.jsonl"), raw).unwrap();

        let got = month_usage(&root, "2026-11-", None);
        assert_eq!(got.len(), 1);
        assert_eq!(got.get("s-edge"), Some(&u64::MAX));

        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn month_usage_filters_month_includes_subagents_all_cwds_and_min_mtime() {
        let root = std::env::temp_dir().join(format!("hive-month-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let mk = |id: &str, cwd: &str, ev: &str| {
            let d = root.join(id);
            fs::create_dir_all(&d).unwrap();
            fs::write(d.join("workspace.yaml"), format!("id: {id}\ncwd: {cwd}\n")).unwrap();
            fs::write(d.join("events.jsonl"), ev).unwrap();
        };
        let u = |ts: &str, agent: Option<&str>, n: u64| {
            let ag = agent
                .map(|a| format!(r#","agentId":"{a}""#))
                .unwrap_or_default();
            format!(
                r#"{{"type":"session.usage_record","timestamp":"{ts}"{ag},"data":{{"usage":{{"copilotUsage":{{"totalNanoAiu":{n}}}}}}}}}"#
            )
        };
        mk(
            "s1",
            "/proj-a",
            &[
                u("2026-10-31T23:59:59Z", None, 9_000),
                u("2026-11-01T00:00:00Z", None, 1_500_000_000),
                u("2026-11-02T10:00:00Z", Some("sub1"), 500_000_000),
                "not json session.usage_record".into(),
                r#"{"type":"user.message","timestamp":"2026-11-02T10:00:00Z","data":{}}"#.into(),
            ]
            .join("\n"),
        );
        mk(
            "s2",
            "/proj-b",
            &u("2026-11-15T08:00:00Z", None, 2_000_000_000),
        );
        mk(
            "s3-prev-only",
            "/proj-c",
            &u("2026-10-15T08:00:00Z", None, 4_000_000_000),
        );

        let got = month_usage(&root, "2026-11-", None);
        assert_eq!(got.len(), 2);
        assert_eq!(got.get("s1"), Some(&2_000_000_000));
        assert_eq!(got.get("s2"), Some(&2_000_000_000));

        // Future min_mtime skips all files written just now.
        let future = SystemTime::now() + Duration::from_secs(3600);
        assert!(month_usage(&root, "2026-11-", Some(future)).is_empty());

        fs::remove_dir_all(&root).unwrap();
    }
}
