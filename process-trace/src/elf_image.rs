use std::{fmt::Display, fs::File, ops::Range, sync::OnceLock};

use blazesym::symbolize::{
    Input, Symbolized, Symbolizer,
    source::{self, Source},
};
use goblin::elf::Elf;
use linux_perf_data::DsoKey;
use memmap2::Mmap;
use pt_fuser::trace::SymbolInfo;
use tracing::{info, warn};

use crate::{intel_pt::ImageProvider, symbol_cache::SymbolCache};

// for now, only support x86_64 PLT stubs since only x86_64 has Intel PT
const X86_64_PLT0_SIZE: u64 = 0x10;
const X86_64_PLT_STUB_SIZE: u64 = 0x10;

static VMLINUX_PATH: OnceLock<Result<String, String>> = OnceLock::new();

pub fn find_vmlinux() -> Result<&'static str, &'static str> {
    let s = VMLINUX_PATH.get_or_init(|| {
        let uname = match uname::uname() {
            Ok(res) => res,
            Err(e) => {
                return Err(format!("Failed to get 'uname' information: {}", e));
            }
        };

        let candidates = [
            format!("/boot/vmlinux-{}", uname.release),
            format!("/usr/lib/debug/boot/vmlinux-{}", uname.release),
            format!("/lib/modules/{}/build/vmlinux", uname.release),
            format!("/usr/lib/debug/lib/modules/{}/vmlinux", uname.release),
            format!("/usr/lib/debug/boot/vmlinux-{}.debug", uname.release),
        ];
        let mut failures = String::new();
        for path in candidates {
            match File::open(&path) {
                Ok(_) => return Ok(path),
                Err(e) => failures.push_str(&format!("\n  {} - {}", path, e)),
            }
        }

        Err(format!(
            "Could not find vmlinux in any of the following paths:{}",
            failures
        ))
    });
    s.as_ref().map(String::as_str).map_err(String::as_str)
}

fn demangle_symbol_name(name: &str) -> String {
    if let Ok(demangled) = rustc_demangle::try_demangle(name) {
        demangled.to_string()
    } else if let Ok(Ok(demangled)) = cpp_demangle::Symbol::new(name).map(|s| s.demangle()) {
        demangled
    } else {
        name.to_string()
    }
}

#[derive(Debug)]
pub enum Error {
    Io(String, std::io::Error),
    BadMapping(String),
    ElfParse(goblin::error::Error),
    MappingRead(String),
    SymbolLookup(blazesym::Error),
    SymbolNotFound(blazesym::symbolize::Reason),
}

impl Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Io(path, e) => write!(f, "{} - {}", path, e),
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
    fn new(elf: &Elf) -> Result<Self, Error> {
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
    mmap: Mmap,
    // the part of `mmap` that backs the mapping
    code_range: Range<usize>,
    // some ELFs (e.g., shared libraries) won't have PLT stubs
    plt_lookup: Option<PltLookup>,
    symbolizer: Symbolizer,
    symbolizer_source: Source<'static>,
}

#[derive(Debug)]
struct Mapping {
    vaddr: u64,
    file_offset: u64,
    size: usize,
    path: DsoKey,
    elf: Option<ElfInfo>,
    // set when prepare_elf failed, so we don't re-open and re-parse the file on every access
    prepare_failed: bool,
}

impl Mapping {
    fn new(vaddr: u64, file_offset: u64, size: usize, path: DsoKey) -> Self {
        Mapping {
            vaddr,
            file_offset,
            size,
            path,
            elf: None,
            prepare_failed: false,
        }
    }

    /// Makes sure `self.elf` is populated, preparing it on first use.
    fn ensure_elf(&mut self) -> Result<(), Error> {
        if self.elf.is_some() {
            return Ok(());
        }
        if self.prepare_failed {
            return Err(Error::MappingRead(format!(
                "ELF for mapping {:?} could not be prepared earlier",
                self.path
            )));
        }
        match self.prepare_elf() {
            Ok(()) => Ok(()),
            Err(e) => {
                self.prepare_failed = true;
                Err(e)
            }
        }
    }

    fn vaddr_end(&self) -> u64 {
        self.vaddr + self.size as u64
    }

    fn overlaps(&self, other: &Mapping) -> bool {
        self.vaddr < other.vaddr_end() && other.vaddr < self.vaddr_end()
    }

