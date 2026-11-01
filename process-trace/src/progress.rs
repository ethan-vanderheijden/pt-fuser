use indicatif::{ProgressBar, ProgressStyle};

const PROGRESS_STYLE_TEMPLATE: &str =
    "[{wide_bar}] {percent}%\n   ↳ Estimated time remaining: {eta}\n     {msg}";

/// Tracks the progress of decoding a single thread's trace data.
pub struct ProgressTracker {
    bar: ProgressBar,
    tid: u32,
    num_produced: usize,
}

impl ProgressTracker {
    /// `total_bytes` is the size of the raw trace data being decoded.
    pub fn new(tid: u32, total_bytes: u64) -> Self {
        let bar = ProgressBar::new(total_bytes);
        bar.set_style(
            ProgressStyle::with_template(PROGRESS_STYLE_TEMPLATE)
                .expect("Failed to produce ProgressStyle"),
        );
        let tracker = Self {
            bar,
            tid,
            num_produced: 0,
        };
        tracker.update_message();
        tracker
    }

    pub fn set_progress(&self, bytes_processed: u64) {
        self.bar.set_position(bytes_processed);
    }

    pub fn incr_produced(&mut self) {
        self.num_produced += 1;
        self.update_message();
    }

    pub fn finish(self) {
        self.bar.finish();
        eprintln!(
            "\n\nProduced {} traces in {:2?}",
            self.num_produced,
            self.bar.elapsed()
        );
    }

    fn update_message(&self) {
        self.bar.set_message(format!(
            "Thread {} - produced {} traces",
            self.tid, self.num_produced
        ));
    }
}
