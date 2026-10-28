pub mod builder;
pub mod frame;
pub mod metrics;
pub mod trace_error;

#[cfg(test)]
mod test;

use std::{
    fmt::Display,
    io::{Read, Write},
    sync::Arc,
};

use bincode_next::config;
use bitflags::bitflags;
use serde::{Deserialize, Serialize};

use crate::trace::{
    frame::{Frame, FrameSerializedFormat},
    metrics::{Metrics, MetricsRange},
};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SymbolInfo {
    pub name: String,
    pub vaddr_start: u64,
    pub size: u64,
}

impl SymbolInfo {
    pub fn contains(&self, vaddr: u64) -> bool {
        self.vaddr_start <= vaddr && vaddr < self.vaddr_start + self.size
    }
}

impl Display for SymbolInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "[0x{:x} - 0x{:x}] {}",
            self.vaddr_start,
            self.vaddr_start + self.size,
            self.name
        )
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

bitflags! {
    struct TraceFlags: u32 {
        const COMPRESSED = 0b1;
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
    //
    // The vX.Y version and flags are stored at the beginning of the trace file:
    // (are items are stored in big-endian format)
    //
    //  0 bits                         32 bits
    //    |--------------------------------|
    //    |     X (16)     |    Y (16)     |
    //    +--------------------------------+
    //    |        VERSION_DELIMITER       |
    //    |            (32 bits)           |
    //    +--------------------------------+
    //    |             Flags              |
    //    |           (32 bits)            |
    //    +--------------------------------+
    //    |          FLAG_DELIMITER        |
    //    |           (32 bits)            |
    //    +--------------------------------+
    //    |             Data               |
    //    |        (remaining bits)        |
    //    |              ...               |
    //    v                                v
    const VERSION_DELIMITER: u32 = 0xDEADBEEF;
    const FLAG_DELIMITER: u32 = 0xFEEDFACE;

    pub const RECOMMENDED_EXTENSION: &'static str = "bin.zst";

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

    fn bincode_config() -> config::Configuration {
        config::standard()
    }

    pub fn bin_serialize(&self, writer: &mut impl Write, compress: bool) -> Result<(), Error> {
        let (major, minor) =
            Self::get_major_minor().map_err(|s| Error::Serialization(s.to_string()))?;
        let mut flags = TraceFlags::empty();
        flags.set(TraceFlags::COMPRESSED, compress);

        let data = major
            .to_be_bytes()
            .iter()
            .chain(minor.to_be_bytes().iter())
            .chain(Self::VERSION_DELIMITER.to_be_bytes().iter())
            .chain(flags.bits().to_be_bytes().iter())
            .chain(Self::FLAG_DELIMITER.to_be_bytes().iter())
            .copied()
            .collect::<Vec<u8>>();
        writer.write_all(&data).map_err(Error::Io)?;

        if compress {
            let mut zstd_compressor = zstd::stream::Encoder::new(writer, 0).map_err(Error::Io)?;
            bincode_next::serde::encode_into_std_write(
                self,
                &mut zstd_compressor,
                Self::bincode_config(),
            )
            .map_err(Error::BincodeEncode)?;
            let remaining_writer = zstd_compressor.finish().map_err(Error::Io)?;
            remaining_writer.flush().map_err(Error::Io)?;
        } else {
            bincode_next::serde::encode_into_std_write(self, writer, Self::bincode_config())
                .map_err(Error::BincodeEncode)?;
            writer.flush().map_err(Error::Io)?;
        }
        Ok(())
    }

    pub fn bin_deserialize(reader: &mut impl Read) -> Result<Self, Error> {
        let mut header = [0u8; 16];
        reader.read_exact(&mut header).map_err(Error::Io)?;

        let (tool_major, tool_minor) =
            Self::get_major_minor().map_err(|s| Error::Deserialization(s.to_string()))?;
        let trace_major = u16::from_be_bytes([header[0], header[1]]);
        let trace_minor = u16::from_be_bytes([header[2], header[3]]);
        let version_delimiter = u32::from_be_bytes([header[4], header[5], header[6], header[7]]);
        let flags = TraceFlags::from_bits_truncate(u32::from_be_bytes([
            header[8], header[9], header[10], header[11],
        ]));
        let flag_delimiter = u32::from_be_bytes([header[12], header[13], header[14], header[15]]);

        if trace_major != tool_major || trace_minor != tool_minor {
            return Err(Error::Deserialization(format!(
                "Version mismatch: trace data is v{}.{} but tool is v{}.{}",
                trace_major, trace_minor, tool_major, tool_minor
            )));
        }
        if version_delimiter != Self::VERSION_DELIMITER {
            return Err(Error::Deserialization(
                "Trace data is corrupted, version delimiter is incorrect!".to_string(),
            ));
        }
        if flag_delimiter != Self::FLAG_DELIMITER {
            return Err(Error::Deserialization(
                "Trace data is corrupted, flag delimiter is incorrect!".to_string(),
            ));
        }

        let trace_format: TraceSerializedFormat = if flags.contains(TraceFlags::COMPRESSED) {
            let mut zstd_decompressor = zstd::stream::Decoder::new(reader).map_err(Error::Io)?;
            bincode_next::serde::decode_from_std_read(
                &mut zstd_decompressor,
                Self::bincode_config(),
            )
            .map_err(Error::BincodeDecode)?
        } else {
            bincode_next::serde::decode_from_std_read(reader, Self::bincode_config())
                .map_err(Error::BincodeDecode)?
        };
        trace_format.into_trace()
    }
}

// helper struct that mirrors the wire format of a trace, only for internal use when deserializing
#[derive(Deserialize)]
struct TraceSerializedFormat {
    symbols: Vec<SymbolInfo>,
    root: FrameSerializedFormat,
    events: Vec<Event>,
}

impl TraceSerializedFormat {
    fn into_trace(self) -> Result<Trace, Error> {
        let symbols: Vec<Arc<SymbolInfo>> = self.symbols.into_iter().map(Arc::new).collect();
        let converted_root = self.root.into_frame(&symbols)?;
        Ok(Trace {
            symbols,
            root: converted_root,
            events: self.events,
        })
    }
}

#[derive(Debug)]
pub enum Error {
    Io(std::io::Error),
    Serialization(String),
    Deserialization(String),
    BincodeEncode(bincode_next::error::EncodeError),
    BincodeDecode(bincode_next::error::DecodeError),
    InvalidRange(MetricsRange),
    NotSorted,
}

impl Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Io(e) => write!(f, "I/O error: {}", e),
            Error::Serialization(e) => write!(f, "Serialization error: {}", e),
            Error::Deserialization(e) => write!(f, "Deserialization error: {}", e),
            Error::BincodeEncode(e) => write!(f, "Bincode encode error: {}", e),
            Error::BincodeDecode(e) => write!(f, "Bincode decode error: {}", e),
            Error::InvalidRange(range) => write!(f, "Invalid range: {}", range),
            Error::NotSorted => write!(f, "Occurences are not sorted by timestamp"),
        }
    }
}
