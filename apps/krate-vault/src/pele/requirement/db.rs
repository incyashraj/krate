//! Ports diagrams/requirement/db.ts: what the requirement parser builds,
//! as Mermaid's RequirementDB keeps it.

use alloc::string::String;
use alloc::vec::Vec;

use super::types::{Relation, Requirement, RequirementClass, RequirementElement};
use crate::pele::class::db::newline_runs;
use crate::pele::common::ordered::OrderedMap;
use crate::pele::errors::{PeleError, PeleErrorCode};
use crate::pele::js;

/// Ports `RequirementType`, `RiskLevel`, `VerifyType` and `Relationships`.
pub mod values {
    pub const REQUIREMENT: &str = "Requirement";
    pub const FUNCTIONAL_REQUIREMENT: &str = "Functional Requirement";
    pub const INTERFACE_REQUIREMENT: &str = "Interface Requirement";
    pub const PERFORMANCE_REQUIREMENT: &str = "Performance Requirement";
    pub const PHYSICAL_REQUIREMENT: &str = "Physical Requirement";
    pub const DESIGN_CONSTRAINT: &str = "Design Constraint";
    pub const LOW_RISK: &str = "Low";
    pub const MED_RISK: &str = "Medium";
    pub const HIGH_RISK: &str = "High";
    pub const VERIFY_ANALYSIS: &str = "Analysis";
    pub const VERIFY_DEMONSTRATION: &str = "Demonstration";
    pub const VERIFY_INSPECTION: &str = "Inspection";
    pub const VERIFY_TEST: &str = "Test";
    pub const CONTAINS: &str = "contains";
    pub const COPIES: &str = "copies";
    pub const DERIVES: &str = "derives";
    pub const SATISFIES: &str = "satisfies";
    pub const VERIFIES: &str = "verifies";
    pub const REFINES: &str = "refines";
    pub const TRACES: &str = "traces";
}

/// Applying a class copies its declarations onto each node, so a short
/// text can ask for hundreds of millions of copies. Past this many the
/// diagram is refused (`MAX_STYLE_COPIES`).
const MAX_STYLE_COPIES: usize = 1_000_000;

/// A node: a requirement or an element, by name.
enum NodeRef {
    Requirement(String),
    Element(String),
}

/// Ports `class RequirementDb`.
#[derive(Debug, Clone)]
pub struct RequirementDb {
    pub direction: String,
    pub requirements: OrderedMap<Requirement>,
    pub elements: OrderedMap<RequirementElement>,
    pub relations: Vec<Relation>,
    pub classes: OrderedMap<RequirementClass>,
    pub title: Option<String>,
    pub acc_title: Option<String>,
    pub acc_descr: Option<String>,
    // The grammar reports a body's fields before the requirement or element they belong to.
    new_id: String,
    new_text: String,
    new_risk: String,
    new_verify_method: String,
    new_element_type: String,
    new_doc_ref: String,
    style_copies: usize,
}

impl Default for RequirementDb {
    fn default() -> Self {
        RequirementDb {
            direction: "TB".into(),
            requirements: OrderedMap::new(),
            elements: OrderedMap::new(),
            relations: Vec::new(),
            classes: OrderedMap::new(),
            title: None,
            acc_title: None,
            acc_descr: None,
            new_id: String::new(),
            new_text: String::new(),
            new_risk: String::new(),
            new_verify_method: String::new(),
            new_element_type: String::new(),
            new_doc_ref: String::new(),
            style_copies: 0,
        }
    }
}

impl RequirementDb {
    pub fn new() -> RequirementDb {
        RequirementDb::default()
    }

    /// Ports `copy`.
    fn copy(&mut self, target: NodeRef, styles: &[String]) -> Result<(), PeleError> {
        self.style_copies += styles.len();
        if self.style_copies > MAX_STYLE_COPIES {
            return Err(PeleError::of(
                "Requirement diagram applies too many style declarations.",
                PeleErrorCode::Limit,
                "requirement",
            ));
        }
        let list = match target {
            NodeRef::Requirement(k) => &mut self.requirements.get_mut(&k).unwrap().css_styles,
            NodeRef::Element(k) => &mut self.elements.get_mut(&k).unwrap().css_styles,
        };
        list.extend(styles.iter().cloned());
        Ok(())
    }

    /// Ports `setDirection`.
    pub fn set_direction(&mut self, dir: &str) {
        self.direction = dir.into();
    }

    /// Ports `addRequirement`.
    pub fn add_requirement(&mut self, name: &str, kind: &str) {
        if !self.requirements.contains_key(name) {
            let requirement = Requirement {
                name: name.into(),
                kind: kind.into(),
                requirement_id: self.new_id.clone(),
                text: self.new_text.clone(),
                risk: self.new_risk.clone(),
                verify_method: self.new_verify_method.clone(),
                css_styles: Vec::new(),
                classes: alloc::vec![String::from("default")],
            };
            self.requirements.insert(name, requirement);
        }
        self.new_id.clear();
        self.new_text.clear();
        self.new_risk.clear();
        self.new_verify_method.clear();
    }

