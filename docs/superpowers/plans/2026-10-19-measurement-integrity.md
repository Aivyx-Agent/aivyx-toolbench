# toolbench Measurement Integrity Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make toolbench's numbers trustworthy:
- every run identifies itself and its settings;
- reports keep runs apart and show 95 % confidence intervals;
- temperature is recorded and can be pinned;
- capture can't save private data unseen.

**Architecture:** Each `run` writes a `run` header line and a closing
`run_end` line around its reply lines, and every reply carries the run's id.
Files without a header are read as legacy runs, one per file. The report keys
its stats by run id, computes cluster-bootstrap CIs with an in-crate
deterministic PRNG, and annotates the unchanged verdict with *clearly* or
*within noise*. Capture redacts, flags, and saves only after `--accept` or a
`y` at the terminal.

**Tech Stack:** Rust 2024; the existing deps, plus `sha2 = "0.10"` and
`regex = "1"`.

**Spec:** `docs/superpowers/specs/2026-10-19-measurement-integrity-design.md`.

## Global Constraints

- **Unchanged rule:** the verdict rule and constants (`MIN_VALID 0.98`,
  `MIN_CHOICE 0.90`, `EDGE 0.05`, `LATENCY_SLACK 1.20`) and the
  Build / DontBuild / Undecided outcome logic.
- **Bootstrap:** 2000 resamples, seed `0x5eed`, splitmix64, percentile 95 %
  interval (2.5th / 97.5th). It resamples scenarios with replacement, keeping
  all of a scenario's runs together.
- **`run_id` format:** `YYYYMMDDTHHMMSSZ-xxxx` (4 lowercase hex). Legacy runs
  are `legacy:<file stem>`.
- **Committed files:** never rewrite committed result JSONL. Legacy files must
  stay readable.
- **No new dependencies** beyond `sha2` and `regex`. No `chrono`, no `rand`.
- **Every commit:** `git commit -s`, ending with
  `Co-Authored-By: <the model's own name> <noreply@anthropic.com>`. Before
  each commit run `cargo fmt --all && cargo clippy --workspace --all-targets
  -- -D warnings && cargo test --workspace`.
- **Privacy:** the public repo must not gain private data; run the privacy grep
  before committing anything under `results/` or `fixtures/`.

## File Structure

```
crates/toolbench/
  build.rs                 NEW  embeds TOOLBENCH_COMMIT
  Cargo.toml               +sha2, +regex
  src/util.rs              NEW  utc timestamp, splitmix64, run id, sha256 hex
  src/bootstrap.rs         NEW  ci(), paired_ci()
  src/result.rs            Line::Run / Line::RunEnd, run_id on lines, read_lines_tagged()
  src/client.rs            reachable() returns the model ids
  src/snapshot.rs          Snapshot.file_sha256 (serde skip), set by load()
  src/config.rs            Endpoint.temperature, [defaults], effective_temperature()
  src/request.rs           sends temperature
  src/runner.rs            header / run_end / run_id; rescore keeps them
  src/report.rs            stats per run, CIs, verdict wording, selection, rendering
  src/capture.rs           redact(), flags(), review flow, accept
  src/main.rs              new flags; wiring
  src/lib.rs               +pub mod util; +pub mod bootstrap;
.github/workflows/ci.yml   +cargo-deny job
README.md, results/*/report.md, results/2026-10-18/DECISION.md
```

---

### Task 1: Run header, run ids, temperature

**Files:** `build.rs`, `Cargo.toml`, `src/util.rs`, `src/lib.rs`,
`src/result.rs`, `src/client.rs`, `src/snapshot.rs`, `src/config.rs`,
`src/request.rs`, `src/runner.rs`, `src/main.rs`.

**Interfaces:**
- **Produces** (used by Task 2):
  - `result::Line::{Reply, EndpointStatus, Run(RunHeader), RunEnd(RunEnd)}`.
    `ReplyLine.run_id` and `StatusLine.run_id` are `Option<String>`
    (`#[serde(default, skip_serializing_if = "Option::is_none")]`).
  - `RunHeader` has these fields:
    - `run_id: String`
    - `started_at: String` (RFC 3339 UTC)
    - `toolbench: BuildInfo { version: String, commit: String }`
    - `endpoint: Endpoint`
    - `effective_temperature: Option<f64>`
    - `repeat: u32`
    - `timeout_s: u64`
    - `snapshots: BTreeMap<String, SnapshotInfo { source: String, sha256: String }>`
    - `scenarios: ScenarioInfo { count: usize, sha256: String }`
    - `server: ServerInfo { models: Vec<String> }`
    - `rescored_by: Option<BuildInfo>` (`#[serde(default, skip_serializing_if…)]`)
  - `RunEnd { run_id: String, fingerprint: Option<String>, replies: usize }`.
  - `result::read_lines_tagged(path) -> Result<Vec<Line>>`: reply and status
    lines with no `run_id` get `Some(format!("legacy:{stem}"))`.
  - `config::Endpoint.temperature: Option<f64>`.
  - `Config.defaults: Option<Defaults { temperature: Option<f64> }>`.
    `Config::load` fills each endpoint's `temperature` from `defaults` when the
    endpoint has none.
  - `config::Endpoint::effective_temperature(&self) -> Option<f64>`:
    `extra_body["temperature"]` as f64, else `self.temperature`.
  - `client::reachable(..) -> Result<Vec<String>>`: the `data[].id` values of
    `GET /models` (empty if absent).
  - `util::{utc_now() -> (String /*rfc3339*/, String /*compact*/), new_run_id() -> String, sha256_hex(&[u8]) -> String, SplitMix64, build_info() -> BuildInfo}`.

