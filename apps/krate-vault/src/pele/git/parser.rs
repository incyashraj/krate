//! Ports diagrams/git/parser.ts: the Langium gitGraph grammar, by hand.

use alloc::rc::Rc;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use crate::pele::common::chars::{is_word, Text};
use crate::pele::common::tokens::{
    acc_descr_type, acc_descr_value, acc_title_type, acc_title_value, directive_type, keyword,
    newline_type, single_line_comment_type, string_type, string_value, title_before, title_value,
    tokenize, whitespace_type, yaml_type, Reader, TokenType,
};
use crate::pele::errors::PeleError;
use crate::pele::js;

// `const enum T`.
const CHERRY_PICK: i32 = 0;
const GIT_GRAPH_COLON: i32 = 1;
const HIGHLIGHT: i32 = 2;
const GIT_GRAPH: i32 = 3;
const CHECKOUT: i32 = 4;
const REVERSE: i32 = 5;
const PARENT: i32 = 6;
const COMMIT: i32 = 7;
const NORMAL: i32 = 8;
const BRANCH: i32 = 9;
const ORDER: i32 = 10;
const SWITCH: i32 = 11;
const TYPE: i32 = 12;
const MERGE: i32 = 13;
const MSG: i32 = 14;
const TAG: i32 = 15;
const ID: i32 = 16;
const LR: i32 = 17;
const TB: i32 = 18;
const BT: i32 = 19;
const COLON: i32 = 20;
const ACC_DESCR: i32 = 21;
const ACC_TITLE: i32 = 22;
const TITLE: i32 = 23;
const INT: i32 = 24;
const STRING: i32 = 25;
const NEWLINE: i32 = 26;
const REFERENCE: i32 = 31;

/// A character of `[-./\w]`.
fn name_char(c: u32) -> bool {
    is_word(c) || c == 45 || c == 46 || c == 47
}

/// REFERENCE's pattern: `/\w([-\./\w]*[-\w])?/y`.
fn reference(src: &Text, p: usize) -> Option<usize> {
    if !is_word(src.c(p)) {
        return None;
    }
    let mut e = p + 1;
    while e < src.len() && name_char(src.c(e)) {
        e += 1;
    }
    // The group ends at its last [-\w] character.
    while e > p + 1 && !(is_word(src.c(e - 1)) || src.c(e - 1) == 45) {
        e -= 1;
    }
    Some(e)
}

/// `[-./\w]*[-\w]` matches at `p` (the lookahead `titleBefore` is given).
fn name_rest(src: &Text, p: usize) -> bool {
    let mut e = p;
    while e < src.len() && name_char(src.c(e)) {
        if is_word(src.c(e)) || src.c(e) == 45 {
            return true;
        }
        e += 1;
    }
    false
}

/// INT's pattern: `/0|[1-9][0-9]*(?!\.)/y`.
fn int(src: &Text, p: usize) -> Option<usize> {
    let c = src.c(p);
    if c == 48 {
        return Some(p + 1);
    }
    if !(49..=57).contains(&c) {
        return None;
    }
    let mut e = p + 1;
    while (48..=57).contains(&src.c(e)) {
        e += 1;
    }
    while e > p {
        if src.c(e) != 46 {
            return Some(e);
        }
        e -= 1;
    }
    None
}

/// `[\t ]*accDescr\s*{`.
fn acc_descr_opener(src: &Text, p: usize) -> bool {
    let mut q = p;
    while src.c(q) == 9 || src.c(q) == 32 {
        q += 1;
    }
    if !src.starts_with("accDescr", q) {
        return false;
    }
    q += 8;
    while crate::pele::common::chars::is_space(src.c(q)) {
        q += 1;
    }
    src.c(q) == 123
}

/// Ports `GIT_TOKENS`.
pub fn git_tokens() -> Vec<TokenType> {
    let reference_at = REFERENCE as usize;
    let word = |name: &str| -> TokenType {
        let mut t = TokenType::lit(name, name);
        t.longer = vec![reference_at];
        t
    };
    let mut git_graph = keyword("gitGraph");
    git_graph.longer = vec![reference_at];
    let mut acc_descr = acc_descr_type();
    acc_descr.opener = Some(Rc::new(acc_descr_opener));
    vec![
        word("cherry-pick"),
        TokenType::lit("gitGraph:", "gitGraph:"),
        word("HIGHLIGHT"),
        git_graph,
        word("checkout"),
        word("REVERSE"),
        TokenType::lit("parent:", "parent:"),
        word("commit"),
        word("NORMAL"),
        word("branch"),
        TokenType::lit("order:", "order:"),
        word("switch"),
        TokenType::lit("type:", "type:"),
        word("merge"),
        TokenType::lit("msg:", "msg:"),
        TokenType::lit("tag:", "tag:"),
        TokenType::lit("id:", "id:"),
        word("LR"),
        word("TB"),
        word("BT"),
        TokenType::lit(":", ":"),
        acc_descr,
        acc_title_type(),
        title_before(name_rest),
        TokenType::re("INT", int, None),
        string_type(),
        newline_type(),
        whitespace_type(),
        yaml_type(),
        directive_type(),
        single_line_comment_type(),
        TokenType::re("REFERENCE", reference, None),
    ]
}

/// Ports `CommitTypeName`: 0 NORMAL, 1 REVERSE, 2 HIGHLIGHT.
pub type CommitTypeName = u8;

