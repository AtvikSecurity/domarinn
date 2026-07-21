//! Statistical treatment of eval results.
//!
//! Single-run pass rates are point estimates; these functions add the rigor most
//! eval tools skip: a Wilson confidence interval on a pass rate, a McNemar paired
//! test for whether two runs over the same cases differ significantly, and a
//! pass@k estimator for repeated trials.

/// z for a 95% two-sided interval.
pub const Z_95: f64 = 1.959_963_984_540_054;

/// A pass rate with a Wilson score confidence interval.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PassRate {
    pub passed: u64,
    pub total: u64,
    pub rate: f64,
    pub lower: f64,
    pub upper: f64,
}

/// Wilson score interval for a binomial proportion.
///
/// More accurate than the normal approximation for small n and rates near 0/1,
/// and it never leaves [0, 1].
pub fn wilson(passed: u64, total: u64, z: f64) -> PassRate {
    if total == 0 {
        return PassRate {
            passed,
            total,
            rate: 0.0,
            lower: 0.0,
            upper: 0.0,
        };
    }
    let n = total as f64;
    let phat = passed as f64 / n;
    let z2 = z * z;
    let denom = 1.0 + z2 / n;
    let center = phat + z2 / (2.0 * n);
    let margin = z * ((phat * (1.0 - phat) + z2 / (4.0 * n)) / n).sqrt();
    PassRate {
        passed,
        total,
        rate: phat,
        lower: ((center - margin) / denom).clamp(0.0, 1.0),
        upper: ((center + margin) / denom).clamp(0.0, 1.0),
    }
}

/// The result of a McNemar paired test between a baseline and a head run.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct McNemar {
    /// Cases that passed in base but fail in head (regressions).
    pub b: u64,
    /// Cases that failed in base but pass in head (fixes).
    pub c: u64,
    /// Continuity-corrected chi-square statistic.
    pub statistic: f64,
    /// True at the 95% level (statistic > 3.841, 1 df).
    pub significant: bool,
}

/// McNemar's test with continuity correction. `b` and `c` are the discordant
/// pair counts (see [`McNemar`]).
pub fn mcnemar(b: u64, c: u64) -> McNemar {
    let bc = (b + c) as f64;
    let statistic = if bc == 0.0 {
        0.0
    } else {
        let diff = (b as f64 - c as f64).abs() - 1.0;
        (diff.max(0.0)).powi(2) / bc
    };
    McNemar {
        b,
        c,
        statistic,
        significant: statistic > 3.841,
    }
}

/// The unbiased pass@k estimator (Codex/HumanEval): given `n` trials of which
/// `passed` succeeded, the probability at least one of `k` sampled trials passes.
pub fn pass_at_k(n: u64, passed: u64, k: u64) -> f64 {
    if k >= n {
        return if passed > 0 { 1.0 } else { 0.0 };
    }
    let fails = n - passed;
    if fails < k {
        return 1.0;
    }
    // 1 - C(n-passed, k) / C(n, k), computed as a product to avoid overflow.
    let mut prob_all_fail = 1.0;
    for i in 0..k {
        prob_all_fail *= (fails - i) as f64 / (n - i) as f64;
    }
    1.0 - prob_all_fail
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wilson_all_pass_is_near_one_but_bounded() {
        let r = wilson(10, 10, Z_95);
        assert_eq!(r.rate, 1.0);
        assert!(r.upper <= 1.0);
        assert!(r.lower < 1.0, "some downward uncertainty with n=10");
        assert!(r.lower > 0.6);
    }

    #[test]
    fn wilson_half_is_symmetric_ish() {
        let r = wilson(50, 100, Z_95);
        assert!((r.rate - 0.5).abs() < 1e-9);
        assert!(r.lower > 0.39 && r.lower < 0.41);
        assert!(r.upper > 0.59 && r.upper < 0.61);
    }

    #[test]
    fn wilson_zero_total() {
        let r = wilson(0, 0, Z_95);
        assert_eq!(r.rate, 0.0);
        assert_eq!(r.lower, 0.0);
        assert_eq!(r.upper, 0.0);
    }

    #[test]
    fn mcnemar_no_change_not_significant() {
        let m = mcnemar(0, 0);
        assert_eq!(m.statistic, 0.0);
        assert!(!m.significant);
    }

    #[test]
    fn mcnemar_large_imbalance_is_significant() {
        // 20 regressions, 2 fixes → clearly significant.
        let m = mcnemar(20, 2);
        assert!(m.significant, "statistic {}", m.statistic);
    }

    #[test]
    fn mcnemar_small_balanced_not_significant() {
        let m = mcnemar(3, 2);
        assert!(!m.significant);
    }

    #[test]
    fn pass_at_k_basics() {
        assert_eq!(pass_at_k(5, 0, 1), 0.0);
        assert_eq!(pass_at_k(5, 5, 1), 1.0);
        // 1 of 4 passed, k=1 → 0.25.
        assert!((pass_at_k(4, 1, 1) - 0.25).abs() < 1e-9);
        // 1 of 4 passed, k=4 → 1.0 (must sample the passing one).
        assert_eq!(pass_at_k(4, 1, 4), 1.0);
    }
}