- [ ] **Step 1: Dependencies and build script**

`crates/toolbench/Cargo.toml` `[dependencies]` add:

```toml
regex = "1"
sha2 = "0.10"
```

`crates/toolbench/build.rs`:

```rust
//! Embeds the git commit toolbench was built from as TOOLBENCH_COMMIT.

use std::process::Command;

fn main() {
    let commit = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".to_string());
    println!("cargo:rustc-env=TOOLBENCH_COMMIT={commit}");
    println!("cargo:rerun-if-changed=../../.git/HEAD");
    println!("cargo:rerun-if-changed=../../.git/refs/heads");
}
```

- [ ] **Step 2: `util.rs` (tests first)**

```rust
//! Small dependency-free helpers: UTC timestamps, a deterministic PRNG,
//! run ids, sha256, build info.

use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// (year, month, day, hour, minute, second) for Unix seconds, UTC.
/// Howard Hinnant's civil-from-days.
pub fn utc_parts(secs: u64) -> (i64, u32, u32, u32, u32, u32) {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d, (rem / 3600) as u32, (rem % 3600 / 60) as u32, (rem % 60) as u32)
}

/// (`2026-10-19T08:15:02Z`, `20261019T081502Z`).
pub fn utc_strings(secs: u64) -> (String, String) {
    let (y, mo, d, h, mi, s) = utc_parts(secs);
    (
        format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}Z"),
        format!("{y:04}{mo:02}{d:02}T{h:02}{mi:02}{s:02}Z"),
    )
}

pub fn utc_now() -> (String, String) {
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    utc_strings(secs)
}

/// splitmix64: tiny, deterministic, good enough for resampling.
pub struct SplitMix64(u64);

impl SplitMix64 {
    pub fn new(seed: u64) -> Self {
        Self(seed)
    }
    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    /// Uniform-ish index in `0..n` (`n > 0`).
    pub fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % n as u64) as usize
    }
}

/// `YYYYMMDDTHHMMSSZ-xxxx`.
pub fn new_run_id() -> String {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    let seed = now.as_nanos() as u64 ^ (u64::from(std::process::id()) << 32);
    let hex = SplitMix64::new(seed).next_u64() & 0xffff;
    format!("{}-{hex:04x}", utc_strings(now.as_secs()).1)
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuildInfo {
    pub version: String,
    pub commit: String,
}

pub fn build_info() -> BuildInfo {
    BuildInfo {
        version: env!("CARGO_PKG_VERSION").to_string(),
        commit: env!("TOOLBENCH_COMMIT").to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utc_conversion_matches_known_instants() {
        assert_eq!(utc_strings(0).0, "1970-01-01T00:00:00Z");
        assert_eq!(utc_strings(951_782_400).0, "2000-02-29T00:00:00Z");
        assert_eq!(utc_strings(1_792_313_735).1, "20261018T085535Z");
    }

    #[test]
    fn splitmix_is_deterministic() {
        let a: Vec<u64> = (0..3).map({ let mut r = SplitMix64::new(7); move |_| r.next_u64() }).collect();
        let b: Vec<u64> = (0..3).map({ let mut r = SplitMix64::new(7); move |_| r.next_u64() }).collect();
        assert_eq!(a, b);
        assert_ne!(a[0], a[1]);
    }

    #[test]
    fn run_ids_have_the_documented_shape() {
        let id = new_run_id();
        let re = regex::Regex::new(r"^\d{8}T\d{6}Z-[0-9a-f]{4}$").unwrap();
        assert!(re.is_match(&id), "{id}");
    }

    #[test]
    fn sha256_of_empty_is_the_known_digest() {
        assert_eq!(sha256_hex(b""), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
    }
}
```

Note: `1_792_313_735` is the instant from the earlier live log. Before
trusting it, check it with `date -u -d @1792313735 +%Y%m%dT%H%M%SZ` and use
that output in the assertion.

Add `pub mod util;` (and, for Task 2, `pub mod bootstrap;`) to `lib.rs`. Run
`cargo test -p toolbench util` and expect PASS.

- [ ] **Step 3: Snapshot hash, config temperature, request temperature**

`snapshot.rs`: add the field

```rust
    /// sha256 of the file this snapshot was loaded from (not serialized).
    #[serde(skip)]
    pub file_sha256: String,
```

Then:
- In `Snapshot::load`, set it to `crate::util::sha256_hex(text.as_bytes())`
  after parsing.
- Update every struct literal in tests to add `file_sha256: String::new()`.
- In `capture::snapshot_from_request`, set `file_sha256: String::new()`.

`config.rs`:
- `Endpoint` gains:

```rust
    /// Sampling temperature; `None` = the server's default (not sent).
    #[serde(default)]
    pub temperature: Option<f64>,
```

- `Config` gains `#[serde(default)] pub defaults: Option<Defaults>` with:

```rust
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Defaults {
    pub temperature: Option<f64>,
}
```

- In `Config::load`, after parsing and before `validate`, run:

```rust
        let default_temp = cfg.defaults.as_ref().and_then(|d| d.temperature);
        for ep in &mut cfg.endpoints {
            ep.temperature = ep.temperature.or(default_temp);
        }
```

  (make `cfg` mutable).
- Add the method:

