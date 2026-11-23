//! Ports diagrams/journey/db.ts.

use alloc::string::String;
use alloc::vec::Vec;

use crate::pele::js::{self, trim, trim_start};
use crate::pele::quadrant::db::tighten_lines;

/// Ports `interface JourneyTask`.
#[derive(Debug, Clone, PartialEq)]
pub struct JourneyTask {
    pub task: String,
    /// As written: any number, or NaN when the score is not one.
    pub score: f64,
    pub people: Vec<String>,
    /// Name of the section in force when the task was declared, '' before the first one.
    pub section: String,
}

/// Ports `journeyActors`: everyone named on a task, once each, in code unit order.
pub fn journey_actors(tasks: &[JourneyTask]) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for task in tasks {
        for name in &task.people {
            if !names.contains(name) {
                names.push(name.clone());
            }
        }
    }
    names.sort_by(|a, b| a.encode_utf16().cmp(b.encode_utf16()));
    names
}

/// Ports `class JourneyDb`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct JourneyDb {
    pub title: Option<String>,
    pub acc_title: Option<String>,
    pub acc_descr: Option<String>,
    pub sections: Vec<String>,
    pub tasks: Vec<JourneyTask>,
    current_section: String,
}

impl JourneyDb {
    pub fn set_diagram_title(&mut self, text: &str) {
        self.title = Some(text.into());
    }

    pub fn set_acc_title(&mut self, text: &str) {
        self.acc_title = Some(trim_start(text).into());
    }

    pub fn set_acc_description(&mut self, text: &str) {
        self.acc_descr = Some(tighten_lines(text));
    }

    pub fn add_section(&mut self, text: &str) {
        self.current_section = text.into();
        self.sections.push(text.into());
    }

    /// Ports `addTask`: `data` is the rest of the line from its first
    /// colon: `:score` or `:score:actor, actor`.
    pub fn add_task(&mut self, task: &str, data: &str) {
        let rest = data.get(1..).unwrap_or("");
        let pieces: Vec<&str> = rest.split(':').collect();
        let people = if pieces.len() == 1 {
            Vec::new()
        } else {
            pieces[1].split(',').map(|name| trim(name).into()).collect()
        };
        self.tasks.push(JourneyTask {
            task: task.into(),
            score: js::number(pieces[0]),
            people,
            section: self.current_section.clone(),
        });
    }
}
