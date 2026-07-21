#[cfg(test)]
mod test;

use std::{
    cmp::{max, min},
    collections::{HashMap, HashSet},
};

use indexmap::IndexMap;
use tracing::{info, warn};

use crate::{
    analysis::Stats,
    trace::{
        Annotation, Chunk, Event, Frame, Trace,
        metrics::{Metrics, MetricsRange},
        trace_error,
    },
};

const FREQUENT_FRAME_THRESH: f32 = 0.7;
const ANNOTATION_COUNT_NAME: &str = "Merging Count";
const ANNOTATION_STATS_NAME: &str = "Merging Stats";
const ANNOTATION_RAW_DATA_NAME: &str = "Merging Raw Latencies";

/// # Merging Algorithm
///
/// If `trace_ids` is provided, it must be a parallel list of unique identifiers for each trace. These
/// IDs will be used to add raw data from the merging algorithm as an annotation to the merged trace.
///
/// We will consider the case where we are merging multiple stack frames.
/// Each stack frame is a sequence of child frames, e.g. a() := [f(), g(), f(), h()].
/// We can think of this as a string, e.g. a() := "f1 g f2 h" of unique characters
/// (each instance of a child frame as a unique entity). Merging becomes first finding
/// the longest common subsequence of these strings. In general, this problem is NP-hard
/// for multiple strings, but for strings with unique characters, it becomes poly-time.
///
/// Note: pause chunks are treated identically to frame chunks.
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
/// all the traces, and if it appears in >70% of the time, we add it to the merged trace.
///
/// ### Example:
/// for the section before the first "f1":                                               \
/// a() := "", b() := "", c() := "" => no common frame found
///
/// for the section between "f1" and "f2":                                               \
/// a() := "x y g z", b() := "r g s", c() := "t z"                                       \
/// => we find that g() appears in >70% of the traces, so we add it to the merged trace   \
/// => r() := "f1 g f2"
///
/// Then, we recurse on the section between "g" and "f2":                                \
/// a() := "z", b() := "s", c() := "z"                                                   \
/// => we find that z() appears in >70% of the traces, so we add it to the merged trace   \
/// => r() := "f1 g z f2"
///
/// Then, we recurse on the section between "z" and "f2":                                \
/// a() := "", b() := "s", c() := "" => no common frame found
///
/// for the section after "f2":                                                          \
/// a() := "h", b() := "h", c() := "e"                                                   \
/// => we find that h() appears in >70% of the traces, so we add it to the merged trace   \
/// => r() := "f1 g z f2 h"
///
/// Then, we recurse on the section after "h":                                           \
/// a() := "", b() := "", c() := "e" => no common frame
///
/// Therefore, the final merged trace is r() := [f(), g(), z(), f(), h()].               \
/// For each of child frame in r(), we create merged versions from the original
/// child frames of a(), b(), and c().
pub fn merge_traces(traces: &[&Trace], raw_trace_ids: Option<&[&str]>) -> Trace {
    if traces.is_empty() {
        panic!("Cannot merge empty list of traces");
    } else if raw_trace_ids.is_some() && raw_trace_ids.unwrap().len() != traces.len() {
        panic!("Found {} traces but {} trace_ids", traces.len(), raw_trace_ids.unwrap().len());
    } else if traces.len() == 1 {
        return traces[0].clone();
    }

    let frames = traces
        .iter()
        .map(|t| t.root_frame())
        .collect::<Vec<&Frame>>();

    info!("Merging frames for {} traces...", traces.len());

    let latencies = frames
        .iter()
        .map(|f| f.metrics.end - f.metrics.start)
        .collect::<Vec<_>>();
    let new_end = latencies.iter().sum::<Metrics>() / (frames.len() as u64);
    let mut new_root = Frame::new(
        MetricsRange::new(Metrics::constant(0), new_end),
        traces[0].root_frame().symbol.clone(),
    );

    fill_annotations(&mut new_root, &latencies, raw_trace_ids);

    let mut lost_frame_occurences = Vec::new();
    merge_children(
        &mut new_root,
        &frames,
        raw_trace_ids,
        &mut lost_frame_occurences,
        FREQUENT_FRAME_THRESH,
    );
    lost_frame_occurences.sort();
    info!("Merging events...");
    let mut merged_events = merge_events(traces, &new_root.metrics);

    if !lost_frame_occurences.is_empty() {
        let lost_frame_event = Event::from_occurences(
            trace_error::LostFrameWhileMerging::ID,
            trace_error::LostFrameWhileMerging::NAME.to_string(),
            trace_error::LostFrameWhileMerging::DESC.to_string(),
            lost_frame_occurences,
        )
        .expect("Failed to create lost frame event");
        merged_events.push(lost_frame_event);
    }

    Trace::new(new_root, merged_events)
}

