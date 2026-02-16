mod builder;
#[cfg(test)]
mod test;

pub use builder::TraceBuilder;

use std::{
    fmt::Display,
    ops::{Add, AddAssign, Sub, SubAssign},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Metrics {
    pub ts: u64,
    pub cycles: u64,
    pub insn_count: u64,
}

impl Add for Metrics {
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

impl PartialOrd for Metrics {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.ts.cmp(&other.ts))
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MetricsRange {
    // start is inclusive and end is exclusive
    pub start: Metrics,
    pub end: Metrics,
}

impl MetricsRange {
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

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SymbolInfo {
    pub name: String,
    pub offset: u64,
    pub size: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Straightline {
    pub metrics: MetricsRange,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Frame {
    pub metrics: MetricsRange,
    pub symbol: SymbolInfo,
    // INVARIANT: sum of time, cycles, and insn across all children must equal this frame's time, cycles, and insn
    chunks: Vec<Chunk>,
}

impl Frame {
    pub fn new(metrics: MetricsRange, symbol: SymbolInfo) -> Self {
        let metrics_clone = metrics.clone();
        Self {
            metrics,
            symbol,
            chunks: vec![
                Straightline {
                    metrics: metrics_clone,
                }
                .into(),
            ],
        }
    }

    pub fn add_child(&mut self, child: Frame) -> Result<(), Error> {
        for mut i in 0..self.chunks.len() {
            match self.chunks[i] {
                Chunk::Frame(_) => continue,
                Chunk::Straightline(straightline) => {
                    if straightline.metrics.includes_range(&child.metrics) {
                        if child.metrics.start.ts > straightline.metrics.start.ts {
                            let before = Straightline {
                                metrics: MetricsRange::from(
                                    straightline.metrics.start,
                                    child.metrics.start,
                                ),
                            };
                            self.chunks.insert(i, before.into());
                            i += 1;
                        }
                        if child.metrics.end.ts < straightline.metrics.end.ts {
                            let after = Straightline {
                                metrics: MetricsRange::from(
                                    child.metrics.end,
                                    straightline.metrics.end,
                                ),
                            };
                            self.chunks.insert(i + 1, after.into());
                        }
                        self.chunks[i] = child.into();
                        return Ok(());
                    }
                }
            }
        }
        Err(Error::InvalidRange(child.metrics))
    }

    pub fn check_invariant(&self) -> bool {
        let mut total_time = 0;
        let mut total_cycles = 0;
        let mut total_insn = 0;
        for chunk in &self.chunks {
            total_time += chunk.total_time();
            total_cycles += chunk.total_cycles();
            total_insn += chunk.total_insn();
        }

        total_time == self.metrics.total_time()
            && total_cycles == self.metrics.total_cycles()
            && total_insn == self.metrics.total_insn()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Chunk {
    Frame(Frame),
    Straightline(Straightline),
}

impl Chunk {
    pub fn total_time(&self) -> u64 {
        match self {
            Chunk::Frame(frame) => frame.metrics.total_time(),
            Chunk::Straightline(straightline) => straightline.metrics.total_time(),
        }
    }

    pub fn total_cycles(&self) -> u64 {
        match self {
            Chunk::Frame(frame) => frame.metrics.total_cycles(),
            Chunk::Straightline(straightline) => straightline.metrics.total_cycles(),
        }
    }

    pub fn total_insn(&self) -> u64 {
        match self {
            Chunk::Frame(frame) => frame.metrics.total_insn(),
            Chunk::Straightline(straightline) => straightline.metrics.total_insn(),
        }
    }
}

impl From<Straightline> for Chunk {
    fn from(straightline: Straightline) -> Self {
        Chunk::Straightline(straightline)
    }
}

impl From<Frame> for Chunk {
    fn from(frame: Frame) -> Self {
        Chunk::Frame(frame)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Event {
    metrics: Metrics,
    description: String,
}

impl Event {
    pub fn new(metrics: Metrics, description: String) -> Self {
        Self {
            metrics,
            description,
        }
    }
}

impl PartialOrd for Event {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.metrics.cmp(&other.metrics))
    }
}

impl Ord for Event {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.metrics.cmp(&other.metrics)
    }
}

#[derive(Debug)]
pub struct Trace {
    root: Frame,
    event_timeline: Vec<Event>,
}

impl Trace {
    pub fn new(root: Frame, mut event_timeline: Vec<Event>) -> Self {
        event_timeline.sort();
        Self {
            root,
            event_timeline,
        }
    }

    pub fn get_root_frame(&self) -> &Frame {
        &self.root
    }

    pub fn get_events(&self) -> &[Event] {
        &self.event_timeline
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub enum Error {
    InvalidRange(MetricsRange),
}

impl Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::InvalidRange(range) => write!(f, "Invalid range: {}", range),
        }
    }
}
