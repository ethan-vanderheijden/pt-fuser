use std::{fmt::Display, fs::File};

use blazesym::symbolize::{
    Input, Symbolized, Symbolizer,
    source::{self, Source},
};
use goblin::elf::Elf;
use memmap2::Mmap;
use pt_fuser::trace::SymbolInfo;
use tracing::warn;

use crate::{elf_image::Error::ElfParse, intel_pt::ImageProvider};

// for now, only support x86_64 PLT stubs since only x86_64 has Intel PT
const X86_64_PLT0_SIZE: u64 = 0x10;
const X86_64_PLT_STUB_SIZE: u64 = 0x10;

#[derive(Debug)]
pub enum Error {
    Io(std::io::Error),
    BadMapping(String),
    ElfParse(goblin::error::Error),
    MappingRead(String),
    SymbolLookup(blazesym::Error),
    SymbolNotFound(blazesym::symbolize::Reason),
}

impl Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Io(e) => write!(f, "I/O error: {}", e),
            Error::BadMapping(msg) => write!(f, "Bad mapping: {}", msg),
            Error::ElfParse(e) => write!(f, "ELF parse error: {}", e),
            Error::MappingRead(msg) => write!(f, "Mapping read error: {}", msg),
            Error::SymbolLookup(e) => write!(f, "Symbol lookup error: {}", e),
            Error::SymbolNotFound(reason) => write!(f, "Symbol not found: {:?}", reason),
        }
    }
}

impl std::error::Error for Error {}

#[derive(Debug)]
struct PltLookup {
    file_base: u64,
    size: u64,
    entries: Vec<String>,
}

impl PltLookup {
    fn new(elf_data: &[u8]) -> Result<Self, Error> {
        let elf = Elf::parse(elf_data).map_err(ElfParse)?;
        if !elf.is_64 {
            return Err(Error::MappingRead(
                "Currently, we only support resolving PLT stub names for x86_64 architecture"
                    .to_string(),
            ));
        }

        let Some(plt) = elf
            .section_headers
            .iter()
            .find(|sh| elf.shdr_strtab.get_at(sh.sh_name) == Some(".plt"))
        else {
            return Err(Error::MappingRead(
                "ELF file does not contain a .plt section".to_string(),
            ));
        };

        let file_base = plt.sh_offset;
        let size = plt.sh_size;

        let mut reloc_names = Vec::with_capacity(elf.pltrelocs.len());
        for reloc in &elf.pltrelocs {
            let sym = elf
                .dynsyms
                .get(reloc.r_sym)
                .and_then(|sym| elf.dynstrtab.get_at(sym.st_name))
                .unwrap_or("<unknown>")
                .to_owned();
            reloc_names.push(sym);
        }

        Ok(Self {
            file_base,
            size,
            entries: reloc_names,
        })
    }

    fn lookup(&self, file_offset: u64) -> Option<&str> {
        let aligned_offset = file_offset & !(X86_64_PLT_STUB_SIZE - 1);
        // first few bytes aren't a valid PLT stub
        if aligned_offset < self.file_base + X86_64_PLT0_SIZE
            || aligned_offset >= self.file_base + self.size
        {
            return None;
        }

        // there is a 1-to-1 mapping between PLT stub indices and relocation entry indices
        let index = (aligned_offset - self.file_base - X86_64_PLT0_SIZE) / X86_64_PLT_STUB_SIZE;
        self.entries.get(index as usize).map(|s| s.as_str())
    }
}

#[derive(Debug)]
struct ElfInfo {
    cached_binary: Vec<u8>,
    // some ELFs (e.g., shared libraries) won't have PLT stubs
    plt_lookup: Option<PltLookup>,
    symbolizer: Symbolizer,
}

#[derive(Debug)]
struct Mapping {
    vaddr: u64,
    file_offset: u64,
    size: usize,
    path: String,
    elf: Option<ElfInfo>,
}

impl Mapping {
    fn new(vaddr: u64, file_offset: u64, size: usize, path: String) -> Self {
        Mapping {
            vaddr,
            file_offset,
            size,
            path,
            elf: None,
        }
    }

    fn vaddr_end(&self) -> u64 {
        self.vaddr + self.size as u64
    }

    fn overlaps(&self, other: &Mapping) -> bool {
        self.vaddr < other.vaddr_end() && other.vaddr < self.vaddr_end()
    }

    /// Reads the ELF and prepares the ElfInfo by reading a byte section of the ELF
    /// into a buffer and preparing the PltLookup.
    fn prepare_elf(&mut self) -> Result<(), Error> {
        if self.path.starts_with("[") && self.path.ends_with("]") {
            return Err(Error::MappingRead(format!(
                "Cannot read from special mapping {} (vaddr: 0x{:x}, size: {})",
                self.path, self.vaddr, self.size
            )));
        }

        let file = File::open(&self.path).map_err(Error::Io)?;
        let mmap = unsafe { Mmap::map(&file).map_err(Error::Io)? };

        let read_offset = self.file_offset as usize;
        if read_offset + self.size > mmap.len() {
            return Err(Error::MappingRead(format!(
                "For ELF {}, requested {} bytes from offset {} but file size is {} bytes",
                self.path,
                self.size,
                read_offset,
                mmap.len()
            )));
        }

        let cached_binary = mmap[read_offset..read_offset + self.size].to_vec();
        let symbolizer = Symbolizer::builder()
            .enable_auto_reload(false)
            .enable_code_info(false)
            .enable_inlined_fns(false)
            .enable_demangling(true)
            .build();
        self.elf = match PltLookup::new(&mmap) {
            Ok(plt_lookup) => Some(ElfInfo {
                cached_binary,
                plt_lookup: Some(plt_lookup),
                symbolizer,
            }),
            Err(e) => {
                warn!("ELF file {} can't parse the PLT stubs: {}", self.path, e);
                Some(ElfInfo {
                    cached_binary,
                    plt_lookup: None,
                    symbolizer,
                })
            }
        };
        Ok(())
    }

