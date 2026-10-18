//! Runs every scenario against one endpoint, sequentially.

use std::collections::BTreeMap;
use std::io::Write;
use std::time::Duration;

use anyhow::{Context, Result};

use crate::check::check;
use crate::client::{post, reachable};
use crate::config::{Endpoint, Mode};
use crate::reply::{Reply, parse_reply};
use crate::request::build_body;
use crate::result::{
    Line, ReplyLine, RunEnd, RunHeader, ScenarioInfo, ServerInfo, SnapshotInfo, StatusLine,
    write_line,
};
use crate::scenario::Scenario;
use crate::snapshot::Snapshot;

#[derive(Debug, Clone, Copy)]
pub struct RunOptions {
    pub repeat: u32,
    pub timeout: Duration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunSummary {
    pub replies: usize,
    pub grammar_unsupported: bool,
}

pub async fn run_endpoint(
    ep: &Endpoint,
    scenarios: &[Scenario],
    snapshots: &BTreeMap<String, Snapshot>,
    opts: &RunOptions,
    out: &mut impl Write,
) -> Result<RunSummary> {
    let client = reqwest::Client::new();
    let models = reachable(&client, ep).await?;
    let run_id = crate::util::new_run_id();
    let (started_at, _) = crate::util::utc_now();
    let scenarios_json = serde_json::to_vec(scenarios)?;
    let header = RunHeader {
        run_id: run_id.clone(),
        started_at,
        toolbench: crate::util::build_info(),
        endpoint: ep.clone(),
        effective_temperature: ep.effective_temperature(),
        repeat: opts.repeat,
        timeout_s: opts.timeout.as_secs(),
        snapshots: snapshots
            .iter()
            .map(|(p, s)| {
                (
                    p.clone(),
                    SnapshotInfo {
                        source: s.source.clone(),
                        sha256: s.file_sha256.clone(),
                    },
                )
            })
            .collect(),
        scenarios: ScenarioInfo {
            count: scenarios.len(),
            sha256: crate::util::sha256_hex(&scenarios_json),
        },
        server: ServerInfo { models },
        rescored_by: None,
    };
    write_line(out, &Line::Run(header))?;
    let mut fingerprint: Option<String> = None;
    let mut summary = RunSummary {
        replies: 0,
        grammar_unsupported: false,
    };
    for sc in scenarios {
        let snap = snapshots
            .get(&sc.product)
            .with_context(|| format!("no snapshot for `{}`", sc.product))?;
        let body = build_body(ep, snap, sc);
        for run in 1..=opts.repeat {
            let ex = post(&client, ep, &body, opts.timeout).await;
            if summary.replies == 0 && ep.mode == Mode::Grammar && ex.http_status == Some(400) {
                write_line(
                    out,
                    &Line::EndpointStatus(StatusLine {
                        run_id: Some(run_id.clone()),
                        endpoint: ep.name.clone(),
                        model: ep.model.clone(),
                        mode: ep.mode,
                        status: "grammar_unsupported".into(),
                        detail: ex.error.unwrap_or_default(),
                    }),
                )?;
                summary.grammar_unsupported = true;
                write_run_end(out, &run_id, fingerprint, summary.replies)?;
                return Ok(summary);
            }
            let reply = reply_for(ep.mode, ex.body.as_ref(), ex.error.as_deref());
            let checks = check(&reply, sc, snap);
            if fingerprint.is_none() && ex.error.is_none() {
                fingerprint = ex
                    .body
                    .as_ref()
                    .and_then(|b| b["system_fingerprint"].as_str())
                    .map(str::to_string);
            }
            let completion_tokens = ex
                .body
                .as_ref()
                .and_then(|b| b.pointer("/usage/completion_tokens"))
                .and_then(|v| v.as_u64());
            write_line(
                out,
                &Line::Reply(ReplyLine {
                    run_id: Some(run_id.clone()),
                    endpoint: ep.name.clone(),
                    model: ep.model.clone(),
                    mode: ep.mode,
                    scenario: sc.id.clone(),
                    product: sc.product.clone(),
                    category: sc.category,
                    run,
                    http_status: ex.http_status,
                    error: ex.error,
                    latency_ms: ex.latency_ms,
                    completion_tokens,
                    raw: ex.body,
                    reply,
                    checks,
                }),
            )?;
            summary.replies += 1;
        }
    }
    write_run_end(out, &run_id, fingerprint, summary.replies)?;
    Ok(summary)
}

fn write_run_end(
    out: &mut impl Write,
    run_id: &str,
    fingerprint: Option<String>,
    replies: usize,
) -> Result<()> {
    write_line(
        out,
        &Line::RunEnd(RunEnd {
            run_id: run_id.to_string(),
            fingerprint,
            replies,
        }),
    )
}

fn reply_for(mode: Mode, body: Option<&serde_json::Value>, error: Option<&str>) -> Reply {
    match (error, body) {
        (Some(e), _) => Reply::Unparseable {
            reason: e.to_string(),
        },
        (None, Some(b)) => parse_reply(mode, b),
        (None, None) => Reply::Unparseable {
            reason: "no response body".into(),
        },
    }
}

/// Re-applies parsing and checks to stored lines with the current
/// scenarios and snapshots. Status and end lines pass through unchanged; the
/// header is kept and marked as rescored by this build. A legacy input has
/// no header and none is added.
pub fn rescore(
    lines: &[Line],
    scenarios: &[Scenario],
    snapshots: &BTreeMap<String, Snapshot>,
) -> Result<Vec<Line>> {
    let by_id: BTreeMap<&str, &Scenario> = scenarios.iter().map(|s| (s.id.as_str(), s)).collect();
    lines
        .iter()
        .map(|line| match line {
            Line::EndpointStatus(_) | Line::RunEnd(_) => Ok(line.clone()),
            Line::Run(h) => Ok(Line::Run(RunHeader {
                rescored_by: Some(crate::util::build_info()),
                ..h.clone()
            })),
            Line::Reply(r) => {
                let sc = by_id
                    .get(r.scenario.as_str())
                    .with_context(|| format!("scenario `{}` no longer exists", r.scenario))?;
                let snap = snapshots
                    .get(&sc.product)
                    .with_context(|| format!("no snapshot for `{}`", sc.product))?;
                let reply = reply_for(r.mode, r.raw.as_ref(), r.error.as_deref());
                let checks = check(&reply, sc, snap);
                Ok(Line::Reply(ReplyLine {
                    reply,
                    checks,
                    category: sc.category,
                    ..r.clone()
                }))
            }
        })
        .collect()
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Endpoint, GrammarField, Mode};
    use crate::fake::{FakeServer, call_reply, text_reply};
    use crate::result::read_lines;
    use aivyx_stencil::ToolSpec;
    use serde_json::json;

