pub mod builder;
pub mod metrics;
pub mod trace_error;

#[cfg(test)]
mod test;

use std::{fmt::Display, io::Read, sync::Arc};

use flate2::Compression;
use flexbuffers::FlexbufferSerializer;
use indexmap::IndexMap;
use serde::{Deserialize, Serialize, de::Error as DeError, ser::Error as SerError};
use thin_vec::ThinVec;

use crate::trace::metrics::{Metrics, MetricsRange};

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
        match value {
            Some(annotations) => indexmap::map::serde_seq::serialize(annotations, serializer),
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D>(
        deserializer: D,
    ) -> Result<Option<Box<IndexMap<String, Annotation>>>, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct SeqWrapper(#[serde(with = "indexmap::map::serde_seq")] IndexMap<String, Annotation>);

        let opt = Option::<SeqWrapper>::deserialize(deserializer)?;
        Ok(opt.map(|wrapper| Box::new(wrapper.0)))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SymbolInfo {
    pub name: String,
    pub offset: u64,
    pub size: u64,
}

impl SymbolInfo {
    pub fn contains(&self, addr: u64) -> bool {
        self.offset <= addr && addr < self.offset + self.size
    }
}

impl Display for SymbolInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "[0x{:x} - 0x{:x}] {}",
            self.offset,
            self.offset + self.size,
            self.name
        )
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
                return Some(Chunk::Straightline(straightline));
            } else {
                self.curr_metrics = chunk.metrics().end();
                self.index += 1;
                return Some(chunk.to_chunk());
            }
        } else {
            if self.ending_metric.ts > self.curr_metrics.ts
                || self.ending_metric.cycles > self.curr_metrics.cycles
                || self.ending_metric.insn_count > self.curr_metrics.insn_count
            {
                let straightline = MetricsRange::new(self.curr_metrics, &self.ending_metric);
                self.curr_metrics = self.ending_metric;
                return Some(Chunk::Straightline(straightline));
            } else {
                return None;
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

        return Err(Error::InvalidRange(chunk.metrics().clone()));
    }

    pub fn add_child(&mut self, child: Frame) -> Result<(), Error> {
        self.insert_chunk(child.into())
    }

    pub fn add_pause(&mut self, pause: MetricsRange) -> Result<(), Error> {
        self.insert_chunk(StoredChunk::Pause(pause))
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
enum StoredChunk {
    Frame(Frame),
    Pause(MetricsRange),
}

impl StoredChunk {
    fn metrics(&self) -> &MetricsRange {
        match self {
            StoredChunk::Frame(frame) => &frame.metrics,
            StoredChunk::Pause(pause) => &pause,
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

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Event {
    pub id: u32,
    // INVARIANT: occurences must be sorted by timestamp
    occurences: Vec<Metrics>,
    pub name: String,
    pub description: String,
}

impl Event {
    pub fn new(id: u32, name: String, description: String) -> Self {
        Self {
            id,
            occurences: Vec::new(),
            name,
            description,
        }
    }

    pub fn from_occurences(
        id: u32,
        name: String,
        description: String,
        occurences: Vec<Metrics>,
    ) -> Result<Self, Error> {
        if !occurences.is_sorted() {
            return Err(Error::NotSorted);
        }
        Ok(Self {
            id,
            occurences,
            name,
            description,
        })
    }

    pub fn add_occurence(&mut self, occurence: Metrics) {
        let idx = self.occurences.partition_point(|&x| x <= occurence);
        self.occurences.insert(idx, occurence);
    }

    pub fn occurences(&self) -> &[Metrics] {
        &self.occurences
    }
}

#[derive(Debug, Serialize, Clone)]
pub struct Trace {
    symbols: Vec<Arc<SymbolInfo>>,
    root: Frame,
    events: Vec<Event>,
}

impl Trace {
    // versioning is vX.Y.Z where vX.Y represent the trace format version
    // in trace serialization, first 16 bits are X, next 16 bits are Y,
    // and next 32 bits are VERSION_DELIMITER (all in big-endian)
    // after that, the actual trace data begins
    const VERSION_DELIMITER: u32 = 0xDEADBEEF;

    pub fn new(symbols: Vec<Arc<SymbolInfo>>, root: Frame, events: Vec<Event>) -> Self {
        Self {
            symbols,
            root,
            events,
        }
    }

    pub fn root_frame(&self) -> &Frame {
        &self.root
    }

    pub fn num_symbols(&self) -> usize {
        self.symbols.len()
    }

    pub fn events(&self) -> &[Event] {
        &self.events
    }

    pub fn get_event(&self, id: u32) -> Option<&Event> {
        self.events.iter().find(|event| event.id == id)
    }

    fn get_major_minor() -> Result<(u16, u16), &'static str> {
        let Ok(major_version) = env!("CARGO_PKG_VERSION_MAJOR").parse::<u16>() else {
            return Err("Tool's major version is not a 16-bit integer");
        };
        let Ok(minor_version) = env!("CARGO_PKG_VERSION_MINOR").parse::<u16>() else {
            return Err("Tool's minor version is not a 16-bit integer");
        };
        Ok((major_version, minor_version))
    }

    pub fn bin_serialize(&self, gzip: bool) -> Result<Vec<u8>, flexbuffers::SerializationError> {
        let (major, minor) =
            Self::get_major_minor().map_err(flexbuffers::SerializationError::custom)?;
        let mut data = major
            .to_be_bytes()
            .iter()
            .chain(minor.to_be_bytes().iter())
            .chain(Self::VERSION_DELIMITER.to_be_bytes().iter())
            .copied()
            .collect::<Vec<u8>>();

        let mut serializer = FlexbufferSerializer::new();
        self.serialize(&mut serializer)?;
        if gzip {
            let encoded = serializer.take_buffer();
            let mut encoder = flate2::read::GzEncoder::new(&encoded[..], Compression::default());
            let mut result = Vec::new();
            encoder.read_to_end(&mut result).unwrap();
            data.extend(result);
            Ok(data)
        } else {
            data.extend(serializer.take_buffer());
            Ok(data)
        }
    }

    fn deserialize_trace(data: &[u8]) -> Result<Self, flexbuffers::DeserializationError> {
        // Helper defs that mirror the serialized shape but are deserializable
        #[derive(Deserialize)]
        struct TraceDef {
            symbols: Vec<SymbolInfo>,
            root: FrameDef,
            events: Vec<Event>,
        }

        #[derive(Deserialize)]
        struct FrameDef {
            symbol_idx: usize,
            metrics: MetricsRange,
            #[serde(with = "ordered_annotations_serde")]
            annotations: Option<Box<IndexMap<String, Annotation>>>,
            chunks: ThinVec<StoredChunkDef>,
        }

        #[derive(Deserialize)]
        enum StoredChunkDef {
            Frame(FrameDef),
            Pause(MetricsRange),
        }

        let traces_def: TraceDef = flexbuffers::from_slice(data)?;

        let symbols: Vec<Arc<SymbolInfo>> = traces_def.symbols.into_iter().map(Arc::new).collect();

        fn build_frame(
            def: FrameDef,
            symbols: &Vec<Arc<SymbolInfo>>,
        ) -> Result<Frame, flexbuffers::DeserializationError> {
            if def.symbol_idx >= symbols.len() {
                return Err(flexbuffers::DeserializationError::custom(format!(
                    "Invalid symbol index {} for frame, only {} symbols available",
                    def.symbol_idx,
                    symbols.len()
                )));
            }

            let symbol_arc = Arc::clone(&symbols[def.symbol_idx]);
            let frame = Frame {
                symbol_idx: def.symbol_idx,
                symbol: symbol_arc,
                metrics: def.metrics,
                annotations: def.annotations,
                chunks: def
                    .chunks
                    .into_iter()
                    .map(|chunk_def| match chunk_def {
                        StoredChunkDef::Frame(frame_def) => {
                            let child_frame = build_frame(frame_def, symbols)?;
                            Ok(StoredChunk::Frame(child_frame))
                        }
                        StoredChunkDef::Pause(pause) => Ok(StoredChunk::Pause(pause)),
                    })
                    .collect::<Result<_, flexbuffers::DeserializationError>>()?,
            };

            Ok(frame)
        }

        let root = build_frame(traces_def.root, &symbols)?;

        Ok(Trace {
            symbols,
            root,
            events: traces_def.events,
        })
    }

    pub fn bin_deserialize(
        data: &[u8],
        gzip: bool,
    ) -> Result<Self, flexbuffers::DeserializationError> {
        let (tool_major, tool_minor) =
            Self::get_major_minor().map_err(flexbuffers::DeserializationError::custom)?;

        if data.len() < 8 {
            return Err(flexbuffers::DeserializationError::custom(
                "Trace data is too short, it can't possible be correct!",
            ));
        }

        let trace_major = u16::from_be_bytes([data[0], data[1]]);
        let trace_minor = u16::from_be_bytes([data[2], data[3]]);
        let trace_delimiter = u32::from_be_bytes([data[4], data[5], data[6], data[7]]);
        let data = &data[8..];
        if trace_delimiter != Self::VERSION_DELIMITER {
            return Err(flexbuffers::DeserializationError::custom(
                "Trace data is corrupted, version delimiter is incorrect!",
            ));
        }
        if trace_major != tool_major || trace_minor != tool_minor {
            return Err(flexbuffers::DeserializationError::custom(format!(
                "Version mismatch: trace data is v{}.{} but tool is v{}.{}",
                trace_major, trace_minor, tool_major, tool_minor
            )));
        }

        let decoded_data = if gzip {
            let mut decoder = flate2::read::GzDecoder::new(data);
            let mut decoded = Vec::new();
            decoder.read_to_end(&mut decoded).unwrap();
            decoded
        } else {
            data.to_vec()
        };
        Self::deserialize_trace(&decoded_data)
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub enum Error {
    InvalidRange(MetricsRange),
    NotSorted,
}

impl Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::InvalidRange(range) => write!(f, "Invalid range: {}", range),
            Error::NotSorted => write!(f, "Occurences are not sorted by timestamp"),
        }
    }
}
