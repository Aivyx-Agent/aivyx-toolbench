//! Aggregation, the pre-agreed verdict, and the Markdown report.

use std::collections::BTreeMap;
use std::fmt::Write;

use crate::bootstrap::paired_ci;
use crate::config::{Mode, VerdictConfig};
use crate::reply::Reply;
use crate::result::Line;
use crate::scenario::Category;

pub const MIN_VALID: f64 = 0.98;
pub const MIN_CHOICE: f64 = 0.90;
pub const EDGE: f64 = 0.05;
pub const LATENCY_SLACK: f64 = 1.20;

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

/// The statistics of one run.
#[derive(Debug, Clone, PartialEq)]
pub struct Stats {
    pub run_id: String,
    /// From the run header; `None` for legacy runs.
    pub started_at: Option<String>,
    /// Index of the run's first line in the input (orders legacy runs).
    pub order: usize,
    pub temperature: Temperature,
    /// `None` without a header (legacy), `Some(true)` once a `run_end` line
    /// is seen, `Some(false)` for a header with no `run_end` (killed mid-run).
    pub complete: Option<bool>,
    /// The largest `replies` count among the run's `run_end` lines.
    pub end_replies: Option<usize>,
    pub endpoint: String,
    pub model: String,
    pub mode: Mode,
    pub replies: usize,
    /// Replies with a transport failure (HTTP error / timeout).
    pub errors: usize,
    /// Replies with more than one tool call.
    pub extra_calls: usize,
    pub valid_num: usize,
    pub valid_den: usize,
    pub choice_num: usize,
    pub args_num: usize,
    pub args_den: usize,
    pub hallucinated: usize,
    pub latencies_ms: Vec<u64>,
    pub tokens: u64,
    pub token_ms: u64,
    pub grammar_unsupported: bool,
    /// (right-choice count, replies) per category.
    pub by_category: BTreeMap<Category, (usize, usize)>,
    /// (num, den) of the valid-call rate per scenario.
    pub scen_valid: BTreeMap<String, (usize, usize)>,
    /// (num, den) of the correct-choice rate per scenario.
    pub scen_choice: BTreeMap<String, (usize, usize)>,
    /// (num, den) of the args rate per scenario.
    pub scen_args: BTreeMap<String, (usize, usize)>,
    pub flaky: Vec<String>,
}

/// (valid call, right choice, args ok) of one run.
type RunOutcome = (bool, bool, Option<bool>);

fn rate(num: usize, den: usize) -> Option<f64> {
    (den > 0).then(|| num as f64 / den as f64)
}

fn add(map: &mut BTreeMap<String, (usize, usize)>, scenario: &str, ok: bool) {
    let c = map.entry(scenario.to_string()).or_default();
    c.0 += usize::from(ok);
    c.1 += 1;
}

impl Stats {
    fn new(run_id: &str, endpoint: &str, model: &str, mode: Mode, order: usize) -> Self {
        Self {
            run_id: run_id.into(),
            started_at: None,
            order,
            temperature: Temperature::Unknown,
            complete: None,
            end_replies: None,
            endpoint: endpoint.into(),
            model: model.into(),
            mode,
            replies: 0,
            errors: 0,
            extra_calls: 0,
            valid_num: 0,
            valid_den: 0,
            choice_num: 0,
            args_num: 0,
            args_den: 0,
            hallucinated: 0,
            latencies_ms: Vec::new(),
            tokens: 0,
            token_ms: 0,
            grammar_unsupported: false,
            by_category: BTreeMap::new(),
            scen_valid: BTreeMap::new(),
            scen_choice: BTreeMap::new(),
            scen_args: BTreeMap::new(),
            flaky: Vec::new(),
        }
    }
    /// More replies than the run's `run_end` reported: the input holds the
    /// run more than once (e.g. a file concatenated with its copy).
    pub fn duplicated(&self) -> bool {
        self.end_replies.is_some_and(|n| self.replies > n)
    }
    pub fn valid_rate(&self) -> Option<f64> {
        rate(self.valid_num, self.valid_den)
    }
    pub fn choice_rate(&self) -> Option<f64> {
        rate(self.choice_num, self.replies)
    }
    pub fn args_rate(&self) -> Option<f64> {
        rate(self.args_num, self.args_den)
    }
    pub fn valid_ci(&self) -> Option<(f64, f64)> {
        crate::bootstrap::ci(&self.scen_valid.values().copied().collect::<Vec<_>>())
    }
    pub fn choice_ci(&self) -> Option<(f64, f64)> {
        crate::bootstrap::ci(&self.scen_choice.values().copied().collect::<Vec<_>>())
    }
    pub fn args_ci(&self) -> Option<(f64, f64)> {
        crate::bootstrap::ci(&self.scen_args.values().copied().collect::<Vec<_>>())
    }
    fn percentile(&self, p: f64) -> Option<u64> {
        let mut v = self.latencies_ms.clone();
        v.sort_unstable();
        (!v.is_empty()).then(|| v[((v.len() - 1) as f64 * p).round() as usize])
    }
    pub fn median_ms(&self) -> Option<u64> {
        self.percentile(0.5)
    }
    pub fn p95_ms(&self) -> Option<u64> {
        self.percentile(0.95)
    }
    pub fn tokens_per_sec(&self) -> Option<f64> {
        (self.token_ms > 0).then(|| self.tokens as f64 / (self.token_ms as f64 / 1000.0))
    }
}

