mod perf;

use pt_fuser::trace::{
    Metrics, SymbolInfo, Trace,
    builder::{BuilderResult, TraceBuilder},
};
use regex::Regex;
use std::{
    collections::HashMap,
    fs,
    os::raw::{c_char, c_int, c_void},
    path::Path,
};
use tracing::{Level, error, info};

const USAGE: &str = "Usage: --dlarg <symbol regex> --dlarg <output_dir>";
const SHORT_DESC: &core::ffi::CStr = c"Parses an Intel PT trace into our internal format for later aggregating, comparing, and exporting.";
const LONG_DESC: &core::ffi::CStr = c"Usage: --dlarg <symbol regex> --dlarg <output_dir>. \
    Only processes trace data for a function matching the given regex and all its sub-calls. \
    Each function invocation is a separate trace file written into the output directory as trace-<tid>-<n>.flex.gz";

#[unsafe(no_mangle)]
pub static mut perf_dlfilter_fns: std::mem::MaybeUninit<perf::perf_dlfilter_fns> =
    std::mem::MaybeUninit::<perf::perf_dlfilter_fns>::uninit();

/// Resolves the symbol name based on the .addr field of the sample.
/// For branch events, .addr is the address of the branch target.
unsafe fn resolve_addr<'a>(sample: &perf::perf_dlfilter_sample, ctx: *mut c_void) -> SymbolInfo {
    if sample.addr_correlates_sym != 0 {
        unsafe {
            let raw_symbol = perf_dlfilter_fns.assume_init().resolve_addr.unwrap()(ctx);
            if !(*raw_symbol).sym.is_null() {
                let symbol = std::ffi::CStr::from_ptr((*raw_symbol).sym);
                return SymbolInfo {
                    name: symbol.to_str().unwrap_or("Non UTF-8 symbol").to_string(),
                    offset: (*raw_symbol).sym_start,
                    size: (*raw_symbol).sym_end - (*raw_symbol).sym_start,
                };
            }
        }
    }
    SymbolInfo {
        name: format!("[ 0x{:x} ]", sample.addr),
        offset: 0,
        size: 0,
    }
}

unsafe fn resolve_ip<'a>(
    sample: &perf::perf_dlfilter_sample,
    ctx: *mut c_void,
    buf: &'a mut [u8],
) -> SymbolInfo {
    unsafe {
        let raw_symbol = perf_dlfilter_fns.assume_init().resolve_ip.unwrap()(ctx);
        if !(*raw_symbol).sym.is_null() {
            let symbol = std::ffi::CStr::from_ptr((*raw_symbol).sym);
            return SymbolInfo {
                name: symbol.to_str().unwrap_or("Non UTF-8 symbol").to_string(),
                offset: (*raw_symbol).sym_start,
                size: (*raw_symbol).sym_end - (*raw_symbol).sym_start,
            };
        }
    }
    SymbolInfo {
        name: format!("[ 0x{:x} ]", sample.ip),
        offset: 0,
        size: 0,
    }
}

struct State {
    sym_regex: Regex,
    output_dir: String,
    trace_nums: HashMap<i32, u32>,
    builders: HashMap<i32, TraceBuilder>,
    insn_cnt: u64,
    cyc_cnt: u64,
}

