//! One JSONL line per reply (or per endpoint status).

use std::collections::{BTreeMap, BTreeSet};
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

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

/// The `run_id` a line carries, if any.
fn line_run_id(line: &Line) -> Option<&str> {
    match line {
        Line::Reply(r) => r.run_id.as_deref(),
        Line::EndpointStatus(s) => s.run_id.as_deref(),
        Line::Run(h) => Some(&h.run_id),
        Line::RunEnd(e) => Some(&e.run_id),
    }
}

/// `legacy:<stem>`, or, when an earlier input already took that,
/// `legacy:<parent dir>/<stem>`, then `legacy:<parent dir>/<stem>#2`, ...
fn legacy_id(path: &Path, used: &BTreeMap<String, &Path>) -> String {
    let name = |p: Option<&std::ffi::OsStr>| {
        p.map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default()
    };
    let stem = name(path.file_stem());
    let plain = format!("legacy:{stem}");
    if !used.contains_key(&plain) {
        return plain;
    }
    let parent = name(path.parent().and_then(Path::file_name));
    let long = format!("legacy:{parent}/{stem}");
    if !used.contains_key(&long) {
        return long;
    }
    (2..)
        .map(|n| format!("{long}#{n}"))
        .find(|id| !used.contains_key(id))
        .expect("an unused suffix exists")
}

/// Reads every input for `report`. A reply or status line without a
/// `run_id` (a file written before run headers existed) is tagged with its
/// file's legacy id (see `legacy_id`), so each such file is one run. A
/// headered `run_id` found in two inputs is an error: the same run would
/// otherwise be counted twice.
pub fn read_inputs(paths: &[PathBuf]) -> Result<Vec<Line>> {
    // Every run id taken so far, headered or legacy, with its file: a legacy
    // id is never reused, and an input already carrying it is rejected.
    let mut seen: BTreeMap<String, &Path> = BTreeMap::new();
    let mut out = Vec::new();
    for path in paths {
        let mut lines = read_lines(path)?;
        let ids: BTreeSet<String> = lines
            .iter()
            .filter_map(line_run_id)
            .map(str::to_string)
            .collect();
        for id in ids {
            if let Some(prev) = seen.get(&id) {
                anyhow::bail!(
                    "run `{id}` appears in {} and {} (a rescored copy?); pass only one",
                    prev.display(),
                    path.display()
                );
            }
            seen.insert(id, path);
        }
        if lines.iter().any(|l| {
            matches!(l, Line::Reply(_) | Line::EndpointStatus(_)) && line_run_id(l).is_none()
        }) {
            let legacy = legacy_id(path, &seen);
            for line in &mut lines {
                match line {
                    Line::Reply(r) if r.run_id.is_none() => r.run_id = Some(legacy.clone()),
                    Line::EndpointStatus(s) if s.run_id.is_none() => {
                        s.run_id = Some(legacy.clone())
                    }
                    _ => {}
                }
            }
            seen.insert(legacy, path);
        }
        out.extend(lines);
    }
    Ok(out)
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
        let lines = read_inputs(&[path]).unwrap();
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

    fn ids(lines: &[Line]) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for l in lines {
            let id = match l {
                Line::Reply(r) => r.run_id.clone(),
                Line::EndpointStatus(s) => s.run_id.clone(),
                Line::Run(h) => Some(h.run_id.clone()),
                Line::RunEnd(e) => Some(e.run_id.clone()),
            };
            if let Some(id) = id
                && !out.contains(&id)
            {
                out.push(id);
            }
        }
        out
    }

    fn results(rel: &str) -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../results")
            .join(rel)
    }

    #[test]
    fn legacy_files_sharing_a_stem_get_distinct_ids() {
        let a = results("2026-10-18/llama-grammar.jsonl");
        let b = results("2026-10-18-name-first/llama-grammar.jsonl");
        let lines = read_inputs(&[a.clone(), b.clone(), a.clone()]).unwrap();
        assert_eq!(
            ids(&lines),
            [
                "legacy:llama-grammar",
                "legacy:2026-10-18-name-first/llama-grammar",
                "legacy:2026-10-18/llama-grammar",
            ]
        );
        let again = read_inputs(&[a.clone(), a.clone(), a]).unwrap();
        assert_eq!(
            ids(&again),
            [
                "legacy:llama-grammar",
                "legacy:2026-10-18/llama-grammar",
                "legacy:2026-10-18/llama-grammar#2",
            ]
        );
    }

    #[test]
    fn legacy_ids_without_a_collision_are_the_plain_stem() {
        let lines = read_inputs(&[
            results("2026-10-18/llama-native.jsonl"),
            results("2026-10-18-name-first/llama-grammar.jsonl"),
        ])
        .unwrap();
        assert_eq!(ids(&lines), ["legacy:llama-native", "legacy:llama-grammar"]);
    }

    #[test]
    fn a_file_already_tagged_with_a_legacy_id_never_merges_with_a_legacy_file() {
        // Output of an earlier `rescore` carried `legacy:<stem>` ids.
        let dir = std::env::temp_dir().join(format!("toolbench-pretag-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("old")).unwrap();
        let tagged = dir.join("old/llama-grammar.jsonl");
        let mut f = std::fs::File::create(&tagged).unwrap();
        for mut line in read_lines(&results("2026-10-18-name-first/llama-grammar.jsonl")).unwrap() {
            if let Line::Reply(r) = &mut line {
                r.run_id = Some("legacy:llama-grammar".into());
            }
            write_line(&mut f, &line).unwrap();
        }
        drop(f);
        let legacy = results("2026-10-18/llama-grammar.jsonl");
        // Tagged first: the true legacy file gets a different id.
        let lines = read_inputs(&[tagged.clone(), legacy.clone()]).unwrap();
        assert_eq!(
            ids(&lines),
            ["legacy:llama-grammar", "legacy:2026-10-18/llama-grammar"]
        );
        // Legacy first: the tagged copy would reuse its id, so it is refused.
        assert!(read_inputs(&[legacy, tagged]).is_err());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_headered_run_in_two_files_is_an_error() {
        let dir = std::env::temp_dir().join(format!("toolbench-dup-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let end = Line::RunEnd(RunEnd {
            run_id: "20261019T081502Z-abcd".into(),
            fingerprint: None,
            replies: 0,
        });
        let (a, b) = (dir.join("a.jsonl"), dir.join("b.jsonl"));
        for p in [&a, &b] {
            let mut f = std::fs::File::create(p).unwrap();
            write_line(&mut f, &end).unwrap();
        }
        let err = read_inputs(&[a.clone(), b.clone()])
            .unwrap_err()
            .to_string();
        assert_eq!(
            err,
            format!(
                "run `20261019T081502Z-abcd` appears in {} and {} (a rescored copy?); pass only one",
                a.display(),
                b.display()
            )
        );
        assert!(read_inputs(&[a]).is_ok());
        std::fs::remove_dir_all(&dir).unwrap();
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