/// Keyed by `run_id` (a line without one falls back to
/// `"<endpoint>/<model>"`). The valid-call denominator is every reply
/// that made a call, should have made one, or could not be parsed. Transport
/// failures (`error` set: HTTP error or timeout) are excluded from the
/// valid-call rate, latency and token totals but still count as replies, errors and wrong choices.
/// A `grammar_unsupported` status only applies to a run with no replies.
pub fn aggregate(lines: &[Line]) -> BTreeMap<String, Stats> {
    let mut out: BTreeMap<String, Stats> = BTreeMap::new();
    let mut runs_by: BTreeMap<(String, String), Vec<RunOutcome>> = BTreeMap::new();
    let mut saw_status: Vec<String> = Vec::new();
    let mut ended: BTreeMap<String, usize> = BTreeMap::new();
    let key_of = |run_id: &Option<String>, endpoint: &str, model: &str| {
        run_id
            .clone()
            .unwrap_or_else(|| format!("{endpoint}/{model}"))
    };
    for (i, line) in lines.iter().enumerate() {
        match line {
            Line::EndpointStatus(s) => {
                let key = key_of(&s.run_id, &s.endpoint, &s.model);
                out.entry(key.clone())
                    .or_insert_with(|| Stats::new(&key, &s.endpoint, &s.model, s.mode, i));
                if s.status == "grammar_unsupported" {
                    saw_status.push(key);
                }
            }
            Line::Run(h) => {
                let ep = &h.endpoint;
                let st = out
                    .entry(h.run_id.clone())
                    .or_insert_with(|| Stats::new(&h.run_id, &ep.name, &ep.model, ep.mode, i));
                st.started_at = Some(h.started_at.clone());
                st.temperature = match h.effective_temperature {
                    Some(t) => Temperature::Value(t),
                    None => Temperature::ServerDefault,
                };
                st.endpoint = ep.name.clone();
                st.model = ep.model.clone();
                st.mode = ep.mode;
                st.complete = Some(false);
            }
            Line::RunEnd(e) => {
                let n = ended.entry(e.run_id.clone()).or_default();
                *n = (*n).max(e.replies);
            }
            Line::Reply(r) => {
                let key = key_of(&r.run_id, &r.endpoint, &r.model);
                let st = out
                    .entry(key.clone())
                    .or_insert_with(|| Stats::new(&key, &r.endpoint, &r.model, r.mode, i));
                st.replies += 1;
                // Made a call, could not be parsed, or answered in text
                // when a call was expected (a wrong-choice Text).
                st.errors += usize::from(r.error.is_some());
                st.extra_calls += usize::from(matches!(
                    r.reply,
                    Reply::ToolCall { extra_calls, .. } if extra_calls > 0
                ));
                let counts = r.error.is_none()
                    && match r.reply {
                        Reply::ToolCall { .. } | Reply::Unparseable { .. } => true,
                        Reply::Text { .. } => !r.checks.right_choice,
                    };
                if counts {
                    st.valid_den += 1;
                    st.valid_num += usize::from(r.checks.valid_call());
                    add(&mut st.scen_valid, &r.scenario, r.checks.valid_call());
                }
                st.choice_num += usize::from(r.checks.right_choice);
                add(&mut st.scen_choice, &r.scenario, r.checks.right_choice);
                if let Some(ok) = r.checks.args_ok {
                    st.args_den += 1;
                    st.args_num += usize::from(ok);
                    add(&mut st.scen_args, &r.scenario, ok);
                }
                st.hallucinated += usize::from(
                    r.checks.parseable
                        && matches!(r.reply, Reply::ToolCall { .. })
                        && !r.checks.real_name,
                );
                if r.error.is_none() {
                    st.latencies_ms.push(r.latency_ms);
                    if let Some(t) = r.completion_tokens {
                        st.tokens += t;
                        st.token_ms += r.latency_ms;
                    }
                }
                let cat = st.by_category.entry(r.category).or_default();
                cat.0 += usize::from(r.checks.right_choice);
                cat.1 += 1;
                runs_by.entry((key, r.scenario.clone())).or_default().push((
                    r.checks.valid_call(),
                    r.checks.right_choice,
                    r.checks.args_ok,
                ));
            }
        }
    }
    for ((key, scenario), runs) in runs_by {
        let first = runs[0];
        let differs = runs
            .iter()
            .any(|r| r.0 != first.0 || r.1 != first.1 || r.2 != first.2);
        if differs && let Some(st) = out.get_mut(&key) {
            st.flaky.push(scenario);
        }
    }
    for key in saw_status {
        if let Some(st) = out.get_mut(&key) {
            st.grammar_unsupported = st.replies == 0;
        }
    }
    for st in out.values_mut() {
        if st.complete.is_some() {
            st.complete = Some(ended.contains_key(&st.run_id));
        }
        st.end_replies = ended.get(&st.run_id).copied();
    }
    out
}

#[derive(Debug, Clone, PartialEq)]
pub enum Verdict {
    Build(Vec<String>),
    DontBuild(Vec<String>),
    Undecided(String),
}

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

fn pct(x: f64) -> String {
    format!("{:.1} %", x * 100.0)
}

/// The runs of `name` the verdict may use, after `--run` / `--latest`.
fn candidates<'a>(
    stats: &'a BTreeMap<String, Stats>,
    name: &str,
    sel: &RunSelection,
) -> Vec<&'a Stats> {
    let mut c: Vec<&Stats> = stats.values().filter(|s| s.endpoint == name).collect();
    if let Some(id) = sel.picks.get(name) {
        c.retain(|s| &s.run_id == id);
    } else if sel.latest && c.len() > 1 {
        let newest = c
            .iter()
            .max_by(|a, b| {
                (a.started_at.as_deref(), a.order).cmp(&(b.started_at.as_deref(), b.order))
            })
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

fn noise(clearly: bool) -> &'static str {
    if clearly { "clearly" } else { "within noise" }
}

fn many_runs(n: usize, name: &str) -> String {
    format!("{n} runs of `{name}`; choose one with --run {name}=<run_id> or --latest")
}

/// The overall difference `b − a` in points, then the paired CI over the
/// shared scenarios: `+15.3 pts [6.1–24.0]` when the paired point equals the
/// overall one (within 0.05), else
/// `+15.3 pts overall; paired over 50 shared scenarios +17.5 pts [8.6–27.7]`,
/// so the printed point always lies in its bracket and matches the table.
fn edge_text(
    a: f64,
    b: f64,
    ci: Option<(f64, f64, f64)>,
    a_map: &BTreeMap<String, (usize, usize)>,
    b_map: &BTreeMap<String, (usize, usize)>,
) -> String {
    let overall = ((b * 1000.0).round() - (a * 1000.0).round()) / 10.0;
    match ci {
        Some((p, lo, hi)) if ((b - a) * 100.0 - p * 100.0).abs() < 0.05 => {
            format!("{overall:+.1} pts [{:.1}–{:.1}]", lo * 100.0, hi * 100.0)
        }
        Some((p, lo, hi)) => {
            let shared = a_map.keys().filter(|k| b_map.contains_key(*k)).count();
            format!(
                "{overall:+.1} pts overall; paired over {shared} shared scenarios {:+.1} pts [{:.1}–{:.1}]",
                p * 100.0,
                lo * 100.0,
                hi * 100.0
            )
        }
        None => format!("{overall:+.1} pts"),
    }
}

/// `--run name=id` that matches no run of `name`.
fn pick_miss(stats: &BTreeMap<String, Stats>, name: &str, sel: &RunSelection) -> Option<String> {
    let id = sel.picks.get(name)?;
    let runs: Vec<&str> = stats
        .values()
        .filter(|s| s.endpoint == name)
        .map(|s| s.run_id.as_str())
        .collect();
    (!runs.contains(&id.as_str()))
        .then(|| format!("no run `{id}` of `{name}` (runs: {})", runs.join(", ")))
}

fn incomplete_note(s: &Stats, notes: &mut Vec<String>) {
    if s.complete == Some(false) {
        notes.push(format!(
            "run `{}` of `{}` is incomplete (no run_end line)",
            s.run_id, s.endpoint
        ));
    }
}

fn duplicate_note(s: &Stats, notes: &mut Vec<String>) {
    if let Some(n) = s.end_replies.filter(|_| s.duplicated()) {
        notes.push(format!(
            "run `{}` of `{}` has {} replies but its run_end says {n} (duplicated input?)",
            s.run_id, s.endpoint, s.replies
        ));
    }
}

pub fn assess(
    stats: &BTreeMap<String, Stats>,
    cfg: &VerdictConfig,
    sel: &RunSelection,
) -> Assessment {
    let mut noisy = false;
    let mut notes = Vec::new();
    let verdict = decide(stats, cfg, sel, &mut noisy, &mut notes);
    Assessment {
        verdict,
        noisy,
        notes,
    }
}

