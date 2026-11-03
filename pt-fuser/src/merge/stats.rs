use indexmap::IndexMap;

use crate::{
    analysis,
    trace::{
        Trace,
        frame::{Annotation, Chunk, Frame},
        trace_annotation::{
            KnownAnnotation, MergingNoiseContribution, MergingRawLatencies, MergingStats,
        },
    },
};

pub type StatsGenerator = Box<dyn StatsProvider>;

pub trait StatsProvider {
    type Annotation: KnownAnnotation
    where
        Self: Sized;

    fn prepare(named_traces: &[(&str, &Trace)]) -> Option<Self>
    where
        Self: Sized;
    fn add_stats(&self, target: &mut Frame, indexed_frames: &[(usize, &Frame)]);
}

pub struct BasicStats {}

impl BasicStats {
    const COUNT_NAME: &'static str = "Count";
}

impl StatsProvider for BasicStats {
    type Annotation = MergingStats;

    fn prepare(_traces: &[(&str, &Trace)]) -> Option<Self> {
        Some(BasicStats {})
    }

    fn add_stats(&self, target: &mut Frame, indexed_frames: &[(usize, &Frame)]) {
        let latencies = indexed_frames
            .iter()
            .map(|(_, frame)| frame.metrics.total_time() as f64)
            .collect::<Vec<_>>();
        if let Some(stats) = analysis::Stats::from_data(latencies) {
            let mut map = IndexMap::new();
            for (name, value) in stats.into_iter() {
                map.insert(name, Annotation::Double(value));
            }
            map.insert(
                Self::COUNT_NAME.to_string(),
                Annotation::Uint64(indexed_frames.len() as u64),
            );
            Self::Annotation::insert(target, map);
        }
    }
}

pub struct RawLatencies {
    trace_names: Vec<String>,
}

impl StatsProvider for RawLatencies {
    type Annotation = MergingRawLatencies;

    fn prepare(named_traces: &[(&str, &Trace)]) -> Option<Self> {
        let trace_names = named_traces
            .iter()
            .map(|(name, _)| name.to_string())
            .collect();
        Some(RawLatencies { trace_names })
    }

    fn add_stats(&self, target: &mut Frame, indexed_frames: &[(usize, &Frame)]) {
        if !indexed_frames.is_empty() {
            let mut map = IndexMap::new();
            for (idx, frame) in indexed_frames {
                map.insert(
                    self.trace_names[*idx].clone(),
                    Annotation::Uint64(frame.metrics.total_time()),
                );
            }
            Self::Annotation::insert(target, map);
        }
    }
}

/// How a frame's latency is measured when computing its noise contribution.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, clap::ValueEnum)]
pub enum NoiseContributionMode {
    /// Total latency of the frame, including time spent in child frames
    #[default]
    Inclusive,
    /// Latency of the frame minus the latencies of its direct child frames
    /// (pauses and straightline execution are kept)
    Exclusive,
}

impl NoiseContributionMode {
    fn latency(&self, frame: &Frame) -> u64 {
        match self {
            NoiseContributionMode::Inclusive => frame.metrics.total_time(),
            NoiseContributionMode::Exclusive => {
                let children_time = frame
                    .chunks()
                    .filter_map(|chunk| match chunk {
                        Chunk::Frame(child) => Some(child.metrics.total_time()),
                        _ => None,
                    })
                    .sum::<u64>();
                frame.metrics.total_time() - children_time
            }
        }
    }
}

pub struct NoiseContribution {
    e2e_latencies: Vec<u64>,
    mode: NoiseContributionMode,
}

impl NoiseContribution {
    pub fn with_mode(mut self, mode: NoiseContributionMode) -> Self {
        self.mode = mode;
        self
    }
}

impl StatsProvider for NoiseContribution {
    type Annotation = MergingNoiseContribution;

    fn prepare(named_traces: &[(&str, &Trace)]) -> Option<Self> {
        let latencies = named_traces
            .iter()
            .map(|(_, trace)| trace.root_frame().metrics.total_time())
            .collect::<Vec<_>>();
        Some(NoiseContribution {
            e2e_latencies: latencies,
            mode: NoiseContributionMode::default(),
        })
    }

    fn add_stats(&self, target: &mut Frame, indexed_frames: &[(usize, &Frame)]) {
        let mut root_latencies = Vec::with_capacity(indexed_frames.len());
        for (idx, _) in indexed_frames {
            root_latencies.push(self.e2e_latencies[*idx]);
        }

        if let Some(root_stats) =
            analysis::Stats::from_data(root_latencies.iter().map(|v| *v as f64))
            && root_stats.stddev > 0.0
        {
            for i in 0..root_latencies.len() {
                root_latencies[i] -= self.mode.latency(indexed_frames[i].1);
            }
            if let Some(new_stats) =
                analysis::Stats::from_data(root_latencies.iter().map(|v| *v as f64))
            {
                // NC(A) = (SD(LE2E) - SD(LE2E - LA)) / SD(LE2E)
                // LA is A's inclusive or exclusive latency, depending on `mode`
                Self::Annotation::insert(target, 1.0 - new_stats.stddev / root_stats.stddev);
            }
        }
    }
}