trait Id: Clone {
    fn id(&self) -> u32;
}

#[derive(Clone, Copy)]
struct FrameIndexed<'a, 'b> {
    raw_trace_id: Option<&'b str>,
    original: &'a Frame,
    offset_in_parent: Metrics,
    // unique within a parent frame, stable across parent frames
    id: u32,
}

#[derive(Clone, Copy)]
struct PauseIndexed<'a> {
    original: &'a MetricsRange,
    offset_in_parent: Metrics,
    // unique within a parent frame, stable across parent frames
    id: u32,
}

#[derive(Clone, Copy)]
enum IndexedChild<'a, 'b> {
    Frame(FrameIndexed<'a, 'b>),
    Pause(PauseIndexed<'a>),
}

impl Id for IndexedChild<'_, '_> {
    fn id(&self) -> u32 {
        match self {
            IndexedChild::Frame(f) => f.id,
            IndexedChild::Pause(p) => p.id,
        }
    }
}

impl IndexedChild<'_, '_> {
    fn offset_in_parent(&self) -> &Metrics {
        match self {
            IndexedChild::Frame(f) => &f.offset_in_parent,
            IndexedChild::Pause(p) => &p.offset_in_parent,
        }
    }

    fn metrics(&self) -> &MetricsRange {
        match self {
            IndexedChild::Frame(f) => &f.original.metrics,
            IndexedChild::Pause(p) => p.original,
        }
    }
}

#[derive(Hash, Eq, PartialEq)]
struct IdMapKey<'a> {
    symbol: Option<&'a str>, // None signifies a Pause chunk
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
fn index_children<'a, 'b>(
    frames: &[&'a Frame],
    trace_ids: Option<&'b [&str]>,
) -> (u32, Vec<Vec<IndexedChild<'a, 'b>>>) {
    let mut indexed_children = Vec::with_capacity(frames.len());
    let mut symbol_ids: HashMap<IdMapKey, u32> = HashMap::new();
    let mut next_id = 0;

    for (i, &parent) in frames.iter().enumerate() {
        let trace_id = trace_ids.map(|ids| &ids[i][..]);

        let mut seen_symbols: HashMap<&str, u32> = HashMap::new();
        let mut seen_pauses = 0;

        let mut children = Vec::new();
        for chunk in parent.chunks() {
            match chunk {
                Chunk::Frame(frame) => {
                    let instance = seen_symbols
                        .entry(&frame.symbol.name)
                        .and_modify(|x| *x += 1)
                        .or_insert(0);
                    let key = IdMapKey {
                        symbol: Some(&frame.symbol.name),
                        instance: *instance,
                    };
                    let id = symbol_ids.entry(key).or_insert_with(|| {
                        next_id += 1;
                        next_id
                    });

                    children.push(IndexedChild::Frame(FrameIndexed {
                        raw_trace_id: trace_id,
                        original: frame,
                        offset_in_parent: frame.metrics.start - parent.metrics.start,
                        id: *id,
                    }));
                }
                Chunk::Pause(metrics) => {
                    let key = IdMapKey {
                        symbol: None,
                        instance: seen_pauses,
                    };
                    seen_pauses += 1;
                    let id = symbol_ids.entry(key).or_insert_with(|| {
                        next_id += 1;
                        next_id
                    });

                    children.push(IndexedChild::Pause(PauseIndexed {
                        original: &metrics,
                        offset_in_parent: metrics.start - parent.metrics.start,
                        id: *id,
                    }));
                }
                _ => continue,
            }
        }
        indexed_children.push(children);
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
/// If so, calls process_child() with all the frames containing that Id and
/// recurses on the part of the sequences before and after that Id.
/// `sequences` is a list of sequences, where each sequence is a list of items
/// with unique ids from 1..n.
///
/// process_child() is guarenteed to be called in original order. E.g., if the input sequences
/// are "a b c", "x, b, c", and "a, b, x", it will be called on "a" then "b" then "c".
///
/// Panics if `thresh` is not between 0 and 1
fn find_frequent_children<I: Id>(
    n: u32,
    sequences: &[&[I]],
    process_child: &mut impl FnMut(&[&I]) -> (),
    thresh: f32,
) {
    if thresh < 0.0 || thresh > 1.0 {
        panic!("Threshold must be between 0 and 1");
    }

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

            let mut matching_frames: Vec<&I> = Vec::with_capacity(sequences.len());
            let mut before: Vec<&[I]> = Vec::with_capacity(sequences.len());
            let mut after: Vec<&[I]> = Vec::with_capacity(sequences.len());
            'next_sequence: for i in 0..sequences.len() {
                let sequence = sequences[i];
                for (j, ele) in sequence.iter().enumerate() {
                    if ele.id() == id {
                        matching_frames.push(ele);
                        before.push(&sequence[0..j]);
                        after.push(&sequence[j + 1..]);
                        continue 'next_sequence;
                    }
                }
                let break_point = (index_avg * (sequence.len() as f32)).round() as usize;
                before.push(&sequence[0..break_point]);
                after.push(&sequence[break_point..]);
            }

            find_frequent_children(n, &before, process_child, thresh);
            process_child(&matching_frames);
            find_frequent_children(n, &after, process_child, thresh);
        }
    }
}

fn fill_annotations(frame: &mut Frame, latencies: &[Metrics], trace_ids: Option<&[&str]>) {
    frame.annotations.insert(
        ANNOTATION_COUNT_NAME.to_string(),
        Annotation::Uint64(latencies.len() as u64),
    );
    let ts_latencies = latencies.iter().map(|l| l.ts as f64);
    if let Some(stats) = Stats::from_data(ts_latencies) {
        let stats = stats
            .into_iter()
            .map(|(k, v)| (k, Annotation::Double(v)))
            .collect::<IndexMap<String, Annotation>>();
        frame
            .annotations
            .insert(ANNOTATION_STATS_NAME.to_string(), Annotation::Map(stats));
    }

    if let Some(ids) = trace_ids {
        let ts_latencies = latencies.iter().map(|l| l.ts);
        let raw_data = ids
            .iter()
            .zip(ts_latencies)
            .map(|(&id, l)| (id.to_string(), Annotation::Uint64(l)))
            .collect::<Vec<_>>();
        let raw_data_map = raw_data
            .into_iter()
            .collect::<IndexMap<String, Annotation>>();
        frame.annotations.insert(
            ANNOTATION_RAW_DATA_NAME.to_string(),
            Annotation::Map(raw_data_map),
        );
    }
}

fn constrain_metrics(
    target: &MetricsRange,
    min_metrics: &Metrics,
    max_metrics: &Metrics,
) -> Option<MetricsRange> {
    let mut result = target.clone();
    result.start.ts = max(result.start.ts, min_metrics.ts);
    result.start.cycles = max(result.start.cycles, min_metrics.cycles);
    result.start.insn_count = max(result.start.insn_count, min_metrics.insn_count);
    result.end.ts = min(result.end.ts, max_metrics.ts);
    result.end.cycles = min(result.end.cycles, max_metrics.cycles);
    result.end.insn_count = min(result.end.insn_count, max_metrics.insn_count);
    if result.start.ts <= result.end.ts
        && result.start.cycles <= result.end.cycles
        && result.start.insn_count <= result.end.insn_count
    {
        Some(result)
    } else {
        None
    }
}

fn merge_children(
    new_parent: &mut Frame,
    frames: &[&Frame],
    raw_trace_ids: Option<&[&str]>, // parallel list to frames
    lost_frame_occurrences: &mut Vec<Metrics>,
    frequent_thresh: f32,
) {
    let mut min_metrics = new_parent.metrics.start;
    let max_metrics = new_parent.metrics.end;

    let (n, indexed_children) = index_children(frames, raw_trace_ids);
    let mut sequences = indexed_children
        .iter()
        .map(|c| c.as_slice())
        .collect::<Vec<&[IndexedChild]>>();

    // precondition: `children` is nonempty
    // precondition: `children` is either all Frame chunks or all Pause chunks
    let mut add_averaged_child = |children: &[&IndexedChild]| {
        let offset_sum = children
            .iter()
            .map(|c| c.offset_in_parent())
            .sum::<Metrics>();
        let new_start = new_parent.metrics.start + offset_sum / (children.len() as u64);

        let latencies = children
            .iter()
            .map(|f| f.metrics().end - f.metrics().start)
            .collect::<Vec<_>>();
        let new_end = new_start + latencies.iter().sum::<Metrics>() / (children.len() as u64);

        let new_child_range = MetricsRange::new(new_start, new_end);
        if let Some(new_child_range) =
            constrain_metrics(&new_child_range, &min_metrics, &max_metrics)
        {
            min_metrics = new_child_range.end;

            match children.first().unwrap() {
                IndexedChild::Frame(first_frame) => {
                    // collect children's corresponding trace_ids, if they exist
                    let mut active_trace_ids = None;
                    if first_frame.raw_trace_id.is_some() {
                        active_trace_ids = Some(
                            children
                                .iter()
                                .map(|c| match c {
                                    IndexedChild::Frame(f) => {
                                        f.raw_trace_id.expect("All children should have trace_ids")
                                    }
                                    IndexedChild::Pause(_) => {
                                        panic!("Expected all children to be Frame chunks")
                                    }
                                })
                                .collect::<Vec<&str>>(),
                        );
                    }

                    // convert children from Vec<&IndexedChild> to Vec<&Frame>
                    let children = children
                        .into_iter()
                        .map(|c| match c {
                            IndexedChild::Frame(f) => f.original,
                            IndexedChild::Pause(_) => {
                                panic!("Expected all children to be Frame chunks")
                            }
                        })
                        .collect::<Vec<&Frame>>();

                    let mut merged_child =
                        Frame::new(new_child_range, first_frame.original.symbol.clone());
                    let active_trace_ids_slice = active_trace_ids.as_ref().map(|v| v.as_slice());
                    merge_children(
                        &mut merged_child,
                        &children,
                        active_trace_ids_slice,
                        lost_frame_occurrences,
                        frequent_thresh,
                    );

                    fill_annotations(&mut merged_child, &latencies, active_trace_ids_slice);

                    new_parent
                        .add_child(merged_child)
                        .expect("Merged child frame should be valid");
                }
                IndexedChild::Pause(_) => {
                    new_parent
                        .add_pause(new_child_range)
                        .expect("Merged pause chunk should be valid");
                }
            }
        } else {
            warn!(
                "At {}, merged frame/pause chunk couldn't be added to parent: {}",
                new_start, new_parent
            );
            lost_frame_occurrences.push(new_start);
        }
    };

    let lcs = find_lcs(n, &sequences);
    for id in lcs {
        let mut common_children = Vec::with_capacity(sequences.len());
        let mut subsequences = Vec::with_capacity(sequences.len());

        for sequence in sequences.iter_mut() {
            for i in 0..sequence.len() {
                let item = &sequence[i];
                if item.id() == id {
                    common_children.push(item);
                    subsequences.push(&sequence[0..i]);
                    *sequence = &sequence[i + 1..];
                    break;
                }
            }
        }
        // INVARIANT: subsequences.len() == sequences.len()

        find_frequent_children(n, &subsequences, &mut add_averaged_child, frequent_thresh);

        add_averaged_child(&common_children);
    }

    find_frequent_children(n, &sequences, &mut add_averaged_child, frequent_thresh);
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

fn merge_events(traces: &[&Trace], new_range: &MetricsRange) -> Vec<Event> {
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
