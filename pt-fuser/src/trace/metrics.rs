use std::{
    fmt::Display,
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
    pub end: Metrics,
}

impl MetricsRange {
    pub fn new(start: Metrics, end: Metrics) -> Self {
        Self { start, end }
    }

    pub fn total_time(&self) -> u64 {
        self.end.ts - self.start.ts
    }

    pub fn total_cycles(&self) -> u64 {
        self.end.cycles - self.start.cycles
    }

    pub fn total_insn(&self) -> u64 {
        self.end.insn_count - self.start.insn_count
    }

    pub fn from(start: Metrics, end: Metrics) -> Self {
        Self { start, end }
    }

    pub fn includes_range(&self, other: &MetricsRange) -> bool {
        self.start.ts <= other.start.ts && other.end.ts <= self.end.ts
    }
}

impl Display for MetricsRange {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "MetricsRange {{ {} - {} }}", self.start, self.end)
    }
}