```rust
    /// The temperature a request actually carries: `extra_body` wins.
    pub fn effective_temperature(&self) -> Option<f64> {
        self.extra_body
            .get("temperature")
            .and_then(serde_json::Value::as_f64)
            .or(self.temperature)
    }
```

`request.rs` `build_body`: insert this before the `extra_body` loop:

```rust
    if let Some(t) = ep.temperature {
        body["temperature"] = json!(t);
    }
```

Update the `Endpoint` literals in tests with `temperature: None`.

**Tests to add:**
- `config.rs`:
  - `[defaults] temperature = 0.6` applies to an endpoint without one.
  - An endpoint value wins over `[defaults]`.
  - `extra_body.temperature` wins in `effective_temperature`.
- `request.rs`:
  - The temperature is sent only when `Some`.

- [ ] **Step 4: `result.rs` lines**

Add the header types (as in the Interfaces above), the `Run(RunHeader)` and
`RunEnd(RunEnd)` variants on `Line`, and the `run_id` fields on `ReplyLine`
and `StatusLine`:

```rust
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
```

Add:

```rust
/// Like `read_lines`, but a reply or status line without a `run_id` (a file
/// written before run headers existed) is tagged `legacy:<file stem>`, so each
/// such file is one run.
pub fn read_lines_tagged(path: &Path) -> Result<Vec<Line>> {
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
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
```

**Tests:**
- A committed legacy file reads with every line tagged
  `legacy:llama-native`. Read
  `../../results/2026-10-18/llama-native.jsonl` via `CARGO_MANIFEST_DIR` and
  check that all 300 replies carry that id.
- A header round-trips through serde.

- [ ] **Step 5: `client.rs` and `runner.rs`**

`reachable` returns the ids:

```rust
pub async fn reachable(client: &reqwest::Client, ep: &Endpoint) -> Result<Vec<String>> {
    let resp = client
        .get(ep.models_url())
        .timeout(Duration::from_secs(5))
        .send()
        .await
        .and_then(|r| r.error_for_status())
        .with_context(|| format!("endpoint `{}` at {} is unreachable", ep.name, ep.base_url))?;
    let body: Value = resp.json().await.unwrap_or(Value::Null);
    Ok(body["data"]
        .as_array()
        .map(|a| a.iter().filter_map(|m| m["id"].as_str().map(str::to_string)).collect())
        .unwrap_or_default())
}
```

`run_endpoint` (same signature):
- after `reachable`, build and write the header;
- set `run_id: Some(run_id.clone())` on every reply and status line;
- remember the first successful reply's `raw["system_fingerprint"]` as a
  string;
- write `Line::RunEnd` before every return (normal end AND the
  grammar-unsupported early return).

```rust
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
            .map(|(p, s)| (p.clone(), SnapshotInfo { source: s.source.clone(), sha256: s.file_sha256.clone() }))
            .collect(),
        scenarios: ScenarioInfo { count: scenarios.len(), sha256: crate::util::sha256_hex(&scenarios_json) },
        server: ServerInfo { models },
        rescored_by: None,
    };
    write_line(out, &Line::Run(header))?;
```

`main.rs` `Run`: the earlier reachability pre-check stays, ignoring the
returned list (`.await?;`).

`rescore`:
- `Line::Run(h)` → clone, with `rescored_by = Some(build_info())`.
- `Line::RunEnd` passes through.
- Reply lines keep their `run_id`, because `..r.clone()` keeps it.
- A legacy input has no header and none is added.

**Runner tests:**
- The first line is `Run`, with the endpoint name, `repeat` and
  `scenarios.count == 1`, and its `server.models` contains `"m"` (the fake
  server's `/v1/models`).
- Every reply carries the same `run_id` as the header.
- The last line is `RunEnd`, with `replies == 3`.
- On grammar-unsupported, the lines are Run, EndpointStatus, RunEnd.
- `rescore` on a run file keeps the header and `run_id`s and sets
  `rescored_by`.

Adjust the existing runner tests that index lines by position: replies now
start at index 1.

`main.rs` `Rescore` and `Report`: use `result::read_lines_tagged` instead of
`read_lines`.

- [ ] **Step 6: Verify and commit**

`cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings &&
cargo test --workspace`, all green.

```bash
git add -A && git commit -s -m "feat(toolbench): every run writes a header with its settings; temperature can be pinned" -m "Co-Authored-By: …"
```

---

### Task 2: Bootstrap CIs, per-run report, verdict certainty

**Files:** `src/bootstrap.rs` (new), `src/report.rs`, `src/main.rs`.

**Interfaces:**
- **Consumes:** Task 1's `Line::Run`, `RunHeader.{run_id, started_at,
  endpoint, effective_temperature}` and `ReplyLine.run_id`.
- **Produces:**
  - `bootstrap::{RESAMPLES = 2000, SEED = 0x5eed, ci(&[(usize, usize)]) -> Option<(f64, f64)>, paired_ci(&BTreeMap<String,(usize,usize)>, &BTreeMap<String,(usize,usize)>) -> Option<(f64, f64, f64)>}`.
  - `report::Stats` gains:
    - `run_id: String`, `started_at: Option<String>` and `order: usize` (the
      index of the run's first line);
    - `temperature: Temperature`;
    - `extra_calls: usize`;
    - `scen_valid`, `scen_choice` and `scen_args`, each a
      `BTreeMap<String, (usize, usize)>`.
  - `report::Temperature { Unknown, ServerDefault, Value(f64) }`, with
    `Display` printing `?`, `default` or the number.
  - `report::aggregate(&[Line]) -> BTreeMap<String, Stats>`, keyed by
    `run_id`.
  - `report::RunSelection { picks: BTreeMap<String, String>, latest: bool }`
    (`Default`).
  - `report::Assessment { verdict: Verdict, noisy: bool, notes: Vec<String> }`
    and `report::assess(&stats, &cfg, &sel) -> Assessment`.
  - `report::verdict(&stats, &cfg, &sel) -> Verdict` is
    `assess(..).verdict`.
  - `report::render(&stats, Option<&Assessment>) -> String`.

- [ ] **Step 1: `bootstrap.rs` (tests first)**

```rust
//! Cluster bootstrap over scenarios: every scenario's runs are resampled
//! together, because the 5 runs of one scenario are not independent.

