//! Aggregation, the pre-agreed verdict, and the Markdown report.

use std::collections::BTreeMap;
use std::fmt::Write;

use crate::config::{Mode, VerdictConfig};
use crate::reply::Reply;
use crate::result::Line;
use crate::scenario::Category;

pub const MIN_VALID: f64 = 0.98;
pub const MIN_CHOICE: f64 = 0.90;
pub const EDGE: f64 = 0.05;
pub const LATENCY_SLACK: f64 = 1.20;

#[derive(Debug, Clone, PartialEq)]
pub struct Stats {
    pub endpoint: String,
    pub model: String,
    pub mode: Mode,
    pub replies: usize,
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
    pub flaky: Vec<String>,
}

fn rate(num: usize, den: usize) -> Option<f64> {
    (den > 0).then(|| num as f64 / den as f64)
}

impl Stats {
    fn new(endpoint: &str, model: &str, mode: Mode) -> Self {
        Self {
            endpoint: endpoint.into(),
            model: model.into(),
            mode,
            replies: 0,
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
            flaky: Vec::new(),
        }
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

/// Keyed `"<endpoint>/<model>"`. The valid-call denominator is every reply
/// that made a call, should have made one, or could not be parsed.
pub fn aggregate(lines: &[Line]) -> BTreeMap<String, Stats> {
    let mut out: BTreeMap<String, Stats> = BTreeMap::new();
    let mut choices: BTreeMap<(String, String), Vec<bool>> = BTreeMap::new();
    for line in lines {
        match line {
            Line::EndpointStatus(s) => {
                let st = out
                    .entry(format!("{}/{}", s.endpoint, s.model))
                    .or_insert_with(|| Stats::new(&s.endpoint, &s.model, s.mode));
                st.grammar_unsupported |= s.status == "grammar_unsupported";
            }
            Line::Reply(r) => {
                let key = format!("{}/{}", r.endpoint, r.model);
                let st = out
                    .entry(key.clone())
                    .or_insert_with(|| Stats::new(&r.endpoint, &r.model, r.mode));
                st.replies += 1;
                // Made a call, could not be parsed, or answered in text
                // when a call was expected (a wrong-choice Text).
                let counts = match r.reply {
                    Reply::ToolCall { .. } | Reply::Unparseable { .. } => true,
                    Reply::Text { .. } => !r.checks.right_choice,
                };
                if counts {
                    st.valid_den += 1;
                    st.valid_num += usize::from(r.checks.valid_call());
                }
                st.choice_num += usize::from(r.checks.right_choice);
                if let Some(ok) = r.checks.args_ok {
                    st.args_den += 1;
                    st.args_num += usize::from(ok);
                }
                st.hallucinated += usize::from(
                    r.checks.parseable
                        && matches!(r.reply, Reply::ToolCall { .. })
                        && !r.checks.real_name,
                );
                st.latencies_ms.push(r.latency_ms);
                if let Some(t) = r.completion_tokens {
                    st.tokens += t;
                    st.token_ms += r.latency_ms;
                }
                let cat = st.by_category.entry(r.category).or_default();
                cat.0 += usize::from(r.checks.right_choice);
                cat.1 += 1;
                choices
                    .entry((key, r.scenario.clone()))
                    .or_default()
                    .push(r.checks.right_choice);
            }
        }
    }
    for ((key, scenario), runs) in choices {
        if runs.iter().any(|c| *c)
            && runs.iter().any(|c| !*c)
            && let Some(st) = out.get_mut(&key)
        {
            st.flaky.push(scenario);
        }
    }
    out
}

#[derive(Debug, Clone, PartialEq)]
pub enum Verdict {
    Build(Vec<String>),
    DontBuild(Vec<String>),
    Undecided(String),
}

fn pct(x: f64) -> String {
    format!("{:.1} %", x * 100.0)
}

pub fn verdict(stats: &BTreeMap<String, Stats>, cfg: &VerdictConfig) -> Verdict {
    // Each model has its own endpoint name, so endpoint names identify runs.
    let get = |ep: &str| stats.values().find(|s| s.endpoint == ep);
    let Some(base) = get(&cfg.baseline).filter(|b| !b.grammar_unsupported && b.replies > 0) else {
        return Verdict::Undecided(format!("no usable results for baseline `{}`", cfg.baseline));
    };
    let (Some(bv), Some(bc)) = (base.valid_rate(), base.choice_rate()) else {
        return Verdict::Undecided("baseline has no rates".into());
    };
    let mut build = Vec::new();
    let mut keep = Vec::new();
    if bv < MIN_VALID {
        build.push(format!(
            "{} valid-call rate {} is below {}",
            cfg.baseline,
            pct(bv),
            pct(MIN_VALID)
        ));
    }
    if bc < MIN_CHOICE {
        build.push(format!(
            "{} correct-choice rate {} is below {}",
            cfg.baseline,
            pct(bc),
            pct(MIN_CHOICE)
        ));
    }
    if build.is_empty() {
        keep.push(format!(
            "{} clears both bars ({} valid, {} correct choice)",
            cfg.baseline,
            pct(bv),
            pct(bc)
        ));
    }
    match get(&cfg.challenger) {
        Some(ch) if ch.grammar_unsupported => keep.push(format!(
            "{} rejected the grammar request; no engine comparison",
            cfg.challenger
        )),
        Some(ch) if ch.replies > 0 => {
            let (cv, cc) = (
                ch.valid_rate().unwrap_or(0.0),
                ch.choice_rate().unwrap_or(0.0),
            );
            let edge = cv - bv >= EDGE - 1e-9 || cc - bc >= EDGE - 1e-9;
            let comparable = match (ch.median_ms(), base.median_ms()) {
                (Some(c), Some(b)) => c as f64 <= b as f64 * LATENCY_SLACK,
                _ => false,
            };
            if edge && comparable {
                build.push(format!(
                    "{} beats {} by ≥ 5 points ({} / {}) at comparable speed",
                    cfg.challenger,
                    cfg.baseline,
                    pct(cv),
                    pct(cc)
                ));
            } else if edge {
                keep.push(format!(
                    "{} is better ({} / {}) but more than 20 % slower",
                    cfg.challenger,
                    pct(cv),
                    pct(cc)
                ));
            } else {
                keep.push(format!(
                    "{} has no 5-point edge ({} / {})",
                    cfg.challenger,
                    pct(cv),
                    pct(cc)
                ));
            }
        }
        _ => keep.push(format!("no results for challenger `{}`", cfg.challenger)),
    }
    if build.is_empty() {
        Verdict::DontBuild(keep)
    } else {
        Verdict::Build(build)
    }
}

fn opt_pct(x: Option<f64>) -> String {
    x.map(pct).unwrap_or_else(|| "—".into())
}

pub fn render(stats: &BTreeMap<String, Stats>, verdict: Option<&Verdict>) -> String {
    let mut md = String::from("# toolbench report\n\n");
    if let Some(v) = verdict {
        let (head, reasons) = match v {
            Verdict::Build(r) => ("**Verdict: build aivyx-engine.**", r.clone()),
            Verdict::DontBuild(r) => (
                "**Verdict: don't build aivyx-engine for tool calls.**",
                r.clone(),
            ),
            Verdict::Undecided(r) => ("**Verdict: undecided.**", vec![r.clone()]),
        };
        let _ = writeln!(md, "{head}\n");
        for r in reasons {
            let _ = writeln!(md, "- {r}");
        }
        md.push('\n');
    }
    md.push_str("| endpoint | model | mode | replies | valid call | correct choice | args | hallucinated | median ms | p95 ms | tok/s |\n|---|---|---|---|---|---|---|---|---|---|---|\n");
    for s in stats.values() {
        if s.grammar_unsupported {
            let _ = writeln!(
                md,
                "| {} | {} | {:?} | grammar unsupported | | | | | | | |",
                s.endpoint, s.model, s.mode
            );
            continue;
        }
        let _ = writeln!(
            md,
            "| {} | {} | {:?} | {} | {} | {} | {} | {} | {} | {} | {} |",
            s.endpoint,
            s.model,
            s.mode,
            s.replies,
            opt_pct(s.valid_rate()),
            opt_pct(s.choice_rate()),
            opt_pct(s.args_rate()),
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
        let _ = writeln!(
            md,
            "- **{} / {}**: {}",
            s.endpoint,
            s.model,
            cats.join(", ")
        );
    }
    md.push_str("\n## Flaky scenarios\n\n");
    for s in stats.values().filter(|s| !s.flaky.is_empty()) {
        let _ = writeln!(
            md,
            "- **{} / {}**: {}",
            s.endpoint,
            s.model,
            s.flaky.join(", ")
        );
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
        let b = &s["base/m"];
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
        assert!(matches!(verdict(&s, &cfg()), Verdict::DontBuild(_)));
    }

    #[test]
    fn a_weak_baseline_means_build() {
        let s = stats(vec![
            lines("base", 100, 95, 1000),
            lines("chal", 100, 95, 1000),
        ]);
        match verdict(&s, &cfg()) {
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
            matches!(verdict(&edge, &cfg()), Verdict::DontBuild(_)),
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
            matches!(verdict(&big, &cfg()), Verdict::Build(_)),
            "10-point choice edge within 20 %"
        );
        let slow = stats(vec![base, lines("chal", 100, 100, 1300)]);
        assert!(
            matches!(verdict(&slow, &cfg()), Verdict::DontBuild(_)),
            "edge but 30 % slower"
        );
    }

    #[test]
    fn missing_or_unsupported_endpoints_are_undecided_or_noted() {
        let s = stats(vec![lines("chal", 10, 10, 1)]);
        assert!(matches!(verdict(&s, &cfg()), Verdict::Undecided(_)));
        let mut parts = lines("base", 100, 99, 1000);
        parts.push(Line::EndpointStatus(StatusLine {
            endpoint: "chal".into(),
            model: "m".into(),
            mode: Mode::Grammar,
            status: "grammar_unsupported".into(),
            detail: "400".into(),
        }));
        let s = aggregate(&parts);
        match verdict(&s, &cfg()) {
            Verdict::DontBuild(reasons) => {
                assert!(reasons.iter().any(|r| r.contains("grammar")), "{reasons:?}")
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn flaky_scenarios_and_render() {
        let mut ls = lines("base", 20, 20, 5);
        if let Line::Reply(r) = &mut ls[10] {
            r.checks.right_choice = false;
        } // s0 run 2 differs
        let s = aggregate(&ls);
        assert_eq!(s["base/m"].flaky, ["s0"]);
        let md = render(&s, Some(&verdict(&s, &cfg())));
        assert!(
            md.contains("| base | m |") && md.contains("s0") && md.contains("Verdict"),
            "{md}"
        );
    }
}
