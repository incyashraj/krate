//! Ports diagrams/state/tokens.ts: the token types of the state lexer,
//! numbered as `const enum T`, and their names as the parser prints them.

#![allow(non_upper_case_globals)]

pub const END: u8 = 0;
pub const ERROR: u8 = 1;
pub const NL: u8 = 2;
pub const SD: u8 = 3;
pub const DESCR: u8 = 4;
pub const ARROW: u8 = 5;
pub const HIDE_EMPTY: u8 = 6;
pub const scale: u8 = 7;
pub const WIDTH: u8 = 8;
pub const COMPOSIT_STATE: u8 = 9;
pub const STRUCT_START: u8 = 10;
pub const STRUCT_STOP: u8 = 11;
pub const STATE_DESCR: u8 = 12;
pub const AS: u8 = 13;
pub const ID: u8 = 14;
pub const FORK: u8 = 15;
pub const JOIN: u8 = 16;
pub const CHOICE: u8 = 17;
pub const CONCURRENT: u8 = 18;
pub const note: u8 = 19;
pub const NOTE_TEXT: u8 = 20;
pub const acc_title: u8 = 21;
pub const acc_title_value: u8 = 22;
pub const acc_descr: u8 = 23;
pub const acc_descr_value: u8 = 24;
pub const acc_descr_multiline_value: u8 = 25;
pub const CLICK: u8 = 26;
pub const STRING: u8 = 27;
pub const HREF: u8 = 28;
pub const classDef: u8 = 29;
pub const CLASSDEF_ID: u8 = 30;
pub const DEFAULT_CLASSDEF_ID: u8 = 31;
pub const CLASSDEF_STYLEOPTS: u8 = 32;
pub const DEFAULT: u8 = 33;
pub const style: u8 = 34;
pub const STYLE_IDS: u8 = 35;
pub const STYLEDEF_STYLEOPTS: u8 = 36;
pub const class: u8 = 37;
pub const CLASSENTITY_IDS: u8 = 38;
pub const STYLECLASS: u8 = 39;
pub const direction_tb: u8 = 40;
pub const direction_bt: u8 = 41;
pub const direction_rl: u8 = 42;
pub const direction_lr: u8 = 43;
pub const EDGE_STATE: u8 = 44;
pub const STYLE_SEPARATOR: u8 = 45;
pub const left_of: u8 = 46;
pub const right_of: u8 = 47;
pub const INVALID: u8 = 48;

/// Ports `TOKEN_NAMES`.
pub const TOKEN_NAMES: [&str; 49] = [
    "$end",
    "INVALID",
    "NL",
    "SD",
    "DESCR",
    "-->",
    "HIDE_EMPTY",
    "scale",
    "WIDTH",
    "COMPOSIT_STATE",
    "STRUCT_START",
    "STRUCT_STOP",
    "STATE_DESCR",
    "AS",
    "ID",
    "FORK",
    "JOIN",
    "CHOICE",
    "CONCURRENT",
    "note",
    "NOTE_TEXT",
    "acc_title",
    "acc_title_value",
    "acc_descr",
    "acc_descr_value",
    "acc_descr_multiline_value",
    "CLICK",
    "STRING",
    "HREF",
    "classDef",
    "CLASSDEF_ID",
    "DEFAULT_CLASSDEF_ID",
    "CLASSDEF_STYLEOPTS",
    "DEFAULT",
    "style",
    "STYLE_IDS",
    "STYLEDEF_STYLEOPTS",
    "class",
    "CLASSENTITY_IDS",
    "STYLECLASS",
    "direction_tb",
    "direction_bt",
    "direction_rl",
    "direction_lr",
    "EDGE_STATE",
    "STYLE_SEPARATOR",
    "left_of",
    "right_of",
    "INVALID",
];
