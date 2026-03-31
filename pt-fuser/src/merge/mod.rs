#[cfg(test)]
mod test;

use std::{
    cmp::{max, min},
    collections::{HashMap, HashSet},
};

use tracing::{info, warn};

use crate::trace::{
    Chunk, Event, Frame, Trace,
    metrics::{Metrics, MetricsRange},
};

const FREQUENT_FRAME_THRESH: f32 = 0.7;

const LOST_FRAME_EVENT_ID: u32 = 555740177;

/// # Merging Algorithm
///
/// We will consider the case where we are merging multiple stack frames.
/// Each stack frame is a sequence of child frames, e.g. a() := [f(), g(), f(), h()].
/// We can think of this as a string, e.g. a() := "f1 g f2 h" of unique characters
/// (each instance of a child frame as a unique entity). Merging becomes first finding
/// the longest common subsequence of these strings. In general, this problem is NP-hard
/// for multiple strings, but for strings with unique characters, it becomes poly-time.
///
/// ### Example:
/// a() := [f(), x(), y(), g(), z(), f(), h()] => "f1 x y g z f2 h"  \
/// b() := [f(), r(), g(), s(), f(), h()] => "f1 r g s f2 h"         \
/// c() := [f(), t(), z(), f(), e()] => "f1 t z f2 e"
///
/// So the longest common subsequence is: r() := "f1 f2".
///
/// ## Step 2
///
/// Then, for each section of the merged string, we find the most common stack frame across
/// all the traces, and if it appears more than 2/3 of the time, we add it to the merged trace.
///
/// ### Example:
/// for the section before the first "f1":                                               \
/// a() := "", b() := "", c() := "" => no common frame found
///
/// for the section between "f1" and "f2":                                               \
/// a() := "x y g z", b() := "r g s", c() := "t z"                                       \
/// => we find that g() appears in 2/3 of the traces, so we add it to the merged trace   \
/// => r() := "f1 g f2"
///
/// Then, we recurse on the section between "g" and "f2":                                \
/// a() := "z", b() := "s", c() := "z"                                                   \
/// => we find that z() appears in 2/3 of the traces, so we add it to the merged trace   \
/// => r() := "f1 g z f2"
///
/// Then, we recurse on the section between "z" and "f2":                                \
/// a() := "", b() := "s", c() := "" => no common frame found
///
/// for the section after "f2":                                                          \
/// a() := "h", b() := "h", c() := "e"                                                   \
/// => we find that h() appears in 2/3 of the traces, so we add it to the merged trace   \
/// => r() := "f1 g z f2 h"
///
/// Then, we recurse on the section after "h":                                           \
/// a() := "", b() := "", c() := "e" => no common frame
///
/// Therefore, the final merged trace is r() := [f(), g(), z(), f(), h()].               \
/// For each of child frame in r(), we create merged versions from the original
/// child frames of a(), b(), and c().
pub fn merge_traces(traces: &[&Trace]) -> Trace {
    if traces.is_empty() {
        panic!("Cannot merge empty list of traces");
    } else if traces.len() == 1 {
        return (*traces.first().unwrap()).clone();
    }

    let frames = traces
        .iter()
        .map(|t| t.root_frame())
        .collect::<Vec<&Frame>>();

    info!("Merging frames for {} traces...", traces.len());
    let mut lost_frame_occurences = Vec::new();
    let merged_frame = merge_frames(
        &frames,
        Metrics::constant(0),
        &mut lost_frame_occurences,
        FREQUENT_FRAME_THRESH,
    );
    info!("Merging events...");
    let mut merged_events = merge_events(traces, merged_frame.metrics);

    if !lost_frame_occurences.is_empty() {
        let lost_frame_event = Event::from_occurences(
            LOST_FRAME_EVENT_ID,
            "Lost Frames".to_string(),
            "A frame could not be added because it overlapped with adjacent frames.".to_string(),
            lost_frame_occurences,
        )
        .expect("Failed to create lost frame event");
        merged_events.push(lost_frame_event);
    }

    let result = Trace::new(merged_frame, merged_events);

    result
}

trait Id: Clone {
    fn id(&self) -> u32;
}

#[derive(Clone, Copy)]
struct FrameIndexed<'a> {
    original: &'a Frame,
    offset_in_parent: Metrics,
    // unique within a parent frame, stable across parent frames
    id: u32,
}

impl Id for FrameIndexed<'_> {
    fn id(&self) -> u32 {
        self.id
    }
}

#[derive(Hash, Eq, PartialEq)]
struct IdMapKey<'a> {
    symbol: &'a str,
    instance: u32,
}

