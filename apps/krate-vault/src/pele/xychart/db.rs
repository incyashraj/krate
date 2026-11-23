//! Ports diagrams/xychart/db.ts.

use alloc::string::String;
use alloc::vec::Vec;

use crate::pele::js::{num_str, trim, trim_start};
use crate::pele::quadrant::db::tighten_lines;

/// Ports `interface XyText` (the parser makes only plain text).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct XyText {
    pub text: String,
}

/// Ports `interface DataPoint`.
#[derive(Debug, Clone, PartialEq)]
pub struct DataPoint {
    pub value: f64,
    pub label: String,
}

/// Ports `BandAxis | LinearAxis`.
#[derive(Debug, Clone, PartialEq)]
pub enum Axis {
    Band {
        title: String,
        categories: Vec<String>,
    },
    Linear {
        title: String,
        min: f64,
        max: f64,
    },
}

impl Axis {
    pub fn title(&self) -> &str {
        match self {
            Axis::Band { title, .. } | Axis::Linear { title, .. } => title,
        }
    }

    fn set_title(&mut self, t: String) {
        match self {
            Axis::Band { title, .. } | Axis::Linear { title, .. } => *title = t,
        }
    }
}

/// Ports `type PlotData`: each value with its place on the x axis; a
/// category with no value has None.
pub type PlotData = Vec<(String, Option<f64>)>;

/// Ports `interface Plot`.
#[derive(Debug, Clone, PartialEq)]
pub struct Plot {
    pub bar: bool,
    pub title: String,
    pub data: PlotData,
    pub point_labels: Option<Vec<String>>,
}

/// Ports `class XyChartDb`.
#[derive(Debug, Clone, PartialEq)]
pub struct XyChartDb {
    pub title: Option<String>,
    pub acc_title: Option<String>,
    pub acc_descr: Option<String>,
    /// Some(true) horizontal, Some(false) vertical.
    pub orientation: Option<bool>,
    pub x_axis: Axis,
    /// The y axis: its title, min and max.
    pub y_title: String,
    pub y_min: f64,
    pub y_max: f64,
    pub has_set_x_axis: bool,
    pub has_set_y_axis: bool,
    pub plots: Vec<Plot>,
}

impl Default for XyChartDb {
    fn default() -> Self {
        XyChartDb::new()
    }
}

impl XyChartDb {
    pub fn new() -> XyChartDb {
        XyChartDb {
            title: None,
            acc_title: None,
            acc_descr: None,
            orientation: None,
            x_axis: Axis::Band {
                title: String::new(),
                categories: Vec::new(),
            },
            y_title: String::new(),
            y_min: f64::INFINITY,
            y_max: f64::NEG_INFINITY,
            has_set_x_axis: false,
            has_set_y_axis: false,
            plots: Vec::new(),
        }
    }

    pub fn set_orientation(&mut self, orientation: &str) {
        self.orientation = Some(orientation == "horizontal");
    }

    pub fn set_x_axis_title(&mut self, title: &XyText) {
        self.x_axis.set_title(trim(&title.text).into());
    }

    pub fn set_x_axis_range_data(&mut self, min: f64, max: f64) {
        self.x_axis = Axis::Linear {
            title: self.x_axis.title().into(),
            min,
            max,
        };
        self.has_set_x_axis = true;
    }

    pub fn set_x_axis_band(&mut self, categories: &[XyText]) {
        self.x_axis = Axis::Band {
            title: self.x_axis.title().into(),
            categories: categories.iter().map(|c| trim(&c.text).into()).collect(),
        };
        self.has_set_x_axis = true;
    }

    pub fn set_y_axis_title(&mut self, title: &XyText) {
        self.y_title = trim(&title.text).into();
    }

    pub fn set_y_axis_range_data(&mut self, min: f64, max: f64) {
        self.y_min = min;
        self.y_max = max;
        self.has_set_y_axis = true;
    }

    /// Ports `plotData`.
    fn plot_data(&mut self, mut values: Vec<f64>) -> PlotData {
        if values.is_empty() {
            return Vec::new();
        }
        // The first series fixes the x range when no x-axis statement did.
        if !self.has_set_x_axis {
            self.set_x_axis_range_data(1.0, values.len() as f64);
        }
        if let Axis::Band { categories, .. } = &self.x_axis {
            if values.len() > categories.len() {
                values.truncate(categories.len());
            }
        }
        if !self.has_set_y_axis {
            let (mut min, mut max) = (self.y_min, self.y_max);
            for &value in &values {
                if value < min {
                    min = value;
                }
                if value > max {
                    max = value;
                }
            }
            self.y_min = min;
            self.y_max = max;
        }
        match &self.x_axis {
            Axis::Band { categories, .. } => categories
                .iter()
                .enumerate()
                .map(|(i, c)| (c.clone(), values.get(i).copied()))
                .collect(),
            Axis::Linear { min, max, .. } => {
                if values.len() == 1 {
                    return alloc::vec![(num_str(*min), Some(values[0]))];
                }
                let step = (max - min) / (values.len() as f64 - 1.0);
                values
                    .iter()
                    .enumerate()
                    .map(|(i, &v)| (num_str(min + i as f64 * step), Some(v)))
                    .collect()
            }
        }
    }

    /// Ports `addPlot`.
    fn add_plot(&mut self, bar: bool, title: &XyText, data: &[DataPoint]) {
        let values: Vec<f64> = data.iter().map(|d| d.value).collect();
        let plot_data = self.plot_data(values);
        let mut plot = Plot {
            bar,
            title: trim(&title.text).into(),
            data: plot_data,
            point_labels: None,
        };
        // Mermaid accepts labels on the points of a bar series and ignores them.
        if !bar && data.iter().any(|d| !trim(&d.label).is_empty()) {
            plot.point_labels = Some(data.iter().map(|d| trim(&d.label).into()).collect());
        }
        self.plots.push(plot);
    }

    pub fn set_line_data(&mut self, title: &XyText, data: &[DataPoint]) {
        self.add_plot(false, title, data);
    }

    pub fn set_bar_data(&mut self, title: &XyText, data: &[DataPoint]) {
        self.add_plot(true, title, data);
    }

    pub fn set_diagram_title(&mut self, text: &str) {
        self.title = Some(text.into());
    }

    pub fn set_acc_title(&mut self, text: &str) {
        self.acc_title = Some(trim_start(text).into());
    }

    pub fn set_acc_description(&mut self, text: &str) {
        self.acc_descr = Some(tighten_lines(text));
    }
}