use std::collections::BTreeMap;

use crate::util::SplitMix64;

pub const RESAMPLES: usize = 2000;
pub const SEED: u64 = 0x5eed;

fn percentile_bounds(v: &mut [f64]) -> Option<(f64, f64)> {
    if v.is_empty() {
        return None;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let at = |p: f64| v[((v.len() - 1) as f64 * p).round() as usize];
    Some((at(0.025), at(0.975)))
}

/// 95 % CI of sum(num)/sum(den), resampling the per-scenario `(num, den)`.
pub fn ci(counts: &[(usize, usize)]) -> Option<(f64, f64)> {
    if counts.is_empty() || counts.iter().all(|c| c.1 == 0) {
        return None;
    }
    let mut rng = SplitMix64::new(SEED);
    let mut rates = Vec::with_capacity(RESAMPLES);
    for _ in 0..RESAMPLES {
        let (mut n, mut d) = (0usize, 0usize);
        for _ in 0..counts.len() {
            let c = counts[rng.below(counts.len())];
            n += c.0;
            d += c.1;
        }
        if d > 0 {
            rates.push(n as f64 / d as f64);
        }
    }
    percentile_bounds(&mut rates)
}

fn diff(shared: &[((usize, usize), (usize, usize))], idx: &[usize]) -> Option<f64> {
    let (mut an, mut ad, mut bn, mut bd) = (0, 0, 0, 0);
    for &i in idx {
        let (a, b) = shared[i];
        an += a.0;
        ad += a.1;
        bn += b.0;
        bd += b.1;
    }
    (ad > 0 && bd > 0).then(|| bn as f64 / bd as f64 - an as f64 / ad as f64)
}

/// (point, lo, hi) of rate(b) − rate(a) over the scenarios both have.
pub fn paired_ci(
    a: &BTreeMap<String, (usize, usize)>,
    b: &BTreeMap<String, (usize, usize)>,
) -> Option<(f64, f64, f64)> {
    let shared: Vec<((usize, usize), (usize, usize))> =
        a.iter().filter_map(|(k, ca)| b.get(k).map(|cb| (*ca, *cb))).collect();
    if shared.is_empty() {
        return None;
    }
    let all: Vec<usize> = (0..shared.len()).collect();
    let point = diff(&shared, &all)?;
    let mut rng = SplitMix64::new(SEED);
    let mut diffs = Vec::with_capacity(RESAMPLES);
    for _ in 0..RESAMPLES {
        let idx: Vec<usize> = (0..shared.len()).map(|_| rng.below(shared.len())).collect();
        if let Some(d) = diff(&shared, &idx) {
            diffs.push(d);
        }
    }
    let (lo, hi) = percentile_bounds(&mut diffs)?;
    Some((point, lo, hi))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_perfect_rate_has_a_degenerate_interval() {
        assert_eq!(ci(&[(5, 5); 60]), Some((1.0, 1.0)));
        assert_eq!(ci(&[]), None);
        assert_eq!(ci(&[(0, 0); 3]), None);
    }

    #[test]
    fn the_interval_is_deterministic_and_brackets_the_point() {
        // 60 scenarios, half always right, half always wrong → 50 %.
        let counts: Vec<(usize, usize)> = (0..60).map(|i| (if i % 2 == 0 { 5 } else { 0 }, 5)).collect();
        let a = ci(&counts).unwrap();
        assert_eq!(a, ci(&counts).unwrap(), "same seed, same CI");
        assert!(a.0 < 0.5 && 0.5 < a.1, "{a:?}");
        // Roughly ±13 points for p=0.5, n_eff=60.
        assert!(a.0 > 0.30 && a.1 < 0.70, "{a:?}");
    }

    #[test]
    fn paired_difference_is_signed_b_minus_a() {
        let a: BTreeMap<String, (usize, usize)> = (0..40).map(|i| (format!("s{i}"), (2, 5))).collect();
        let b: BTreeMap<String, (usize, usize)> = (0..40).map(|i| (format!("s{i}"), (4, 5))).collect();
        let (p, lo, hi) = paired_ci(&a, &b).unwrap();
        assert!((p - 0.4).abs() < 1e-9);
        assert!(lo <= p && p <= hi);
        assert!(paired_ci(&b, &a).unwrap().0 < 0.0);
        // No shared scenarios.
        let c: BTreeMap<String, (usize, usize)> = BTreeMap::from([("x".into(), (1, 1))]);
        assert_eq!(paired_ci(&a, &c), None);
    }
}
```

- [ ] **Step 2: `aggregate` per run (tests first)**

In `report.rs`:

**Changes to `Stats` and `aggregate`:**
- `Stats::new(run_id, endpoint, model, mode, order)`.
- `aggregate` keys on `run_id`. A `ReplyLine` or `StatusLine` with `None`
  falls back to `format!("{}/{}", endpoint, model)`, which only happens in
  unit tests that build lines by hand.
- For a `Line::Run(h)`, create or update the entry with `started_at`,
  `temperature`, endpoint, model and mode:
  - `temperature` = `Value(t)` when `h.effective_temperature` is `Some(t)`;
  - otherwise `ServerDefault`.
- Runs with no header line keep `Temperature::Unknown`.
- `Line::RunEnd` is ignored.
- Per reply, also update the per-scenario maps:
  - `scen_choice[scenario] += (right_choice as usize, 1)`;
  - `scen_valid` only when the reply `counts` (the same rule as `valid_den`);
  - `scen_args` only when `args_ok` is `Some`.
- `extra_calls += 1` when `Reply::ToolCall { extra_calls > 0 }`.
- Flaky detection is keyed by `(run_id, scenario)`.

```rust
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Temperature {
    Unknown,
    ServerDefault,
    Value(f64),
}

impl std::fmt::Display for Temperature {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unknown => f.write_str("?"),
            Self::ServerDefault => f.write_str("default"),
            Self::Value(t) => write!(f, "{t}"),
        }
    }
}
```

Add CI accessors on `Stats`:

```rust
    pub fn valid_ci(&self) -> Option<(f64, f64)> {
        crate::bootstrap::ci(&self.scen_valid.values().copied().collect::<Vec<_>>())
    }
    pub fn choice_ci(&self) -> Option<(f64, f64)> {
        crate::bootstrap::ci(&self.scen_choice.values().copied().collect::<Vec<_>>())
    }
    pub fn args_ci(&self) -> Option<(f64, f64)> {
        crate::bootstrap::ci(&self.scen_args.values().copied().collect::<Vec<_>>())
    }