    fn snaps() -> BTreeMap<String, Snapshot> {
        let snap = Snapshot {
            product: "pa".into(),
            source: "t".into(),
            system_prompt: None,
            file_sha256: String::new(),
            tools: vec![ToolSpec {
                name: "fs.read".into(),
                description: "d".into(),
                input_schema: json!({"type": "object", "properties": {"path": {"type": "string"}}, "required": ["path"]}),
            }],
        };
        BTreeMap::from([("pa".to_string(), snap)])
    }

    fn scenarios() -> Vec<Scenario> {
        vec![
            toml::from_str(
                r#"
id = "s1"
product = "pa"
category = "simple"
messages = [{ role = "user", content = "read a" }]
[expect]
kind = "tool_call"
tool = "fs.read"
"#,
            )
            .unwrap(),
        ]
    }

    fn ep(base_url: String, mode: Mode) -> Endpoint {
        Endpoint {
            name: "e".into(),
            base_url,
            model: "m".into(),
            mode,
            grammar_field: GrammarField::JsonSchema,
            max_tokens: 64,
            extra_body: Default::default(),
            temperature: None,
        }
    }

    fn opts(repeat: u32, secs: u64) -> RunOptions {
        RunOptions {
            repeat,
            timeout: Duration::from_secs(secs),
        }
    }

