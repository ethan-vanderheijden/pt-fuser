#[allow(clippy::all)]
mod perf;
mod transform;

use clap::{CommandFactory, Parser};
use pt_fuser::trace::{SymbolInfo, builder::FrameCompletionOptions};
use regex::Regex;
use std::os::raw::{c_char, c_int, c_void};
use tracing::{Level, error};

use crate::transform::State;

const SHORT_DESC: &core::ffi::CStr = c"Parses an Intel PT trace into pt-fuser format for later aggregating, comparing, and exporting.";
const LONG_DESC: &core::ffi::CStr = c"Usage: --dlarg [OPTIONS] --dlarg <symbol regex> --dlarg <output_dir> \
    Only processes trace data for a function matching the given regex and all its sub-calls. \
    Each function invocation is a separate trace file written into the output directory as trace-<tid>-<n>.flex.gz";

#[derive(Parser)]
#[command(
    about = "Converts a trace from pt-fuser representation to a Perfetto trace",
    override_usage = concat!(env!("CARGO_PKG_NAME"), " --dlarg [OPTIONS] --dlarg <symbol regex> --dlarg <output_dir>")
)]
pub struct Args {
    symbol_regex: String,
    output_dir: String,
    #[clap(
        long,
        default_value_t = 0,
        help = "Maximum number of traces to process (0 = no limit)"
    )]
    max_traces: usize,
    #[clap(
        long,
        default_value_t = true,
        help = "Remove @plt stub functions from cluttering up the trace"
    )]
    remove_plt_stubs: bool,
    #[clap(
        long,
        short = 'o',
        default_value_t = true,
        help = "Whether to compress the output trace files (zstd)"
    )]
    compress_output: bool,
}

#[unsafe(no_mangle)]
pub static mut perf_dlfilter_fns: std::mem::MaybeUninit<perf::perf_dlfilter_fns> =
    std::mem::MaybeUninit::<perf::perf_dlfilter_fns>::uninit();

/// Resolves the symbol name based on the .addr field of the sample.
/// For branch events, .addr is the address of the branch target.
pub(crate) unsafe fn resolve_addr(
    sample: &perf::perf_dlfilter_sample,
    ctx: *mut c_void,
) -> Option<SymbolInfo> {
    if sample.addr_correlates_sym != 0 {
        unsafe {
            let raw_symbol = perf_dlfilter_fns.assume_init().resolve_addr.unwrap()(ctx);
            if !(*raw_symbol).sym.is_null() {
                let symbol = std::ffi::CStr::from_ptr((*raw_symbol).sym);
                return Some(SymbolInfo {
                    name: symbol.to_str().unwrap_or("Non UTF-8 symbol").to_string(),
                    offset: (*raw_symbol).sym_start,
                    size: (*raw_symbol).sym_end - (*raw_symbol).sym_start,
                });
            }
        }
    }
    None
}

/// Symbols returned by `resolve_addr` use the symbol addresses found in the ELF's .symtab.
/// At runtime, the actual address will change due to ASLR and other factors.
/// If we have the target address of a branch, we can find it's offset from the start of the symbol
/// and normalize the symbol's address as follows: symbol.address = branch_target - offset_in_symbol
pub(crate) unsafe fn normalize_symbol_addr(
    branch_sample: &perf::perf_dlfilter_sample,
    mut symbol: SymbolInfo,
    ctx: *mut c_void,
) -> SymbolInfo {
    unsafe {
        let dlfitler_fns = perf_dlfilter_fns.assume_init();

        let mut data: perf::perf_dlfilter_al = std::mem::zeroed();
        data.size = std::mem::size_of::<perf::perf_dlfilter_al>() as u32;
        dlfitler_fns.resolve_address.unwrap()(ctx, branch_sample.addr, &mut data);
        let offset = data.symoff as u64;
        if let Some(cleanup) = dlfitler_fns.al_cleanup {
            cleanup(ctx, &mut data);
        }

        symbol.offset = branch_sample.addr - offset;
        symbol
    }
}

#[unsafe(no_mangle)]
#[allow(clippy::missing_safety_doc)]
pub unsafe extern "C" fn filter_event_early(
    raw_state: *mut c_void,
    sample: &perf::perf_dlfilter_sample,
    ctx: *mut c_void,
) -> c_int {
    let state = unsafe { raw_state.cast::<State>().as_mut().unwrap() };

    let event_ltr = unsafe { *sample.event as u8 as char };
    if event_ltr == 'i' {
        transform::process_insn_event(state, sample, ctx);
    } else if event_ltr == 'b' {
        if !transform::process_branch_event(state, sample, ctx) {
            // ECANCELED to signal that we are done processing the trace
            return -125;
        }
    }
    1
}

#[unsafe(no_mangle)]
#[allow(clippy::missing_safety_doc)]
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
            let mut args = Args::command();
            error!(
                "Failed to retrieve dlargs. {}",
                args.render_usage().to_string()
            );
            return -1;
        }
        args = std::slice::from_raw_parts(argv, argc as usize);
    }

    let args = Args::parse_from(std::iter::once(env!("CARGO_PKG_NAME").to_string()).chain(
        args.iter().map(|s| {
            let arg1 = unsafe { std::ffi::CStr::from_ptr(*s).to_bytes() };
            String::from_utf8(arg1.to_vec()).expect("Invalid UTF-8 in argument")
        }),
    ));

    let max_traces = if args.max_traces == 0 {
        None
    } else {
        Some(args.max_traces as u32)
    };

    let trace_options = FrameCompletionOptions {
        remove_plt_stubs: args.remove_plt_stubs,
    };

    let state = Box::new(State::new(
        Regex::new(&args.symbol_regex).expect("Provided regex is invalid"),
        args.output_dir,
        args.compress_output,
        trace_options,
        max_traces,
    ));
    *raw_state = Box::into_raw(state) as *mut c_void;
    0
}

#[unsafe(no_mangle)]
#[allow(clippy::missing_safety_doc)]
pub unsafe extern "C" fn stop(_raw_state: *mut c_void, _ctx: *mut c_void) -> c_int {
    transform::finish_exporting();
    0
}

#[unsafe(no_mangle)]
#[allow(clippy::missing_safety_doc)]
pub unsafe extern "C" fn filter_description(long_desc: *mut *const c_char) -> *const c_char {
    unsafe {
        *long_desc = LONG_DESC.as_ptr() as *const c_char;
    }
    SHORT_DESC.as_ptr() as *const c_char
}
