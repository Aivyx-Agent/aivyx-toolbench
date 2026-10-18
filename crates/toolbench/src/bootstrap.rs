//! Cluster bootstrap over scenarios: every scenario's runs are resampled
//! together, because the 5 runs of one scenario are not independent.

use std::collections::BTreeMap;

use crate::util::SplitMix64;

pub const RESAMPLES: usize = 2000;
pub const SEED: u64 = 0x5eed;

/// One scenario's `(num, den)` in run a and in run b.
type Pair = ((usize, usize), (usize, usize));

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

fn diff(shared: &[Pair], idx: &[usize]) -> Option<f64> {
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
    let shared: Vec<Pair> = a
        .iter()
        .filter_map(|(k, ca)| b.get(k).map(|cb| (*ca, *cb)))
        .collect();
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
        let counts: Vec<(usize, usize)> = (0..60)
            .map(|i| (if i % 2 == 0 { 5 } else { 0 }, 5))
            .collect();
        let a = ci(&counts).unwrap();
        assert_eq!(a, ci(&counts).unwrap(), "same seed, same CI");
        assert!(a.0 < 0.5 && 0.5 < a.1, "{a:?}");
        // Roughly ±13 points for p=0.5, n_eff=60.
        assert!(a.0 > 0.30 && a.1 < 0.70, "{a:?}");
    }

    #[test]
    fn paired_difference_is_signed_b_minus_a() {
        let a: BTreeMap<String, (usize, usize)> =
            (0..40).map(|i| (format!("s{i}"), (2, 5))).collect();
        let b: BTreeMap<String, (usize, usize)> =
            (0..40).map(|i| (format!("s{i}"), (4, 5))).collect();
        let (p, lo, hi) = paired_ci(&a, &b).unwrap();
        assert!((p - 0.4).abs() < 1e-9);
        assert!(lo <= p && p <= hi);
        assert!(paired_ci(&b, &a).unwrap().0 < 0.0);
        // No shared scenarios.
        let c: BTreeMap<String, (usize, usize)> = BTreeMap::from([("x".into(), (1, 1))]);
        assert_eq!(paired_ci(&a, &c), None);
    }
}
