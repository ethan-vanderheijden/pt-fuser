use std::{
    fmt::Display,
    iter::Sum,
    ops::{Add, AddAssign, Div, DivAssign, Mul, MulAssign, Sub, SubAssign},
};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Metrics {
    pub ts: u64,
    pub cycles: u64,
    pub insn_count: u64,
}

impl Metrics {
    pub fn new(ts: u64, cycles: u64, insn_count: u64) -> Self {
        Self {
            ts,
            cycles,
            insn_count,
        }
    }

    pub fn constant(c: u64) -> Self {
        Self {
            ts: c,
            cycles: c,
            insn_count: c,
        }
    }
}

impl Add for Metrics {
    type Output = Metrics;

    fn add(self, rhs: Self) -> Self::Output {
        &self + &rhs
    }
}

impl Add for &Metrics {
    type Output = Metrics;

    fn add(self, rhs: Self) -> Self::Output {
        Metrics {
            ts: self.ts + rhs.ts,
            cycles: self.cycles + rhs.cycles,
            insn_count: self.insn_count + rhs.insn_count,
        }
    }
}

impl AddAssign for Metrics {
    fn add_assign(&mut self, rhs: Self) {
        self.ts += rhs.ts;
        self.cycles += rhs.cycles;
        self.insn_count += rhs.insn_count;
    }
}

impl Sub for Metrics {
    type Output = Metrics;

    fn sub(self, rhs: Self) -> Self::Output {
        &self - &rhs
    }
}

impl Sub for &Metrics {
    type Output = Metrics;

    fn sub(self, rhs: Self) -> Self::Output {
        Metrics {
            ts: self.ts - rhs.ts,
            cycles: self.cycles - rhs.cycles,
            insn_count: self.insn_count - rhs.insn_count,
        }
    }
}

impl SubAssign for Metrics {
    fn sub_assign(&mut self, rhs: Self) {
        self.ts -= rhs.ts;
        self.cycles -= rhs.cycles;
        self.insn_count -= rhs.insn_count;
    }
}

impl Div<u64> for Metrics {
    type Output = Metrics;

    fn div(self, rhs: u64) -> Self::Output {
        &self / rhs
    }
}

impl Div<u64> for &Metrics {
    type Output = Metrics;

    fn div(self, rhs: u64) -> Self::Output {
        Metrics {
            ts: self.ts / rhs,
            cycles: self.cycles / rhs,
            insn_count: self.insn_count / rhs,
        }
    }
}

impl DivAssign<u64> for Metrics {
    fn div_assign(&mut self, rhs: u64) {
        self.ts /= rhs;
        self.cycles /= rhs;
        self.insn_count /= rhs;
    }
}

impl Div for Metrics {
    type Output = Metrics;

    fn div(self, rhs: Self) -> Self::Output {
        &self / &rhs
    }
}

impl Div for &Metrics {
    type Output = Metrics;

    fn div(self, rhs: Self) -> Self::Output {
        Metrics {
            ts: self.ts / rhs.ts,
            cycles: self.cycles / rhs.cycles,
            insn_count: self.insn_count / rhs.insn_count,
        }
    }
}

impl DivAssign for Metrics {
    fn div_assign(&mut self, rhs: Self) {
        self.ts /= rhs.ts;
        self.cycles /= rhs.cycles;
        self.insn_count /= rhs.insn_count;
    }
}

impl Mul<u64> for Metrics {
    type Output = Metrics;

    fn mul(self, rhs: u64) -> Self::Output {
        &self * rhs
    }
}

impl Mul<u64> for &Metrics {
    type Output = Metrics;

    fn mul(self, rhs: u64) -> Self::Output {
        Metrics {
            ts: self.ts * rhs,
            cycles: self.cycles * rhs,
            insn_count: self.insn_count * rhs,
        }
    }
}

