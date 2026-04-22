use egui_plot::{Bar, BarChart, Plot};

struct Bins {
    start: u64,
    step: u64,
    counts: Vec<u64>,
}

impl Bins {
    fn add_datapoint(&mut self, value: f64) {
        if value >= self.start as f64 {
            let index = ((value - self.start as f64) / self.step as f64).floor() as usize;
            if index < self.counts.len() {
                self.counts[index] += 1;
            }
        }
    }
}

impl Iterator for Bins {
    type Item = (f64, u64);

    fn next(&mut self) -> Option<Self::Item> {
        if self.counts.is_empty() {
            return None;
        }
        let count = self.counts.remove(0);
        let center = self.start as f64 + self.step as f64 / 2.0;
        self.start += self.step;
        Some((center, count))
    }
}

/// Uses Rice's rule to determine bin width.
fn bins(data: &[f64]) -> Bins {
    if data.is_empty() {
        return Bins {
            start: 0,
            step: 0,
            counts: Vec::new(),
        };
    }

    let n = data.len() as f64;
    let min = data.iter().cloned().fold(f64::INFINITY, f64::min);
    let max = data.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    let range = max - min;
    let bin_width = f64::max(range / (2.0 * n.cbrt()), 1f64);
    let step = bin_width.ceil() as u64;
    let start = min.floor() as u64;
    Bins {
        start,
        step,
        counts: vec![0; ((max.ceil() as u64 - start) / step + 1) as usize],
    }
}

fn create_histogram(name: &str, data: &[f64]) -> BarChart {
    if data.is_empty() {
        return BarChart::new(name, vec![]);
    }

    let mut bins = bins(data);
    for value in data {
        bins.add_datapoint(*value);
    }

    let mut bars = Vec::new();
    let width = bins.step as f64;
    for (center, count) in bins {
        bars.push(Bar::new(center, count as f64).width(width));
    }

    BarChart::new(name, bars)
}

pub struct HistogramApp<'a> {
    data: &'a [f64],
    name: String,
    x_axis: String,
    y_axis: String,
}

impl<'a> HistogramApp<'a> {
    pub fn new(name: String, data: &'a [f64], x_axis: String, y_axis: String) -> Self {
        Self {
            name,
            data,
            x_axis,
            y_axis,
        }
    }
}

impl<'a> eframe::App for HistogramApp<'a> {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default()
            .frame(egui::Frame::default().inner_margin(30.0))
            .show_inside(ui, |ui| {
                ui.vertical_centered(|ui| ui.heading(&self.name));
                Plot::new(&self.name)
                    .x_axis_label(&self.x_axis)
                    .y_axis_label(&self.y_axis)
                    .set_margin_fraction(egui::vec2(0.05, 0.05))
                    .show(ui, |plot_ui| {
                        let histogram = create_histogram(&self.name, self.data)
                            .element_formatter(Box::new(|bar, _chart| {
                                let width = bar.bar_width;
                                format!(
                                    "Bucket: [{}, {})\nFrequency: {}",
                                    bar.argument - width / 2.0,
                                    bar.argument + width / 2.0,
                                    bar.value
                                )
                            }))
                            .color(egui::Color32::LIGHT_BLUE);
                        plot_ui.bar_chart(histogram)
                    });
            });
    }
}
