//! One JSONL line per reply (or per endpoint status).

use std::io::{BufRead, Write};
use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::check::Checks;
use crate::config::Mode;
use crate::reply::Reply;
use crate::scenario::Category;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Line {
    Reply(ReplyLine),
    EndpointStatus(StatusLine),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReplyLine {
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
