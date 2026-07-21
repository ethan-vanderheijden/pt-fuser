use std::{iter, str::FromStr};

use regex::Regex;

use crate::{
    analysis::FrameFinder,
    trace::{Event, Frame, Trace, metrics::MetricsRange, trace_error},
};

#[derive(Debug, Clone)]
pub struct Filter {
    pub target: Option<Regex>,
    pub min_errors: Option<u32>,
    pub max_errors: Option<u32>,
    pub min_latency: Option<u64>,
    pub max_latency: Option<u64>,
    pub min_interrupts: Option<u32>,
    pub max_interrupts: Option<u32>,
}

impl Filter {
    pub const HELP: &'static str = "A filter in the form \"[target=regex,] [min_errors=num,] [max_errors=num,] \
                                    [min_latency=µs,] [max_latency=µs,] [min_interrupts=num,] [max_interrupts=num]\". \
                                    Traces with frames violating a filter are ignored. If target regex is not \
                                    provided, filter applies to root frames.";
}

impl Default for Filter {
    fn default() -> Self {
        Filter {
            target: None,
            min_errors: None,
            max_errors: None,
            min_latency: None,
            max_latency: None,
            min_interrupts: None,
            max_interrupts: None,
        }
    }
}

impl FromStr for Filter {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut filter = Filter::default();
        for part in s.split(',') {
            let part = part.trim();
            let mut kv = part.splitn(2, '=');
            let Some(key) = kv.next() else {
                return Err(format!("Invalid filter part: {}", part));
            };
            let Some(value) = kv.next() else {
                return Err(format!("Invalid filter part: {}", part));
            };
            match key {
                "target" => {
                    filter.target =
                        Some(Regex::new(value).map_err(|e| format!("Invalid regex: {}", e))?);
                }
                "min_errors" => {
                    filter.min_errors = Some(
                        value
                            .parse::<u32>()
                            .map_err(|e| format!("Invalid number for min_errors: {}", e))?,
                    );
                }
                "max_errors" => {
                    filter.max_errors = Some(
                        value
                            .parse::<u32>()
                            .map_err(|e| format!("Invalid number for max_errors: {}", e))?,
                    );
                }
                "min_latency" => {
                    filter.min_latency = Some(
                        value
                            .parse::<u64>()
                            .map(|v| v * 1000) // Convert µs to ns
                            .map_err(|e| format!("Invalid number for min_latency: {}", e))?,
                    );
                }
                "max_latency" => {
                    filter.max_latency = Some(
                        value
                            .parse::<u64>()
                            .map(|v| v * 1000) // Convert µs to ns
                            .map_err(|e| format!("Invalid number for max_latency: {}", e))?,
                    );
                }
                "min_interrupts" => {
                    filter.min_interrupts = Some(
                        value
                            .parse::<u32>()
                            .map_err(|e| format!("Invalid number for min_interrupts: {}", e))?,
                    );
                }
                "max_interrupts" => {
                    filter.max_interrupts = Some(
                        value
                            .parse::<u32>()
                            .map_err(|e| format!("Invalid number for max_interrupts: {}", e))?,
                    );
                }
                _ => {
                    return Err(format!("Unknown filter keyword: {}", key));
                }
            }
        }
        Ok(filter)
    }
}

fn scan_event(event: &Event, cur_index: &mut usize, metric_range: &MetricsRange) -> u32 {
    let mut count = 0;
    let occurences = event.occurences();
    while *cur_index < occurences.len() && occurences[*cur_index] < metric_range.end {
        if occurences[*cur_index] >= metric_range.start {
            count += 1;
        }
        *cur_index += 1;
    }
    count
}

fn run_filter<'a>(
    frames: impl IntoIterator<Item = &'a Frame>,
    filter: &Filter,
    trace: &Trace,
) -> bool {
    let decoding_errors = trace.get_event(trace_error::DataCollectionError::ID);
    let interrupts = trace.get_event(trace_error::TraceInterrupted::ID);

    let mut num_errors = 0;
    let mut error_index = 0;

    let mut num_interrupts = 0;
    let mut interrupt_index = 0;

    for frame in frames {
        if let Some(duration_min) = filter.min_latency {
            if frame.metrics.total_time() < duration_min {
                return false;
            }
        }
        if let Some(duration_max) = filter.max_latency {
            if frame.metrics.total_time() > duration_max {
                return false;
            }
        }

        if let Some(decoding_errors) = decoding_errors {
            num_errors += scan_event(decoding_errors, &mut error_index, &frame.metrics);
        }

        if let Some(interrupts) = interrupts {
            num_interrupts += scan_event(interrupts, &mut interrupt_index, &frame.metrics);
        }
    }

    if filter.min_errors.is_some() && num_errors < filter.min_errors.unwrap() {
        return false;
    }
    if filter.max_errors.is_some() && num_errors > filter.max_errors.unwrap() {
        return false;
    }

    if filter.min_interrupts.is_some() && num_interrupts < filter.min_interrupts.unwrap() {
        return false;
    }
    if filter.max_interrupts.is_some() && num_interrupts > filter.max_interrupts.unwrap() {
        return false;
    }

    true
}

pub fn filter_traces(mut traces: Vec<Trace>, filter: &Filter) -> Vec<Trace> {
    traces.retain(|trace| {
        if let Some(target) = &filter.target {
            let pred = |frame: &Frame| target.is_match(&frame.symbol.name);
            run_filter(FrameFinder::new(trace.root_frame(), &pred), filter, trace)
        } else {
            run_filter(iter::once(trace.root_frame()), filter, trace)
        }
    });
    traces
}

pub fn filter_bitmap(traces: &Vec<Trace>, filter: &Filter) -> Vec<bool> {
    traces
        .iter()
        .map(|trace| {
            if let Some(target) = &filter.target {
                let pred = |frame: &Frame| target.is_match(&frame.symbol.name);
                run_filter(FrameFinder::new(trace.root_frame(), &pred), filter, trace)
            } else {
                run_filter(iter::once(trace.root_frame()), filter, trace)
            }
        })
        .collect()
}
