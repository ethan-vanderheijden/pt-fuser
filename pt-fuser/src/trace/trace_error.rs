// Note: in the Perfetto trace, events will be displayed from low ID to high ID
// the execution trace will always be below all events

pub struct DataCollectionError;

impl DataCollectionError {
    pub const ID: u32 = 1;
    pub const NAME: &'static str = "Trace Overflow";
    pub const DESC: &'static str =
        "Intel PT's buffer overflowed while tracing. Callstacks may be corrupted.";
}

pub struct TraceInterrupted;

impl TraceInterrupted {
    pub const ID: u32 = 20;
    pub const NAME: &'static str = "Async Interrupts";
    pub const DESC: &'static str =
        "Trace was paused while the CPU serviced an asynchronous interrupt.";
}

pub struct LostFrameWhileMerging;

impl LostFrameWhileMerging {
    pub const ID: u32 = 300;
    pub const NAME: &'static str = "Lost Frames";
    pub const DESC: &'static str =
        "A frame could not be added while merging because it overlapped with adjacent frames.";
}
