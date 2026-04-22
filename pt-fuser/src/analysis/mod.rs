pub mod filter;
pub mod histogram;

use crate::trace::{Chunk, Frame};

pub struct FrameFinder<'a, 'b, F>
where
    F: Fn(&'a Frame) -> bool,
{
    curr_frame: &'a Frame,
    child_index: usize,
    child_frame_finder: Option<Box<FrameFinder<'a, 'b, F>>>,
    pred: &'b F,
    produced_self: bool,
}

impl<'a, 'b, F> FrameFinder<'a, 'b, F>
where
    F: Fn(&'a Frame) -> bool,
{
    pub fn new(root: &'a Frame, pred: &'b F) -> FrameFinder<'a, 'b, F> {
        FrameFinder {
            curr_frame: root,
            child_index: 0,
            child_frame_finder: None,
            pred,
            produced_self: false,
        }
    }
}

impl<'a, 'b, F> Iterator for FrameFinder<'a, 'b, F>
where
    F: Fn(&'a Frame) -> bool,
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
            for i in self.child_index..self.curr_frame.chunks().len() {
                let chunk = &self.curr_frame.chunks()[i];
                if let Chunk::Frame(frame) = chunk {
                    self.child_index = i + 1;
                    self.child_frame_finder = Some(Box::new(FrameFinder::new(frame, self.pred)));
                    break;
                }
            }

            // child frames have been exhausted
            if self.child_frame_finder.is_none() {
                break;
            }
        }
        None
    }
}
