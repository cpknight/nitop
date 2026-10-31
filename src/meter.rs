//! Level-meter ballistics, like an LED meter on a stereo: the bar jumps up instantly and
//! falls smoothly, and a peak marker holds at the top before dropping back.
//! Levels are log10(bytes/s), which suits traffic spanning many orders of magnitude.

/// Falls across the whole meter in about 2.5 s.
const LEVEL_FALL_PER_SEC: f64 = 3.0;
const PEAK_HOLD_SECS: f64 = 2.5;
const PEAK_FALL_PER_SEC: f64 = 1.0;

#[derive(Default, Clone, Copy)]
pub struct Meter {
    target: f64,
    pub level: f64,
    pub peak: f64,
    hold: f64,
}

impl Meter {
    /// Feeds a new measured rate.
    pub fn feed(&mut self, bytes_per_sec: f64) {
        self.target = if bytes_per_sec >= 1.0 {
            bytes_per_sec.log10()
        } else {
            0.0
        };
        self.level = self.level.max(self.target);
        if self.level >= self.peak {
            self.peak = self.level;
            self.hold = PEAK_HOLD_SECS;
        }
    }

    /// Advances the animation by `dt` seconds. Returns whether anything moved.
    pub fn tick(&mut self, dt: f64) -> bool {
        let before = (self.level, self.peak);
        self.level = (self.level - LEVEL_FALL_PER_SEC * dt).max(self.target);
        self.hold -= dt;
        if self.hold <= 0.0 {
            self.peak -= PEAK_FALL_PER_SEC * dt;
        }
        self.peak = self.peak.max(self.level);
        before != (self.level, self.peak)
    }
}

/// Inbound and outbound channels, like left and right.
#[derive(Default)]
pub struct Meters {
    pub rx: Meter,
    pub tx: Meter,
}

impl Meters {
    pub fn tick(&mut self, dt: f64) -> bool {
        // Not short-circuited: both channels must always advance.
        self.rx.tick(dt) | self.tx.tick(dt)
    }
}

/// Fixed log scale, in bytes/s: 1K to 10G of the display unit.
pub struct Scale {
    low: f64,
    high: f64,
    pub ticks: Vec<(f64, &'static str)>,
    pub unit: &'static str,
}

impl Scale {
    pub fn new(bits: bool) -> Self {
        let (k, per_unit, unit): (f64, f64, _) = if bits {
            (1000.0, 8.0, "bps")
        } else {
            (1024.0, 1.0, "B/s")
        };
        let v = |power: i32, times: f64| k.powi(power) * times / per_unit;
        let ticks = vec![
            (v(1, 1.0), "1K"),
            (v(1, 10.0), "10K"),
            (v(1, 100.0), "100K"),
            (v(2, 1.0), "1M"),
            (v(2, 10.0), "10M"),
            (v(2, 100.0), "100M"),
            (v(3, 1.0), "1G"),
            (v(3, 10.0), "10G"),
        ];
        Scale {
            low: v(1, 1.0).log10(),
            high: v(3, 10.0).log10(),
            ticks,
            unit,
        }
    }

    /// 0..=1 position of a log10 level.
    pub fn position(&self, log_level: f64) -> f64 {
        ((log_level - self.low) / (self.high - self.low)).clamp(0.0, 1.0)
    }

    /// Lit segments out of `n`. Any activity lights at least one, so it's never invisible.
    pub fn segments(&self, log_level: f64, n: usize) -> usize {
        if log_level <= 0.0 {
            return 0;
        }
        ((self.position(log_level) * n as f64).round() as usize).clamp(1, n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attacks_instantly_and_falls_gradually() {
        let mut m = Meter::default();
        m.feed(1_000_000.0);
        assert_eq!(m.level, 6.0);
        m.feed(0.0);
        assert_eq!(m.level, 6.0);
        assert!(m.tick(0.5));
        assert_eq!(m.level, 4.5);
        for _ in 0..10 {
            m.tick(0.5);
        }
        assert_eq!(m.level, 0.0);
    }

    #[test]
    fn level_stops_falling_at_a_steady_rate() {
        let mut m = Meter::default();
        m.feed(1_000_000.0);
        m.feed(1000.0);
        for _ in 0..100 {
            m.tick(0.1);
        }
        assert_eq!(m.level, 3.0);
    }

    #[test]
    fn peak_holds_then_falls_and_never_goes_below_level() {
        let mut m = Meter::default();
        m.feed(1_000_000.0);
        m.feed(10.0);
        m.tick(2.0);
        assert_eq!(m.peak, 6.0);
        m.tick(1.0); // hold has now expired
        assert_eq!(m.peak, 5.0);
        for _ in 0..50 {
            m.tick(1.0);
            assert!(m.peak >= m.level);
        }
        assert_eq!(m.peak, 1.0);
    }

    #[test]
    fn a_new_high_restarts_the_hold() {
        let mut m = Meter::default();
        m.feed(1000.0);
        m.tick(2.0);
        m.feed(100_000.0);
        m.tick(2.0);
        assert_eq!(m.peak, 5.0);
    }

    #[test]
    fn settled_meter_reports_no_movement() {
        let mut m = Meter::default();
        m.feed(0.0);
        assert!(!m.tick(0.05));
    }

    #[test]
    fn segments_span_the_scale() {
        let s = Scale::new(false);
        assert_eq!(s.segments(0.0, 40), 0);
        assert_eq!(s.segments(1.0, 40), 1); // under the scale, but active
        assert_eq!(s.segments(s.high, 40), 40);
        assert_eq!(s.segments(20.0, 40), 40);
        let mid = (s.low + s.high) / 2.0;
        assert_eq!(s.segments(mid, 40), 20);
    }

    #[test]
    fn bits_scale_reads_true_in_bits() {
        let s = Scale::new(true);
        let one_mbps = (1_000_000.0_f64 / 8.0).log10();
        let (tick, label) = s.ticks[3];
        assert_eq!(label, "1M");
        assert!((tick.log10() - one_mbps).abs() < 1e-9);
    }
}