pub fn verdict(
    stats: &BTreeMap<String, Stats>,
    cfg: &VerdictConfig,
    sel: &RunSelection,
) -> Verdict {
    assess(stats, cfg, sel).verdict
}

fn decide(
    stats: &BTreeMap<String, Stats>,
    cfg: &VerdictConfig,
    sel: &RunSelection,
    noisy: &mut bool,
    notes: &mut Vec<String>,
) -> Verdict {
    // Each model has its own endpoint name, so endpoint names identify
    // runs; several runs of one name need `--run` or `--latest`.
    if let Some(miss) = pick_miss(stats, &cfg.baseline, sel) {
        return Verdict::Undecided(miss);
    }
    let bases = candidates(stats, &cfg.baseline, sel);
    if bases.len() > 1 {
        return Verdict::Undecided(many_runs(bases.len(), &cfg.baseline));
    }
    let Some(base) = bases
        .first()
        .copied()
        .filter(|b| !b.grammar_unsupported && b.replies > 0)
    else {
        return Verdict::Undecided(format!("no usable results for baseline `{}`", cfg.baseline));
    };
    incomplete_note(base, notes);
    duplicate_note(base, notes);
    let (Some(bv), Some(bc)) = (base.valid_rate(), base.choice_rate()) else {
        return Verdict::Undecided("baseline has no rates".into());
    };
    let chs = candidates(stats, &cfg.challenger, sel);
    let challenger = match chs.as_slice() {
        [] => None,
        [one] => {
            incomplete_note(one, notes);
            duplicate_note(one, notes);
            if one.temperature != base.temperature
                || one.temperature == Temperature::Unknown
                || base.temperature == Temperature::Unknown
            {
                notes.push(format!(
                    "baseline and challenger temperatures differ or are unknown ({} vs {})",
                    base.temperature, one.temperature
                ));
            }
            Some(*one)
        }
        // Ambiguous: decided below, once it is known whether it matters.
        _ => None,
    };
    let mut build = Vec::new();
    let mut keep = Vec::new();
    if bv < MIN_VALID {
        let (_, hi) = base.valid_ci().unwrap_or((bv, bv));
        let clearly = hi < MIN_VALID;
        *noisy |= !clearly;
        build.push(format!(
            "{} valid-call rate {} is below {}, {}",
            cfg.baseline,
            with_ci(bv, base.valid_ci()),
            pct(MIN_VALID),
            noise(clearly)
        ));
    }
    if bc < MIN_CHOICE {
        let (_, hi) = base.choice_ci().unwrap_or((bc, bc));
        let clearly = hi < MIN_CHOICE;
        *noisy |= !clearly;
        build.push(format!(
            "{} correct-choice rate {} is below {}, {}",
            cfg.baseline,
            with_ci(bc, base.choice_ci()),
            pct(MIN_CHOICE),
            noise(clearly)
        ));
    }
    let cleared = build.is_empty();
    if cleared {
        let valid_lo = base.valid_ci().map_or(bv, |c| c.0);
        let choice_lo = base.choice_ci().map_or(bc, |c| c.0);
        let clearly = valid_lo >= MIN_VALID && choice_lo >= MIN_CHOICE;
        *noisy |= !clearly;
        keep.push(format!(
            "{} clears both bars ({} valid, {} correct choice), {}",
            cfg.baseline,
            with_ci(bv, base.valid_ci()),
            with_ci(bc, base.choice_ci()),
            noise(clearly)
        ));
    }
    if cleared {
        let note = keep.join("; ");
        if let Some(miss) = pick_miss(stats, &cfg.challenger, sel) {
            return Verdict::Undecided(miss);
        }
        if chs.len() > 1 {
            return Verdict::Undecided(many_runs(chs.len(), &cfg.challenger));
        }
        match challenger {
            None => {
                return Verdict::Undecided(format!(
                    "challenger `{}` was not measured; condition 2 is untested ({note})",
                    cfg.challenger
                ));
            }
            Some(ch) if ch.grammar_unsupported => {
                return Verdict::Undecided(format!(
                    "challenger `{}` rejected the grammar request; condition 2 is untested ({note})",
                    cfg.challenger
                ));
            }
            Some(ch) if ch.replies == 0 => {
                return Verdict::Undecided(format!(
                    "challenger `{}` was not measured; condition 2 is untested ({note})",
                    cfg.challenger
                ));
            }
            Some(ch) if ch.valid_rate().is_none() => {
                return Verdict::Undecided(format!(
                    "challenger `{}` produced no scorable replies (all {} failed: transport errors); condition 2 is untested ({note})",
                    cfg.challenger, ch.errors
                ));
            }
            _ => {}
        }
    }
    if !cleared {
        // The outcome is Build either way; the edge is just not compared.
        if let Some(miss) = pick_miss(stats, &cfg.challenger, sel) {
            notes.push(miss);
        } else if chs.len() > 1 {
            notes.push(many_runs(chs.len(), &cfg.challenger));
        }
    }
    if let Some(ch) =
        challenger.filter(|c| !c.grammar_unsupported && c.replies > 0 && c.valid_rate().is_some())
    {
        let (cv, cc) = (
            ch.valid_rate().unwrap_or(0.0),
            ch.choice_rate().unwrap_or(0.0),
        );
        let pv = paired_ci(&base.scen_valid, &ch.scen_valid);
        let pc = paired_ci(&base.scen_choice, &ch.scen_choice);
        // The outcome compares the overall-rate points, exactly as before
        // the CIs; the paired CI (and its point) is only what gets printed.
        let v_edge = cv - bv >= EDGE - 1e-9;
        let c_edge = cc - bc >= EDGE - 1e-9;
        let edge = v_edge || c_edge;
        let comparable = match (ch.median_ms(), base.median_ms()) {
            (Some(c), Some(b)) => c as f64 <= b as f64 * LATENCY_SLACK,
            _ => false,
        };
        let edge_clearly = |ci: Option<(f64, f64, f64)>| ci.is_some_and(|c| c.1 >= EDGE - 1e-9);
        if edge && comparable {
            let mut beats = Vec::new();
            let mut others = Vec::new();
            for (fires, label, a, b, ci, am, bm) in [
                (
                    v_edge,
                    "valid call",
                    bv,
                    cv,
                    pv,
                    &base.scen_valid,
                    &ch.scen_valid,
                ),
                (
                    c_edge,
                    "correct choice",
                    bc,
                    cc,
                    pc,
                    &base.scen_choice,
                    &ch.scen_choice,
                ),
            ] {
                if fires {
                    let clearly = edge_clearly(ci);
                    if cleared {
                        *noisy |= !clearly;
                    }
                    beats.push(format!(
                        "on {label} by {} ({})",
                        edge_text(a, b, ci, am, bm),
                        noise(clearly)
                    ));
                } else {
                    others.push(format!("{label} {} vs {}", pct(b), pct(a)));
                }
            }
            let mut reason = format!(
                "{} beats {} {}, at comparable median speed",
                cfg.challenger,
                cfg.baseline,
                beats.join(" and ")
            );
            if !others.is_empty() {
                reason.push_str("; ");
                reason.push_str(&others.join(", "));
            }
            build.push(reason);
        } else if edge {
            keep.push(format!(
                "{} is better ({} / {}) but more than 20 % slower",
                cfg.challenger,
                pct(cv),
                pct(cc)
            ));
        } else {
            // An edge is not excluded while either CI reaches 5 points.
            let open = |ci: Option<(f64, f64, f64)>| ci.is_none_or(|c| c.2 >= EDGE);
            let clearly = !open(pv) && !open(pc);
            if cleared {
                *noisy |= !clearly;
            }
            keep.push(format!(
                "{} has no 5-point edge (valid call {}, correct choice {}), {}",
                cfg.challenger,
                edge_text(bv, cv, pv, &base.scen_valid, &ch.scen_valid),
                edge_text(bc, cc, pc, &base.scen_choice, &ch.scen_choice),
                noise(clearly)
            ));
        }
    }
    if build.is_empty() {
        Verdict::DontBuild(keep)
    } else {
        Verdict::Build(build)
    }
}