/// Map each frame's symbol into a unique id.
/// Ids will range from 1..N.
/// Ids for the same symbol across frames will be the same.
/// Ids for the same symbols within the same frame will be different
/// (i.e. multiple instances of the same symbol are assigned unique ids).
///
/// Returns N and a list of lists of indexed frames. Each list of indexed frames
/// corresponds to the child frames of the original frame.
fn index_children<'a>(frames: &[&'a Frame]) -> (u32, Vec<Vec<FrameIndexed<'a>>>) {
    let mut indexed_children = Vec::with_capacity(frames.len());
    let mut symbol_ids: HashMap<IdMapKey, u32> = HashMap::new();
    let mut seen_symbols: HashMap<&str, u32> = HashMap::new();
    let mut next_id = 0;

    for &parent in frames {
        let mut children = Vec::new();
        for chunk in parent.chunks() {
            match chunk {
                Chunk::Frame(frame) => {
                    let instance = seen_symbols
                        .entry(&frame.symbol.name)
                        .and_modify(|x| *x += 1)
                        .or_insert(0);
                    let key = IdMapKey {
                        symbol: &frame.symbol.name,
                        instance: *instance,
                    };
                    let id = symbol_ids.entry(key).or_insert_with(|| {
                        next_id += 1;
                        next_id
                    });

                    children.push(FrameIndexed {
                        original: frame,
                        offset_in_parent: frame.metrics.start - parent.metrics.start,
                        id: *id,
                    });
                }
                _ => continue,
            }
        }
        indexed_children.push(children);
        seen_symbols.clear();
    }

    (next_id, indexed_children)
}

/// Algorithm inspired from: https://stackoverflow.com/a/5752321.
/// Complexity is O(N^2 * M) where N is the length of sequences and M is the number of sequences.
///
/// Arguments: `n` means the ids are numbered 1..n; `sequences` is a list of sequences where
/// each sequence is a list of items with unique ids.
///
/// Panics if `sequences` is empty.
fn find_lcs<I: Id>(n: u32, sequences: &[&[I]]) -> Vec<u32> {
    // subproblems[i] represents the longest common subsequence ending with id=(i + 1)
    let mut subproblems: Vec<Option<Vec<u32>>> = vec![None; n as usize];

    let first_seq = sequences.first().unwrap();
    'nexti: for (ele_index, ele) in first_seq.iter().enumerate() {
        let i = ele.id() as usize;
        let mut longest_subsequence_i = vec![ele.id()];
        // if other frames don't have an child with id=i, then it's not part of any common subsequence
        for other_seq in sequences[1..].iter() {
            if other_seq.iter().all(|c| c.id() != i as u32) {
                continue 'nexti;
            }
        }

        if ele_index > 0 {
            'nextj: for prev in (0..ele_index).rev() {
                let j = first_seq[prev].id() as usize;
                if let Some(subproblem) = &subproblems[j - 1] {
                    // if all other frames have child with id=j before child with id=i,
                    // then we can extend longest_subsequence with subproblem[j]
                    for other_seq in sequences[1..].iter() {
                        let index_of_i = other_seq.iter().position(|c| c.id() == i as u32); // must exist
                        let index_of_j = other_seq.iter().position(|c| c.id() == j as u32);
                        if index_of_j.is_none() || index_of_j.unwrap() > index_of_i.unwrap() {
                            continue 'nextj;
                        }
                    }

                    if subproblem.len() + 1 > longest_subsequence_i.len() {
                        longest_subsequence_i = subproblem.clone();
                        longest_subsequence_i.push(ele.id());
                    }
                }
            }
        }

        subproblems[i - 1] = Some(longest_subsequence_i);
    }

    let mut longest_subsequence = Vec::new();
    for subproblem in subproblems {
        if let Some(subproblem) = subproblem {
            if subproblem.len() > longest_subsequence.len() {
                longest_subsequence = subproblem;
            }
        }
    }
    longest_subsequence
}

