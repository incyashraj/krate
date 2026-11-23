//! Ports diagrams/xychart/tokens.ts: the Jison grammar's token numbers.

pub const END: i32 = 0;
pub const ERROR: i32 = 1;
pub const NEWLINE: i32 = 2;
pub const TITLE: i32 = 3;
pub const ACC_TITLE: i32 = 4;
pub const ACC_TITLE_VALUE: i32 = 5;
pub const ACC_DESCR: i32 = 6;
pub const ACC_DESCR_VALUE: i32 = 7;
pub const ACC_DESCR_MULTILINE_VALUE: i32 = 8;
pub const XYCHART: i32 = 9;
pub const CHART_ORIENTATION: i32 = 10;
pub const X_AXIS: i32 = 11;
pub const Y_AXIS: i32 = 12;
pub const ARROW_DELIMITER: i32 = 13;
pub const LINE: i32 = 14;
pub const BAR: i32 = 15;
pub const NUMBER_WITH_DECIMAL: i32 = 16;
pub const SQUARE_BRACES_START: i32 = 17;
pub const SQUARE_BRACES_END: i32 = 18;
pub const STR: i32 = 19;
pub const ALPHA: i32 = 20;
pub const COLON: i32 = 21;
pub const PLUS: i32 = 22;
pub const COMMA: i32 = 23;
pub const EQUALS: i32 = 24;
pub const MULT: i32 = 25;
pub const BRKT: i32 = 26;
pub const UNDERSCORE: i32 = 27;
pub const DOT: i32 = 28;
pub const AMP: i32 = 29;
pub const MINUS: i32 = 30;
pub const NUM: i32 = 31;
pub const SEMI: i32 = 32;
pub const EOF: i32 = 33;

/// Ports `TOKEN_NAMES`.
pub const TOKEN_NAMES: [&str; 34] = [
    "$end",
    "INVALID",
    "NEWLINE",
    "title",
    "acc_title",
    "acc_title_value",
    "acc_descr",
    "acc_descr_value",
    "acc_descr_multiline_value",
    "XYCHART",
    "CHART_ORIENTATION",
    "X_AXIS",
    "Y_AXIS",
    "ARROW_DELIMITER",
    "LINE",
    "BAR",
    "NUMBER_WITH_DECIMAL",
    "SQUARE_BRACES_START",
    "SQUARE_BRACES_END",
    "STR",
    "ALPHA",
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
    "SEMI",
    "EOF",
];
