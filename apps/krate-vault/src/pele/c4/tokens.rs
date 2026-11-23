//! Ports diagrams/c4/tokens.ts: the token types of the C4 lexer, numbered
//! as `const enum T`, and their names as the parser prints them.

#![allow(non_upper_case_globals)]

pub const END: u8 = 0;
pub const ERROR: u8 = 1;
pub const direction_tb: u8 = 2;
pub const direction_bt: u8 = 3;
pub const direction_rl: u8 = 4;
pub const direction_lr: u8 = 5;
pub const title: u8 = 6;
pub const accDescription: u8 = 7;
pub const acc_title: u8 = 8;
pub const acc_title_value: u8 = 9;
pub const acc_descr: u8 = 10;
pub const acc_descr_value: u8 = 11;
pub const acc_descr_multiline_value: u8 = 12;
pub const NEWLINE: u8 = 13;
pub const C4_CONTEXT: u8 = 14;
pub const C4_CONTAINER: u8 = 15;
pub const C4_COMPONENT: u8 = 16;
pub const C4_DYNAMIC: u8 = 17;
pub const C4_DEPLOYMENT: u8 = 18;
pub const PERSON: u8 = 19;
pub const PERSON_EXT: u8 = 20;
pub const SYSTEM: u8 = 21;
pub const SYSTEM_DB: u8 = 22;
pub const SYSTEM_QUEUE: u8 = 23;
pub const SYSTEM_EXT: u8 = 24;
pub const SYSTEM_EXT_DB: u8 = 25;
pub const SYSTEM_EXT_QUEUE: u8 = 26;
pub const CONTAINER: u8 = 27;
pub const CONTAINER_DB: u8 = 28;
pub const CONTAINER_QUEUE: u8 = 29;
pub const CONTAINER_EXT: u8 = 30;
pub const CONTAINER_EXT_DB: u8 = 31;
pub const CONTAINER_EXT_QUEUE: u8 = 32;
pub const COMPONENT: u8 = 33;
pub const COMPONENT_DB: u8 = 34;
pub const COMPONENT_QUEUE: u8 = 35;
pub const COMPONENT_EXT: u8 = 36;
pub const COMPONENT_EXT_DB: u8 = 37;
pub const COMPONENT_EXT_QUEUE: u8 = 38;
pub const ENTERPRISE_BOUNDARY: u8 = 39;
pub const SYSTEM_BOUNDARY: u8 = 40;
pub const BOUNDARY: u8 = 41;
pub const CONTAINER_BOUNDARY: u8 = 42;
pub const NODE: u8 = 43;
pub const NODE_L: u8 = 44;
pub const NODE_R: u8 = 45;
pub const REL: u8 = 46;
pub const BIREL: u8 = 47;
pub const REL_U: u8 = 48;
pub const REL_D: u8 = 49;
pub const REL_L: u8 = 50;
pub const REL_R: u8 = 51;
pub const REL_B: u8 = 52;
pub const REL_INDEX: u8 = 53;
pub const UPDATE_EL_STYLE: u8 = 54;
pub const UPDATE_REL_STYLE: u8 = 55;
pub const UPDATE_LAYOUT_CONFIG: u8 = 56;
pub const EOF_IN_STRUCT: u8 = 57;
pub const ATTRIBUTE_EMPTY: u8 = 58;
pub const STR: u8 = 59;
pub const STR_KEY: u8 = 60;
pub const STR_VALUE: u8 = 61;
pub const LBRACE: u8 = 62;
pub const RBRACE: u8 = 63;
pub const EOF: u8 = 64;