    /// Reads the ELF and prepares the ElfInfo by mapping the ELF into memory
    /// and preparing the PltLookup.
    fn prepare_elf(&mut self) -> Result<(), Error> {
        let filesystem_path = match &self.path {
            DsoKey::User {
                file_name,
                full_path,
            } => str::from_utf8(full_path).map_err(|e| {
                Error::MappingRead(format!(
                    "Failed to read full path of mapping {} as UTF-8: {}",
                    file_name, e
                ))
            }),
            DsoKey::Kernel => find_vmlinux().map_err(|_| {
                Error::MappingRead("Could not find vmlinux for kernel mapping".to_string())
            }),
            _ => Err(Error::MappingRead(format!(
                "Unsupported file mapping type: {:?}",
                self.path
            ))),
        }?;

        let file =
            File::open(filesystem_path).map_err(|e| Error::Io(filesystem_path.to_string(), e))?;
        let mmap =
            unsafe { Mmap::map(&file).map_err(|e| Error::Io(filesystem_path.to_string(), e))? };
        let elf = Elf::parse(&mmap).map_err(Error::ElfParse)?;

        let mut read_offset = self.file_offset as usize;
        if read_offset + self.size > mmap.len()
            && matches!(self.path, DsoKey::Kernel)
            && let Some(text_section) = elf
                .section_headers
                .iter()
                .find(|sh| elf.shdr_strtab.get_at(sh.sh_name) == Some(".text"))
        {
            // for some reason, the PERF_RECORD_MMAP2 file offset for kernel mappings
            // is set to the virtual address offset, which is wildly off
            // it seems to work if we assume the actual file offset is the .text section
            read_offset = text_section.sh_offset as usize;
            self.file_offset = read_offset as u64;
        }

        if read_offset + self.size > mmap.len() {
            return Err(Error::MappingRead(format!(
                "When preparing ELF {}, requested bytes 0x{:x} - 0x{:x} but file size is only 0x{:x}",
                filesystem_path,
                read_offset,
                read_offset + self.size,
                mmap.len()
            )));
        }
        let code_range = read_offset..read_offset + self.size;

        let symbolizer = Symbolizer::builder()
            .enable_auto_reload(false)
            .enable_code_info(false)
            .enable_inlined_fns(false)
            .build();
        let symbolizer_source = Source::Elf(source::Elf::new(filesystem_path));

        let plt_lookup = match PltLookup::new(&elf) {
            Ok(lookup) => Some(lookup),
            Err(e) => {
                warn!(
                    "ELF file {} can't parse the PLT stubs: {}",
                    filesystem_path, e
                );
                None
            }
        };

        self.elf = Some(ElfInfo {
            mmap,
            code_range,
            plt_lookup,
            symbolizer,
            symbolizer_source,
        });

        Ok(())
    }

    fn read(&mut self, buffer: &mut [u8], vaddr: u64) -> Result<usize, Error> {
        let buffer_offset = (vaddr - self.vaddr) as usize;
        if buffer_offset >= self.size {
            return Err(Error::MappingRead(format!(
                "For ELF {:?}, requested {} bytes from offset 0x{:x} but mapping is only 0x{:x}-0x{:x}",
                self.path,
                buffer.len(),
                buffer_offset,
                self.vaddr,
                self.vaddr_end()
            )));
        }

        self.ensure_elf()?;
        let elf = self.elf.as_ref().unwrap();
        // a read may run past the end of the mapping, in which case it is truncated
        let len = buffer.len().min(self.size - buffer_offset);
        let code = &elf.mmap[elf.code_range.clone()];
        buffer[..len].copy_from_slice(&code[buffer_offset..buffer_offset + len]);
        Ok(len)
    }

    fn resolve_symbol(&mut self, vaddr: u64) -> Result<SymbolInfo, Error> {
        self.ensure_elf()?;
        // must come after ensure_elf, which may adjust self.file_offset
        let file_offset = self.file_offset + (vaddr - self.vaddr);
        let elf = self.elf.as_mut().unwrap();
        if let Some(lookup) = &elf.plt_lookup
            && let Some(plt_sym) = lookup.lookup(file_offset)
        {
            let mut demangled = demangle_symbol_name(plt_sym);
            demangled.push_str("@plt");

            let aligned_vaddr = vaddr & !(X86_64_PLT_STUB_SIZE - 1);
            Ok(SymbolInfo {
                name: demangled,
                vaddr_start: aligned_vaddr,
                size: X86_64_PLT_STUB_SIZE,
            })
        } else {
            let resolved = elf
                .symbolizer
                .symbolize_single(&elf.symbolizer_source, Input::FileOffset(file_offset))
                .map_err(Error::SymbolLookup)?;

            match resolved {
                Symbolized::Sym(sym) => {
                    // represents start of symbol in virtual address space
                    let virtual_addr_start = vaddr - sym.offset as u64;
                    let demangled = demangle_symbol_name(sym.name.as_ref());

                    Ok(SymbolInfo {
                        name: demangled,
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
    symbol_cache: SymbolCache,
    mappings: Vec<Mapping>,
}

impl ElfImage {
    pub fn new() -> Self {
        ElfImage {
            symbol_cache: SymbolCache::new(),
            mappings: Vec::new(),
        }
    }

    pub fn add_mapping(
        &mut self,
        vaddr: u64,
        file_offset: u64,
        size: usize,
        path: DsoKey,
    ) -> Result<(), Error> {
        info!(
            "Adding mapping: vaddr=0x{:x} to 0x{:x}, file_offset=0x{:x}, path={:?}",
            vaddr,
            vaddr + size as u64,
            file_offset,
            path
        );
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
        if let Some(symbol_info) = self.symbol_cache.get(vaddr) {
            return Ok(symbol_info.clone());
        }

        if let Some(mapping) = self.lookup_mapping(vaddr) {
            let new_sym = mapping.resolve_symbol(vaddr)?;
            self.symbol_cache.insert(new_sym.clone());
            Ok(new_sym)
        } else {
            Err(Error::MappingRead(format!(
                "No mapping found for requested offset 0x{:x}",
                vaddr
            )))
        }
    }
}
