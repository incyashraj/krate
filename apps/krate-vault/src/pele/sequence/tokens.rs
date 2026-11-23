//! Ports diagrams/sequence/tokens.ts: the Jison grammar's token numbers.

pub const END: i32 = 0;
pub const ERROR: i32 = 1;
pub const INVALID: i32 = 2;
pub const NEWLINE: i32 = 3;
pub const SD: i32 = 4;
pub const T_CREATE: i32 = 5;
pub const T_BOX: i32 = 6;
pub const T_REST_OF_LINE: i32 = 7;
pub const T_END: i32 = 8;
pub const T_AUTONUMBER: i32 = 9;
pub const NUM: i32 = 10;
pub const T_OFF: i32 = 11;
pub const T_ACTIVATE: i32 = 12;
pub const T_DEACTIVATE: i32 = 13;
pub const T_TITLE: i32 = 14;
pub const T_LEGACY_TITLE: i32 = 15;
pub const T_ACC_TITLE: i32 = 16;
pub const T_ACC_TITLE_VALUE: i32 = 17;
pub const T_ACC_DESCR: i32 = 18;
pub const T_ACC_DESCR_VALUE: i32 = 19;
pub const T_ACC_DESCR_MULTILINE_VALUE: i32 = 20;
pub const T_LOOP: i32 = 21;
pub const T_RECT: i32 = 22;
pub const T_OPT: i32 = 23;
pub const T_ALT: i32 = 24;
pub const T_PAR: i32 = 25;
pub const T_PAR_OVER: i32 = 26;
pub const T_CRITICAL: i32 = 27;
pub const T_BREAK: i32 = 28;
pub const T_OPTION: i32 = 29;
pub const T_AND: i32 = 30;
pub const T_ELSE: i32 = 31;
pub const T_PARTICIPANT: i32 = 32;
pub const AS: i32 = 33;
pub const T_PARTICIPANT_ACTOR: i32 = 34;
pub const T_DESTROY: i32 = 35;
pub const T_NOTE: i32 = 36;
pub const T_OVER: i32 = 37;
pub const T_LINKS: i32 = 38;
pub const T_LINK: i32 = 39;
pub const T_PROPERTIES: i32 = 40;
pub const T_DETAILS: i32 = 41;
pub const COMMA: i32 = 42;
pub const T_LEFT_OF: i32 = 43;
pub const T_RIGHT_OF: i32 = 44;
pub const PLUS: i32 = 45;
pub const MINUS: i32 = 46;
pub const CENTRAL: i32 = 47;
pub const ACTOR: i32 = 48;
pub const CONFIG_START: i32 = 49;
pub const CONFIG_CONTENT: i32 = 50;
pub const CONFIG_END: i32 = 51;
pub const TXT: i32 = 52;
pub const SOLID_OPEN_ARROW: i32 = 53;
pub const DOTTED_OPEN_ARROW: i32 = 54;
pub const SOLID_ARROW: i32 = 55;
pub const BIDIRECTIONAL_SOLID_ARROW: i32 = 56;
pub const DOTTED_ARROW: i32 = 57;
pub const BIDIRECTIONAL_DOTTED_ARROW: i32 = 58;
pub const SOLID_CROSS: i32 = 59;
pub const DOTTED_CROSS: i32 = 60;
pub const SOLID_POINT: i32 = 61;
pub const DOTTED_POINT: i32 = 62;
pub const SOLID_ARROW_TOP: i32 = 63;
pub const SOLID_ARROW_BOTTOM: i32 = 64;
pub const STICK_ARROW_TOP: i32 = 65;
pub const STICK_ARROW_BOTTOM: i32 = 66;
pub const SOLID_ARROW_TOP_DOTTED: i32 = 67;
pub const SOLID_ARROW_BOTTOM_DOTTED: i32 = 68;
pub const STICK_ARROW_TOP_DOTTED: i32 = 69;
pub const STICK_ARROW_BOTTOM_DOTTED: i32 = 70;
pub const SOLID_ARROW_TOP_REVERSE: i32 = 71;
pub const SOLID_ARROW_BOTTOM_REVERSE: i32 = 72;
pub const STICK_ARROW_TOP_REVERSE: i32 = 73;
pub const STICK_ARROW_BOTTOM_REVERSE: i32 = 74;
pub const SOLID_ARROW_TOP_REVERSE_DOTTED: i32 = 75;
pub const SOLID_ARROW_BOTTOM_REVERSE_DOTTED: i32 = 76;
pub const STICK_ARROW_TOP_REVERSE_DOTTED: i32 = 77;
pub const STICK_ARROW_BOTTOM_REVERSE_DOTTED: i32 = 78;

/// Ports `TOKEN_NAMES`.
pub const TOKEN_NAMES: [&str; 79] = [
    "$end",
    "INVALID",
    "INVALID",
    "NEWLINE",
    "SD",
    "create",
    "box",
    "restOfLine",
    "end",
    "autonumber",
    "NUM",
    "off",
    "activate",
    "deactivate",
    "title",
    "legacy_title",
    "acc_title",
    "acc_title_value",
    "acc_descr",
    "acc_descr_value",
    "acc_descr_multiline_value",
    "loop",
    "rect",
    "opt",
    "alt",
    "par",
    "par_over",
    "critical",
    "break",
    "option",
    "and",
    "else",
    "participant",
    "AS",
    "participant_actor",
    "destroy",
    "note",
    "over",
    "links",
    "link",
    "properties",
    "details",
    ",",
    "left_of",
    "right_of",
    "+",
    "-",
    "()",
    "ACTOR",
    "CONFIG_START",
    "CONFIG_CONTENT",
    "CONFIG_END",
    "TXT",
    "SOLID_OPEN_ARROW",
    "DOTTED_OPEN_ARROW",
    "SOLID_ARROW",
    "BIDIRECTIONAL_SOLID_ARROW",
    "DOTTED_ARROW",
    "BIDIRECTIONAL_DOTTED_ARROW",
    "SOLID_CROSS",
    "DOTTED_CROSS",
    "SOLID_POINT",
    "DOTTED_POINT",
    "SOLID_ARROW_TOP",
    "SOLID_ARROW_BOTTOM",
    "STICK_ARROW_TOP",
    "STICK_ARROW_BOTTOM",
    "SOLID_ARROW_TOP_DOTTED",
    "SOLID_ARROW_BOTTOM_DOTTED",
    "STICK_ARROW_TOP_DOTTED",
    "STICK_ARROW_BOTTOM_DOTTED",
    "SOLID_ARROW_TOP_REVERSE",
    "SOLID_ARROW_BOTTOM_REVERSE",
    "STICK_ARROW_TOP_REVERSE",
    "STICK_ARROW_BOTTOM_REVERSE",
    "SOLID_ARROW_TOP_REVERSE_DOTTED",
    "SOLID_ARROW_BOTTOM_REVERSE_DOTTED",
    "STICK_ARROW_TOP_REVERSE_DOTTED",
    "STICK_ARROW_BOTTOM_REVERSE_DOTTED",
];
