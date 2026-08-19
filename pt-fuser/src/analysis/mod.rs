pub mod filter;
pub mod histogram;

use std::fmt::Display;

use crate::trace::{Chunk, Frame};

pub struct FrameFinder<'a, 'b, P>
where
    P: Fn(&Frame) -> bool,
{
    curr_frame: &'a Frame,
    remaining_chunks: Box<dyn Iterator<Item = Chunk<'a>> + 'a>,
    child_frame_finder: Option<Box<FrameFinder<'a, 'b, P>>>,
    pred: &'b P,
    produced_self: bool,
}

impl<'a, 'b, P> FrameFinder<'a, 'b, P>
where
    P: Fn(&Frame) -> bool,
{
    pub fn new(root: &'a Frame, pred: &'b P) -> FrameFinder<'a, 'b, P> {
        FrameFinder {
            curr_frame: root,
            remaining_chunks: Box::new(root.chunks()),
            child_frame_finder: None,
            pred,
            produced_self: false,
        }
    }
}

impl<'a, 'b, P> Iterator for FrameFinder<'a, 'b, P>
where
    P: Fn(&Frame) -> bool,
{
    type Item = &'a Frame;

    fn next(&mut self) -> Option<Self::Item> {
        if !self.produced_self {
            self.produced_self = true;
            if (self.pred)(self.curr_frame) {
                return Some(self.curr_frame);
            }
        }

        loop {
            if let Some(child_frame_finder) = &mut self.child_frame_finder {
                if let Some(frame) = child_frame_finder.next() {
                    return Some(frame);
                } else {
                    self.child_frame_finder = None;
                }
            }

            let next_chunk = self.remaining_chunks.next();
            if let Some(next_chunk) = next_chunk {
                match next_chunk {
                    Chunk::Frame(frame) => {
                        self.child_frame_finder =
                            Some(Box::new(FrameFinder::new(frame, self.pred)));
                    }
                    _ => {}
                }
            } else {
                break;
            }
        }
        None
    }
}

const MIN_DATAPOINTS_FOR_STATS: usize = 10;

pub(crate) struct Stats {
    pub(crate) min: f64,
    pub(crate) q1: f64,
    pub(crate) median: f64,
    pub(crate) q3: f64,
    pub(crate) max: f64,
    pub(crate) mean: f64,
    pub(crate) stddev: f64,
}

impl Stats {
    pub(crate) fn from_data(data: impl IntoIterator<Item = f64>) -> Option<Self> {
        let mut sorted = data.into_iter().collect::<Vec<_>>();
        if sorted.len() < MIN_DATAPOINTS_FOR_STATS {
            return None;
        }

        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let min = *sorted.first().unwrap();
        let max = *sorted.last().unwrap();
        let (median, first_half, second_half) = if sorted.len() % 2 == 0 {
            (
                (sorted[sorted.len() / 2 - 1] + sorted[sorted.len() / 2]) / 2.0,
                &sorted[..sorted.len() / 2],
                &sorted[sorted.len() / 2..],
            )
        } else {
            (
                sorted[sorted.len() / 2],
                &sorted[..sorted.len() / 2],
                &sorted[sorted.len() / 2 + 1..],
            )
        };

        let (q1, q3) = if first_half.len() % 2 == 0 {
            (
                (first_half[first_half.len() / 2 - 1] + first_half[first_half.len() / 2]) / 2.0,
                (second_half[second_half.len() / 2 - 1] + second_half[second_half.len() / 2]) / 2.0,
            )
        } else {
            (
                first_half[first_half.len() / 2],
                second_half[second_half.len() / 2],
            )
        };

        let mean = sorted.iter().sum::<f64>() / sorted.len() as f64;
        let squared_diffs = sorted.iter().map(|x| (x - mean).powi(2));
        let variance = squared_diffs.sum::<f64>() / (sorted.len() as f64 - 1.0);
        let stddev = variance.sqrt();

        Some(Stats {
            min,
            q1,
            median,
            q3,
            max,
            mean,
            stddev,
        })
    }
}

impl IntoIterator for Stats {
    type Item = (String, f64);
    type IntoIter = std::vec::IntoIter<Self::Item>;

    fn into_iter(self) -> Self::IntoIter {
        vec![
            ("Min".to_string(), self.min),
            ("Q1".to_string(), self.q1),
            ("Median".to_string(), self.median),
            ("Q3".to_string(), self.q3),
            ("Max".to_string(), self.max),
            ("Mean".to_string(), self.mean),
            ("Std Dev".to_string(), self.stddev),
        ]
        .into_iter()
    }
}

impl Display for Stats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(
            f,
            "Min: {:.1}, Q1: {:.1}, Median: {:.1}, Q3: {:.1}, Max: {:.1}",
            self.min, self.q1, self.median, self.q3, self.max
        )?;
        writeln!(f, "Mean: {:.1}, Std Dev: {:.1}", self.mean, self.stddev)
    }
}
