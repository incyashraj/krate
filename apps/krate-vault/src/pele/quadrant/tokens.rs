//! Ports diagrams/quadrant/tokens.ts: the Jison grammar's token numbers.

pub const END: i32 = 0;
pub const ERROR: i32 = 1;
pub const NEWLINE: i32 = 2;
pub const TITLE: i32 = 3;
pub const TITLE_VALUE: i32 = 4;
pub const ACC_TITLE: i32 = 5;
pub const ACC_TITLE_VALUE: i32 = 6;
pub const ACC_DESCR: i32 = 7;
pub const ACC_DESCR_VALUE: i32 = 8;
pub const ACC_DESCR_MULTILINE_VALUE: i32 = 9;
pub const X_AXIS: i32 = 10;
pub const Y_AXIS: i32 = 11;
pub const DELIMITER: i32 = 12;
pub const QUADRANT_1: i32 = 13;
pub const QUADRANT_2: i32 = 14;
pub const QUADRANT_3: i32 = 15;
pub const QUADRANT_4: i32 = 16;
pub const CLASSDEF: i32 = 17;
pub const MD_STR: i32 = 18;
pub const STR: i32 = 19;
pub const CLASS_NAME: i32 = 20;
pub const POINT_START: i32 = 21;
pub const POINT_X: i32 = 22;
pub const POINT_Y: i32 = 23;
pub const QUADRANT: i32 = 24;
pub const ALPHA: i32 = 25;
pub const UNICODE_TEXT: i32 = 26;
pub const COLON: i32 = 27;
pub const PLUS: i32 = 28;
pub const COMMA: i32 = 29;
pub const EQUALS: i32 = 30;
pub const MULT: i32 = 31;
pub const BRKT: i32 = 32;
pub const UNDERSCORE: i32 = 33;
pub const DOT: i32 = 34;
pub const AMP: i32 = 35;
pub const MINUS: i32 = 36;
pub const NUM: i32 = 37;
pub const SPACE: i32 = 38;
pub const SEMI: i32 = 39;
pub const PUNCTUATION: i32 = 40;
pub const EOF: i32 = 41;

/// Ports `TOKEN_NAMES`.
pub const TOKEN_NAMES: [&str; 42] = [
    "$end",
    "INVALID",
    "NEWLINE",
    "title",
    "title_value",
    "acc_title",
    "acc_title_value",
    "acc_descr",
    "acc_descr_value",
    "acc_descr_multiline_value",
    "X-AXIS",
    "Y-AXIS",
    "AXIS-TEXT-DELIMITER",
    "QUADRANT_1",
    "QUADRANT_2",
    "QUADRANT_3",
    "QUADRANT_4",
    "CLASSDEF",
    "MD_STR",
    "STR",
    "class_name",
    "point_start",
    "point_x",
    "point_y",
    "QUADRANT",
    "ALPHA",
    "UNICODE_TEXT",
    "COLON",
    "PLUS",
    "COMMA",
    "EQUALS",
    "MULT",
    "BRKT",
    "UNDERSCORE",
    "DOT",
    "AMP",
    "MINUS",
    "NUM",
    "SPACE",
    "SEMI",
    "PUNCTUATION",
    "EOF",
];
