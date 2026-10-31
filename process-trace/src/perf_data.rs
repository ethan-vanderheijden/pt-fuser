use std::{collections::HashMap, fmt::Display, fs::File, io::BufReader};

use linux_perf_data::{
    AuxtraceInfo, DsoKey, PerfFileReader, PerfFileRecord, UserRecord,
    linux_perf_event_reader::{EventRecord, PerfEventType},
};

use crate::{
    elf_image::{self, ElfImage},
    intel_pt::{Cpu, CpuVendor, Frequency, TraceMetadata, TscConversion},
};

const AUXTRACE_TYPE_INTEL_PT: u32 = 1;

#[derive(Debug)]
pub enum Error {
    Io(String, std::io::Error),
    PerfData(linux_perf_data::Error),
    BadCpuID(String),
    BadAuxtrace(String),
    ElfImage(elf_image::Error),
    NoAuxtraceInfo,
    BadFrequencyInfo(String),
}

impl Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Io(path, e) => write!(f, "{} - {}", path, e),
            Error::PerfData(e) => write!(f, "Perf data error: {}", e),
            Error::BadCpuID(msg) => write!(f, "Bad CPU ID: {}", msg),
            Error::BadAuxtrace(msg) => write!(f, "Bad auxtrace data: {}", msg),
            Error::ElfImage(e) => write!(f, "ELF image error: {}", e),
            Error::NoAuxtraceInfo => write!(f, "No auxtrace info found"),
            Error::BadFrequencyInfo(msg) => write!(f, "Bad frequency info: {}", msg),
        }
    }
}

impl std::error::Error for Error {}

#[derive(Debug)]
pub struct RawTrace {
    pub tid: u32,
    pub data: Vec<u8>,
    pub image: ElfImage,
}

struct IncompleteAuxtrace {
    data: Vec<u8>,
    cur_index: Option<u32>,
    cur_offset: u64,
    image: ElfImage,
}

impl IncompleteAuxtrace {
    fn new() -> Self {
        Self::new_with_image(ElfImage::new())
    }

    fn new_with_image(image: ElfImage) -> Self {
        IncompleteAuxtrace {
            data: Vec::new(),
            cur_index: None,
            cur_offset: 0,
            image,
        }
    }

    fn add_part(&mut self, index: u32, offset: u64, part_data: &[u8]) -> Result<(), Error> {
        if let Some(cur_index) = self.cur_index
            && cur_index != index
        {
            // thread switch to new CPU, buffer offset restarts
            self.cur_offset = 0;
        }
        self.cur_index = Some(index);

        if offset <= self.cur_offset {
            // Don't need to worry about checking offset
            // sometimes, Intel PT will pad the output stream with 0x00s
            // but 0x00 is a valid pad packet, so keeping them is fine
            self.data.extend_from_slice(part_data);
            self.cur_offset += part_data.len() as u64;
            Ok(())
        } else {
            Err(Error::BadAuxtrace("Auxtrace records must be read in order. We don't currently support out-of-order auxtrace records.".to_string()))
        }
    }

    fn finalize(self) -> (Vec<u8>, ElfImage) {
        (self.data, self.image)
    }
}

fn extract_cpu_info(cpu_string: &str) -> Result<Cpu, Error> {
    let parts = cpu_string.split(',').collect::<Vec<&str>>();
    if parts.len() != 4 {
        Err(Error::BadCpuID(format!(
            "Unexpected CPU ID format \"Vendor,Family,Model,Stepping\" but got: \"{}\"",
            cpu_string
        )))
    } else {
        let vendor = parts[0].to_string();
        let Ok(family) = parts[1].parse::<u16>() else {
            return Err(Error::BadCpuID(format!(
                "Failed to parse u16 CPU family from CPU ID string: \"{}\"",
                cpu_string
            )));
        };
        let Ok(model) = parts[2].parse::<u8>() else {
            return Err(Error::BadCpuID(format!(
                "Failed to parse u8 CPU model from CPU ID string: \"{}\"",
                cpu_string
            )));
        };
        let Ok(stepping) = parts[3].parse::<u8>() else {
            return Err(Error::BadCpuID(format!(
                "Failed to parse u8 CPU stepping from CPU ID string: \"{}\"",
                cpu_string
            )));
        };
        if vendor != "GenuineIntel" {
            Ok(Cpu::new(CpuVendor::UNKNOWN, family, model, stepping))
        } else {
            Ok(Cpu::new(CpuVendor::INTEL, family, model, stepping))
        }
    }
}

fn extract_freq_info(auxtrace_info: &AuxtraceInfo, pt_config: u64) -> Result<Frequency, Error> {
    let Some(mtc_period_mask) = auxtrace_info.mtc_freq else {
        return Err(Error::BadFrequencyInfo(
            "No MTC period mask in Auxtrace Info".to_string(),
        ));
    };
    let mtc_period_shift = mtc_period_mask.trailing_zeros();
    let mtc_period = (pt_config & mtc_period_mask) >> mtc_period_shift;

    let Some(max_nonturbo_ratio_mask) = auxtrace_info.max_nonturbo_ratio else {
        return Err(Error::BadFrequencyInfo(
            "No max non-turbo ratio mask in Auxtrace Info".to_string(),
        ));
    };

    let Some(tsc_ctc_numerator) = auxtrace_info.tsc_ctc_numerator else {
        return Err(Error::BadFrequencyInfo(
            "No TSC/CTC numerator in Auxtrace Info".to_string(),
        ));
    };

    let Some(tsc_ctc_denominator) = auxtrace_info.tsc_ctc_denominator else {
        return Err(Error::BadFrequencyInfo(
            "No TSC/CTC denominator in Auxtrace Info".to_string(),
        ));
    };

    Ok(Frequency::new(
        mtc_period as u8,
        max_nonturbo_ratio_mask as u8,
        tsc_ctc_numerator,
        tsc_ctc_denominator,
    ))
}

