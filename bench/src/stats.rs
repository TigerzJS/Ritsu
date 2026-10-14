//! Robust summary statistics: median and median absolute deviation (MAD).
//!
//! Wall-clock samples on a shared machine have a long right tail (preemption, frequency
//! changes). Mean and standard deviation follow the outliers; median and MAD do not.

/// Median of `values` (mean of the two middle values for an even count). `NaN` for no values.
#[must_use]
pub fn median(values: &[f64]) -> f64 {
    if values.is_empty() {
        return f64::NAN;
    }
    let mut v = values.to_vec();
    v.sort_by(f64::total_cmp);
    let n = v.len();
    if n % 2 == 1 {
        v[n / 2]
    } else {
        (v[n / 2 - 1] + v[n / 2]) / 2.0
    }
}

/// Median absolute deviation from `center` (normally the median). Raw, not scaled by 1.4826.
#[must_use]
pub fn mad(values: &[f64], center: f64) -> f64 {
    let dev: Vec<f64> = values.iter().map(|x| (x - center).abs()).collect();
    median(&dev)
}

/// MAD scaled so that it estimates the standard deviation of normally distributed data.
pub const MAD_TO_SIGMA: f64 = 1.482_602_218_505_602;

/// Summary of one series of samples.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Summary {
    /// Median.
    pub median: f64,
    /// Raw MAD around the median.
    pub mad: f64,
    /// Smallest sample.
    pub min: f64,
    /// Largest sample.
    pub max: f64,
}

impl Summary {
    /// Summarises `values`.
    #[must_use]
    pub fn of(values: &[f64]) -> Self {
        let m = median(values);
        Self {
            median: m,
            mad: mad(values, m),
            min: values.iter().copied().fold(f64::INFINITY, f64::min),
            max: values.iter().copied().fold(f64::NEG_INFINITY, f64::max),
        }
    }

    /// Spread as a percentage of the median: `100 * 1.4826 * MAD / median`.
    #[must_use]
    pub fn spread_pct(&self) -> f64 {
        if self.median == 0.0 {
            0.0
        } else {
            100.0 * MAD_TO_SIGMA * self.mad / self.median.abs()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn median_of_odd_and_even_counts() {
        assert_eq!(median(&[3.0, 1.0, 2.0]), 2.0);
        assert_eq!(median(&[4.0, 1.0, 3.0, 2.0]), 2.5);
        assert!(median(&[]).is_nan());
    }

    #[test]
    fn mad_ignores_a_single_outlier() {
        // Hand computed: median 10, deviations 0 0 1 1 90 -> median 1.
        let v = [10.0, 10.0, 9.0, 11.0, 100.0];
        let s = Summary::of(&v);
        assert_eq!(s.median, 10.0);
        assert_eq!(s.mad, 1.0);
        assert_eq!(s.min, 9.0);
        assert_eq!(s.max, 100.0);
        // 100 * 1.4826 * 1 / 10
        assert!((s.spread_pct() - 14.826_022_185_056_02).abs() < 1e-9);
    }

    #[test]
    fn constant_series_has_zero_spread() {
        let s = Summary::of(&[5.0; 7]);
        assert_eq!((s.median, s.mad, s.spread_pct()), (5.0, 0.0, 0.0));
    }
}
