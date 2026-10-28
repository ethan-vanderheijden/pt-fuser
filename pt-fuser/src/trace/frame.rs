use std::{fmt::Display, sync::Arc};

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use thin_vec::ThinVec;

use crate::trace::{
    Error, SymbolInfo,
    metrics::{Metrics, MetricsRange},
};

mod ordered_annotations_serde {
    use super::*;
    use serde::{Deserializer, Serializer};

    pub fn serialize<S>(
        value: &Option<Box<IndexMap<String, Annotation>>>,
        serializer: S,
    ) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        #[derive(Serialize)]
        struct Helper<'a>(
            #[serde(with = "indexmap::map::serde_seq")] &'a IndexMap<String, Annotation>,
        );

        value.as_ref().map(|v| Helper(v)).serialize(serializer)
    }

    pub fn deserialize<'de, D>(
        deserializer: D,
    ) -> Result<Option<Box<IndexMap<String, Annotation>>>, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct Helper(#[serde(with = "indexmap::map::serde_seq")] IndexMap<String, Annotation>);

        let helper = Option::deserialize(deserializer)?;
        Ok(helper.map(|Helper(external)| Box::new(external)))
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Annotation {
    Bool(bool),
    Uint64(u64),
    Int64(i64),
    /// Displayed differently from Uint64 in Perfetto UI
    Pointer(u64),
    Double(f64),
    String(String),
    Array(Vec<Annotation>),
    #[serde(with = "indexmap::map::serde_seq")]
    Map(IndexMap<String, Annotation>),
}

struct ChunkIterator<'a> {
    curr_metrics: Metrics,
    ending_metric: Metrics,
    chunks: &'a [StoredChunk],
    index: usize,
}

impl ChunkIterator<'_> {
    fn new<'a>(overall_metrics: &MetricsRange, chunks: &'a [StoredChunk]) -> ChunkIterator<'a> {
        ChunkIterator {
            curr_metrics: overall_metrics.start,
            ending_metric: overall_metrics.end(),
            chunks,
            index: 0,
        }
    }
}

impl<'a> Iterator for ChunkIterator<'a> {
    type Item = Chunk<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.index < self.chunks.len() {
            let chunk = &self.chunks[self.index];

            if chunk.metrics().start.ts > self.curr_metrics.ts
                || chunk.metrics().start.cycles > self.curr_metrics.cycles
                || chunk.metrics().start.insn_count > self.curr_metrics.insn_count
            {
                let straightline = MetricsRange::new(self.curr_metrics, &chunk.metrics().start);
                self.curr_metrics = chunk.metrics().start;
                Some(Chunk::Straightline(straightline))
            } else {
                self.curr_metrics = chunk.metrics().end();
                self.index += 1;
                Some(chunk.to_chunk())
            }
        } else {
            if self.ending_metric.ts > self.curr_metrics.ts
                || self.ending_metric.cycles > self.curr_metrics.cycles
                || self.ending_metric.insn_count > self.curr_metrics.insn_count
            {
                let straightline = MetricsRange::new(self.curr_metrics, &self.ending_metric);
                self.curr_metrics = self.ending_metric;
                Some(Chunk::Straightline(straightline))
            } else {
                None
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Frame {
    pub symbol_idx: usize,
    #[serde(skip)]
    pub symbol: Arc<SymbolInfo>,
    pub metrics: MetricsRange,
    #[serde(with = "ordered_annotations_serde")]
    pub annotations: Option<Box<IndexMap<String, Annotation>>>,
    // To stay memory efficient, only store chunks for Frames, Pauses, etc.
    // Straightline chunks will be injected on the fly when calling chunks() function
    chunks: ThinVec<StoredChunk>,
}

impl Display for Frame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} ({} - {})",
            self.symbol,
            self.metrics.start,
            self.metrics.end()
        )
    }
}

impl Frame {
    pub fn new(metrics: MetricsRange, symbol_idx: usize, symbol: Arc<SymbolInfo>) -> Self {
        Self {
            symbol_idx,
            symbol,
            metrics,
            annotations: None,
            chunks: ThinVec::new(),
        }
    }

