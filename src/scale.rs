//! Axis scaling, tick placement and number/time formatting.

/// Round a raw step up to 1, 2 or 5 times a power of ten.
pub fn nice_step(raw: f64) -> f64 {
    if !(raw.is_finite() && raw > 0.0) {
        return 1.0;
    }
    let mag = 10f64.powf(raw.log10().floor());
    let f = raw / mag;
    let n = if f <= 1.0 {
        1.0
    } else if f <= 2.0 {
        2.0
    } else if f <= 5.0 {
        5.0
    } else {
        10.0
    };
    n * mag
}

/// Pad a data range by 5% and snap it outward to nice values, so the scale only
/// changes when the data crosses a tick boundary.
pub fn auto_range(lo: f64, hi: f64) -> (f64, f64) {
    padded_range(lo, hi, 0.05)
}

/// Like [`auto_range`], with `pad` (a fraction of the span) added on each side.
pub fn padded_range(lo: f64, hi: f64, pad: f64) -> (f64, f64) {
    if !(lo.is_finite() && hi.is_finite()) {
        return (-1.0, 1.0);
    }
    let (mut lo, mut hi) = (lo.min(hi), lo.max(hi));
    let span = hi - lo;
    if span <= f64::EPSILON * 16.0 * lo.abs().max(1.0) {
        let d = if lo == 0.0 { 1.0 } else { lo.abs() * 0.1 };
        lo -= d;
        hi += d;
    } else {
        lo -= span * pad;
        hi += span * pad;
    }
    let step = nice_step((hi - lo) / 10.0);
    ((lo / step).floor() * step, (hi / step).ceil() * step)
}

/// Tick values within `[lo, hi]`, roughly `target` of them, and the step between them.
pub fn ticks(lo: f64, hi: f64, target: usize) -> (Vec<f64>, f64) {
    let step = nice_step((hi - lo) / target.max(1) as f64);
    let mut out = Vec::new();
    let mut k = (lo / step).ceil();
    while out.len() < 100 {
        let t = k * step;
        if t > hi + step * 1e-9 {
            break;
        }
        out.push(if t.abs() < step * 1e-9 { 0.0 } else { t });
        k += 1.0;
    }
    (out, step)
}

/// Format an axis value with just enough decimals for the tick step.
pub fn fmt_value(v: f64, step: f64) -> String {
    let a = v.abs();
    let v = if a <= step * 1e-6 { 0.0 } else { v };
    if a != 0.0 && !(1e-4..1e6).contains(&a) {
        return format!("{v:.1e}");
    }
    let dec = if step > 0.0 { (-step.log10().floor()).clamp(0.0, 8.0) as usize } else { 2 };
    format!("{v:.dec$}")
}

/// Format a reading with about four significant digits.
pub fn fmt_reading(v: f64) -> String {
    if !v.is_finite() {
        return "—".into();
    }
    let a = v.abs();
    if a != 0.0 && !(1e-3..1e6).contains(&a) {
        return format!("{v:.3e}");
    }
    let dec = if a == 0.0 { 0 } else { (3.0 - a.log10().floor()).clamp(0.0, 6.0) as usize };
    format!("{v:.dec$}")
}

/// Time-grid steps (seconds), chosen so lines fall on round clock values.
pub const TIME_STEPS: &[f64] = &[
    0.001, 0.002, 0.005, 0.01, 0.02, 0.05, 0.1, 0.2, 0.5, 1.0, 2.0, 5.0, 10.0, 15.0, 30.0, 60.0,
    120.0, 300.0, 600.0, 900.0, 1800.0, 3600.0, 7200.0, 10800.0, 21600.0, 43200.0, 86400.0,
];

/// Smallest round time step that is at least `min_step` seconds.
pub fn time_step(min_step: f64) -> f64 {
    TIME_STEPS
        .iter()
        .copied()
        .find(|&s| s >= min_step)
        .unwrap_or_else(|| nice_step(min_step / 86400.0) * 86400.0)
}

/// Format elapsed seconds as `m:ss`, `h:mm:ss`, with fractional digits when `step` < 1s.
pub fn fmt_elapsed(t: f64, step: f64) -> String {
    let dec = if step < 1.0 { (-step.log10()).ceil().clamp(0.0, 3.0) as u32 } else { 0 };
    let scale = 10u64.pow(dec);
    let units = (t.max(0.0) * scale as f64).round() as u64;
    let secs = units / scale;
    let (h, m, s) = (secs / 3600, secs / 60 % 60, secs % 60);
    let mut out = if h > 0 { format!("{h}:{m:02}:{s:02}") } else { format!("{m}:{s:02}") };
    if dec > 0 {
        out.push_str(&format!(".{:0width$}", units % scale, width = dec as usize));
    }
    out
}

/// Format a span of time compactly: `500ms`, `30s`, `5m`, `2h`.
pub fn fmt_span(secs: f64) -> String {
    let (v, unit) = if secs < 1.0 {
        (secs * 1000.0, "ms")
    } else if secs < 60.0 {
        (secs, "s")
    } else if secs < 3600.0 {
        (secs / 60.0, "m")
    } else if secs < 86400.0 {
        (secs / 3600.0, "h")
    } else {
        (secs / 86400.0, "d")
    };
    let n = format!("{v:.1}");
    format!("{}{unit}", n.trim_end_matches('0').trim_end_matches('.'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steps() {
        assert_eq!(nice_step(0.7), 1.0);
        assert_eq!(nice_step(1.5), 2.0);
        assert_eq!(nice_step(3.0), 5.0);
        assert_eq!(nice_step(7.0), 10.0);
        assert!((nice_step(0.03) - 0.05).abs() < 1e-12);
    }

    #[test]
    fn ranges() {
        assert_eq!(auto_range(0.0, 10.0), (-2.0, 12.0));
        let (lo, hi) = auto_range(5.0, 5.0);
        assert!(lo < 5.0 && hi > 5.0);
        assert_eq!(auto_range(f64::NAN, 1.0), (-1.0, 1.0));
    }

    #[test]
    fn tick_values() {
        let (t, step) = ticks(-1.0, 11.0, 4);
        assert_eq!(step, 5.0);
        assert_eq!(t, [0.0, 5.0, 10.0]);
        assert_eq!(fmt_value(0.30000000004, 0.1), "0.3");
        assert_eq!(fmt_value(-0.0, 1.0), "0");
    }

    #[test]
    fn times() {
        assert_eq!(fmt_elapsed(75.0, 5.0), "1:15");
        assert_eq!(fmt_elapsed(3725.0, 60.0), "1:02:05");
        assert_eq!(fmt_elapsed(1.25, 0.05), "0:01.25");
        assert_eq!(fmt_span(0.5), "500ms");
        assert_eq!(fmt_span(90.0), "1.5m");
        assert_eq!(fmt_span(60.0), "1m");
        assert_eq!(time_step(7.0), 10.0);
    }
}
