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
use crate::result::{Line, ReplyLine, StatusLine, write_line};
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
    reachable(&client, ep).await?;
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
                        endpoint: ep.name.clone(),
                        model: ep.model.clone(),
                        mode: ep.mode,
                        status: "grammar_unsupported".into(),
                        detail: ex.error.unwrap_or_default(),
                    }),
                )?;
                summary.grammar_unsupported = true;
                return Ok(summary);
            }
            let reply = reply_for(ep.mode, ex.body.as_ref(), ex.error.as_deref());
            let checks = check(&reply, sc, snap);
            let completion_tokens = ex
                .body
                .as_ref()
                .and_then(|b| b.pointer("/usage/completion_tokens"))
                .and_then(|v| v.as_u64());
            write_line(
                out,
                &Line::Reply(ReplyLine {
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
    Ok(summary)
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
/// scenarios and snapshots; status lines pass through unchanged.
pub fn rescore(
    lines: &[Line],
    scenarios: &[Scenario],
    snapshots: &BTreeMap<String, Snapshot>,
) -> Result<Vec<Line>> {
    let by_id: BTreeMap<&str, &Scenario> = scenarios.iter().map(|s| (s.id.as_str(), s)).collect();
    lines
        .iter()
        .map(|line| match line {
            Line::EndpointStatus(_) => Ok(line.clone()),
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
            .map(|l| match l {
                Line::Reply(r) => r.checks,
                _ => panic!(),
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
        match &rescored[0] {
            Line::Reply(r) => assert!(r.checks.right_choice),
            _ => panic!(),
        }
    }
}