```

**Existing tests:**
- Give the `lines(endpoint, …)` helper `run_id: Some(endpoint.to_string())`.
- Change `s["base/m"]` to `s["base"]`.
- Change `verdict(&s, &cfg())` to
  `verdict(&s, &cfg(), &RunSelection::default())`.
- Add `run_id: Some(..)` to any hand-built `ReplyLine` or `StatusLine`.

**New tests:**
- Two runs of one endpoint (`run_id` "r1" and "r2", 100 replies each) give
  two `Stats`.
- A header line sets `started_at` and `temperature`, a file without one is
  `Unknown`, and `extra_calls` counts.
- `choice_ci` on fabricated 50 % data brackets 0.5.

- [ ] **Step 3: Selection and `assess` (tests first)**

```rust
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RunSelection {
    /// endpoint name → run_id (`report --run name=id`).
    pub picks: BTreeMap<String, String>,
    /// For each endpoint name, use its newest run (`report --latest`).
    pub latest: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Assessment {
    pub verdict: Verdict,
    /// At least one deciding condition is within noise.
    pub noisy: bool,
    /// Extra lines (temperature mismatch, ...).
    pub notes: Vec<String>,
}

/// The runs of `name` the verdict may use, after `--run` / `--latest`.
fn candidates<'a>(stats: &'a BTreeMap<String, Stats>, name: &str, sel: &RunSelection) -> Vec<&'a Stats> {
    let mut c: Vec<&Stats> = stats.values().filter(|s| s.endpoint == name).collect();
    if let Some(id) = sel.picks.get(name) {
        c.retain(|s| &s.run_id == id);
    } else if sel.latest && c.len() > 1 {
        let newest = c
            .iter()
            .max_by(|a, b| (a.started_at.as_deref(), a.order).cmp(&(b.started_at.as_deref(), b.order)))
            .copied();
        c = newest.into_iter().collect();
    }
    c
}

fn with_ci(rate: f64, ci: Option<(f64, f64)>) -> String {
    match ci {
        Some((lo, hi)) => format!("{} [{:.1}–{:.1}]", pct(rate), lo * 100.0, hi * 100.0),
        None => pct(rate),
    }
}
```

`assess` follows today's `verdict` body step by step, with these changes:

**1. Run selection.** Use `candidates` for the baseline and for the challenger:
- 0 baseline candidates → `Undecided("no usable results for baseline …")`, as
  today.
- More than 1 → `Undecided(format!("{n} runs of `{name}`; choose one with --run {name}=<run_id> or --latest"))`.
- The same applies to the challenger, but only where the challenger is
  consulted.

**2. Bar reasons:**

```rust
        let (lo, hi) = base.valid_ci().unwrap_or((bv, bv));
        let clearly = hi < MIN_VALID;
        noisy |= !clearly;
        build.push(format!(
            "{} valid-call rate {} is below {}, {}",
            cfg.baseline, with_ci(bv, base.valid_ci()), pct(MIN_VALID),
            if clearly { "clearly" } else { "within noise" }
        ));
