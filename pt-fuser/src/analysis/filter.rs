use std::str::FromStr;

use regex::Regex;

use crate::{
    analysis::FrameFinder,
    trace::{Frame, Trace, TraceError},
};

#[derive(Debug, Clone)]
pub struct Filter {
    pub target: Option<Regex>,
    pub errors_min: Option<u32>,
    pub errors_max: Option<u32>,
    pub duration_min: Option<u64>,
    pub duration_max: Option<u64>,
}

impl Filter {
    pub const HELP: &'static str = "A filter in the form \"[target=regex,] [errors_min=num,] [errors_max=num,] \
                                    [duration_min=num,] [duration_max=num]\". Traces with frames violating a filter \
                                    are ignored. If target regex is not provided, filter applies to root frames.";
}

impl Default for Filter {
    fn default() -> Self {
        Filter {
            target: None,
            errors_min: None,
            errors_max: None,
            duration_min: None,
            duration_max: None,
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
                "errors_min" => {
                    filter.errors_min = Some(
                        value
                            .parse::<u32>()
                            .map_err(|e| format!("Invalid number for errors_min: {}", e))?,
                    );
                }
                "errors_max" => {
                    filter.errors_max = Some(
                        value
                            .parse::<u32>()
                            .map_err(|e| format!("Invalid number for errors_max: {}", e))?,
                    );
                }
                "duration_min" => {
                    filter.duration_min = Some(
                        value
                            .parse::<u64>()
                            .map_err(|e| format!("Invalid number for duration_min: {}", e))?,
                    );
                }
                "duration_max" => {
                    filter.duration_max = Some(
                        value
                            .parse::<u64>()
                            .map_err(|e| format!("Invalid number for duration_max: {}", e))?,
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

pub fn filter_traces(mut traces: Vec<Trace>, filter: &Filter) -> Vec<Trace> {
    traces.retain(|trace| {
        let pred = |frame: &Frame| {
            if let Some(target) = &filter.target {
                target.is_match(&frame.symbol.name)
            } else {
                frame == trace.root_frame()
            }
        };
        let frame_finder = FrameFinder::new(trace.root_frame(), &pred);
        let mut num_errors = 0;
        let mut error_index = 0;
        let error_events = trace.get_event(TraceError::DataCollectionError as u32);
        for frame in frame_finder {
            if let Some(duration_min) = filter.duration_min {
                if frame.metrics.total_time() < duration_min {
                    return false;
                }
            }
            if let Some(duration_max) = filter.duration_max {
                if frame.metrics.total_time() > duration_max {
                    return false;
                }
            }

            if let Some(error_events) = error_events {
                let error_events = error_events.occurences();
                while error_index < error_events.len()
                    && error_events[error_index] < frame.metrics.end
                {
                    if error_events[error_index] >= frame.metrics.start {
                        num_errors += 1;
                    }
                    error_index += 1;
                }
            }
        }

        if let Some(errors_min) = filter.errors_min {
            if num_errors < errors_min {
                return false;
            }
        }
        if let Some(errors_max) = filter.errors_max {
            if num_errors > errors_max {
                return false;
            }
        }

        true
    });
    traces
}
