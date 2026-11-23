//! Ports diagrams/timeline/db.ts.

use alloc::string::String;
use alloc::vec::Vec;

use crate::pele::errors::{PeleError, PeleErrorCode};
use crate::pele::js::trim_start;
use crate::pele::quadrant::db::tighten_lines;

/// Ports `interface TimelinePeriod`.
#[derive(Debug, Clone, PartialEq)]
pub struct TimelinePeriod {
    pub text: String,
    pub events: Vec<String>,
    /// Name of the section in force when the period was declared, '' before the first one.
    pub section: String,
    /// Index into `sections` of that section, -1 before the first one.
    pub section_index: i64,
}

/// Ports `class TimelineDb`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TimelineDb {
    pub title: Option<String>,
    pub acc_title: Option<String>,
    pub acc_descr: Option<String>,
    /// True for `TD`, false for `LR`.
    pub td: bool,
    pub sections: Vec<String>,
    pub periods: Vec<TimelinePeriod>,
    current_section: String,
}

impl TimelineDb {
    pub fn set_diagram_title(&mut self, text: &str) {
        self.title = Some(text.into());
    }

    pub fn set_acc_title(&mut self, text: &str) {
        self.acc_title = Some(trim_start(text).into());
    }

    pub fn set_acc_description(&mut self, text: &str) {
        self.acc_descr = Some(tighten_lines(text));
    }

    /// `setDirection`: true for TD.
    pub fn set_direction(&mut self, td: bool) {
        self.td = td;
    }

    pub fn add_section(&mut self, text: &str) {
        self.current_section = text.into();
        self.sections.push(text.into());
    }

    /// Ports `addTask`.
    pub fn add_task(&mut self, period: &str, event: &str) {
        self.periods.push(TimelinePeriod {
            text: period.into(),
            events: if event.is_empty() {
                Vec::new()
            } else {
                alloc::vec![event.into()]
            },
            section: self.current_section.clone(),
            section_index: self.sections.len() as i64 - 1,
        });
    }

    /// Ports `addEvent`.
    pub fn add_event(&mut self, event: &str) -> Result<(), PeleError> {
        // Mermaid fails here too, with a TypeError.
        match self.periods.last_mut() {
            Some(period) => {
                period.events.push(event.into());
                Ok(())
            }
            None => Err(PeleError::of(
                "An event needs a time period before it.",
                PeleErrorCode::Semantic,
                "timeline",
            )),
        }
    }
}
