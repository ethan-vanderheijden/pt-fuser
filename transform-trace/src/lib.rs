mod perf;

use regex::Regex;
use std::os::raw::{c_char, c_int, c_void};

#[unsafe(no_mangle)]
pub static mut perf_dlfilter_fns: std::mem::MaybeUninit<perf::perf_dlfilter_fns> =
    std::mem::MaybeUninit::<perf::perf_dlfilter_fns>::uninit();

struct State {
    sym_regex: Regex,
    output_dir: String,
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn filter_event_early(
    raw_state: *mut c_void,
    sample: &perf::perf_dlfilter_sample,
    ctx: *mut c_void,
) -> c_int {
    let state = unsafe { raw_state.cast::<State>().as_mut().unwrap() };

    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn start(raw_state: &mut *mut c_void, ctx: *mut c_void) -> c_int {
    let mut argc: c_int = 0;
    let args;
    unsafe {
        let argv = (perf_dlfilter_fns.assume_init().args.unwrap())(ctx, &mut argc as *mut c_int);
        args = std::slice::from_raw_parts(argv, argc as usize);
    }

    if argc != 2 {
        eprintln!("Expected two arguments. Usage: --dlarg <symbol regex> --dlarg <output_dir>");
        return -1;
    }

    let arg1 = unsafe { std::ffi::CStr::from_ptr(args[0]).to_bytes() };
    let arg2 = unsafe { std::ffi::CStr::from_ptr(args[1]).to_bytes() };

    let arg1_string = String::from_utf8(arg1.to_vec()).expect("Invalid UTF-8 in first arg");
    let arg2_string = String::from_utf8(arg2.to_vec()).expect("Invalid UTF-8 in second arg");

    let state = Box::new(State {
        sym_regex: Regex::new(&arg1_string).expect("Provided regex is invalid"),
        output_dir: arg2_string,
    });
    *raw_state = Box::into_raw(state) as *mut c_void;
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn filter_description(long_desc: *mut *const c_char) -> *const c_char {
    let short = "Parses an Intel PT trace into our internal format for later aggregating, comparing, and exporting.";
    let leaked_short = Box::leak(Box::new(short));

    let long = "Usage: --dlarg <symbol regex> --dlarg <output_dir>\
        Only processes trace data for function matching the given regex and all its subfunctions.\
        Each instance of the function invocation is a separate file written into the output directory as trace-<pid>-<n>.txt.";
    let leaked_long = Box::leak(Box::new(long));

    unsafe {
        *long_desc = leaked_long.as_ptr() as *const c_char;
    }
    leaked_short.as_ptr() as *const c_char
}
