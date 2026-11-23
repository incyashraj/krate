//! The chart kinds against Pele itself (pie, quadrant, xychart, sankey,
//! timeline, journey, gantt, gitGraph, mindmap, sequence): each kind's
//! corpus diagram, and the cases in `oracle_<kind>.json`, made by
//! oracle_gen/gen-kinds.js from Pele's own specs, docs and parser tests
//! with Pele's build under node. Every SVG and every error message is
//! compared as [`super::oracle`] compares flowcharts.

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

/// The corpus diagram (rendered with `{idPrefix: 'x'}` and the clock of
/// the day it was captured) and the generated cases.
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
fn pies_match_pele() {
    run_kind(corpus!("pie"), include_str!("oracle_pie.json"));
}

#[test]
fn quadrants_match_pele() {
    run_kind(corpus!("quadrant"), include_str!("oracle_quadrant.json"));
}

#[test]
fn xycharts_match_pele() {
    run_kind(corpus!("xy"), include_str!("oracle_xychart.json"));
}

#[test]
fn sankeys_match_pele() {
    run_kind(corpus!("sankey"), include_str!("oracle_sankey.json"));
}

#[test]
fn timelines_match_pele() {
    run_kind(corpus!("timeline"), include_str!("oracle_timeline.json"));
}

#[test]
fn journeys_match_pele() {
    run_kind(corpus!("journey"), include_str!("oracle_journey.json"));
}

#[test]
fn gantts_match_pele() {
    run_kind(corpus!("gantt"), include_str!("oracle_gantt.json"));
}

#[test]
fn gits_match_pele() {
    run_kind(corpus!("git"), include_str!("oracle_git.json"));
}

#[test]
fn mindmaps_match_pele() {
    run_kind(corpus!("mindmap"), include_str!("oracle_mindmap.json"));
}

#[test]
fn sequences_match_pele() {
    run_kind(corpus!("sequence"), include_str!("oracle_sequence.json"));
}