    fn insert_chunk(&mut self, chunk: StoredChunk) -> Result<(), Error> {
        // Iterate backwards through existing chunks
        // that way, inserting chunks in chronological order is O(n)
        let chunk_end = chunk.metrics().end();
        let mut straightline_end = self.metrics.end();
        for i in (0..self.chunks.len()).rev() {
            if straightline_end.ts < chunk_end.ts
                || straightline_end.cycles < chunk_end.cycles
                || straightline_end.insn_count < chunk_end.insn_count
            {
                return Err(Error::InvalidRange(chunk.metrics().clone()));
            }

            let existing_chunk = &self.chunks[i];
            let straightline = MetricsRange::new(existing_chunk.metrics().end(), &straightline_end);
            if straightline.includes_range(chunk.metrics()) {
                self.chunks.insert(i + 1, chunk);
                return Ok(());
            }

            straightline_end = existing_chunk.metrics().start;
        }

        let straightline = MetricsRange::new(self.metrics.start, &straightline_end);
        if straightline.includes_range(chunk.metrics()) {
            self.chunks.insert(0, chunk);
            return Ok(());
        }

        Err(Error::InvalidRange(chunk.metrics().clone()))
    }

    pub fn add_child(&mut self, child: Frame) -> Result<(), Error> {
        self.insert_chunk(child.into())
    }

    pub fn add_pause(&mut self, pause: MetricsRange) -> Result<(), Error> {
        self.insert_chunk(StoredChunk::Pause(pause))
    }

    pub fn get_or_default_annotation(&mut self) -> &mut IndexMap<String, Annotation> {
        if self.annotations.is_none() {
            self.annotations = Some(Box::new(IndexMap::new()));
        }
        self.annotations.as_mut().unwrap()
    }