```

The choice bar works the same way, with `choice_ci` and `MIN_CHOICE`. The
"clears both bars" reason is *clearly* when `valid_lo >= MIN_VALID &&
choice_lo >= MIN_CHOICE`, otherwise *within noise* (and it sets `noisy`).

**3. Challenger edge.** Use
`paired_ci(&base.scen_valid, &ch.scen_valid)` and
`paired_ci(&base.scen_choice, &ch.scen_choice)`. The edge on a rate fires when
its point ≥ `EDGE − 1e-9`, the same as today, so it compares point values.
- Wording: `"on valid call by +15.3 pts [6.1–24.0], {clearly|within noise}"`.
- *clearly* when `lo >= EDGE`. Otherwise it's *within noise*, and that sets
  `noisy` whenever the condition took part in the outcome.
- The "has no 5-point edge" reason also prints both paired CIs. It's
  *within noise* when either CI's `hi` ≥ `EDGE` (an edge isn't excluded),
  otherwise *clearly*.

**4. Temperature.** When base and challenger are both selected and their
`temperature` differ, or either is `Unknown`, add the note
`"baseline and challenger temperatures differ or are unknown ({} vs {})"`.

**5. Return** `Assessment { verdict, noisy, notes }`.
`pub fn verdict(stats, cfg, sel) -> Verdict { assess(stats, cfg, sel).verdict }`.

**New tests** (fabricated `Stats` via `lines(...)` with distinct run ids and
started_at via a header `Line::Run` where needed):
- 2 runs of the baseline → Undecided mentioning `--latest`. `latest: true`
  picks the newest. `picks` picks the named one.
- A baseline at 95/100 with 100 % valid on 59 scenarios and one scenario
  failing → "below 98.0 %" + "within noise". A baseline at 50/100 → "clearly".
- A challenger edge of +10 points with a CI excluding 5 → "clearly"; with the
  CI including 5 → "within noise", and `noisy` is true.
- Different temperatures add the note.
- Each existing verdict test still gets the same `Verdict` variant.

- [ ] **Step 4: `render`**

The signature becomes `render(stats: &BTreeMap<String, Stats>, assessment: Option<&Assessment>) -> String`.

**Headline:** as today, plus ` (at least one deciding condition is within noise)` when `noisy`. The reasons follow, then `notes` as extra `- ` lines.

**Table header:**

```
| endpoint | model | run | date | temp | mode | replies | errors | extra calls | valid call | correct choice | args | hallucinated | median ms | p95 ms | tok/s |
```

**Columns:**
- `run`: the first 20 characters of `run_id`.
- `date`: the first 10 characters of `started_at`, or `—`.
- `temp`: the `Temperature` display.
- Rates: `with_ci(rate, ci)`, or `—` when there's no rate.

**Escaping:** a `cell()` helper replaces `|` with `\|`, used for endpoint,
model and run.

**Per-run sections:** the category and flaky lines label each run
`**{endpoint} / {model} ({run})**`.

**Tests:**
- A `|` in an endpoint name is escaped.
- The CI brackets appear.
- The headline gains the noise clause when `noisy`.
- The `extra calls` column is present.

- [ ] **Step 5: `main.rs` report flags**

`Report` gains:

```rust
        /// Use this run for an endpoint in the verdict: NAME=RUN_ID (repeatable).
        #[arg(long = "run", value_parser = parse_pick)]
        picks: Vec<(String, String)>,
        /// Use each endpoint's newest run in the verdict.
        #[arg(long)]
        latest: bool,
```

with

```rust
fn parse_pick(s: &str) -> Result<(String, String), String> {
    s.split_once('=')
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .ok_or_else(|| format!("expected NAME=RUN_ID, got `{s}`"))
}
```

Build
`RunSelection { picks: picks.into_iter().collect(), latest }`, then
`let assessment = cfg.verdict.as_ref().map(|v| report::assess(&stats, v, &sel));`,
then `report::render(&stats, assessment.as_ref())`.

When `--out` is given, create its parent directory first.

- [ ] **Step 6: Verify and commit**

All green. Then:
- `cargo run -q --release -p toolbench -- report results/2026-10-18/*.jsonl --config toolbench.toml`
  must print one row per file, each `legacy:<stem>`, with CIs. The rates must
  equal the committed report.md's.
- Commit: `feat(toolbench): one report row per run, 95 % CIs, verdict says what is within noise`.

---

### Task 3: Capture guard

**Files:** `src/capture.rs`, `src/main.rs`.

**Interfaces:**
- **Produces:**
  - `capture::Redactor::from_env() -> Redactor`, with
    `redact(&self, &str) -> (String, BTreeMap<&'static str, usize>)`.
  - `capture::flags(&str) -> Vec<(usize /*line*/, String /*context*/)>`.
  - `capture::serve(listen, product, source, out, CaptureOptions { accept: bool, redact: bool })`.

- [ ] **Step 1: Redactor and flags (tests first)**

```rust
pub struct Redactor {
    home: Vec<String>,
    user: Option<String>,
}

impl Redactor {
    pub fn from_env() -> Self {
        let home = std::env::var("HOME").ok();
        let mut homes = Vec::new();
        if let Some(h) = &home {
            if let Ok(c) = std::fs::canonicalize(h) {
                let c = c.to_string_lossy().into_owned();
                if &c != h {
                    homes.push(c);
                }
            }
            homes.push(h.clone());
        }
        let user = std::env::var("USER")
            .ok()
            .or_else(|| std::env::var("LOGNAME").ok())
            .or_else(|| home.as_deref().and_then(|h| h.rsplit('/').next()).map(str::to_string))
            .filter(|u| u.len() >= 3 && u != "user" && u != "root");
        Self { home: homes.into_iter().filter(|h| h.len() > 1).collect(), user }
    }

    #[cfg(test)]
    fn for_test(home: &str, user: &str) -> Self {
        Self { home: vec![home.to_string()], user: Some(user.to_string()) }
    }