/// Ports `TOKEN_NAMES`.
pub const TOKEN_NAMES: [&str; 65] = [
    "$end",
    "INVALID",
    "direction_tb",
    "direction_bt",
    "direction_rl",
    "direction_lr",
    "title",
    "accDescription",
    "acc_title",
    "acc_title_value",
    "acc_descr",
    "acc_descr_value",
    "acc_descr_multiline_value",
    "NEWLINE",
    "C4_CONTEXT",
    "C4_CONTAINER",
    "C4_COMPONENT",
    "C4_DYNAMIC",
    "C4_DEPLOYMENT",
    "PERSON",
    "PERSON_EXT",
    "SYSTEM",
    "SYSTEM_DB",
    "SYSTEM_QUEUE",
    "SYSTEM_EXT",
    "SYSTEM_EXT_DB",
    "SYSTEM_EXT_QUEUE",
    "CONTAINER",
    "CONTAINER_DB",
    "CONTAINER_QUEUE",
    "CONTAINER_EXT",
    "CONTAINER_EXT_DB",
    "CONTAINER_EXT_QUEUE",
    "COMPONENT",
    "COMPONENT_DB",
    "COMPONENT_QUEUE",
    "COMPONENT_EXT",
    "COMPONENT_EXT_DB",
    "COMPONENT_EXT_QUEUE",
    "ENTERPRISE_BOUNDARY",
    "SYSTEM_BOUNDARY",
    "BOUNDARY",
    "CONTAINER_BOUNDARY",
    "NODE",
    "NODE_L",
    "NODE_R",
    "REL",
    "BIREL",
    "REL_U",
    "REL_D",
    "REL_L",
    "REL_R",
    "REL_B",
    "REL_INDEX",
    "UPDATE_EL_STYLE",
    "UPDATE_REL_STYLE",
    "UPDATE_LAYOUT_CONFIG",
    "EOF_IN_STRUCT",
    "ATTRIBUTE_EMPTY",
    "STR",
    "STR_KEY",
    "STR_VALUE",
    "LBRACE",
    "RBRACE",
    "EOF",
];

/// Ports `KEYWORDS` (lexer.ts): case-sensitive.
pub fn keyword(word: &str) -> Option<u8> {
    Some(match word {
        "C4Context" => C4_CONTEXT,
        "C4Container" => C4_CONTAINER,
        "C4Component" => C4_COMPONENT,
        "C4Dynamic" => C4_DYNAMIC,
        "C4Deployment" => C4_DEPLOYMENT,
        "Person" => PERSON,
        "Person_Ext" => PERSON_EXT,
        "System" => SYSTEM,
        "SystemDb" => SYSTEM_DB,
        "SystemQueue" => SYSTEM_QUEUE,
        "System_Ext" => SYSTEM_EXT,
        "SystemDb_Ext" => SYSTEM_EXT_DB,
        "SystemQueue_Ext" => SYSTEM_EXT_QUEUE,
        "Container" => CONTAINER,
        "ContainerDb" => CONTAINER_DB,
        "ContainerQueue" => CONTAINER_QUEUE,
        "Container_Ext" => CONTAINER_EXT,
        "ContainerDb_Ext" => CONTAINER_EXT_DB,
        "ContainerQueue_Ext" => CONTAINER_EXT_QUEUE,
        "Component" => COMPONENT,
        "ComponentDb" => COMPONENT_DB,
        "ComponentQueue" => COMPONENT_QUEUE,
        "Component_Ext" => COMPONENT_EXT,
        "ComponentDb_Ext" => COMPONENT_EXT_DB,
        "ComponentQueue_Ext" => COMPONENT_EXT_QUEUE,
        "Enterprise_Boundary" => ENTERPRISE_BOUNDARY,
        "System_Boundary" => SYSTEM_BOUNDARY,
        "Boundary" => BOUNDARY,
        "Container_Boundary" => CONTAINER_BOUNDARY,
        "Deployment_Node" => NODE,
        "Node" => NODE,
        "Node_L" => NODE_L,
        "Node_R" => NODE_R,
        "Rel" => REL,
        "BiRel" => BIREL,
        "Rel_Up" => REL_U,
        "Rel_U" => REL_U,
        "Rel_Down" => REL_D,
        "Rel_D" => REL_D,
        "Rel_Left" => REL_L,
        "Rel_L" => REL_L,
        "Rel_Right" => REL_R,
        "Rel_R" => REL_R,
        "Rel_Back" => REL_B,
        "RelIndex" => REL_INDEX,
        "UpdateElementStyle" => UPDATE_EL_STYLE,
        "UpdateRelStyle" => UPDATE_REL_STYLE,
        "UpdateLayoutConfig" => UPDATE_LAYOUT_CONFIG,
        _ => return None,
    })
}