    /// Ports `setNewReqId`.
    pub fn set_new_req_id(&mut self, id: &str) {
        self.new_id = id.into();
    }

    /// Ports `setNewReqText`.
    pub fn set_new_req_text(&mut self, text: &str) {
        self.new_text = text.into();
    }

    /// Ports `setNewReqRisk`.
    pub fn set_new_req_risk(&mut self, risk: &str) {
        self.new_risk = risk.into();
    }

    /// Ports `setNewReqVerifyMethod`.
    pub fn set_new_req_verify_method(&mut self, verify_method: &str) {
        self.new_verify_method = verify_method.into();
    }

    /// Ports `addElement`.
    pub fn add_element(&mut self, name: &str) {
        if !self.elements.contains_key(name) {
            let element = RequirementElement {
                name: name.into(),
                kind: self.new_element_type.clone(),
                doc_ref: self.new_doc_ref.clone(),
                css_styles: Vec::new(),
                classes: alloc::vec![String::from("default")],
            };
            self.elements.insert(name, element);
        }
        self.new_element_type.clear();
        self.new_doc_ref.clear();
    }

    /// Ports `setNewElementType`.
    pub fn set_new_element_type(&mut self, kind: &str) {
        self.new_element_type = kind.into();
    }

    /// Ports `setNewElementDocRef`.
    pub fn set_new_element_doc_ref(&mut self, doc_ref: &str) {
        self.new_doc_ref = doc_ref.into();
    }

    /// Ports `addRelationship`.
    pub fn add_relationship(&mut self, kind: &str, src: &str, dst: &str) {
        self.relations.push(Relation {
            kind: kind.into(),
            src: src.into(),
            dst: dst.into(),
        });
    }

    /// Ports `node`.
    fn node(&self, id: &str) -> Option<NodeRef> {
        if self.requirements.contains_key(id) {
            Some(NodeRef::Requirement(id.into()))
        } else if self.elements.contains_key(id) {
            Some(NodeRef::Element(id.into()))
        } else {
            None
        }
    }

    /// Ports `setCssStyle`.
    pub fn set_css_style(&mut self, ids: &[String], styles: &[String]) -> Result<(), PeleError> {
        for id in ids {
            // Mermaid stops at the first unknown id, leaving the rest of the list unstyled.
            if self.node(id).is_none() {
                return Ok(());
            }
            for s in styles {
                let parts: Vec<String> = if s.contains(',') {
                    s.split(',').map(String::from).collect()
                } else {
                    alloc::vec![s.clone()]
                };
                let node = self.node(id).unwrap();
                self.copy(node, &parts)?;
            }
        }
        Ok(())
    }

    /// Ports `setClass`.
    pub fn set_class(&mut self, ids: &[String], class_names: &[String]) -> Result<(), PeleError> {
        for id in ids {
            if self.node(id).is_none() {
                continue;
            }
            for name in class_names {
                match self.node(id).unwrap() {
                    NodeRef::Requirement(k) => self
                        .requirements
                        .get_mut(&k)
                        .unwrap()
                        .classes
                        .push(name.clone()),
                    NodeRef::Element(k) => self
                        .elements
                        .get_mut(&k)
                        .unwrap()
                        .classes
                        .push(name.clone()),
                }
                if let Some(styles) = self.classes.get(name).map(|c| c.styles.clone()) {
                    let node = self.node(id).unwrap();
                    self.copy(node, &styles)?;
                }
            }
        }
        Ok(())
    }

    /// Ports `defineClass`.
    pub fn define_class(&mut self, ids: &[String], style: &[String]) -> Result<(), PeleError> {
        for id in ids {
            if !self.classes.contains_key(id) {
                self.classes.insert(
                    id,
                    RequirementClass {
                        id: id.clone(),
                        styles: Vec::new(),
                        text_styles: Vec::new(),
                    },
                );
            }
            let style_class = self.classes.get_mut(id).unwrap();
            for s in style {
                if s.contains("color") {
                    style_class
                        .text_styles
                        .push(js::replace_first(s, "fill", "bgFill"));
                }
                style_class.styles.push(s.clone());
            }
            let split: Vec<String> = style
                .iter()
                .flat_map(|s| s.split(','))
                .map(String::from)
                .collect();
            let reqs: Vec<String> = self
                .requirements
                .values()
                .filter(|r| r.classes.contains(id))
                .map(|r| r.name.clone())
                .collect();
            for r in reqs {
                self.copy(NodeRef::Requirement(r), &split)?;
            }
            let els: Vec<String> = self
                .elements
                .values()
                .filter(|e| e.classes.contains(id))
                .map(|e| e.name.clone())
                .collect();
            for e in els {
                self.copy(NodeRef::Element(e), &split)?;
            }
        }
        Ok(())
    }

    /// Ports `setAccTitle`.
    pub fn set_acc_title(&mut self, text: &str) {
        self.acc_title = Some(js::trim_start(text).into());
    }

    /// Ports `setAccDescription`.
    pub fn set_acc_description(&mut self, text: &str) {
        self.acc_descr = Some(newline_runs(text));
    }
}
