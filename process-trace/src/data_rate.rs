use circular_buffer::HeapCircularBuffer;

/// All units are in nanoseconds.
/// Tracks data rate over a sliding window of the last `num_bins` * `bin_width` nanoseconds.
/// You can think of time as segmented into an infinite number of `bin_width` sized bins, and
/// each bin stores the number of bytes in the trace that were produced during that time window.
pub struct DataRateHistogram {
    bin_width: u64,
    // if current_bytes/current_time is None, then histogram isn't initialized yet
    current_bytes: Option<u64>,
    current_time: Option<u64>,
    /// bin for current time is stored at the head of the circular buffer
    bins: HeapCircularBuffer<u32>,
}

impl DataRateHistogram {
    /// `window_size` is the sliding window (in nanoseconds) over which the data rate is calculated.
    /// `granularity` is roughly how often (in nanoseconds) the data rate is updated.
    pub fn new(window_size: u64, granularity: u64) -> Self {
        let num_bins = (window_size / granularity) as u32;
        let bin_width = granularity;

        let mut bins = HeapCircularBuffer::with_capacity(num_bins as usize);
        bins.fill(0);
        Self {
            bin_width,
            current_bytes: None,
            current_time: None,
            bins,
        }
    }

    /// Returns the average data rate (in MiB/s) over the sliding window.
    pub fn rate(&self) -> f64 {
        let total_bytes: u64 = self.bins.iter().fold(0, |acc, x| acc + *x as u64);
        let total_time = self.bins.capacity() as u64 * self.bin_width;
        (total_bytes as f64 / total_time as f64) * 1_000_000_000.0 / 1024.0 / 1024.0
    }