fn opt_ci(x: Option<f64>, ci: Option<(f64, f64)>) -> String {
    x.map(|r| with_ci(r, ci)).unwrap_or_else(|| "—".into())
}

/// A Markdown table cell: `|` escaped.
fn cell(s: &str) -> String {
    s.replace('|', "\\|")
}

fn run_cell(s: &Stats) -> String {
    let mut out = cell(&s.run_id);
    if s.complete == Some(false) {
        out.push_str(" (incomplete)");
    }
    if s.duplicated() {
        out.push_str(" (duplicated?)");
    }
    out
}

fn label(s: &Stats) -> String {
    format!(
        "**{} / {} ({})**",
        cell(&s.endpoint),
        cell(&s.model),
        run_cell(s)
    )
}

pub fn render(stats: &BTreeMap<String, Stats>, assessment: Option<&Assessment>) -> String {
    let mut md = String::from("# toolbench report\n\n");
    if let Some(a) = assessment {
        let (head, reasons) = match &a.verdict {
            Verdict::Build(r) => ("**Verdict: build aivyx-engine.**", r.clone()),
            Verdict::DontBuild(r) => (
                "**Verdict: don't build aivyx-engine for tool calls.**",
                r.clone(),
            ),
            Verdict::Undecided(r) => ("**Verdict: undecided.**", vec![r.clone()]),
        };
        let clause = if a.noisy {
            " (at least one deciding condition is within noise)"
        } else {
            ""
        };
        let _ = writeln!(md, "{head}{clause}\n");
        for r in reasons.iter().chain(&a.notes) {
            let _ = writeln!(md, "- {r}");
        }
        md.push('\n');
    }
    md.push_str("| endpoint | model | run | date | temp | mode | replies | errors | extra calls | valid call | correct choice | args | hallucinated | median ms | p95 ms | tok/s |\n|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|\n");
    for s in stats.values() {
        let date: String = s
            .started_at
            .as_deref()
            .map_or("—".into(), |d| d.chars().take(10).collect());
        let _ = write!(
            md,
            "| {} | {} | {} | {} | {} | {:?} ",
            cell(&s.endpoint),
            cell(&s.model),
            run_cell(s),
            date,
            s.temperature,
            s.mode
        );
        if s.grammar_unsupported {
            let _ = writeln!(md, "| grammar unsupported | | | | | | | | | |");
            continue;
        }
        let _ = writeln!(
            md,
            "| {} | {} | {} | {} | {} | {} | {} | {} | {} | {} |",
            s.replies,
            s.errors,
            s.extra_calls,
            opt_ci(s.valid_rate(), s.valid_ci()),
            opt_ci(s.choice_rate(), s.choice_ci()),
            opt_ci(s.args_rate(), s.args_ci()),
            s.hallucinated,
            s.median_ms().map_or("—".into(), |v| v.to_string()),
            s.p95_ms().map_or("—".into(), |v| v.to_string()),
            s.tokens_per_sec().map_or("—".into(), |v| format!("{v:.0}"))
        );
    }
    md.push_str("\n## Correct choice by category\n\n");
    for s in stats.values().filter(|s| !s.grammar_unsupported) {
        let cats: Vec<String> = s
            .by_category
            .iter()
            .map(|(c, (ok, n))| format!("{c:?} {ok}/{n}"))
            .collect();
        let _ = writeln!(md, "- {}: {}", label(s), cats.join(", "));
    }
    md.push_str("\n## Flaky scenarios\n\n");
    if stats.values().all(|s| s.flaky.is_empty()) {
        md.push_str("- none\n");
    }
    for s in stats.values().filter(|s| !s.flaky.is_empty()) {
        let _ = writeln!(md, "- {}: {}", label(s), s.flaky.join(", "));
    }
    md
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::check::Checks;
    use crate::reply::Reply;
    use crate::result::{ReplyLine, StatusLine};
    use serde_json::json;

    /// `n` replies for `endpoint`, the first `good` of them perfect calls,
    /// the rest hallucinated names, each `ms` long.
    fn lines(endpoint: &str, n: usize, good: usize, ms: u64) -> Vec<Line> {
        (0..n)
            .map(|i| {
                let ok = i < good;
                Line::Reply(ReplyLine {
                    run_id: Some(endpoint.to_string()),
                    endpoint: endpoint.into(),
                    model: "m".into(),
                    mode: Mode::Grammar,
                    scenario: format!("s{}", i % 10),
                    product: "pa".into(),
                    category: Category::Simple,
                    run: (i / 10) as u32 + 1,
                    http_status: Some(200),
                    error: None,
                    latency_ms: ms,
                    completion_tokens: Some(10),
                    raw: None,
                    reply: Reply::ToolCall {
                        name: if ok {
                            "fs.read".into()
                        } else {
                            "fs.nope".into()
                        },
                        arguments: json!({}),
                        extra_calls: 0,
                    },
                    checks: Checks {
                        parseable: true,
                        real_name: ok,
                        schema_valid: ok,
                        right_choice: ok,
                        args_ok: None,
                    },
                })
            })
            .collect()
    }

    fn cfg() -> VerdictConfig {
        VerdictConfig {
            baseline: "base".into(),
            challenger: "chal".into(),
        }
    }

    fn stats(parts: Vec<Vec<Line>>) -> BTreeMap<String, Stats> {
        aggregate(&parts.concat())
    }

    #[test]
    fn rates_and_latency() {
        let s = stats(vec![lines("base", 100, 99, 1000)]);
        let b = &s["base"];
        assert_eq!(b.valid_rate(), Some(0.99));
        assert_eq!(b.choice_rate(), Some(0.99));
        assert_eq!(b.median_ms(), Some(1000));
        assert_eq!(b.tokens_per_sec(), Some(10.0));
        assert_eq!(b.hallucinated, 1);
    }

    #[test]
    fn a_good_baseline_and_no_edge_means_dont_build() {
        let s = stats(vec![
            lines("base", 100, 99, 1000),
            lines("chal", 100, 100, 1000),
        ]);
        assert!(matches!(
            verdict(&s, &cfg(), &RunSelection::default()),
            Verdict::DontBuild(_)
        ));
    }

    #[test]
    fn a_weak_baseline_means_build() {
        let s = stats(vec![
            lines("base", 100, 95, 1000),
            lines("chal", 100, 95, 1000),
        ]);
        match verdict(&s, &cfg(), &RunSelection::default()) {
            Verdict::Build(reasons) => {
                assert!(reasons.iter().any(|r| r.contains("98")), "{reasons:?}")
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_challenger_edge_counts_only_at_comparable_speed() {
        // A baseline that clears both bars, a challenger 1.5 points better.
        let edge = stats(vec![
            lines("base", 1000, 985, 1000),
            lines("chal", 1000, 1000, 1100),
        ]);
        assert!(
            matches!(
                verdict(&edge, &cfg(), &RunSelection::default()),
                Verdict::DontBuild(_)
            ),
            "1.5-point edge is not enough"
        );
        let mut base = lines("base", 100, 98, 1000);
        // Make the baseline's choice rate 90 % while validity stays 98 %.
        for l in base.iter_mut().take(8) {
            if let Line::Reply(r) = l {
                r.checks.right_choice = false;
            }
        }
        let big = stats(vec![base.clone(), lines("chal", 100, 100, 1150)]);
        assert!(
            matches!(
                verdict(&big, &cfg(), &RunSelection::default()),
                Verdict::Build(_)
            ),
            "10-point choice edge within 20 %"
        );
        match verdict(&big, &cfg(), &RunSelection::default()) {
            Verdict::Build(r) => assert!(
                r.iter()
                    .any(|x| x.contains("on correct choice by +10.0 pts")
                        && !x.contains("on valid call")),
                "{r:?}"
            ),
            other => panic!("{other:?}"),
        }
        let slow = stats(vec![base, lines("chal", 100, 100, 1300)]);
        assert!(
            matches!(
                verdict(&slow, &cfg(), &RunSelection::default()),
                Verdict::DontBuild(_)
            ),
            "edge but 30 % slower"
        );
    }

    fn status(endpoint: &str) -> Line {
        Line::EndpointStatus(StatusLine {
            run_id: Some(endpoint.to_string()),
            endpoint: endpoint.into(),
            model: "m".into(),
            mode: Mode::Grammar,
            status: "grammar_unsupported".into(),
            detail: "400".into(),
        })
    }

    fn one(reply: Reply, checks: Checks, error: Option<&str>) -> Vec<Line> {
        let mut l = lines("e", 1, 1, 1);
        if let Line::Reply(r) = &mut l[0] {
            r.reply = reply;
            r.checks = checks;
            r.error = error.map(Into::into);
        }
        l
    }

    fn no_checks() -> Checks {
        Checks {
            parseable: false,
            real_name: false,
            schema_valid: false,
            right_choice: false,
            args_ok: None,
        }
    }

    #[test]
    fn counting_rules() {
        let text = || Reply::Text { text: "hi".into() };
        let unp = || Reply::Unparseable { reason: "x".into() };
        // Respond scenario answered correctly with Text: excluded.
        let mut c = no_checks();
        c.right_choice = true;
        let s = aggregate(&one(text(), c, None));
        assert_eq!((s["e"].valid_den, s["e"].choice_num), (0, 1));
        // Text where a call was expected: counts and fails.
        let s = aggregate(&one(text(), no_checks(), None));
        assert_eq!((s["e"].valid_den, s["e"].valid_num), (1, 0));
        // Parse failure: counts and fails.
        let s = aggregate(&one(unp(), no_checks(), None));
        assert_eq!((s["e"].valid_den, s["e"].valid_num), (1, 0));
        assert_eq!(s["e"].errors, 0);
        // Transport failure: excluded from valid_den, counted as error.
        let s = aggregate(&one(unp(), no_checks(), Some("timeout")));
        let e = &s["e"];
        assert_eq!(
            (e.valid_den, e.errors, e.replies, e.choice_num),
            (0, 1, 1, 0)
        );
        assert_eq!(e.choice_rate(), Some(0.0));
        assert!(render(&s, None).contains("| replies | errors |"));
    }

    #[test]
    fn missing_challenger_is_undecided_when_baseline_clears_bars() {
        let s = stats(vec![lines("base", 100, 99, 1000)]);
        match verdict(&s, &cfg(), &RunSelection::default()) {
            Verdict::Undecided(r) => {
                assert!(
                    r.contains("not measured") && r.contains("clears both bars"),
                    "{r}"
                )
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn unsupported_challenger_is_undecided_when_baseline_clears_bars() {
        let mut parts = lines("base", 100, 99, 1000);
        parts.push(status("chal"));
        let s = aggregate(&parts);
        match verdict(&s, &cfg(), &RunSelection::default()) {
            Verdict::Undecided(r) => {
                assert!(
                    r.contains("rejected the grammar") && r.contains("clears both bars"),
                    "{r}"
                )
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn weak_baseline_is_build_without_a_challenger() {
        let s = stats(vec![lines("base", 100, 90, 1000)]);
        assert!(matches!(
            verdict(&s, &cfg(), &RunSelection::default()),
            Verdict::Build(_)
        ));
        let mut parts = lines("base", 100, 90, 1000);
        parts.push(status("chal"));
        assert!(matches!(
            verdict(&aggregate(&parts), &cfg(), &RunSelection::default()),
            Verdict::Build(_)
        ));
    }

    #[test]
    fn missing_baseline_is_undecided() {
        let s = stats(vec![lines("chal", 10, 10, 1)]);
        assert!(matches!(
            verdict(&s, &cfg(), &RunSelection::default()),
            Verdict::Undecided(_)
        ));
    }

    fn all_errors(endpoint: &str, n: usize) -> Vec<Line> {
        let mut ls = lines(endpoint, n, 0, 30000);
        for l in ls.iter_mut() {
            if let Line::Reply(r) = l {
                r.error = Some("timeout".into());
                r.reply = Reply::Unparseable { reason: "t".into() };
                r.checks = no_checks();
                r.completion_tokens = None;
            }
        }
        ls
    }

    #[test]
    fn all_error_challenger_is_undecided() {
        let mut parts = lines("base", 100, 99, 1000);
        parts.extend(all_errors("chal", 5));
        match verdict(&aggregate(&parts), &cfg(), &RunSelection::default()) {
            Verdict::Undecided(r) => {
                assert!(
                    r.contains("no scorable replies") && r.contains("clears both bars"),
                    "{r}"
                )
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn zero_reply_unsupported_reason_differs() {
        let mut parts = lines("base", 100, 99, 1000);
        parts.push(status("chal"));
        match verdict(&aggregate(&parts), &cfg(), &RunSelection::default()) {
            Verdict::Undecided(r) => assert!(r.contains("rejected the grammar"), "{r}"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn timeouts_do_not_move_latency() {
        let mut ls = lines("e", 3, 3, 100);
        ls.extend(all_errors("e", 2));
        let s = aggregate(&ls);
        let e = &s["e"];
        assert_eq!((e.replies, e.errors), (5, 2));
        assert_eq!((e.median_ms(), e.p95_ms()), (Some(100), Some(100)));
        assert_eq!(e.tokens_per_sec(), Some(100.0));
    }

    #[test]
    fn render_says_none_when_nothing_is_flaky() {
        let s = aggregate(&lines("base", 10, 10, 5));
        assert!(render(&s, None).ends_with("## Flaky scenarios\n\n- none\n"));
    }

    #[test]
    fn flaky_when_args_ok_differs() {
        let mut ls = lines("base", 20, 20, 5);
        for (i, l) in ls.iter_mut().enumerate() {
            if let Line::Reply(r) = l {
                r.checks.args_ok = Some(i != 10);
            }
        }
        let s = aggregate(&ls);
        assert_eq!(s["base"].flaky, ["s0"]);
    }

    #[test]
    fn status_line_does_not_hide_real_replies() {
        let mut ls = vec![status("base")];
        ls.extend(lines("base", 10, 10, 1));
        let s = aggregate(&ls);
        assert!(!s["base"].grammar_unsupported);
        let only = aggregate(&[status("base")]);
        assert!(only["base"].grammar_unsupported);
    }

    #[test]
    fn flaky_scenarios_and_render() {
        let mut ls = lines("base", 20, 20, 5);
        if let Line::Reply(r) = &mut ls[10] {
            r.checks.right_choice = false;
        } // s0 run 2 differs
        let s = aggregate(&ls);
        assert_eq!(s["base"].flaky, ["s0"]);
        let md = render(&s, Some(&assess(&s, &cfg(), &RunSelection::default())));
        assert!(
            md.contains("| base | m |") && md.contains("s0") && md.contains("Verdict"),
            "{md}"
        );
    }
    /// Retag every line with `run_id`.
    fn tagged(mut ls: Vec<Line>, run_id: &str) -> Vec<Line> {
        for l in ls.iter_mut() {
            match l {
                Line::Reply(r) => r.run_id = Some(run_id.into()),
                Line::EndpointStatus(s) => s.run_id = Some(run_id.into()),
                _ => {}
            }
        }
        ls
    }

    fn header(run_id: &str, endpoint: &str, started_at: &str, temp: Option<f64>) -> Line {
        use crate::config::{Endpoint, GrammarField};
        use crate::result::{RunHeader, ScenarioInfo, ServerInfo};
        Line::Run(RunHeader {
            run_id: run_id.into(),
            started_at: started_at.into(),
            toolbench: crate::util::BuildInfo {
                version: "0".into(),
                commit: "c".into(),
            },
            endpoint: Endpoint {
                name: endpoint.into(),
                base_url: "http://x/v1".into(),
                model: "m".into(),
                mode: Mode::Grammar,
                grammar_field: GrammarField::JsonSchema,
                max_tokens: 64,
                extra_body: Default::default(),
                temperature: temp,
            },
            effective_temperature: temp,
            repeat: 5,
            timeout_s: 120,
            snapshots: BTreeMap::new(),
            scenarios: ScenarioInfo {
                count: 10,
                sha256: "a".into(),
            },
            server: ServerInfo { models: vec![] },
            rescored_by: None,
        })
    }

    fn run_end(run_id: &str, replies: usize) -> Line {
        Line::RunEnd(crate::result::RunEnd {
            run_id: run_id.into(),
            fingerprint: None,
            replies,
        })
    }

    /// A complete run: header, replies, run_end.
    fn run(run_id: &str, started_at: &str, temp: Option<f64>, ls: Vec<Line>) -> Vec<Line> {
        let endpoint = match &ls[0] {
            Line::Reply(r) => r.endpoint.clone(),
            _ => unreachable!(),
        };
        let n = ls.iter().filter(|l| matches!(l, Line::Reply(_))).count();
        let mut out = vec![header(run_id, &endpoint, started_at, temp)];
        out.extend(tagged(ls, run_id));
        out.push(run_end(run_id, n));
        out
    }

    /// `n_scen` scenarios × `runs` runs of perfect calls; `bad(scen, run)`
    /// makes a reply a hallucinated call, `wrong(scen, run)` a valid call to
    /// the wrong tool.
    fn grid(
        endpoint: &str,
        n_scen: usize,
        runs: usize,
        ms: u64,
        bad: &dyn Fn(usize, usize) -> bool,
        wrong: &dyn Fn(usize, usize) -> bool,
    ) -> Vec<Line> {
        let mut ls = lines(endpoint, n_scen * runs, n_scen * runs, ms);
        for (i, l) in ls.iter_mut().enumerate() {
            if let Line::Reply(r) = l {
                let (sc, rn) = (i % n_scen, i / n_scen);
                r.scenario = format!("s{sc}");
                r.run = rn as u32 + 1;
                if bad(sc, rn) {
                    r.checks.real_name = false;
                    r.checks.schema_valid = false;
                    r.checks.right_choice = false;
                } else if wrong(sc, rn) {
                    r.checks.right_choice = false;
                }
            }
        }
        ls
    }

    fn never(_: usize, _: usize) -> bool {
        false
    }

    #[test]
    fn two_runs_of_one_endpoint_are_two_stats() {
        let s = aggregate(
            &[
                tagged(lines("base", 100, 99, 1000), "r1"),
                tagged(lines("base", 100, 90, 1000), "r2"),
            ]
            .concat(),
        );
        assert_eq!(s.len(), 2);
        assert_eq!(s["r1"].valid_rate(), Some(0.99));
        assert_eq!(s["r2"].valid_rate(), Some(0.90));
        assert!(s["r1"].order < s["r2"].order);
    }

    #[test]
    fn a_header_sets_date_temperature_and_completeness() {
        let mut ls = run(
            "r1",
            "2026-10-19T08:00:00Z",
            Some(0.6),
            lines("a", 10, 10, 1),
        );
        ls.extend(run(
            "r2",
            "2026-10-19T09:00:00Z",
            None,
            lines("b", 10, 10, 1),
        ));
        // Killed mid-run: header but no run_end.
        ls.push(header("r3", "c", "2026-10-19T10:00:00Z", None));
        ls.extend(tagged(lines("c", 10, 10, 1), "r3"));
        ls.extend(lines("legacy", 10, 10, 1));
        if let Line::Reply(r) = &mut ls[1] {
            r.reply = Reply::ToolCall {
                name: "fs.read".into(),
                arguments: json!({}),
                extra_calls: 2,
            };
        }
        let s = aggregate(&ls);
        assert_eq!(s["r1"].started_at.as_deref(), Some("2026-10-19T08:00:00Z"));
        assert_eq!(s["r1"].temperature, Temperature::Value(0.6));
        assert_eq!(s["r2"].temperature, Temperature::ServerDefault);
        assert_eq!(s["legacy"].temperature, Temperature::Unknown);
        assert_eq!(s["legacy"].started_at, None);
        assert_eq!(
            (s["r1"].complete, s["r3"].complete, s["legacy"].complete),
            (Some(true), Some(false), None)
        );
        assert_eq!((s["r1"].extra_calls, s["r2"].extra_calls), (1, 0));
        assert_eq!(s["r1"].replies, 10);
    }

    #[test]
    fn choice_ci_brackets_a_half() {
        // Even scenarios always wrong, odd always right.
        let ls = grid("base", 10, 10, 1, &never, &|sc, _| sc % 2 == 0);
        let s = aggregate(&ls);
        assert_eq!(s["base"].choice_rate(), Some(0.5));
        let (lo, hi) = s["base"].choice_ci().unwrap();
        assert!(lo < 0.5 && 0.5 < hi, "{lo} {hi}");
        assert_eq!(s["base"].valid_ci(), Some((1.0, 1.0)));
        assert_eq!(s["base"].args_ci(), None);
    }

    #[test]
    fn several_baseline_runs_need_a_selection() {
        let mut ls = run(
            "r1",
            "2026-10-19T08:00:00Z",
            None,
            lines("base", 100, 99, 1000),
        );
        ls.extend(run(
            "r2",
            "2026-10-19T09:00:00Z",
            None,
            lines("base", 100, 90, 1000),
        ));
        ls.extend(run(
            "c1",
            "2026-10-19T08:00:00Z",
            None,
            lines("chal", 100, 100, 1000),
        ));
        let s = aggregate(&ls);
        match verdict(&s, &cfg(), &RunSelection::default()) {
            Verdict::Undecided(r) => {
                assert!(
                    r.contains("2 runs of `base`") && r.contains("--latest"),
                    "{r}"
                )
            }
            other => panic!("{other:?}"),
        }
        let latest = RunSelection {
            latest: true,
            ..Default::default()
        };
        assert!(matches!(verdict(&s, &cfg(), &latest), Verdict::Build(_)));
        let pick = RunSelection {
            picks: BTreeMap::from([("base".to_string(), "r1".to_string())]),
            latest: false,
        };
        assert!(matches!(verdict(&s, &cfg(), &pick), Verdict::DontBuild(_)));
        // A pick that matches nothing leaves no baseline.
        let none = RunSelection {
            picks: BTreeMap::from([("base".to_string(), "zz".to_string())]),
            latest: false,
        };
        match verdict(&s, &cfg(), &none) {
            Verdict::Undecided(r) => {
                assert_eq!(r, "no run `zz` of `base` (runs: r1, r2)")
            }
            other => panic!("{other:?}"),
        }
        // A challenger pick that matches nothing, once the challenger decides.
        let chal_miss = RunSelection {
            picks: BTreeMap::from([
                ("base".to_string(), "r1".to_string()),
                ("chal".to_string(), "zz".to_string()),
            ]),
            latest: false,
        };
        match verdict(&s, &cfg(), &chal_miss) {
            Verdict::Undecided(r) => assert_eq!(r, "no run `zz` of `chal` (runs: c1)"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn the_run_cell_shows_the_full_run_id() {
        let ls = [
            run(
                "20261019T081502Z-abcd",
                "2026-10-19T08:15:02Z",
                None,
                lines("a", 10, 10, 1),
            ),
            tagged(lines("b", 10, 10, 1), "legacy:mistralrs-grammar-think"),
        ]
        .concat();
        let md = render(&aggregate(&ls), None);
        assert!(
            md.contains("| 20261019T081502Z-abcd | 2026-10-19 |"),
            "{md}"
        );
        assert!(
            md.contains("| legacy:mistralrs-grammar-think | — |"),
            "{md}"
        );
        assert!(md.contains("(legacy:mistralrs-grammar-think)**"), "{md}");
    }

    #[test]
    fn the_printed_edge_point_lies_in_its_bracket() {
        // The challenger also answered a scenario the baseline never saw:
        // the overall difference differs from the shared-scenario one.
        let base = grid("base", 40, 10, 1000, &never, &|_, rn| rn == 0);
        let mut chal = grid("chal", 40, 10, 1000, &never, &never);
        // 20 extra replies, half wrong: overall choice +7.6, shared +10.0.
        chal.extend(
            grid("chal", 1, 20, 1000, &never, &|_, rn| rn < 10)
                .into_iter()
                .map(|mut l| {
                    if let Line::Reply(r) = &mut l {
                        r.scenario = "extra".into();
                    }
                    l
                }),
        );
        let a = assess(
            &aggregate(&[base, chal].concat()),
            &cfg(),
            &RunSelection::default(),
        );
        match &a.verdict {
            Verdict::Build(r) => assert!(
                r.iter()
                    .any(|x| x.contains(
                        "on correct choice by +7.6 pts overall; paired over 40 shared scenarios +10.0 pts [10.0–10.0] (clearly)"
                    )),
                "{r:?}"
            ),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn legacy_runs_sort_by_file_order_for_latest() {
        let ls = [
            tagged(lines("base", 100, 90, 1000), "legacy:old"),
            tagged(lines("base", 100, 99, 1000), "legacy:new"),
            lines("chal", 100, 100, 1000),
        ]
        .concat();
        let s = aggregate(&ls);
        let latest = RunSelection {
            latest: true,
            ..Default::default()
        };
        assert!(matches!(
            verdict(&s, &cfg(), &latest),
            Verdict::DontBuild(_)
        ));
    }

    #[test]
    fn several_challenger_runs_matter_only_when_it_is_consulted() {
        let mut ls = lines("base", 100, 99, 1000);
        ls.extend(tagged(lines("chal", 100, 100, 1000), "c1"));
        ls.extend(tagged(lines("chal", 100, 100, 1000), "c2"));
        match verdict(&aggregate(&ls), &cfg(), &RunSelection::default()) {
            Verdict::Undecided(r) => assert!(r.contains("2 runs of `chal`"), "{r}"),
            other => panic!("{other:?}"),
        }
        // A weak baseline is Build either way; the ambiguity is a note.
        let mut ls = lines("base", 100, 90, 1000);
        ls.extend(tagged(lines("chal", 100, 100, 1000), "c1"));
        ls.extend(tagged(lines("chal", 100, 100, 1000), "c2"));
        let a = assess(&aggregate(&ls), &cfg(), &RunSelection::default());
        assert!(matches!(a.verdict, Verdict::Build(_)));
        assert!(
            a.notes.iter().any(|n| n.contains("2 runs of `chal`")),
            "{a:?}"
        );
    }

    #[test]
    fn a_bar_miss_says_whether_it_is_within_noise() {
        // 20 scenarios × 5 runs, one scenario always fails: 95/100.
        let ls = grid("base", 20, 5, 1000, &|sc, _| sc == 0, &never);
        let a = assess(&aggregate(&ls), &cfg(), &RunSelection::default());
        match &a.verdict {
            Verdict::Build(r) => assert!(
                r.iter()
                    .any(|x| x.contains("below 98.0 %") && x.contains("within noise")),
                "{r:?}"
            ),
            other => panic!("{other:?}"),
        }
        assert!(a.noisy);
        let a = assess(
            &aggregate(&lines("base", 100, 50, 1000)),
            &cfg(),
            &RunSelection::default(),
        );
        match &a.verdict {
            Verdict::Build(r) => assert!(
                r.iter()
                    .all(|x| x.contains("clearly") && !x.contains("within noise")),
                "{r:?}"
            ),
            other => panic!("{other:?}"),
        }
        assert!(!a.noisy);
    }

    #[test]
    fn a_challenger_edge_says_whether_it_is_within_noise() {
        // Baseline wrong on one run in ten of every scenario: 90 % choice,
        // the same in every scenario, so the paired CI is tight at +10.
        let base = grid("base", 40, 10, 1000, &never, &|_, rn| rn == 0);
        let chal = grid("chal", 40, 10, 1000, &never, &never);
        let a = assess(
            &aggregate(&[base, chal.clone()].concat()),
            &cfg(),
            &RunSelection::default(),
        );
        match &a.verdict {
            Verdict::Build(r) => assert!(
                r.iter()
                    .any(|x| x.contains(
                    "on correct choice by +10.0 pts [10.0–10.0] (clearly), at comparable median speed"
                )),
                "{r:?}"
            ),
            other => panic!("{other:?}"),
        }
        assert!(!a.noisy, "{a:?}");
        // Baseline always wrong in 4 of 40 scenarios: also +10, but the CI
        // reaches below 5 points.
        let base = grid("base", 40, 10, 1000, &never, &|sc, _| sc < 4);
        let a = assess(
            &aggregate(&[base, chal].concat()),
            &cfg(),
            &RunSelection::default(),
        );
        match &a.verdict {
            Verdict::Build(r) => assert!(
                r.iter()
                    .any(|x| x.contains("on correct choice by +10.0 pts [")
                        && x.contains("within noise")),
                "{r:?}"
            ),
            other => panic!("{other:?}"),
        }
        assert!(a.noisy);
    }

    #[test]
    fn no_edge_says_whether_one_is_excluded() {
        let s = stats(vec![
            lines("base", 100, 100, 1000),
            lines("chal", 100, 100, 1000),
        ]);
        let a = assess(&s, &cfg(), &RunSelection::default());
        match &a.verdict {
            Verdict::DontBuild(r) => assert!(
                r.iter().any(|x| x.contains("has no 5-point edge")
                    && x.contains("+0.0 pts [0.0–0.0]")
                    && x.contains("clearly")),
                "{r:?}"
            ),
            other => panic!("{other:?}"),
        }
        assert!(!a.noisy, "{a:?}");
    }

    #[test]
    fn different_temperatures_add_a_note() {
        let mut ls = run(
            "b1",
            "2026-10-19T08:00:00Z",
            Some(0.6),
            lines("base", 100, 99, 1000),
        );
        ls.extend(run(
            "c1",
            "2026-10-19T08:00:00Z",
            None,
            lines("chal", 100, 100, 1000),
        ));
        let a = assess(&aggregate(&ls), &cfg(), &RunSelection::default());
        assert!(
            a.notes
                .iter()
                .any(|n| n.contains("temperatures differ or are unknown (0.6 vs default)")),
            "{a:?}"
        );
        let mut ls = run(
            "b1",
            "2026-10-19T08:00:00Z",
            Some(0.6),
            lines("base", 100, 99, 1000),
        );
        ls.extend(run(
            "c1",
            "2026-10-19T08:00:00Z",
            Some(0.6),
            lines("chal", 100, 100, 1000),
        ));
        let a = assess(&aggregate(&ls), &cfg(), &RunSelection::default());
        assert!(a.notes.is_empty(), "{a:?}");
        // Legacy runs are unknown.
        let s = stats(vec![
            lines("base", 100, 99, 1000),
            lines("chal", 100, 100, 1000),
        ]);
        let a = assess(&s, &cfg(), &RunSelection::default());
        assert!(a.notes.iter().any(|n| n.contains("(? vs ?)")), "{a:?}");
    }

    #[test]
    fn an_incomplete_run_is_flagged() {
        let mut ls = vec![header("b1", "base", "2026-10-19T08:00:00Z", None)];
        ls.extend(tagged(lines("base", 100, 99, 1000), "b1"));
        ls.extend(run(
            "c1",
            "2026-10-19T08:00:00Z",
            None,
            lines("chal", 100, 100, 1000),
        ));
        let s = aggregate(&ls);
        let a = assess(&s, &cfg(), &RunSelection::default());
        assert!(matches!(a.verdict, Verdict::DontBuild(_)));
        assert_eq!(
            a.notes,
            ["run `b1` of `base` is incomplete (no run_end line)"],
            "{a:?}"
        );
        let md = render(&s, Some(&a));
        assert!(md.contains("| b1 (incomplete) |"), "{md}");
        assert!(md.contains("| c1 |"), "{md}");
        assert!(md.contains("- run `b1` of `base` is incomplete"), "{md}");
    }

    #[test]
    fn a_run_with_more_replies_than_its_end_line_is_flagged() {
        let mut ls = run(
            "b1",
            "2026-10-19T08:00:00Z",
            None,
            lines("base", 100, 99, 1000),
        );
        // The same run concatenated twice into one file.
        ls.extend(ls.clone());
        ls.extend(run(
            "c1",
            "2026-10-19T08:00:00Z",
            None,
            lines("chal", 100, 100, 1000),
        ));
        let s = aggregate(&ls);
        assert_eq!(s["b1"].replies, 200);
        assert!(s["b1"].duplicated() && !s["c1"].duplicated());
        let a = assess(&s, &cfg(), &RunSelection::default());
        assert_eq!(
            a.notes,
            ["run `b1` of `base` has 200 replies but its run_end says 100 (duplicated input?)"],
            "{a:?}"
        );
        let md = render(&s, Some(&a));
        assert!(md.contains("| b1 (duplicated?) |"), "{md}");
        assert!(md.contains("| c1 |"), "{md}");
    }

    #[test]
    fn a_build_reason_with_both_edges_has_no_dangling_separator() {
        // 90 % valid and 90 % choice against a perfect challenger: both fire.
        let s = stats(vec![
            lines("base", 100, 90, 1000),
            lines("chal", 100, 100, 1000),
        ]);
        let Verdict::Build(r) = verdict(&s, &cfg(), &RunSelection::default()) else {
            panic!()
        };
        let reason = r.iter().find(|x| x.contains("chal beats base")).unwrap();
        assert!(
            reason.contains("on valid call") && reason.contains("and on correct choice"),
            "{reason}"
        );
        assert!(reason.ends_with("at comparable median speed"), "{reason:?}");
    }

    #[test]
    fn render_escapes_pipes_and_prints_cis_and_extra_calls() {
        let s = aggregate(&lines("a|b", 100, 50, 5));
        let md = render(&s, None);
        assert!(
            md.contains("| a\\|b | m | a\\|b | — | ? | Grammar |"),
            "{md}"
        );
        assert!(md.contains("| extra calls |"), "{md}");
        assert!(md.contains("50.0 % [50.0–50.0]"), "{md}");
        assert!(md.contains("**a\\|b / m (a\\|b)**"), "{md}");
    }

    #[test]
    fn render_adds_the_noise_clause() {
        let ls = grid("base", 20, 5, 1000, &|sc, _| sc == 0, &never);
        let s = aggregate(&ls);
        let a = assess(&s, &cfg(), &RunSelection::default());
        assert!(a.noisy);
        let md = render(&s, Some(&a));
        assert!(
            md.contains(
                "**Verdict: build aivyx-engine.** (at least one deciding condition is within noise)"
            ),
            "{md}"
        );
        let quiet = Assessment { noisy: false, ..a };
        assert!(!render(&s, Some(&quiet)).contains("within noise)"));
    }
}
