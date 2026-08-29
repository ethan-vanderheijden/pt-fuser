use crate::{
    analysis,
    trace::{Annotation, Frame, Trace},
};

pub(super) const ANNOTATION_STATS_CATEGORY: &str = "Merging Stats";
pub(super) const ANNOTATION_RAW_LATENCY_CATEGORY: &str = "Merging Raw Latencies";

pub(super) const ANNOTATION_COUNT_NAME: &str = "Count";
pub(super) const ANNOTATION_NOISE_CONTRIBUTION_NAME: &str = "Noise Contribution (NC)";

pub type StatsGenerator = Box<dyn StatsProvider>;

pub trait StatsProvider {
    fn prepare(named_traces: &[(&str, &Trace)]) -> Option<Self>
    where
        Self: Sized;
    fn category(&self) -> Option<&'static str>;
    fn compute(&self, indexed_frames: &[(usize, &Frame)]) -> Vec<(String, Annotation)>;
}

pub struct BasicStats {}

impl StatsProvider for BasicStats {
    fn prepare(_traces: &[(&str, &Trace)]) -> Option<Self> {
        Some(BasicStats {})
    }

    fn category(&self) -> Option<&'static str> {
        Some(ANNOTATION_STATS_CATEGORY)
    }

    fn compute(&self, indexed_frames: &[(usize, &Frame)]) -> Vec<(String, Annotation)> {
        let latencies = indexed_frames
            .iter()
            .map(|(_, frame)| frame.metrics.total_time() as f64)
            .collect::<Vec<_>>();
        if let Some(stats) = analysis::Stats::from_data(latencies) {
            let mut stats = stats
                .into_iter()
                .map(|(name, v)| (name, Annotation::Double(v)))
                .collect::<Vec<_>>();
            stats.push((
                ANNOTATION_COUNT_NAME.to_string(),
                Annotation::Uint64(indexed_frames.len() as u64),
            ));
            stats
        } else {
            Vec::new()
        }
    }
}

pub struct RawLatencies {
    trace_names: Vec<String>,
}

impl StatsProvider for RawLatencies {
    fn prepare(named_traces: &[(&str, &Trace)]) -> Option<Self> {
        let trace_names = named_traces
            .iter()
            .map(|(name, _)| name.to_string())
            .collect();
        Some(RawLatencies { trace_names })
    }

    fn category(&self) -> Option<&'static str> {
        Some(ANNOTATION_RAW_LATENCY_CATEGORY)
    }

    fn compute(&self, indexed_frames: &[(usize, &Frame)]) -> Vec<(String, Annotation)> {
        indexed_frames
            .iter()
            .map(|(idx, frame)| {
                (
                    self.trace_names[*idx].clone(),
                    Annotation::Uint64(frame.metrics.total_time()),
                )
            })
            .collect::<Vec<_>>()
    }
}

pub struct NoiseContribution {
    e2e_latencies: Vec<u64>,
    e2e_stddev: f64,
}

impl StatsProvider for NoiseContribution {
    fn prepare(named_traces: &[(&str, &Trace)]) -> Option<Self> {
        let latencies = named_traces
            .iter()
            .map(|(_, trace)| trace.root_frame().metrics.total_time())
            .collect::<Vec<_>>();
        let stats = analysis::Stats::from_data(latencies.iter().map(|&v| v as f64))?;
        if stats.stddev == 0.0 {
            return None;
        }
        Some(NoiseContribution {
            e2e_latencies: latencies,
            e2e_stddev: stats.stddev,
        })
    }

    fn category(&self) -> Option<&'static str> {
        Some(ANNOTATION_STATS_CATEGORY)
    }

    fn compute(&self, indexed_frames: &[(usize, &Frame)]) -> Vec<(String, Annotation)> {
        // A frame that did not occur in a trace already completes instantaneously in that trace,
        // so its latency is zero rather than a reason to remove the trace from the population.
        let mut frame_latencies = vec![0.0; self.e2e_latencies.len()];
        for (idx, frame) in indexed_frames {
            frame_latencies[*idx] = frame.metrics.total_time() as f64;
        }

        let latency_without_frame = self
            .e2e_latencies
            .iter()
            .zip(frame_latencies)
            .map(|(e2e, frame)| *e2e as f64 - frame);
        if let Some(stats) = analysis::Stats::from_data(latency_without_frame) {
            // NC(A) = (SD(LE2E) - SD(LE2E - LA)) / SD(LE2E)
            vec![(
                ANNOTATION_NOISE_CONTRIBUTION_NAME.to_string(),
                Annotation::Double(1.0 - stats.stddev / self.e2e_stddev),
            )]
        } else {
            Vec::new()
        }
    }
}