    pub fn update(&mut self, bytes: u64, timestamp: u64) {
        if let Some(current_time) = self.current_time
            && let Some(current_bytes) = self.current_bytes
        {
            if timestamp < current_time || bytes < current_bytes {
                return;
            }

            if timestamp == current_time {
                // corner case: add bytes to the current bin if timestamp hasn't advanced
                let head = self.bins.front_mut().unwrap();
                *head += (bytes - current_bytes) as u32;
                self.current_bytes = Some(bytes);
                return;
            }

            let byte_delta = bytes - current_bytes;
            let time_delta = timestamp - current_time;

            let mut cursor = current_time;
            let mut bytes_assigned = 0;
            while cursor < timestamp {
                let next_boundary = (cursor / self.bin_width + 1) * self.bin_width;
                let next_cursor = next_boundary.min(timestamp);

                // assume bytes were evenly distributed over the time since the last update.

                // we have four timestamps:
                //       |------------|-------------|--------------|----->
                // current_time     cursor     next_cursor     timestamp
                let bytes_through_next_cursor =
                    byte_delta * (next_cursor - current_time) / time_delta;
                let bytes_portion = (bytes_through_next_cursor - bytes_assigned) as u32;
                bytes_assigned = bytes_through_next_cursor;

                let head = self.bins.front_mut().unwrap();
                *head += bytes_portion;
                if next_cursor == next_boundary {
                    self.bins.push_front(0);
                }

                cursor = next_cursor;
            }
            self.current_bytes = Some(bytes);
            self.current_time = Some(cursor);
        } else {
            self.current_bytes = Some(bytes);
            self.current_time = Some(timestamp);
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;

    // 10 bins of 100ns each
    const WINDOW: u64 = 1000;
    const BIN_WIDTH: u64 = 100;
    // rate() currently subtracts a hard-coded timestamp for logging, so tests that call it need
    // timestamps larger than that. Aligned to BIN_WIDTH to keep bin boundaries easy to reason about.
    const BASE: u64 = 1_790_560_333_700;

    fn histogram() -> DataRateHistogram {
        DataRateHistogram::new(WINDOW, BIN_WIDTH)
    }

    /// Bins from newest (head) to oldest.
    fn bins(h: &DataRateHistogram) -> Vec<u32> {
        h.bins.iter().copied().collect()
    }

    fn total(h: &DataRateHistogram) -> u32 {
        h.bins.iter().sum()
    }

    #[test]
    fn new_histogram_is_zeroed() {
        let h = histogram();
        assert_eq!(bins(&h), vec![0; 10]);
    }

    #[test]
    fn first_update_only_sets_baseline() {
        let mut h = histogram();
        h.update(500, 250);
        assert_eq!(total(&h), 0);
    }

    #[test]
    fn update_within_single_bin() {
        let mut h = histogram();
        h.update(0, 10);
        h.update(100, 60);
        let mut expected = vec![0; 10];
        expected[0] = 100;
        assert_eq!(bins(&h), expected);
    }

    #[test]
    fn update_ending_on_boundary_starts_new_bin() {
        let mut h = histogram();
        h.update(0, 0);
        h.update(100, 100);
        let mut expected = vec![0; 10];
        expected[1] = 100;
        assert_eq!(bins(&h), expected);

        h.update(150, 150);
        expected[0] = 50;
        assert_eq!(bins(&h), expected);
    }

    #[test]
    fn update_crossing_boundary_splits_proportionally() {
        let mut h = histogram();
        h.update(0, 50);
        h.update(100, 150);
        let mut expected = vec![0; 10];
        expected[0] = 50;
        expected[1] = 50;
        assert_eq!(bins(&h), expected);
    }

    #[test]
    fn update_spanning_many_bins_conserves_bytes() {
        let mut h = histogram();
        h.update(0, 0);
        // 10 bytes over 3 bins can't split evenly; none should be lost
        h.update(10, 300);
        assert_eq!(total(&h), 10);
    }

    #[test]
    fn update_with_uneven_split_conserves_bytes() {
        let mut h = histogram();
        h.update(0, 50);
        // 5 bytes over two equal 50ns halves
        h.update(5, 150);
        assert_eq!(total(&h), 5);
    }

    #[test]
    fn old_bins_slide_out_of_window() {
        let mut h = histogram();
        h.update(0, 0);
        h.update(100, 99);
        assert_eq!(total(&h), 100);

        // advance exactly one full window with no new bytes
        h.update(100, 99 + WINDOW);
        assert_eq!(total(&h), 0);
    }

    #[test]
    fn out_of_order_updates_are_ignored() {
        let mut h = histogram();
        h.update(100, 500);
        h.update(50, 600); // bytes went backwards
        h.update(200, 400); // time went backwards
        assert_eq!(total(&h), 0);

        // baseline is still (100, 500)
        h.update(300, 550);
        assert_eq!(total(&h), 200);
    }

    #[test]
    fn bytes_at_repeated_timestamp_are_not_lost() {
        let mut h = histogram();
        h.update(0, 10);
        h.update(50, 10);
        h.update(100, 60);
        assert_eq!(total(&h), 100);
    }

    #[test]
    fn rate_with_small_timestamps_does_not_panic() {
        let mut h = histogram();
        h.update(0, 0);
        h.update(1024 * 1024, 50);
        assert!(h.rate() > 0.0);
    }

    #[test]
    fn rate_is_mib_per_second_over_whole_window() {
        let mut h = histogram();
        h.update(0, BASE);
        h.update(1024 * 1024, BASE + 50);
        // 1 MiB over a 1000ns window
        let expected = 1_000_000.0;
        assert!((h.rate() - expected).abs() < 1e-6 * expected);
    }

    #[test]
    fn rate_handles_more_than_u32_max_bytes_in_window() {
        // two bins of ~3GB each
        let mut h = DataRateHistogram::new(200, 100);
        h.update(0, BASE);
        h.update(3_000_000_000, BASE + 99);
        h.update(6_000_000_000, BASE + 199);
        let expected = 6_000_000_000.0 / 200.0 * 1e9 / 1024.0 / 1024.0;
        assert!((h.rate() - expected).abs() < 1e-6 * expected);
    }
}
