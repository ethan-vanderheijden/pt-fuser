// Note: in the Perfetto trace, events will be displayed from low ID to high ID
// the execution trace will always be below all events

pub trait KnownError {
    const ID: u32;
    const NAME: &'static str;
    const DESC: &'static str;
}

pub struct DataCollectionError;

impl KnownError for DataCollectionError {
    const ID: u32 = 1;
    const NAME: &'static str = "Trace Overflow";
    const DESC: &'static str =
        "Intel PT's buffer overflowed while tracing. Callstacks may be corrupted.";
}

pub struct TraceInterrupted;

impl KnownError for TraceInterrupted {
    const ID: u32 = 20;
    const NAME: &'static str = "Async Interrupts";
    const DESC: &'static str = "Trace was paused while the CPU serviced an asynchronous interrupt.";
}

pub struct LostFrameWhileMerging;

impl KnownError for LostFrameWhileMerging {
    const ID: u32 = 300;
    const NAME: &'static str = "Lost Frames";
    const DESC: &'static str =
        "A frame could not be added while merging because it overlapped with adjacent frames.";
}