/// Checks if any Id appears in at least thresh% of sequences.
/// If so, adds the Id with the highest frequency to the result and then
/// recurses on the section of the sequences before and after that Id.
/// `sequences` is a list of sequences, where each sequence is a list of items
/// with unique ids from 1..n.
///
/// Panics if `thresh` is not between 0 and 1
fn find_frequent_frames<I: Id>(n: u32, sequences: &[&[I]], thresh: f32) -> Vec<u32> {
    if thresh < 0.0 || thresh > 1.0 {
        panic!("Threshold must be between 0 and 1");
    }

    let mut result = Vec::new();
    // counts[i] is None if id=(i + 1) does not appear in any sequence
    // otherwise, it is (count, item.id(), index_cum)
    // if id=(i + 1) appears at index j out of length k, then index_cum += j / k
    let mut counts: Vec<Option<(u32, u32, f32)>> = vec![None; n as usize];
    for &sequence in sequences {
        for (index, item) in sequence.iter().enumerate() {
            let i = item.id() as usize - 1;
            if let Some((count, _, index_sum)) = &mut counts[i] {
                *count += 1;
                *index_sum += index as f32 / sequence.len() as f32;
            } else {
                counts[i] = Some((1, item.id(), index as f32 / sequence.len() as f32));
            }
        }
    }

    if let Some(Some((count, id, index_sum))) = counts.into_iter().max_by_key(|x| {
        if let Some((count, _, _)) = x {
            *count
        } else {
            0
        }
    }) {
        if (count as f32) / (sequences.len() as f32) >= thresh {
            let index_avg = index_sum / (count as f32);

            let mut before: Vec<&[I]> = Vec::with_capacity(sequences.len());
            let mut after: Vec<&[I]> = Vec::with_capacity(sequences.len());
            'next_sequence: for i in 0..sequences.len() {
                let sequence = sequences[i];
                for (j, ele) in sequence.iter().enumerate() {
                    if ele.id() == id {
                        before.push(&sequence[0..j]);
                        after.push(&sequence[j + 1..]);
                        continue 'next_sequence;
                    }
                }
                let break_point = (index_avg * (sequence.len() as f32)).round() as usize;
                before.push(&sequence[0..break_point]);
                after.push(&sequence[break_point..]);
            }

            result.extend(find_frequent_frames(n, &before, thresh));
            result.push(id.clone());
            result.extend(find_frequent_frames(n, &after, thresh));
        }
    }

    result
}

fn add_within_bounds(
    frame: &mut Frame,
    mut child: Frame,
    min_metrics: &mut Metrics,
    max_metrics: &Metrics,
    lost_frame_occurrences: &mut Vec<Metrics>,
) {
    let original_child_start = child.metrics.start.clone();
    child.metrics.start.ts = max(child.metrics.start.ts, min_metrics.ts);
    child.metrics.start.cycles = max(child.metrics.start.cycles, min_metrics.cycles);
    child.metrics.start.insn_count = max(child.metrics.start.insn_count, min_metrics.insn_count);
    child.metrics.end.ts = min(child.metrics.end.ts, max_metrics.ts);
    child.metrics.end.cycles = min(child.metrics.end.cycles, max_metrics.cycles);
    child.metrics.end.insn_count = min(child.metrics.end.insn_count, max_metrics.insn_count);
    if child.metrics.start.ts <= child.metrics.end.ts
        && child.metrics.start.cycles <= child.metrics.end.cycles
        && child.metrics.start.insn_count <= child.metrics.end.insn_count
    {
        *min_metrics = child.metrics.end;
        frame
            .add_child(child)
            .expect("Adding merged child frame should be valid");
    } else {
        warn!(
            "At {}, Merged child frame {} couldn't be added to parent {}",
            original_child_start, child.symbol, frame.symbol
        );
        lost_frame_occurrences.push(original_child_start);
    }
}