    #[tokio::test]
    async fn scores_good_bad_and_malformed_replies() {
        let server = FakeServer::start(vec![
            (200, call_reply("fs.read", "{\"path\":\"a\"}")),
            (200, call_reply("fs.nope", "{}")),
            (200, call_reply("fs.read", "{oops")),
        ])
        .await;
        let mut out = Vec::new();
        let summary = run_endpoint(
            &ep(server.base_url(), Mode::Native),
            &scenarios(),
            &snaps(),
            &opts(3, 10),
            &mut out,
        )
        .await
        .unwrap();
        assert_eq!(
            summary,
            RunSummary {
                replies: 3,
                grammar_unsupported: false
            }
        );
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("r.jsonl");
        std::fs::write(&path, &out).unwrap();
        let lines = read_lines(&path).unwrap();
        let checks: Vec<_> = lines
            .iter()
            .filter_map(|l| match l {
                Line::Reply(r) => Some(r.checks),
                _ => None,
            })
            .collect();
        assert!(checks[0].valid_call() && checks[0].right_choice);
        assert!(checks[1].parseable && !checks[1].real_name);
        assert!(!checks[2].parseable);
        assert_eq!(server.requests().len(), 3);
    }

    #[tokio::test]
    async fn http_errors_and_timeouts_are_failed_replies() {
        let server = FakeServer::start(vec![(500, json!({"error": "boom"}))]).await;
        let mut out = Vec::new();
        run_endpoint(
            &ep(server.base_url(), Mode::Native),
            &scenarios(),
            &snaps(),
            &opts(1, 10),
            &mut out,
        )
        .await
        .unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(
            text.contains("HTTP 500") && text.contains("\"unparseable\""),
            "{text}"
        );

        let slow = FakeServer::start_slow(Duration::from_secs(3)).await;
        let mut out = Vec::new();
        run_endpoint(
            &ep(slow.base_url(), Mode::Native),
            &scenarios(),
            &snaps(),
            &opts(1, 1),
            &mut out,
        )
        .await
        .unwrap();
        assert!(String::from_utf8(out).unwrap().contains("timed out"));
    }

    #[tokio::test]
    async fn a_400_on_the_first_grammar_request_marks_the_endpoint() {
        let server =
            FakeServer::start(vec![(400, json!({"error": "unknown field json_schema"}))]).await;
        let mut out = Vec::new();
        let summary = run_endpoint(
            &ep(server.base_url(), Mode::Grammar),
            &scenarios(),
            &snaps(),
            &opts(5, 10),
            &mut out,
        )
        .await
        .unwrap();
        assert!(summary.grammar_unsupported);
        assert_eq!(server.requests().len(), 1, "stops after the first request");
        assert!(
            String::from_utf8(out)
                .unwrap()
                .contains("grammar_unsupported")
        );
    }

    #[tokio::test]
    async fn an_unreachable_endpoint_stops_the_run() {
        let mut out = Vec::new();
        let err = run_endpoint(
            &ep("http://127.0.0.1:9/v1".into(), Mode::Native),
            &scenarios(),
            &snaps(),
            &opts(1, 1),
            &mut out,
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("unreachable"), "{err}");
    }

    #[tokio::test]
    async fn rescore_reapplies_checks_to_raw_bodies() {
        let server = FakeServer::start(vec![(200, text_reply("no tool"))]).await;
        let mut out = Vec::new();
        run_endpoint(
            &ep(server.base_url(), Mode::Native),
            &scenarios(),
            &snaps(),
            &opts(1, 10),
            &mut out,
        )
        .await
        .unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("r.jsonl");
        std::fs::write(&path, &out).unwrap();
        let lines = read_lines(&path).unwrap();
        let mut changed = scenarios();
        changed[0].expect = toml::from_str("kind = \"respond\"").unwrap();
        let rescored = rescore(&lines, &changed, &snaps()).unwrap();
        match &rescored[1] {
            Line::Reply(r) => assert!(r.checks.right_choice),
            _ => panic!(),
        }
    }

