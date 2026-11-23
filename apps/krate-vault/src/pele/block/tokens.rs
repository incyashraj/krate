//! Ports diagrams/block/tokens.ts: the token types of the block lexer,
//! numbered as `const enum T`, and their names as the parser prints them.

#![allow(non_upper_case_globals)]

pub const END: u8 = 0;
pub const ERROR: u8 = 1;
pub const KEY: u8 = 2;
pub const ID_BLOCK: u8 = 3;
pub const COLUMNS: u8 = 4;
pub const MD_STR: u8 = 5;
pub const STR: u8 = 6;
pub const SPACE_BLOCK: u8 = 7;
pub const DEFAULT: u8 = 8;
pub const LINKSTYLE: u8 = 9;
pub const INTERPOLATE: u8 = 10;
pub const CLASSDEF: u8 = 11;
pub const DEFAULT_CLASSDEF_ID: u8 = 12;
pub const CLASSDEF_ID: u8 = 13;
pub const CLASSDEF_STYLEOPTS: u8 = 14;
pub const CLASS: u8 = 15;
pub const CLASSENTITY_IDS: u8 = 16;
pub const STYLECLASS: u8 = 17;
pub const STYLE: u8 = 18;
pub const STYLE_ENTITY_IDS: u8 = 19;
pub const STYLE_DEFINITION_DATA: u8 = 20;
pub const acc_title: u8 = 21;
pub const acc_title_value: u8 = 22;
pub const acc_descr: u8 = 23;
pub const acc_descr_value: u8 = 24;
pub const acc_descr_multiline_value: u8 = 25;
pub const end: u8 = 26;
pub const NODE_DSTART: u8 = 27;
pub const NODE_DEND: u8 = 28;
pub const BLOCK_ARROW_START: u8 = 29;
pub const BLOCK_ARROW_END: u8 = 30;
pub const DIR: u8 = 31;
pub const NODE_ID: u8 = 32;
pub const LINK: u8 = 33;
pub const START_LINK: u8 = 34;
pub const LINK_LABEL: u8 = 35;
pub const SIZE: u8 = 36;
pub const EOF: u8 = 37;

/// Ports `TOKEN_NAMES`.
pub const TOKEN_NAMES: [&str; 38] = [
    "$end",
    "INVALID",
    "BLOCK_DIAGRAM_KEY",
    "id-block",
    "COLUMNS",
    "MD_STR",
    "STR",
    "SPACE_BLOCK",
    "DEFAULT",
    "LINKSTYLE",
    "INTERPOLATE",
    "classDef",
    "DEFAULT_CLASSDEF_ID",
    "CLASSDEF_ID",
    "CLASSDEF_STYLEOPTS",
    "class",
    "CLASSENTITY_IDS",
    "STYLECLASS",
    "style",
    "STYLE_ENTITY_IDS",
    "STYLE_DEFINITION_DATA",
    "acc_title",
    "acc_title_value",
    "acc_descr",
    "acc_descr_value",
    "acc_descr_multiline_value",
    "end",
    "NODE_DSTART",
    "NODE_DEND",
    "BLOCK_ARROW_START",
    "BLOCK_ARROW_END",
    "DIR",
    "NODE_ID",
    "LINK",
    "START_LINK",
    "LINK_LABEL",
    "SIZE",
    "EOF",
];
