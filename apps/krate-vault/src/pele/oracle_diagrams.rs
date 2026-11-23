//! The diagram kinds against Pele itself (class, state, ER, requirement,
//! C4, block): each kind's corpus diagram, and the cases in
//! `oracle_<kind>.json`, made by oracle_gen/gen-diagrams.js from Pele's own
//! specs, docs and parser tests with Pele's build under node. Every SVG and
//! every error message is compared as [`super::oracle`] compares
//! flowcharts.

use alloc::vec::Vec;

use super::oracle::run;
use crate::json;

macro_rules! corpus {
    ($name:literal) => {
        (
            $name,
            include_str!(concat!(
                "../../../../evidence/benchmarks/obsidian-vs-krate/pele-corpus/",
                $name,
                ".mmd"
            )),
            include_str!(concat!(
                "../../../../evidence/benchmarks/obsidian-vs-krate/pele-oracle/",
                $name,
                ".json"
            )),
        )
    };
}

/// The corpus diagram (rendered with `{idPrefix: 'x'}`) and the generated cases.
fn run_kind(corpus: (&str, &str, &str), extra: &str) {
    let (name, src, oracle) = corpus;
    let mut first = json::parse(oracle).expect("oracle json");
    first.set("name", json::Value::s(name));
    first.set("src", json::Value::s(src));
    let mut cases: Vec<json::Value> = alloc::vec![first];
    match json::parse(extra) {
        Some(json::Value::Arr(extra)) => cases.extend(extra),
        _ => panic!("the cases file is not a JSON array"),
    }
    run(cases);
}

#[test]
fn class_diagrams_match_pele() {
    run_kind(corpus!("class"), include_str!("oracle_class.json"));
}

#[test]
fn state_diagrams_match_pele() {
    run_kind(corpus!("state"), include_str!("oracle_state.json"));
}

#[test]
fn er_diagrams_match_pele() {
    run_kind(corpus!("er"), include_str!("oracle_er.json"));
}

#[test]
fn requirement_diagrams_match_pele() {
    run_kind(
        corpus!("requirement"),
        include_str!("oracle_requirement.json"),
    );
}

#[test]
fn c4_diagrams_match_pele() {
    run_kind(corpus!("c4"), include_str!("oracle_c4.json"));
}

#[test]
fn block_diagrams_match_pele() {
    run_kind(corpus!("block"), include_str!("oracle_block.json"));
}