    async fn run_lines(server: &FakeServer, mode: Mode, repeat: u32) -> Vec<Line> {
        let mut out = Vec::new();
        run_endpoint(
            &ep(server.base_url(), mode),
            &scenarios(),
            &snaps(),
            &opts(repeat, 10),
            &mut out,
        )
        .await
        .unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("r.jsonl");
        std::fs::write(&path, &out).unwrap();
        read_lines(&path).unwrap()
    }

    #[tokio::test]
    async fn a_run_is_a_header_replies_with_its_run_id_and_an_end_line() {
        let server = FakeServer::start(vec![
            (200, call_reply("fs.read", "{\"path\":\"a\"}")),
            (200, call_reply("fs.read", "{\"path\":\"a\"}")),
            (200, call_reply("fs.read", "{\"path\":\"a\"}")),
        ])
        .await;
        let lines = run_lines(&server, Mode::Native, 3).await;
        assert_eq!(lines.len(), 5);
        let Line::Run(h) = &lines[0] else {
            panic!("first line is not the header")
        };
        assert_eq!(h.endpoint.name, "e");
        assert_eq!(h.repeat, 3);
        assert_eq!(h.scenarios.count, 1);
        assert!(h.server.models.contains(&"m".to_string()), "{:?}", h.server);
        assert_eq!(h.snapshots["pa"].source, "t");
        for l in &lines[1..4] {
            let Line::Reply(r) = l else { panic!() };
            assert_eq!(r.run_id.as_deref(), Some(h.run_id.as_str()));
        }
        let Line::RunEnd(end) = &lines[4] else {
            panic!("last line is not RunEnd")
        };
        assert_eq!(end.run_id, h.run_id);
        assert_eq!(end.replies, 3);
    }

    #[tokio::test]
    async fn grammar_unsupported_is_header_status_end() {
        let server = FakeServer::start(vec![(400, json!({"error": "no"}))]).await;
        let lines = run_lines(&server, Mode::Grammar, 5).await;
        assert!(matches!(lines[0], Line::Run(_)));
        let Line::EndpointStatus(s) = &lines[1] else {
            panic!()
        };
        assert!(s.run_id.is_some());
        let Line::RunEnd(end) = &lines[2] else {
            panic!()
        };
        assert_eq!(end.replies, 0);
        assert_eq!(lines.len(), 3);
    }

    #[tokio::test]
    async fn rescore_keeps_the_header_and_run_ids_and_marks_it_rescored() {
        let server = FakeServer::start(vec![(200, text_reply("no tool"))]).await;
        let lines = run_lines(&server, Mode::Native, 1).await;
        let rescored = rescore(&lines, &scenarios(), &snaps()).unwrap();
        let (Line::Run(before), Line::Run(after)) = (&lines[0], &rescored[0]) else {
            panic!()
        };
        assert_eq!(before.rescored_by, None);
        assert_eq!(after.rescored_by, Some(crate::util::build_info()));
        assert_eq!(after.run_id, before.run_id);
        let Line::Reply(r) = &rescored[1] else {
            panic!()
        };
        assert_eq!(r.run_id.as_deref(), Some(before.run_id.as_str()));
        assert!(matches!(rescored[2], Line::RunEnd(_)));
    }

    #[test]
    fn rescoring_a_legacy_input_adds_no_header() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../results/2026-10-18/llama-native.jsonl");
        let lines = crate::result::read_lines(&path).unwrap();
        assert!(lines.iter().all(|l| !matches!(l, Line::Run(_))));
    }
}
