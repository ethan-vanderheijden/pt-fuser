pub mod builder;
pub mod metrics;
pub mod trace_error;

#[cfg(test)]
mod test;

use std::{fmt::Display, io::Read};

use flate2::Compression;
use flexbuffers::FlexbufferSerializer;
use indexmap::IndexMap;
use serde::{Deserialize, Serialize, de::Error as DeError, ser::Error as SerError};

use crate::trace::metrics::{Metrics, MetricsRange};

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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Frame {
    pub symbol: SymbolInfo,
    pub metrics: MetricsRange,
    #[serde(with = "indexmap::map::serde_seq")]
    pub annotations: IndexMap<String, Annotation>,
    // INVARIANT: sum of time, cycles, and insn across all children must equal this frame's time, cycles, and insn
    chunks: Vec<Chunk>,
}

impl Display for Frame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} ({} - {})",
            self.symbol, self.metrics.start, self.metrics.end
        )
    }
}

impl Frame {
    pub fn new(metrics: MetricsRange, symbol: SymbolInfo) -> Self {
        Self {
            symbol,
            metrics,
            annotations: IndexMap::new(),
            chunks: vec![Chunk::Straightline(metrics)],
        }
    }

    pub fn insert_chunk(&mut self, chunk: Chunk) -> Result<(), Error> {
        for mut i in 0..self.chunks.len() {
            match self.chunks[i] {
                Chunk::Straightline(straightline) => {
                    if straightline.includes_range(&chunk.metrics()) {
                        if chunk.metrics().start != straightline.start {
                            let before =
                                MetricsRange::new(straightline.start, chunk.metrics().start);
                            self.chunks.insert(i, Chunk::Straightline(before));
                            i += 1;
                        }
                        if chunk.metrics().end != straightline.end {
                            let after = MetricsRange::new(chunk.metrics().end, straightline.end);
                            self.chunks.insert(i + 1, Chunk::Straightline(after));
                        }
                        self.chunks[i] = chunk;
                        return Ok(());
                    }
                }
                Chunk::Frame(_) | Chunk::Pause(_) => continue,
            }
        }

        if chunk.metrics().total_time() == 0
            && chunk.metrics().total_cycles() == 0
            && chunk.metrics().total_insn() == 0
        {
            if chunk.metrics().start == self.metrics.start {
                self.chunks.insert(0, chunk);
                return Ok(());
            } else if chunk.metrics().end == self.metrics.end {
                self.chunks.push(chunk);
                return Ok(());
            }
        }

        Err(Error::InvalidRange(chunk.metrics().clone()))
    }

    pub fn add_child(&mut self, child: Frame) -> Result<(), Error> {
        self.insert_chunk(child.into())
    }

    pub fn add_pause(&mut self, pause: MetricsRange) -> Result<(), Error> {
        self.insert_chunk(Chunk::Pause(pause))
    }

    #[inline]
    pub fn chunks(&self) -> &[Chunk] {
        &self.chunks
    }

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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Chunk {
    Frame(Frame),
    Straightline(MetricsRange),
    Pause(MetricsRange),
}

impl Chunk {
    pub fn metrics(&self) -> &MetricsRange {
        match self {
            Chunk::Frame(frame) => &frame.metrics,
            Chunk::Straightline(straightline) => &straightline,
            Chunk::Pause(pause) => &pause,
        }
    }
}

impl From<Frame> for Chunk {
    fn from(frame: Frame) -> Self {
        Chunk::Frame(frame)
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

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Trace {
    root: Frame,
    events: Vec<Event>,
}

impl Trace {
    // versioning is vX.Y.Z where vX.Y represent the trace format version
    // in trace serialization, first 16 bits are X, next 16 bits are Y,
    // and next 32 bits are VERSION_DELIMITER (all in big-endian)
    // after that, the actual trace data begins
    const VERSION_DELIMITER: u32 = 0xDEADBEEF;

    pub fn new(root: Frame, events: Vec<Event>) -> Self {
        Self { root, events }
    }

    pub fn root_frame(&self) -> &Frame {
        &self.root
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
        flexbuffers::from_slice(&decoded_data)
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