pub fn parse_perf_data(file_path: &str) -> Result<(TraceMetadata, Vec<RawTrace>), Error> {
    let file = File::open(file_path).map_err(|e| Error::Io(file_path.to_string(), e))?;
    let reader = BufReader::new(file);
    let PerfFileReader {
        mut perf_file,
        mut record_iter,
    } = PerfFileReader::parse_file(reader).map_err(Error::PerfData)?;

    let cpu_string = perf_file.cpu_id().map_err(Error::PerfData)?;
    let cpu = if let Some(cpu_string) = cpu_string {
        extract_cpu_info(cpu_string)?
    } else {
        return Err(Error::BadCpuID(
            "CPU ID information not found in perf.data".to_string(),
        ));
    };

    let mut kernel_image = ElfImage::new();
    let mut tid_to_parent: HashMap<u32, u32> = HashMap::new();
    let mut tid_auxtrace: HashMap<u32, IncompleteAuxtrace> = HashMap::new();
    let mut frequency_info = None;
    let mut tsc_info = None;

    while let Some(record) = record_iter
        .next_record(&mut perf_file)
        .map_err(Error::PerfData)?
    {
        match record {
            PerfFileRecord::UserRecord(user_record) => {
                match user_record
                    .parse()
                    .map_err(|e| Error::Io(file_path.to_string(), e))?
                {
                    UserRecord::Auxtrace(auxtrace) => {
                        let incomplete_aux = tid_auxtrace
                            .entry(auxtrace.tid)
                            .or_insert_with(IncompleteAuxtrace::new);
                        incomplete_aux.add_part(
                            auxtrace.index,
                            auxtrace.offset,
                            &auxtrace.aux_data.as_slice(),
                        )?;
                    }
                    UserRecord::AuxtraceInfo(info) if info.type_ == AUXTRACE_TYPE_INTEL_PT => {
                        for attr_desc in perf_file.event_attributes() {
                            if let PerfEventType::DynamicPmu(pmu_type, config, _, _) =
                                attr_desc.attr.type_
                                && info.pmu_type == pmu_type
                            {
                                frequency_info = Some(extract_freq_info(&info, config)?);
                                break;
                            }
                        }
                        tsc_info = Some(TscConversion {
                            time_shift: info.time_shift,
                            time_mult: info.time_mult,
                        });
                    }
                    _ => {}
                }
            }
            PerfFileRecord::EventRecord {
                attr_index: _,
                record,
            } => match record
                .parse()
                .map_err(|e| Error::Io(file_path.to_string(), e))?
            {
                EventRecord::Mmap2(mut mmap2) => {
                    let path = mmap2.path.read_string().unwrap_or(mmap2.path);
                    let path_slice = path.as_slice();
                    if let Some(dso_key) = DsoKey::detect(&path_slice, mmap2.cpu_mode) {
                        let target_image = if mmap2.pid > 0 {
                            &mut tid_auxtrace
                                .entry(mmap2.tid as u32)
                                .or_insert_with(IncompleteAuxtrace::new)
                                .image
                        } else {
                            // pid=-1 means the mapping applied to all threads
                            &mut kernel_image
                        };
                        target_image
                            .add_mapping(
                                mmap2.address,
                                mmap2.page_offset,
                                mmap2.length as usize,
                                dso_key,
                            )
                            .map_err(Error::ElfImage)?;
                    }
                }
                EventRecord::Comm(comm) if comm.pid > 0 && comm.tid > 0 && comm.pid != comm.tid => {
                    tid_to_parent.insert(comm.tid as u32, comm.pid as u32);
                }
                _ => {}
            },
        }
    }

    let Some(frequency_info) = frequency_info else {
        return Err(Error::NoAuxtraceInfo);
    };

    let Some(tsc_info) = tsc_info else {
        return Err(Error::NoAuxtraceInfo);
    };

    for tid in tid_auxtrace.keys().cloned().collect::<Vec<_>>() {
        if let Some(parent_tid) = tid_to_parent.get(&tid)
            && tid_auxtrace.contains_key(parent_tid)
        {
            let [Some(parent), Some(child)] = tid_auxtrace.get_disjoint_mut([parent_tid, &tid])
            else {
                unreachable!("Both keys must exist");
            };
            child.image.copy_mappings(&parent.image);
        }
    }

    let mut final_traces = Vec::new();
    for (tid, aux) in tid_auxtrace.into_iter() {
        let (finalized, mut image) = aux.finalize();
        if !finalized.is_empty() {
            image.copy_mappings(&kernel_image);
            final_traces.push(RawTrace {
                tid,
                data: finalized,
                image,
            });
        }
    }
    Ok((
        TraceMetadata {
            cpu_info: cpu,
            frequency_info,
            tsc_info,
        },
        final_traces,
    ))
}