/// Ports the statements of `GitGraphAst`.
#[derive(Debug, Clone, PartialEq)]
pub enum Statement {
    Commit {
        id: Option<String>,
        message: Option<String>,
        tags: Vec<String>,
        kind: Option<CommitTypeName>,
    },
    Branch {
        name: String,
        order: Option<f64>,
    },
    Merge {
        branch: String,
        id: Option<String>,
        tags: Vec<String>,
        kind: Option<CommitTypeName>,
    },
    Checkout {
        branch: String,
    },
    CherryPicking {
        id: Option<String>,
        tags: Vec<String>,
        parent: Option<String>,
    },
}

/// Ports `interface GitGraphAst`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GitGraphAst {
    pub dir: Option<&'static str>,
    pub title: Option<String>,
    pub acc_title: Option<String>,
    pub acc_descr: Option<String>,
    pub statements: Vec<Statement>,
}

/// Ports `parseGit`.
pub fn parse_git(src: &str) -> Result<GitGraphAst, PeleError> {
    let types = git_tokens();
    let mut r = Reader::new(tokenize(src, &types, "gitGraph")?, &types, "gitGraph");
    let mut ast = GitGraphAst::default();

    let end_of_line = |r: &mut Reader| -> Result<(), PeleError> {
        if r.kind() == -1 {
            return Ok(());
        }
        if r.kind() != NEWLINE {
            return Err(r.fail("a line break or the end of input"));
        }
        while r.kind() == NEWLINE {
            r.i += 1;
        }
        Ok(())
    };
    let name = |r: &mut Reader| -> Result<String, PeleError> {
        if r.kind() == REFERENCE {
            return Ok(r.take());
        }
        if r.kind() == STRING {
            return Ok(string_value(&r.take()));
        }
        Err(r.fail("a branch name"))
    };
    let string =
        |r: &mut Reader| -> Result<String, PeleError> { Ok(string_value(&r.expect(STRING)?)) };
    let commit_type = |r: &mut Reader| -> Result<CommitTypeName, PeleError> {
        let k = r.kind();
        if k != NORMAL && k != REVERSE && k != HIGHLIGHT {
            return Err(r.fail("NORMAL, REVERSE, or HIGHLIGHT"));
        }
        r.i += 1;
        Ok(match k {
            REVERSE => 1,
            HIGHLIGHT => 2,
            _ => 0,
        })
    };

    while r.kind() == NEWLINE {
        r.i += 1;
    }
    if !r.accept(GIT_GRAPH_COLON) {
        r.expect(GIT_GRAPH)?;
        let next = r.kind_at(r.i + 1);
        let k = r.kind();
        if (k == LR || k == TB || k == BT) && next == COLON {
            ast.dir = Some(match k {
                LR => "LR",
                TB => "TB",
                _ => "BT",
            });
            r.i += 2;
        } else {
            r.accept(COLON);
        }
    }

    while r.kind() != -1 {
        match r.kind() {
            NEWLINE => r.i += 1,
            ACC_DESCR => {
                ast.acc_descr = Some(acc_descr_value(&r.take()));
                end_of_line(&mut r)?;
            }
            ACC_TITLE => {
                ast.acc_title = Some(acc_title_value(&r.take()));
                end_of_line(&mut r)?;
            }
            TITLE => {
                ast.title = Some(title_value(&r.take()));
                end_of_line(&mut r)?;
            }
            COMMIT => {
                r.i += 1;
                let (mut id, mut message, mut tags, mut kind) = (None, None, Vec::new(), None);
                loop {
                    if r.accept(ID) {
                        id = Some(string(&mut r)?);
                    } else if r.accept(TAG) {
                        tags.push(string(&mut r)?);
                    } else if r.accept(TYPE) {
                        kind = Some(commit_type(&mut r)?);
                    } else if r.accept(MSG) || r.kind() == STRING {
                        message = Some(string(&mut r)?);
                    } else {
                        break;
                    }
                }
                end_of_line(&mut r)?;
                ast.statements.push(Statement::Commit {
                    id,
                    message,
                    tags,
                    kind,
                });
            }
            BRANCH => {
                r.i += 1;
                let n = name(&mut r)?;
                let mut order = None;
                if r.accept(ORDER) {
                    order = Some(js::parse_int(&r.expect(INT)?, 10));
                }
                end_of_line(&mut r)?;
                ast.statements.push(Statement::Branch { name: n, order });
            }
            MERGE => {
                r.i += 1;
                let branch = name(&mut r)?;
                let (mut id, mut tags, mut kind) = (None, Vec::new(), None);
                loop {
                    if r.accept(ID) {
                        id = Some(string(&mut r)?);
                    } else if r.accept(TAG) {
                        tags.push(string(&mut r)?);
                    } else if r.accept(TYPE) {
                        kind = Some(commit_type(&mut r)?);
                    } else {
                        break;
                    }
                }
                end_of_line(&mut r)?;
                ast.statements.push(Statement::Merge {
                    branch,
                    id,
                    tags,
                    kind,
                });
            }
            CHECKOUT | SWITCH => {
                r.i += 1;
                let branch = name(&mut r)?;
                end_of_line(&mut r)?;
                ast.statements.push(Statement::Checkout { branch });
            }
            CHERRY_PICK => {
                r.i += 1;
                let (mut id, mut tags, mut parent) = (None, Vec::new(), None);
                loop {
                    if r.accept(ID) {
                        id = Some(string(&mut r)?);
                    } else if r.accept(TAG) {
                        tags.push(string(&mut r)?);
                    } else if r.accept(PARENT) {
                        parent = Some(string(&mut r)?);
                    } else {
                        break;
                    }
                }
                end_of_line(&mut r)?;
                ast.statements
                    .push(Statement::CherryPicking { id, tags, parent });
            }
            _ => return Err(r.fail("a statement, a title, or a line break")),
        }
    }
    Ok(ast)
}