impl MulAssign<u64> for Metrics {
    fn mul_assign(&mut self, rhs: u64) {
        self.ts *= rhs;
        self.cycles *= rhs;
        self.insn_count *= rhs;
    }
}

impl Mul for Metrics {
    type Output = Metrics;

    fn mul(self, rhs: Self) -> Self::Output {
        &self * &rhs
    }
}

impl Mul for &Metrics {
    type Output = Metrics;

    fn mul(self, rhs: Self) -> Self::Output {
        Metrics {
            ts: self.ts * rhs.ts,
            cycles: self.cycles * rhs.cycles,
            insn_count: self.insn_count * rhs.insn_count,
        }
    }
}

impl MulAssign for Metrics {
    fn mul_assign(&mut self, rhs: Self) {
        self.ts *= rhs.ts;
        self.cycles *= rhs.cycles;
        self.insn_count *= rhs.insn_count;
    }
}

impl PartialOrd for Metrics {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Metrics {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.ts.cmp(&other.ts)
    }
}

impl Sum for Metrics {
    fn sum<I: Iterator<Item = Self>>(iter: I) -> Self {
        iter.fold(Metrics::constant(0), |acc, x| acc + x)
    }
}

impl<'a> Sum<&'a Metrics> for Metrics {
    fn sum<I: Iterator<Item = &'a Self>>(iter: I) -> Self {
        iter.fold(Metrics::constant(0), |acc, x| acc + *x)
    }
}

impl Display for Metrics {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "(ts: {}, cycles: {}, insn_count: {})",
            self.ts, self.cycles, self.insn_count
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MetricsRange {
    // start is inclusive and end is exclusive
    pub start: Metrics,
    ts_incr: u32,
    cycles_incr: u32,
    insn_incr: u32,
}

impl MetricsRange {
    /// Only supports ranges up to 2^32-1 in size for each metric.
    pub const fn new(start: Metrics, end: &Metrics) -> Self {
        assert!(end.ts >= start.ts);
        assert!(end.ts - start.ts <= u32::MAX as u64);
        assert!(end.cycles >= start.cycles);
        assert!(end.cycles - start.cycles <= u32::MAX as u64);
        assert!(end.insn_count >= start.insn_count);
        assert!(end.insn_count - start.insn_count <= u32::MAX as u64);
        Self {
            start,
            ts_incr: (end.ts - start.ts) as u32,
            cycles_incr: (end.cycles - start.cycles) as u32,
            insn_incr: (end.insn_count - start.insn_count) as u32,
        }
    }

    #[inline]
    pub fn total_time(&self) -> u64 {
        self.ts_incr as u64
    }

    #[inline]
    pub fn total_cycles(&self) -> u64 {
        self.cycles_incr as u64
    }

    #[inline]
    pub fn total_insn(&self) -> u64 {
        self.insn_incr as u64
    }

    #[inline]
    pub fn end(&self) -> Metrics {
        Metrics {
            ts: self.start.ts + self.ts_incr as u64,
            cycles: self.start.cycles + self.cycles_incr as u64,
            insn_count: self.start.insn_count + self.insn_incr as u64,
        }
    }

    #[inline]
    pub fn includes_range(&self, other: &MetricsRange) -> bool {
        let other_end = other.end();
        let self_end = self.end();
        self.start.ts <= other.start.ts
            && other_end.ts <= self_end.ts
            && self.start.cycles <= other.start.cycles
            && other_end.cycles <= self_end.cycles
            && self.start.insn_count <= other.start.insn_count
            && other_end.insn_count <= self_end.insn_count
    }
}

impl Display for MetricsRange {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let end = self.end();
        write!(
            f,
            "MetricsRange {{ (ts: {}, cycles: {}, insn_count: {}) - (ts: {}, cycles: {}, insn_count: {}) }}",
            self.start.ts,
            self.start.cycles,
            self.start.insn_count,
            end.ts,
            end.cycles,
            end.insn_count
        )
    }
}