    /// Replaces private details; returns the text and counts per kind.
    pub fn redact(&self, text: &str) -> (String, BTreeMap<&'static str, usize>) {
        let mut counts = BTreeMap::new();
        let mut s = text.to_string();
        let mut replace = |s: &mut String, re: &regex::Regex, with: &str, kind: &'static str| {
            let n = re.find_iter(s).count();
            if n > 0 {
                *s = re.replace_all(s, with).into_owned();
                *counts.entry(kind).or_insert(0) += n;
            }
        };
        for h in &self.home {
            let re = regex::Regex::new(&regex::escape(h)).unwrap();
            replace(&mut s, &re, "/home/user", "home path");
        }
        let url_user = regex::Regex::new(r"([a-z][a-z0-9+.-]*://)[^/\s@]+@").unwrap();
        replace(&mut s, &url_user, "$1", "url credentials");
        let email = regex::Regex::new(r"[\w.+-]+@[\w-]+\.[\w.]+").unwrap();
        // Keep the placeholder itself.
        let n = email.find_iter(&s).filter(|m| m.as_str() != "user@example.com").count();
        if n > 0 {
            s = email.replace_all(&s, "user@example.com").into_owned();
            *counts.entry("email").or_insert(0) += n;
        }
        let ip = regex::Regex::new(r"\b(\d{1,3})\.\d{1,3}\.\d{1,3}\.\d{1,3}\b").unwrap();
        let n = ip
            .captures_iter(&s)
            .filter(|c| &c[1] != "127" && !c[0].starts_with("192.0.2.") && &c[0] != "0.0.0.0")
            .count();
        if n > 0 {
            s = ip
                .replace_all(&s, |c: &regex::Captures| {
                    if &c[1] == "127" || c[0].starts_with("192.0.2.") || &c[0] == "0.0.0.0" {
                        c[0].to_string()
                    } else {
                        "192.0.2.1".to_string()
                    }
                })
                .into_owned();
            *counts.entry("ip address").or_insert(0) += n;
        }
        if let Some(u) = &self.user {
            let re = regex::Regex::new(&format!(r"\b{}\b", regex::escape(u))).unwrap();
            replace(&mut s, &re, "user", "user name");
        }
        (s, counts)
    }
}

/// Lines worth a human look after redaction: token-like strings and
/// personal-data headings. (line number, up to 80 chars of context).
pub fn flags(text: &str) -> Vec<(usize, String)> {
    let token = regex::Regex::new(r"[A-Za-z0-9+/=_-]{32,}").unwrap();
    let personal = regex::Regex::new(r"(?i)\b(memory|memories|persona|about the user|learned|preferences)\b").unwrap();
    text.lines()
        .enumerate()
        .filter(|(_, l)| token.is_match(l) || personal.is_match(l))
        .map(|(i, l)| (i + 1, l.chars().take(80).collect()))
        .collect()
}
```

**Tests:**
- `for_test("/home/alice", "alice")` replaces `/home/alice/x` →
  `/home/user/x`, and "alice said" → "user said", but not "malice"
  (whole-word).
- An email → `user@example.com`.
- `10.1.2.3` → `192.0.2.1`, while `127.0.0.1` and `192.0.2.7` stay.
- `https://bob:pw@host/x` → `https://host/x`.
- The counts are correct.
- `flags` finds a 40-character token line and a `## Learned skills` line, and
  not a normal line.

- [ ] **Step 2: Review and accept flow (tests first)**

Replace `Shared.done: Arc<Mutex<bool>>` with a state enum, plus options:

```rust
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum CaptureState { Waiting, Reviewing, Done }

#[derive(Clone, Copy, Debug)]
pub struct CaptureOptions { pub accept: bool, pub redact: bool }
```

In `chat`:
1. Lock. If the state is `Waiting` and `snapshot_from_request` succeeds, set
   `Reviewing`, clone what's needed, and unlock.
2. **Redact** (unless `!opts.redact`, which prints a warning instead) the
   system prompt and each tool description with one `Redactor::from_env()`.
   Merge the counts, and append
   `"; redacted: home path ×6, user name ×1"` (kinds sorted, `×` U+00D7) to
   `source` when any count is non-zero.
3. **Review:** compute `flags` over the redacted prompt and descriptions, and
   print the review to stderr. Then decide:
   - `opts.accept`: write the file.
   - `std::io::IsTerminal::is_terminal(&std::io::stdin())`: ask
     `save this snapshot? [y/N] ` inside `tokio::task::spawn_blocking`.
   - Otherwise: print
     `not saved: re-run with --accept after reviewing`.
4. **Save or decline:** on save, write and set `Done`. On decline, set
   `Waiting`, so the next tools request is reviewed again.
5. The HTTP response is sent without waiting for the decision. Do the
   review/decision in a `tokio::spawn` task, so the server keeps answering.

Make `snapshot_from_request` unchanged. Redaction happens in a separate
`fn redacted_snapshot(snap: Snapshot, r: &Redactor) -> (Snapshot, BTreeMap<&'static str, usize>)`,
so tests can call it.

**Tests (non-interactive: stdin is not a terminal under `cargo test`):**
- With `accept: false`, a tools request returns 200 and **no file** is
  written.
- With `accept: true`, the file is written, and its `source` contains
  `redacted:` when the fixture prompt holds the test user's home. Use
  `redacted_snapshot` with `Redactor::for_test` for the content assertion,
  and the server path only for accept/decline.