    #[inline]
    pub fn chunks(&'_ self) -> impl Iterator<Item = Chunk<'_>> {
        ChunkIterator::new(&self.metrics, &self.chunks)
    }

    // INVARIANT: sum of time, cycles, and insn across all children must equal this frame's time, cycles, and insn
    pub fn check_invariant(&self) -> bool {
        let mut total_time = 0;
        let mut total_cycles = 0;
        let mut total_insn = 0;
        for chunk in self.chunks() {
            total_time += chunk.metrics().total_time();
            total_cycles += chunk.metrics().total_cycles();
            total_insn += chunk.metrics().total_insn();
        }

        total_time == self.metrics.total_time()
            && total_cycles == self.metrics.total_cycles()
            && total_insn == self.metrics.total_insn()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub(super) enum StoredChunk {
    Frame(Frame),
    Pause(MetricsRange),
}

impl StoredChunk {
    fn metrics(&self) -> &MetricsRange {
        match self {
            StoredChunk::Frame(frame) => &frame.metrics,
            StoredChunk::Pause(pause) => pause,
        }
    }

    fn to_chunk(&'_ self) -> Chunk<'_> {
        match self {
            StoredChunk::Frame(frame) => Chunk::Frame(frame),
            StoredChunk::Pause(pause) => Chunk::Pause(pause),
        }
    }
}

impl From<Frame> for StoredChunk {
    fn from(frame: Frame) -> Self {
        StoredChunk::Frame(frame)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Chunk<'a> {
    Frame(&'a Frame),
    Straightline(MetricsRange),
    Pause(&'a MetricsRange),
}

impl Chunk<'_> {
    pub fn metrics(&self) -> MetricsRange {
        match self {
            Chunk::Frame(frame) => frame.metrics.clone(),
            Chunk::Straightline(straightline) => straightline.clone(),
            Chunk::Pause(pause) => (*pause).clone(),
        }
    }
}

#[derive(Deserialize)]
pub(super) struct FrameSerializedFormat {
    symbol_idx: usize,
    metrics: MetricsRange,
    #[serde(with = "ordered_annotations_serde")]
    annotations: Option<Box<IndexMap<String, Annotation>>>,
    chunks: ThinVec<StoredChunkSerializedFormat>,
}

impl FrameSerializedFormat {
    pub fn into_frame(self, symbol_cache: &[Arc<SymbolInfo>]) -> Result<Frame, Error> {
        if self.symbol_idx >= symbol_cache.len() {
            return Err(Error::Deserialization(format!(
                "Invalid symbol index {} for frame, only {} symbols available",
                self.symbol_idx,
                symbol_cache.len()
            )));
        }

        let symbol_arc = Arc::clone(&symbol_cache[self.symbol_idx]);
        Ok(Frame {
            symbol_idx: self.symbol_idx,
            symbol: symbol_arc,
            metrics: self.metrics,
            annotations: self.annotations,
            chunks: self
                .chunks
                .into_iter()
                .map(|chunk_def| chunk_def.into_stored_chunk(symbol_cache))
                .collect::<Result<_, Error>>()?,
        })
    }
}

#[derive(Deserialize)]
pub(super) enum StoredChunkSerializedFormat {
    Frame(FrameSerializedFormat),
    Pause(MetricsRange),
}

impl StoredChunkSerializedFormat {
    pub fn into_stored_chunk(self, symbol_cache: &[Arc<SymbolInfo>]) -> Result<StoredChunk, Error> {
        match self {
            StoredChunkSerializedFormat::Frame(frame_def) => {
                let frame = frame_def.into_frame(symbol_cache)?;
                Ok(StoredChunk::Frame(frame))
            }
            StoredChunkSerializedFormat::Pause(pause) => Ok(StoredChunk::Pause(pause)),
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::trace::test::*;

    #[test]
    fn range_totals() {
        let chunk = Chunk::Frame(&new_frame(&SAMPLE_RANGE));
        assert_eq!(SAMPLE_RANGE.total_time(), 100);
        assert_eq!(SAMPLE_RANGE.total_cycles(), 300);
        assert_eq!(SAMPLE_RANGE.total_insn(), 800);
        assert_eq!(chunk.metrics().total_time(), SAMPLE_RANGE.total_time());
        assert_eq!(chunk.metrics().total_cycles(), SAMPLE_RANGE.total_cycles());
        assert_eq!(chunk.metrics().total_insn(), SAMPLE_RANGE.total_insn());
    }

    #[test]
    fn zero_duration_frame() {
        let chunk = Chunk::Frame(&new_frame(&MetricsRange::new(
            SAMPLE_RANGE.start,
            &SAMPLE_RANGE.start,
        )));
        assert_eq!(chunk.metrics().total_time(), 0);
        assert_eq!(chunk.metrics().total_cycles(), 0);
        assert_eq!(chunk.metrics().total_insn(), 0);
    }

    #[test]
    fn empty_frame_invariant() {
        let frame = new_frame(&SAMPLE_RANGE);
        assert!(frame.check_invariant());
    }

    #[test]
    fn fails_invariant() {
        let mut frame = new_frame(&SAMPLE_RANGE);
        let inner_frame = new_frame(&MetricsRange::new(
            SAMPLE_RANGE.start - METRICS_ONE,
            &SAMPLE_RANGE_END,
        ));
        frame.chunks.push(inner_frame.into());
        assert!(!frame.check_invariant());
    }

    #[test]
    fn child_frame_invariant() {
        let mut frame = new_frame(&SAMPLE_RANGE);
        let child1 = new_frame(&INNER_RANGE1);
        let child2 = new_frame(&INNER_RANGE2);
        frame.add_child(child1).unwrap();
        frame.add_child(child2).unwrap();
        assert!(frame.check_invariant());
    }

    #[test]
    fn child_overlaps_parent() {
        let mut outer = new_frame(&SAMPLE_RANGE);
        let inner = new_frame(&SAMPLE_RANGE);
        outer.add_child(inner).unwrap();
        assert_eq!(outer.chunks().count(), 1);
        assert!(outer.check_invariant());
    }

    #[test]
    fn child_overlapping_complex() {
        let trace = test_trace();
        let outer = trace.root_frame();
        let chunks = outer.chunks().collect::<Vec<_>>();
        assert_eq!(chunks.len(), 5);
        assert!(outer.check_invariant());
        assert!(matches!(chunks[0], Chunk::Frame(_)));
        assert!(matches!(chunks[1], Chunk::Straightline(_)));
        assert!(matches!(chunks[2], Chunk::Frame(_)));
        assert!(matches!(chunks[3], Chunk::Straightline(_)));
        assert!(matches!(chunks[4], Chunk::Frame(_)));
    }

    #[test]
    fn add_invalid_child() {
        let mut frame = new_frame(&SAMPLE_RANGE);
        let too_early = new_frame(&MetricsRange::new(
            SAMPLE_RANGE.start - METRICS_ONE,
            &INNER_RANGE1_END,
        ));
        let too_late = new_frame(&MetricsRange::new(
            INNER_RANGE2.start,
            &(SAMPLE_RANGE_END + METRICS_ONE),
        ));
        assert!(frame.add_child(too_early).is_err());
        assert!(frame.add_child(too_late).is_err());
    }

    #[test]
    fn add_child_no_space() {
        let mut outer = new_frame(&SAMPLE_RANGE);
        let middle = new_frame(&INNER_RANGE1);
        outer.add_child(middle).unwrap();
        let beginning = new_frame(&MetricsRange::new(
            SAMPLE_RANGE.start + METRICS_ONE,
            &(INNER_RANGE1.start + METRICS_ONE),
        ));
        let end = new_frame(&MetricsRange::new(
            INNER_RANGE1_END - METRICS_ONE,
            &(SAMPLE_RANGE_END - METRICS_ONE),
        ));
        assert!(outer.add_child(beginning).is_err());
        assert!(outer.add_child(end).is_err());
    }

    #[test]
    fn add_child_instant() {
        let mut outer = new_frame(&SAMPLE_RANGE);
        let child = new_frame(&MetricsRange::new(
            SAMPLE_RANGE.start + METRICS_ONE,
            &(SAMPLE_RANGE.start + METRICS_ONE),
        ));
        assert!(outer.add_child(child).is_ok());
    }

    #[test]
    fn add_child_nested_instant() {
        let mut outer = new_frame(&SAMPLE_RANGE);
        let range = MetricsRange::new(
            SAMPLE_RANGE.start + METRICS_ONE,
            &(SAMPLE_RANGE.start + METRICS_ONE),
        );
        let mut child1 = new_frame(&range);
        let mut child2 = new_frame(&range);
        let child3 = new_frame(&range);
        assert!(child2.add_child(child3).is_ok());
        assert!(child1.add_child(child2).is_ok());
        assert!(outer.add_child(child1).is_ok());
    }

    #[test]
    fn add_child_multiple_instant() {
        let mut outer = new_frame(&SAMPLE_RANGE);
        let range = MetricsRange::new(
            SAMPLE_RANGE.start + METRICS_ONE,
            &(SAMPLE_RANGE.start + METRICS_ONE),
        );
        let child1 = new_frame(&range);
        let child2 = new_frame(&range);
        assert!(outer.add_child(child1).is_ok());
        assert!(outer.add_child(child2).is_ok());
    }

    #[test]
    fn add_child_multiple_nested_instant() {
        let mut outer = new_frame(&SAMPLE_RANGE);
        let range = MetricsRange::new(
            SAMPLE_RANGE.start + METRICS_ONE,
            &(SAMPLE_RANGE.start + METRICS_ONE),
        );
        let mut child1 = new_frame(&range);
        let child2 = new_frame(&range);
        let child3 = new_frame(&range);
        assert!(child1.add_child(child2).is_ok());
        assert!(child1.add_child(child3).is_ok());
        assert!(outer.add_child(child1).is_ok());
    }

    #[test]
    fn add_child_adjacent_ends_no_straightline() {
        let mut outer = new_frame(&SAMPLE_RANGE);
        let child1 = new_frame(&SAMPLE_RANGE);
        let beginning = new_frame(&MetricsRange::new(SAMPLE_RANGE.start, &SAMPLE_RANGE.start));
        let end = new_frame(&MetricsRange::new(SAMPLE_RANGE_END, &SAMPLE_RANGE_END));
        outer.add_child(child1).unwrap();
        assert!(outer.add_child(beginning).is_ok());
        assert!(outer.add_child(end).is_ok());
    }

    #[test]
    fn add_adjacent_start_invariant() {
        let start = Metrics {
            ts: 100,
            cycles: 100,
            insn_count: 100,
        };
        let start_off = Metrics {
            ts: 100,
            cycles: 110,
            insn_count: 110,
        };
        let end = Metrics {
            ts: 200,
            cycles: 200,
            insn_count: 200,
        };
        let mut outer = new_frame(&MetricsRange::new(start, &end));
        let child = new_frame(&MetricsRange::new(start_off, &(start_off + METRICS_ONE)));
        outer.add_child(child).unwrap();
        assert!(outer.check_invariant());
    }

    #[test]
    fn add_adjacent_end_invariant() {
        let start = Metrics {
            ts: 100,
            cycles: 100,
            insn_count: 100,
        };
        let end = Metrics {
            ts: 200,
            cycles: 200,
            insn_count: 200,
        };
        let end_off = Metrics {
            ts: 200,
            cycles: 190,
            insn_count: 190,
        };
        let mut outer = new_frame(&MetricsRange::new(start, &end));
        let child = new_frame(&MetricsRange::new(end_off - METRICS_ONE, &end_off));
        outer.add_child(child).unwrap();
        assert!(outer.check_invariant());
    }
}
