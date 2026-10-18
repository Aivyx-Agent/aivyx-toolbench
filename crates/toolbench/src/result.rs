//! One JSONL line per reply (or per endpoint status).

use std::collections::BTreeMap;
use std::io::{BufRead, Write};
use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::check::Checks;
use crate::config::{Endpoint, Mode};
use crate::reply::Reply;
use crate::scenario::Category;
use crate::util::BuildInfo;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Line {
    Reply(ReplyLine),
    EndpointStatus(StatusLine),
    /// First line of a run: everything needed to interpret and reproduce it.
    Run(RunHeader),
    /// Last line of a run.
    RunEnd(RunEnd),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SnapshotInfo {
    pub source: String,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScenarioInfo {
    pub count: usize,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ServerInfo {
    /// The ids `GET /models` listed (empty if it listed none).
    pub models: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunHeader {
    pub run_id: String,
    /// RFC 3339, UTC.
    pub started_at: String,
    pub toolbench: BuildInfo,
    pub endpoint: Endpoint,
    pub effective_temperature: Option<f64>,
    pub repeat: u32,
    pub timeout_s: u64,
    pub snapshots: BTreeMap<String, SnapshotInfo>,
    pub scenarios: ScenarioInfo,
    pub server: ServerInfo,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rescored_by: Option<BuildInfo>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunEnd {
    pub run_id: String,
    /// The first successful reply's `system_fingerprint`, if the server sent one.
    pub fingerprint: Option<String>,
    pub replies: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReplyLine {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    pub endpoint: String,
    pub model: String,
    pub mode: Mode,
    pub scenario: String,
    pub product: String,
    pub category: Category,
    pub run: u32,
    pub http_status: Option<u16>,
    pub error: Option<String>,
    pub latency_ms: u64,
    pub completion_tokens: Option<u64>,
    /// The response body, kept so `rescore` can re-apply the checks.
    pub raw: Option<Value>,
    pub reply: Reply,
    pub checks: Checks,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StatusLine {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    pub endpoint: String,
    pub model: String,
    pub mode: Mode,
    /// `grammar_unsupported`.
    pub status: String,
    pub detail: String,
}

pub fn write_line(out: &mut impl Write, line: &Line) -> Result<()> {
    serde_json::to_writer(&mut *out, line)?;
    out.write_all(b"\n")?;
    out.flush()?;
    Ok(())
}

pub fn read_lines(path: &Path) -> Result<Vec<Line>> {
    let file = std::fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let mut out = Vec::new();
    for (i, line) in std::io::BufReader::new(file).lines().enumerate() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        out.push(
            serde_json::from_str(&line)
                .with_context(|| format!("{} line {}", path.display(), i + 1))?,
        );
    }
    Ok(out)
}

/// Like `read_lines`, but a reply or status line without a `run_id` (a file
/// written before run headers existed) is tagged `legacy:<file stem>`, so each
/// such file is one run.
pub fn read_lines_tagged(path: &Path) -> Result<Vec<Line>> {
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let legacy = format!("legacy:{stem}");
    Ok(read_lines(path)?
        .into_iter()
        .map(|mut line| {
            match &mut line {
                Line::Reply(r) if r.run_id.is_none() => r.run_id = Some(legacy.clone()),
                Line::EndpointStatus(s) if s.run_id.is_none() => s.run_id = Some(legacy.clone()),
                _ => {}
            }
            line
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Endpoint, GrammarField};
    use crate::util::BuildInfo;
    use std::collections::BTreeMap;

    #[test]
    fn a_legacy_file_reads_with_every_line_tagged_by_its_stem() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../results/2026-10-18/llama-native.jsonl");
        let lines = read_lines_tagged(&path).unwrap();
        let replies: Vec<_> = lines
            .iter()
            .filter_map(|l| match l {
                Line::Reply(r) => Some(r),
                _ => None,
            })
            .collect();
        assert_eq!(replies.len(), 300);
        assert!(
            replies
                .iter()
                .all(|r| r.run_id.as_deref() == Some("legacy:llama-native"))
        );
    }

    #[test]
    fn a_header_round_trips_through_serde() {
        let header = RunHeader {
            run_id: "20261019T081502Z-abcd".into(),
            started_at: "2026-10-19T08:15:02Z".into(),
            toolbench: BuildInfo {
                version: "0.1.0".into(),
                commit: "abc1234".into(),
            },
            endpoint: Endpoint {
                name: "e".into(),
                base_url: "http://x/v1".into(),
                model: "m".into(),
                mode: Mode::Native,
                grammar_field: GrammarField::JsonSchema,
                max_tokens: 64,
                extra_body: Default::default(),
                temperature: Some(0.6),
            },
            effective_temperature: Some(0.6),
            repeat: 5,
            timeout_s: 120,
            snapshots: BTreeMap::from([(
                "pa".to_string(),
                SnapshotInfo {
                    source: "s".into(),
                    sha256: "ff".into(),
                },
            )]),
            scenarios: ScenarioInfo {
                count: 60,
                sha256: "aa".into(),
            },
            server: ServerInfo {
                models: vec!["m".into()],
            },
            rescored_by: None,
        };
        let line = Line::Run(header);
        let text = serde_json::to_string(&line).unwrap();
        assert!(text.contains("\"type\":\"run\"") && !text.contains("rescored_by"));
        assert_eq!(serde_json::from_str::<Line>(&text).unwrap(), line);
        let end = Line::RunEnd(RunEnd {
            run_id: "r".into(),
            fingerprint: None,
            replies: 3,
        });
        let text = serde_json::to_string(&end).unwrap();
        assert!(text.contains("\"type\":\"run_end\""));
        assert_eq!(serde_json::from_str::<Line>(&text).unwrap(), end);
    }
}
