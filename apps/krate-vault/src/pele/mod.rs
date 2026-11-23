//! A port of Pele 1.13.0 (https://github.com/obsidianmd/pele, MIT, see
//! PELE-LICENSE.txt), the renderer Obsidian 1.15 draws ```mermaid blocks
//! with, so Vault lays diagrams out exactly as Obsidian does.
//!
//! `compound` and `layered` are its layout engine (src/layout/*.ts);
//! `measure` is its text measurer's fallback width table, used where the
//! port is checked against Pele itself (the oracle in
//! evidence/benchmarks/obsidian-vs-krate/pele-oracle, made by running
//! Pele headless under node).

pub mod layered;
pub mod layout;