fn export_trace(state: &mut State, tid: i32, trace: Trace) {
    let trace_num = state.trace_nums.entry(tid).or_insert(0);
    let filename = format!("trace-{}-{}.bin", tid, trace_num);
    *trace_num += 1;

    let path = Path::new(&state.output_dir).join(filename);
    let binary_encoded = trace
        .bin_serialize(true)
        .expect("Failed to binary encode trace");
    fs::write(path, binary_encoded).expect("Failed to write trace file");
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn filter_event_early(
    raw_state: *mut c_void,
    sample: &perf::perf_dlfilter_sample,
    ctx: *mut c_void,
) -> c_int {
    let state = unsafe { raw_state.cast::<State>().as_mut().unwrap() };

    // each branch instruction is processed first as 'i' then as 'b'
    // so the instruction is added to the parent's insn_cnt and cyc_cnt will be up to date

    // Note: updating cycle count every 'i' event is most reliable since 'i' events that don't represent
    // taken branches may still produce a CYC packet. Conversely, cycle count for 'b' events is only
    // updated when a CYC packet happens to be produced for that 'b' event, which is a matter of luck.
    let event_ltr = unsafe { *sample.event as u8 as char };
    if event_ltr == 'i' {
        state.insn_cnt += 1;
        state.cyc_cnt += sample.cyc_cnt;
        if let Some(builder) = state.builders.get(&sample.tid) {
        }
    } else if event_ltr == 'b' {
        if sample.ip == 0 {
            panic!("Branch event with ip=0. Probably tr end");
        }
        if sample.flags & perf::PERF_DLFILTER_FLAG_CALL != 0 {
            let symbol = unsafe { resolve_addr(sample, ctx) };
            if let Some(builder) = state.builders.get_mut(&sample.tid) {
                info!(
                    "Adding frame for tid: {}, symbol: {}. Callstack depth: {}",
                    sample.tid,
                    symbol.name,
                    builder.callstack_depth()
                );
                builder
                    .push_frame(
                        Metrics {
                            ts: sample.time,
                            cycles: state.cyc_cnt,
                            insn_count: state.insn_cnt,
                        },
                        symbol,
                    )
                    .expect("Failed to push new stack frame");
            } else {
                if state.sym_regex.is_match(&symbol.name) {
                    info!(
                        "Starting trace for tid: {}, symbol: {}",
                        sample.tid, symbol.name
                    );
                    state.builders.insert(
                        sample.tid,
                        TraceBuilder::new(
                            Metrics {
                                ts: sample.time,
                                cycles: state.cyc_cnt,
                                insn_count: state.insn_cnt,
                            },
                            symbol,
                        ),
                    );
                }
            }
        } else if sample.flags & perf::PERF_DLFILTER_FLAG_RETURN != 0
            && let Some(builder) = state.builders.remove(&sample.tid)
        {
            match builder
                .complete_frame(Metrics {
                    ts: sample.time,
                    cycles: state.cyc_cnt,
                    insn_count: state.insn_cnt,
                })
                .expect("Failed to complete stack frame")
            {
                BuilderResult::Completed(trace) => {
                    info!("Completed trace for tid: {}", sample.tid);
                    export_trace(state, sample.tid, trace);
                }
                BuilderResult::Builder(builder) => {
                    info!(
                        "Popped frame for tid: {}. Callstack depth: {}",
                        sample.tid,
                        builder.callstack_depth()
                    );
                    state.builders.insert(sample.tid, builder);
                }
            }
        }
    }
    1
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn start(raw_state: &mut *mut c_void, ctx: *mut c_void) -> c_int {
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(Level::INFO)
        .finish();
    tracing::subscriber::set_global_default(subscriber).unwrap();

    let mut argc: c_int = 0;
    let args;
    unsafe {
        let argv = (perf_dlfilter_fns.assume_init().args.unwrap())(ctx, &mut argc as *mut c_int);
        if argv.is_null() {
            error!("Failed to retrieve dlargs. {}", USAGE);
            return -1;
        }
        args = std::slice::from_raw_parts(argv, argc as usize);
    }

    if argc != 2 {
        error!("Expected two arguments. {}", USAGE);
        return -1;
    }

    let arg1 = unsafe { std::ffi::CStr::from_ptr(args[0]).to_bytes() };
    let arg2 = unsafe { std::ffi::CStr::from_ptr(args[1]).to_bytes() };

    let arg1_string = String::from_utf8(arg1.to_vec()).expect("Invalid UTF-8 in first arg");
    let arg2_string = String::from_utf8(arg2.to_vec()).expect("Invalid UTF-8 in second arg");

    let state = Box::new(State {
        sym_regex: Regex::new(&arg1_string).expect("Provided regex is invalid"),
        output_dir: arg2_string,
        trace_nums: HashMap::new(),
        builders: HashMap::new(),
        insn_cnt: 0,
        cyc_cnt: 0,
    });
    *raw_state = Box::into_raw(state) as *mut c_void;
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn filter_description(long_desc: *mut *const c_char) -> *const c_char {
    unsafe {
        *long_desc = LONG_DESC.as_ptr() as *const c_char;
    }
    SHORT_DESC.as_ptr() as *const c_char
}