    fn read(&mut self, buffer: &mut [u8], vaddr: u64) -> Result<usize, Error> {
        let buffer_offset = (vaddr - self.vaddr) as usize;
        if buffer_offset >= self.size {
            return Err(Error::MappingRead(format!(
                "For ELF {}, requested {} bytes from offset {} but mapped size is {} bytes",
                self.path,
                buffer.len(),
                buffer_offset,
                self.size
            )));
        }

        if self.elf.is_none() {
            self.prepare_elf()?;
        }

        let elf = self.elf.as_ref().unwrap();
        let mut buffer_end = buffer_offset + buffer.len();
        if buffer_end > self.size {
            buffer_end = self.size;
        }
        buffer.copy_from_slice(&elf.cached_binary[buffer_offset..buffer_end]);
        Ok(buffer_end - buffer_offset)
    }

    fn resolve_symbol(&mut self, vaddr: u64) -> Result<SymbolInfo, Error> {
        if self.elf.is_none() {
            self.prepare_elf()?;
        }
        let elf = self.elf.as_ref().unwrap();

        let file_offset = self.file_offset + (vaddr - self.vaddr);
        if let Some(lookup) = &elf.plt_lookup
            && let Some(plt_sym) = lookup.lookup(file_offset)
        {
            let sym_name = if let Ok(Ok(mut demangled)) =
                cpp_demangle::Symbol::new(plt_sym).map(|s| s.demangle())
            {
                demangled.push_str("@plt");
                demangled
            } else if let Ok(demangled) = rustc_demangle::try_demangle(plt_sym) {
                format!("{}@plt", demangled)
            } else {
                format!("{}@plt", plt_sym)
            };
            let aligned_vaddr = vaddr & !(X86_64_PLT_STUB_SIZE - 1);
            Ok(SymbolInfo {
                name: sym_name,
                vaddr_start: aligned_vaddr,
                size: X86_64_PLT_STUB_SIZE,
            })
        } else {
            let source = Source::Elf(source::Elf::new(&self.path));
            let resolved = elf
                .symbolizer
                .symbolize_single(&source, Input::FileOffset(file_offset))
                .map_err(Error::SymbolLookup)?;

            match resolved {
                Symbolized::Sym(sym) => {
                    // represents start of symbol in virtual address space
                    let virtual_addr_start = vaddr - sym.offset as u64;
                    Ok(SymbolInfo {
                        name: sym.name.to_string(),
                        vaddr_start: virtual_addr_start,
                        size: sym.size.unwrap_or(0) as u64,
                    })
                }
                Symbolized::Unknown(reason) => Err(Error::SymbolNotFound(reason)),
            }
        }
    }
}

#[derive(Debug)]
pub struct ElfImage {
    mappings: Vec<Mapping>,
}

impl ElfImage {
    pub fn new() -> Self {
        ElfImage {
            mappings: Vec::new(),
        }
    }

    pub fn add_mapping(
        &mut self,
        vaddr: u64,
        file_offset: u64,
        size: usize,
        path: String,
    ) -> Result<(), Error> {
        let new_mapping = Mapping::new(vaddr, file_offset, size, path);
        for mapping in &self.mappings {
            if mapping.overlaps(&new_mapping) {
                return Err(Error::BadMapping(format!(
                    "PERF_RECORD_MMAP2 defines a mapping ({} - {}) that overlaps with an existing mapping",
                    new_mapping.vaddr,
                    new_mapping.vaddr_end(),
                )));
            }
        }
        let pos = self
            .mappings
            .partition_point(|m| m.vaddr < new_mapping.vaddr);
        self.mappings.insert(pos, new_mapping);
        Ok(())
    }

    pub fn copy_mappings(&mut self, other: &ElfImage) {
        for mapping in &other.mappings {
            let _ = self.add_mapping(
                mapping.vaddr,
                mapping.file_offset,
                mapping.size,
                mapping.path.clone(),
            );
        }
    }

    fn lookup_mapping(&mut self, vaddr: u64) -> Option<&mut Mapping> {
        self.mappings
            .binary_search_by(|m| {
                if m.vaddr > vaddr {
                    std::cmp::Ordering::Greater
                } else if m.vaddr_end() <= vaddr {
                    std::cmp::Ordering::Less
                } else {
                    std::cmp::Ordering::Equal
                }
            })
            .ok()
            .map(|idx| &mut self.mappings[idx])
    }
}

impl ImageProvider for ElfImage {
    type ReadError = Error;
    type SymbolError = Error;

    fn read_chunk(&mut self, buffer: &mut [u8], mut vaddr: u64) -> Result<usize, Self::ReadError> {
        let mut read = 0;
        while read < buffer.len()
            && let Some(mapping) = self.lookup_mapping(vaddr)
        {
            let bytes = mapping.read(&mut buffer[read..], vaddr)?;
            read += bytes;
            vaddr += bytes as u64;
        }

        if read == 0 {
            Err(Error::MappingRead(format!(
                "No mapping found for requested offset 0x{:x}",
                vaddr
            )))
        } else {
            Ok(read)
        }
    }

    fn resolve_symbol(&mut self, vaddr: u64) -> Result<SymbolInfo, Self::SymbolError> {
        if let Some(mapping) = self.lookup_mapping(vaddr) {
            mapping.resolve_symbol(vaddr)
        } else {
            Err(Error::MappingRead(format!(
                "No mapping found for requested offset 0x{:x}",
                vaddr
            )))
        }
    }
}