fn merge_frames(
    frames: &[&Frame],
    new_start: Metrics,
    lost_frame_occurrences: &mut Vec<Metrics>,
    frequent_thresh: f32,
) -> Frame {
    let avg_len = frames
        .iter()
        .map(|f| &f.metrics.end - &f.metrics.start)
        .sum::<Metrics>()
        / (frames.len() as u64);
    let new_end = new_start + avg_len;
    let mut merged_parent = Frame::new(
        MetricsRange::new(new_start, new_end),
        frames[0].symbol.clone(),
    );
    let mut min_metrics = merged_parent.metrics.start;
    let max_metrics = merged_parent.metrics.end;

    let (n, indexed_children) = index_children(frames);
    let mut sequences = indexed_children
        .iter()
        .map(|c| c.as_slice())
        .collect::<Vec<&[FrameIndexed]>>();

    let lcs = find_lcs(n, &sequences);

    for id in lcs {
        let mut common_frames = Vec::with_capacity(sequences.len());
        let mut common_offset_sum = Metrics::constant(0);
        let mut subsequences = Vec::with_capacity(sequences.len());

        for sequence in sequences.iter_mut() {
            for i in 0..sequence.len() {
                let item = &sequence[i];
                if item.id() == id {
                    common_frames.push(item.original);
                    common_offset_sum += item.offset_in_parent;
                    subsequences.push(&sequence[0..i]);
                    *sequence = &sequence[i + 1..];
                    break;
                }
            }
        }
        // INVARIANT: subsequences.len() == sequences.len()

        let freq_frame_ids = find_frequent_frames(n, &subsequences, frequent_thresh);
        let mut freq_frames = Vec::new();
        let mut freq_offset_sum = Metrics::constant(0);
        for freq_id in freq_frame_ids {
            for sequence in subsequences.iter_mut() {
                for i in 0..sequence.len() {
                    let item = &sequence[i];
                    if item.id() == freq_id {
                        freq_frames.push(item.original);
                        freq_offset_sum += item.offset_in_parent;
                        *sequence = &sequence[i + 1..];
                        break;
                    }
                }
            }

            let avg_freq_offset = freq_offset_sum / (freq_frames.len() as u64);
            let merged_freq_frame = merge_frames(
                &freq_frames,
                new_start + avg_freq_offset,
                lost_frame_occurrences,
                frequent_thresh,
            );
            add_within_bounds(
                &mut merged_parent,
                merged_freq_frame,
                &mut min_metrics,
                &max_metrics,
                lost_frame_occurrences,
            );
        }

        let avg_common_offset = common_offset_sum / (common_frames.len() as u64);
        let merged_common_frame = merge_frames(
            &common_frames,
            new_start + avg_common_offset,
            lost_frame_occurrences,
            frequent_thresh,
        );
        add_within_bounds(
            &mut merged_parent,
            merged_common_frame,
            &mut min_metrics,
            &max_metrics,
            lost_frame_occurrences,
        );
    }

    let freq_frames_ids = find_frequent_frames(n, &sequences, frequent_thresh);
    let mut freq_frames = Vec::new();
    let mut freq_offset_sum = Metrics::constant(0);
    for freq_id in freq_frames_ids {
        for sequence in sequences.iter_mut() {
            for i in 0..sequence.len() {
                let item = &sequence[i];
                if item.id() == freq_id {
                    freq_frames.push(item.original);
                    freq_offset_sum += item.offset_in_parent;
                    *sequence = &sequence[i + 1..];
                    break;
                }
            }
        }

        let avg_freq_offset = freq_offset_sum / (freq_frames.len() as u64);
        let merged_freq_frame = merge_frames(
            &freq_frames,
            new_start + avg_freq_offset,
            lost_frame_occurrences,
            frequent_thresh,
        );
        add_within_bounds(
            &mut merged_parent,
            merged_freq_frame,
            &mut min_metrics,
            &max_metrics,
            lost_frame_occurrences,
        );
    }

    merged_parent
}

fn zip_events(
    id: u32,
    name: &str,
    desc: &str,
    events: &mut [impl Iterator<Item = Metrics>],
    total_occurences: Option<usize>,
) -> Event {
    let mut new_occurences = Vec::with_capacity(total_occurences.unwrap_or(0));

    let mut next_elems = Vec::with_capacity(events.len());
    for event in events.iter_mut() {
        next_elems.push(event.next());
    }

    loop {
        let mut min_metrics = None;
        for (i, next) in next_elems.iter().enumerate() {
            if let Some(n) = next {
                if let Some((_, min)) = min_metrics {
                    if n < min {
                        min_metrics = Some((i, n));
                    }
                } else {
                    min_metrics = Some((i, n));
                }
            }
        }

        if let Some((i, min)) = min_metrics {
            new_occurences.push(min.clone());
            next_elems[i] = events[i].next();
        } else {
            break;
        }
    }

    Event::from_occurences(id, name.to_string(), desc.to_string(), new_occurences)
        .expect("Failed to create merged Event")
}

fn merge_events(traces: &[&Trace], new_range: MetricsRange) -> Vec<Event> {
    let new_range_len = new_range.end - new_range.start;
    let mut events = Vec::new();
    let mut seen_ids = HashSet::new();
    for &trace in traces {
        for event in trace.events() {
            if !seen_ids.contains(&event.id) {
                seen_ids.insert(event.id);

                let mut original_events = traces
                    .iter()
                    .filter_map(|trace| {
                        trace.events().iter().find_map(|e| {
                            if e.id == event.id {
                                let trace_start = trace.root_frame().metrics.start;
                                let trace_range = trace.root_frame().metrics.end - trace_start;
                                // scale each occurence so it is within new_range
                                Some(e.occurences().iter().map(move |o| {
                                    new_range_len * (o - &trace_start) / trace_range
                                        + new_range.start
                                }))
                            } else {
                                None
                            }
                        })
                    })
                    .collect::<Vec<_>>();
                let total_occurences = original_events.iter().map(|e| e.len()).sum();

                let zipped = zip_events(
                    event.id,
                    &event.name,
                    &event.description,
                    &mut original_events,
                    Some(total_occurences),
                );
                events.push(zipped);
            }
        }
    }

    events
}