- Existing capture tests: pass `CaptureOptions { accept: true, redact: false }`.

- [ ] **Step 3: CLI**

`Capture` gains `--accept`, `--no-redact` and `--allow-remote`. In `main`:

```rust
            if !listen.ip().is_loopback() {
                anyhow::ensure!(allow_remote, "--listen {listen} is not a loopback address; pass --allow-remote to expose the capture server");
                eprintln!("capture: warning: listening on a non-loopback address ({listen})");
            }
```

Add a test for the guard, extracting it as
`fn check_listen(addr: SocketAddr, allow_remote: bool) -> Result<()>` in
`capture.rs`.

- [ ] **Step 4: Verify and commit**

All green. Commit: `feat(toolbench): capture redacts, flags and saves only after review`.

---

### Task 4: CI, Dependabot, README

**Files:** `.github/workflows/ci.yml`, `deny.toml` (only if needed),
`README.md`.

- [ ] **Step 1: cargo-deny job.** Append to `ci.yml`, under `jobs:`:

```yaml
  deny:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: EmbarkStudios/cargo-deny-action@v2
        with:
          command: check
```

If `cargo install cargo-deny --locked` is acceptable locally, run
`cargo deny check` before pushing. Otherwise push and read the CI log. Fix
any failures in `deny.toml` (for example a missing allowed licence for a new
dep): add only what's needed, each with a comment saying why.

- [ ] **Step 2: Dependabot alerts**

```bash
gh api -X PUT repos/Aivyx-Agent/aivyx-toolbench/vulnerability-alerts
gh api repos/Aivyx-Agent/aivyx-toolbench/vulnerability-alerts -i | head -1   # expect HTTP 204
```

Leave automated security fixes off.

- [ ] **Step 3: README.** In Usage, document:
- **Run files:** the run header and `run_end` lines, and that old files are
  read as `legacy:<file>` runs.
- **Report flags:** `report --run NAME=RUN_ID` and `--latest`, and that a
  report shows one row per run with 95 % CIs (cluster bootstrap over
  scenarios).
- **Temperature:** `temperature` per endpoint, and `[defaults] temperature`.
- **Capture:** the review, `--accept`, `--no-redact` and `--allow-remote`.

Results lists `results/2026-10-18/` and `results/2026-10-18-name-first/`, with
DECISION.md.

- [ ] **Step 4: Verify and commit**, then push and confirm both CI jobs are
  green. Commit: `ci: cargo deny; docs: run headers, CIs, temperature, capture review`.

---

### Task 5: Regenerate reports; correct the decision record

**Files:** `results/2026-10-18/report.md`,
`results/2026-10-18-name-first/report.md`,
`results/2026-10-18/DECISION.md`, and `aivyx-ecosystem/ROADMAP.md`
(a separate repo).

- [ ] **Step 1: Regenerate**

```bash
cargo run -q --release -p toolbench -- report results/2026-10-18/*.jsonl --config toolbench.toml --out results/2026-10-18/report.md
cargo run -q --release -p toolbench -- report results/2026-10-18-name-first/*.jsonl results/2026-10-18/llama-native.jsonl --config toolbench.toml --out results/2026-10-18-name-first/report.md
git diff --stat results/
```

Check that every rate in the new tables equals the old one. Only the CIs,
run, date, temp and extra-call columns, and the verdict wording are new.

The name-first report adds `results/2026-10-18/llama-native.jsonl`, which
is a different endpoint, so its verdict is not ambiguous. If `report` still
says a name has several runs, pass `--run <name>=legacy:<file stem>`.

- [ ] **Step 2: DECISION.md**

In `results/2026-10-18/DECISION.md`:

**1. The addendum's mistral.rs comparison.** Replace the sentence
"mistral.rs with matching settings is slightly behind llama-server, so the
engine comparison is no longer confounded" with:

> mistral.rs (72.0 %) and llama-server (76.7 %) are within noise of each
> other; the engine comparison is no longer confounded by thinking, but it
> shows no difference.

Use the CIs from the regenerated report.

**2. The headline difference.** Add a short "Uncertainty" paragraph after the
addendum table. It gives:
- the 95 % CIs of the three headline correct-choice rates, from the
  regenerated name-first report;
- the paired difference native − name-first grammar, computed with
  `bootstrap::paired_ci` over `scen_choice`.

To get that number, add a throwaway `#[test] #[ignore]` that prints it, or a
`toolbench report` note line if it is already printed. Delete the throwaway
test afterwards.

Then state that the native lead is outside the noise, if its CI's `lo` > 0.

**3. Verify** every number you write against the regenerated reports.

- [ ] **Step 3: Commit (toolbench), then the ecosystem ROADMAP**

```bash
git add results && git commit -s -m "results: regenerated reports with CIs; DECISION corrects a within-noise claim" -m "Co-Authored-By: …"
```

In the sibling repo `aivyx-ecosystem/ROADMAP.md`'s 2026-10-18
entry, change "and mistral.rs from 37.0 % to 72.0 % correct choice" to add
"(within noise of llama-server's 76.7 %)", and add one sentence on the CIs.
Commit there (`-s` not needed; that repo doesn't use DCO).

- [ ] **Step 4: Privacy grep before pushing**

```bash
grep -rnE "$HOME|$USER|@gmail|@proton" results/ README.md || echo CLEAN
```

Push toolbench. Ask the user before pushing aivyx-ecosystem.
