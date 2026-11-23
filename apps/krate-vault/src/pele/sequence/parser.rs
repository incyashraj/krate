//! Ports diagrams/sequence/parser.ts: the Jison grammar, by hand.
//!
//! Pele builds Mermaid's nested statement lists and walks them in order;
//! the walk meets every statement in source order (a block's start, its
//! sections with their dividers, its end), so the port keeps each frame's
//! statements as one flat list in that order.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use super::db::{extract_wrap, line, Menu, NoteActor, SeqDb, SeqText, Stmt, LEFTOF, OVER, RIGHTOF};
use super::lexer::tokenize;
use super::tokens::*;
use crate::pele::common::chars::Text;
use crate::pele::errors::{syntax_error, PeleError};
use crate::pele::js::{self, trim};

/// `SIGNAL[t]`: the line type of an arrow token.
fn signal_of(t: i32) -> Option<i32> {
    Some(match t {
        SOLID_OPEN_ARROW => line::SOLID_OPEN,
        DOTTED_OPEN_ARROW => line::DOTTED_OPEN,
        SOLID_ARROW => line::SOLID,
        DOTTED_ARROW => line::DOTTED,
        BIDIRECTIONAL_SOLID_ARROW => line::BIDIRECTIONAL_SOLID,
        BIDIRECTIONAL_DOTTED_ARROW => line::BIDIRECTIONAL_DOTTED,
        SOLID_CROSS => line::SOLID_CROSS,
        DOTTED_CROSS => line::DOTTED_CROSS,
        SOLID_POINT => line::SOLID_POINT,
        DOTTED_POINT => line::DOTTED_POINT,
        SOLID_ARROW_TOP => line::SOLID_TOP,
        SOLID_ARROW_BOTTOM => line::SOLID_BOTTOM,
        STICK_ARROW_TOP => line::STICK_TOP,
        STICK_ARROW_BOTTOM => line::STICK_BOTTOM,
        SOLID_ARROW_TOP_DOTTED => line::SOLID_TOP_DOTTED,
        SOLID_ARROW_BOTTOM_DOTTED => line::SOLID_BOTTOM_DOTTED,
        STICK_ARROW_TOP_DOTTED => line::STICK_TOP_DOTTED,
        STICK_ARROW_BOTTOM_DOTTED => line::STICK_BOTTOM_DOTTED,
        SOLID_ARROW_TOP_REVERSE => line::SOLID_ARROW_TOP_REVERSE,
        SOLID_ARROW_BOTTOM_REVERSE => line::SOLID_ARROW_BOTTOM_REVERSE,
        STICK_ARROW_TOP_REVERSE => line::STICK_ARROW_TOP_REVERSE,
        STICK_ARROW_BOTTOM_REVERSE => line::STICK_ARROW_BOTTOM_REVERSE,
        SOLID_ARROW_TOP_REVERSE_DOTTED => line::SOLID_ARROW_TOP_REVERSE_DOTTED,
        SOLID_ARROW_BOTTOM_REVERSE_DOTTED => line::SOLID_ARROW_BOTTOM_REVERSE_DOTTED,
        STICK_ARROW_TOP_REVERSE_DOTTED => line::STICK_ARROW_TOP_REVERSE_DOTTED,
        STICK_ARROW_BOTTOM_REVERSE_DOTTED => line::STICK_ARROW_BOTTOM_REVERSE_DOTTED,
        _ => return None,
    })
}

// `const enum K`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum K {
    Top,
    Box,
    Loop,
    Rect,
    Opt,
    Break,
    Alt,
    Par,
    Critical,
}

/// Ports `interface Frame`.
struct Frame {
    kind: K,
    text: String,
    start_type: i32,
    list: Vec<Stmt>,
}

/// `BLOCK[t]`.
fn block_of(t: i32) -> Option<K> {
    Some(match t {
        T_LOOP => K::Loop,
        T_RECT => K::Rect,
        T_OPT => K::Opt,
        T_BREAK => K::Break,
        T_ALT => K::Alt,
        T_PAR | T_PAR_OVER => K::Par,
        T_CRITICAL => K::Critical,
        T_BOX => K::Box,
        _ => return None,
    })
}

const SIGNAL_TOKENS: &str = "'SOLID_OPEN_ARROW', 'DOTTED_OPEN_ARROW', 'SOLID_ARROW', 'DOTTED_ARROW', 'SOLID_CROSS', 'DOTTED_CROSS', 'SOLID_POINT', 'DOTTED_POINT'";

/// `text.substring(k)` past `k` characters of the Basic Multilingual Plane.
fn after(text: &str, k: usize) -> &str {
    match text.char_indices().nth(k) {
        Some((i, _)) => &text[i..],
        None => "",
    }
}

/// Ports `close`: a finished block's statements, with its start and end.
fn close(db: &SeqDb, frame: Frame) -> Vec<Stmt> {
    let text = frame.text;
    let msg = || Some(extract_wrap(&text));
    let (start, end) = match frame.kind {
        K::Box => (Stmt::BoxStart(db.parse_box_data(&text)), Stmt::BoxEnd),
        K::Loop => (
            Stmt::Block {
                signal_type: line::LOOP_START,
                text: msg(),
            },
            Stmt::Block {
                signal_type: line::LOOP_END,
                text: None,
            },
        ),
        K::Rect => (
            Stmt::Block {
                signal_type: line::RECT_START,
                text: msg(),
            },
            Stmt::Block {
                signal_type: line::RECT_END,
                text: None,
            },
        ),
        K::Opt => (
            Stmt::Block {
                signal_type: line::OPT_START,
                text: msg(),
            },
            Stmt::Block {
                signal_type: line::OPT_END,
                text: None,
            },
        ),
        K::Break => (
            Stmt::Block {
                signal_type: line::BREAK_START,
                text: msg(),
            },
            Stmt::Block {
                signal_type: line::BREAK_END,
                text: None,
            },
        ),
        K::Alt => (
            Stmt::Block {
                signal_type: line::ALT_START,
                text: msg(),
            },
            Stmt::Block {
                signal_type: line::ALT_END,
                text: None,
            },
        ),
        K::Par => (
            Stmt::Block {
                signal_type: frame.start_type,
                text: msg(),
            },
            Stmt::Block {
                signal_type: line::PAR_END,
                text: None,
            },
        ),
        _ => (
            Stmt::Block {
                signal_type: line::CRITICAL_START,
                text: msg(),
            },
            Stmt::Block {
                signal_type: line::CRITICAL_END,
                text: None,
            },
        ),
    };
    let mut out = Vec::with_capacity(frame.list.len() + 2);
    out.push(start);
    out.extend(frame.list);
    out.push(end);
    out
}

/// Ports `parseSequence`.
pub fn parse_sequence(source: &str, db: &mut SeqDb) -> Result<(), PeleError> {
    let src = Text::new(source);
    let toks = tokenize(&src);
    let types = &toks.types;
    let texts = &toks.texts;
    let starts = &toks.starts;
    let mut i = 0usize;

    let fail = |i: usize, expected: &str| -> PeleError {
        let t = types[i];
        let at = src.byte(starts[i]);
        if t == ERROR {
            return syntax_error("sequence", source, at, "", true);
        }
        syntax_error(
            "sequence",
            source,
            at,
            &format!("Expecting {}, got '{}'", expected, TOKEN_NAMES[t as usize]),
            false,
        )
    };
    let expect = |i: &mut usize, ty: i32| -> Result<String, PeleError> {
        if types[*i] != ty {
            return Err(fail(*i, &format!("'{}'", TOKEN_NAMES[ty as usize])));
        }
        *i += 1;
        Ok(texts[*i - 1].clone())
    };
    let actor = |i: &mut usize| -> Result<String, PeleError> {
        let t = types[*i];
        if t != ACTOR && t != T_LINK {
            return Err(fail(*i, "'ACTOR'"));
        }
        *i += 1;
        Ok(texts[*i - 1].clone())
    };
    let text2 = |i: &mut usize| -> Result<SeqText, PeleError> {
        let t = expect(i, TXT)?;
        Ok(extract_wrap(after(trim(&t), 1)))
    };
    let mention = |actor: &str| Stmt::Participant {
        create: false,
        destroy: false,
        actor: actor.into(),
        draw: None,
        description: None,
        config: None,
    };

    // Ports `participant`: the statement, and whether it was `destroy`.
    let participant = |i: &mut usize| -> Result<(String, bool, Stmt), PeleError> {
        let t = types[*i];
        *i += 1;
        if t == T_DESTROY {
            let a = actor(i)?;
            expect(i, NEWLINE)?;
            return Ok((
                a.clone(),
                true,
                Stmt::Participant {
                    create: false,
                    destroy: true,
                    actor: a,
                    draw: None,
                    description: None,
                    config: None,
                },
            ));
        }
        let a = actor(i)?;
        let mut config = None;
        if types[*i] == CONFIG_START && types[*i - 1] == ACTOR {
            *i += 1;
            let content = expect(i, CONFIG_CONTENT)?;
            expect(i, CONFIG_END)?;
            config = Some(String::from(trim(&content)));
        }
        let draw = Some(if t == T_PARTICIPANT {
            "participant"
        } else {
            "actor"
        });
        let mut description = None;
        if types[*i] == AS {
            *i += 1;
            let alias = expect(i, T_REST_OF_LINE)?;
            expect(i, NEWLINE)?;
            description = Some(extract_wrap(&alias));
        } else if types[*i] == NEWLINE {
            *i += 1;
        } else {
            return Err(fail(*i, "'AS', 'NEWLINE'"));
        }
        Ok((
            a.clone(),
            false,
            Stmt::Participant {
                create: false,
                destroy: false,
                actor: a,
                draw,
                description,
                config,
            },
        ))
    };

    // Ports `signal`.
    let signal = |i: &mut usize, from: String, out: &mut Vec<Stmt>| -> Result<(), PeleError> {
        let mut central_from = false;
        if types[*i] == CENTRAL {
            central_from = true;
            *i += 1;
        }
        let Some(signal_type) = signal_of(types[*i]) else {
            return Err(fail(*i, SIGNAL_TOKENS));
        };
        *i += 1;
        let mark = types[*i];
        if mark == CENTRAL || (!central_from && (mark == PLUS || mark == MINUS)) {
            *i += 1;
        }
        let to = actor(i)?;
        let msg = text2(i)?;
        out.push(mention(&from));
        out.push(mention(&to));
        let (mut activate, mut central) = (false, None);
        let mut extra: Vec<Stmt> = Vec::new();
        if central_from {
            let dual = mark == CENTRAL;
            activate = dual;
            central = Some(if dual {
                line::CENTRAL_CONNECTION_DUAL
            } else {
                line::CENTRAL_CONNECTION_REVERSE
            });
            if dual {
                extra.push(Stmt::Signal {
                    signal_type: line::CENTRAL_CONNECTION,
                    actor: to.clone(),
                });
            }
            extra.push(Stmt::Signal {
                signal_type: line::CENTRAL_CONNECTION_REVERSE,
                actor: from.clone(),
            });
        } else if mark == PLUS {
            activate = true;
            extra.push(Stmt::Signal {
                signal_type: line::ACTIVE_START,
                actor: to.clone(),
            });
        } else if mark == MINUS {
            extra.push(Stmt::Signal {
                signal_type: line::ACTIVE_END,
                actor: from.clone(),
            });
        } else if mark == CENTRAL {
            activate = true;
            central = Some(line::CENTRAL_CONNECTION);
            extra.push(Stmt::Signal {
                signal_type: line::CENTRAL_CONNECTION,
                actor: to.clone(),
            });
        }
        out.push(Stmt::Message {
            from,
            to,
            signal_type,
            msg,
            activate,
            central,
        });
        out.extend(extra);
        Ok(())
    };

    // Ports `menu`.
    let menu = |i: &mut usize, kind: Menu, out: &mut Vec<Stmt>| -> Result<(), PeleError> {
        *i += 1;
        let who = actor(i)?;
        let text = text2(i)?;
        out.push(mention(&who));
        out.push(Stmt::Menu {
            kind,
            actor: who,
            text,
        });
        Ok(())
    };

    // Ports `note`.
    let note = |i: &mut usize, out: &mut Vec<Stmt>| -> Result<(), PeleError> {
        *i += 1;
        let t = types[*i];
        if t == T_OVER {
            *i += 1;
            let first = actor(i)?;
            let mut ids = NoteActor::Two(first.clone(), first.clone());
            let mut who = alloc::vec![mention(&first)];
            if types[*i] == COMMA {
                *i += 1;
                let second = actor(i)?;
                who.push(mention(&second));
                ids = NoteActor::Two(first, second);
            }
            let text = text2(i)?;
            out.extend(who);
            out.push(Stmt::Note {
                actor: ids,
                placement: OVER,
                text,
            });
            return Ok(());
        }
        if t != T_LEFT_OF && t != T_RIGHT_OF {
            return Err(fail(*i, "'over', 'left_of', 'right_of'"));
        }
        *i += 1;
        let who = actor(i)?;
        let placement = if t == T_LEFT_OF { LEFTOF } else { RIGHTOF };
        let text = text2(i)?;
        out.push(mention(&who));
        out.push(Stmt::Note {
            actor: NoteActor::One(who),
            placement,
            text,
        });
        Ok(())
    };

    // Ports `autonumber`.
    let autonumber = |i: &mut usize| -> Result<Stmt, PeleError> {
        *i += 1;
        let (mut start, mut step, mut visible) = (None, None, true);
        if types[*i] == NUM {
            start = Some(js::number(&texts[*i]));
            *i += 1;
            step = Some(if types[*i] == NUM {
                *i += 1;
                js::number(&texts[*i - 1])
            } else {
                1.0
            });
        } else if types[*i] == T_OFF {
            *i += 1;
            visible = false;
        }
        if types[*i] != NEWLINE {
            return Err(fail(*i, "'NEWLINE', 'NUM', 'off'"));
        }
        *i += 1;
        Ok(Stmt::Autonumber {
            start,
            step,
            visible,
        })
    };

    while types[i] == NEWLINE {
        i += 1;
    }
    expect(&mut i, SD)?;

    let mut stack: Vec<Frame> = Vec::new();
    let mut frame = Frame {
        kind: K::Top,
        text: String::new(),
        start_type: 0,
        list: Vec::new(),
    };

    loop {
        let t = types[i];
        if frame.kind == K::Box {
            if t == T_PARTICIPANT || t == T_PARTICIPANT_ACTOR || t == T_DESTROY {
                let (_, _, node) = participant(&mut i)?;
                frame.list.push(node);
            } else if t == NEWLINE {
                i += 1;
            } else if t == T_END {
                i += 1;
                let parent = stack.pop().unwrap_or(Frame {
                    kind: K::Top,
                    text: String::new(),
                    start_type: 0,
                    list: Vec::new(),
                });
                let done = close(db, core::mem::replace(&mut frame, parent));
                frame.list.extend(done);
            } else {
                return Err(fail(
                    i,
                    "'end', 'NEWLINE', 'participant', 'participant_actor', 'destroy'",
                ));
            }
            continue;
        }
        match t {
            NEWLINE | INVALID => i += 1,
            T_PARTICIPANT | T_PARTICIPANT_ACTOR | T_DESTROY => {
                let (_, _, node) = participant(&mut i)?;
                frame.list.push(node);
            }
            T_CREATE => {
                i += 1;
                let next = types[i];
                if next != T_PARTICIPANT && next != T_PARTICIPANT_ACTOR && next != T_DESTROY {
                    return Err(fail(i, "'participant', 'participant_actor', 'destroy'"));
                }
                let (_, _, node) = participant(&mut i)?;
                let node = match node {
                    Stmt::Participant {
                        actor,
                        draw,
                        description,
                        config,
                        ..
                    } => Stmt::Participant {
                        create: true,
                        destroy: false,
                        actor,
                        draw,
                        description,
                        config,
                    },
                    other => other,
                };
                frame.list.push(node);
            }
            ACTOR => {
                let from = actor(&mut i)?;
                signal(&mut i, from, &mut frame.list)?;
                expect(&mut i, NEWLINE)?;
            }
            T_LINK => {
                let next = types[i + 1];
                if next == ACTOR || next == T_LINK {
                    menu(&mut i, Menu::ALink, &mut frame.list)?;
                } else {
                    let from = actor(&mut i)?;
                    signal(&mut i, from, &mut frame.list)?;
                }
                expect(&mut i, NEWLINE)?;
            }
            T_LINKS => {
                menu(&mut i, Menu::Links, &mut frame.list)?;
                expect(&mut i, NEWLINE)?;
            }
            T_PROPERTIES => {
                menu(&mut i, Menu::Properties, &mut frame.list)?;
                expect(&mut i, NEWLINE)?;
            }
            T_DETAILS => {
                menu(&mut i, Menu::Details, &mut frame.list)?;
                expect(&mut i, NEWLINE)?;
            }
            T_NOTE => {
                note(&mut i, &mut frame.list)?;
                expect(&mut i, NEWLINE)?;
            }
            T_AUTONUMBER => {
                let node = autonumber(&mut i)?;
                frame.list.push(node);
            }
            T_ACTIVATE | T_DEACTIVATE => {
                i += 1;
                let who = actor(&mut i)?;
                expect(&mut i, NEWLINE)?;
                frame.list.push(Stmt::Signal {
                    signal_type: if t == T_ACTIVATE {
                        line::ACTIVE_START
                    } else {
                        line::ACTIVE_END
                    },
                    actor: who,
                });
            }
            T_TITLE | T_LEGACY_TITLE => {
                let title = String::from(after(&texts[i], if t == T_TITLE { 6 } else { 7 }));
                i += 1;
                db.set_diagram_title(&title);
            }
            T_ACC_TITLE => {
                i += 1;
                let value = expect(&mut i, T_ACC_TITLE_VALUE)?;
                db.set_acc_title(trim(&value));
            }
            T_ACC_DESCR => {
                i += 1;
                let value = expect(&mut i, T_ACC_DESCR_VALUE)?;
                db.set_acc_description(trim(&value));
            }
            T_ACC_DESCR_MULTILINE_VALUE => {
                let value = String::from(trim(&texts[i]));
                i += 1;
                db.set_acc_description(&value);
            }
            T_LOOP | T_RECT | T_OPT | T_BREAK | T_ALT | T_PAR | T_PAR_OVER | T_CRITICAL | T_BOX => {
                i += 1;
                let text = expect(&mut i, T_REST_OF_LINE)?;
                let next = Frame {
                    kind: block_of(t).unwrap_or(K::Loop),
                    text,
                    start_type: if t == T_PAR_OVER {
                        line::PAR_OVER_START
                    } else {
                        line::PAR_START
                    },
                    list: Vec::new(),
                };
                stack.push(core::mem::replace(&mut frame, next));
            }
            T_ELSE | T_AND | T_OPTION => {
                let want = if t == T_ELSE {
                    K::Alt
                } else if t == T_AND {
                    K::Par
                } else {
                    K::Critical
                };
                if frame.kind != want {
                    return Err(fail(i, "'end'"));
                }
                i += 1;
                let text = expect(&mut i, T_REST_OF_LINE)?;
                let signal_type = if t == T_ELSE {
                    line::ALT_ELSE
                } else if t == T_AND {
                    line::PAR_AND
                } else {
                    line::CRITICAL_OPTION
                };
                frame.list.push(Stmt::Block {
                    signal_type,
                    text: Some(extract_wrap(&text)),
                });
            }
            T_END => {
                if frame.kind == K::Top {
                    return Err(fail(i, "'NEWLINE', 'EOF'"));
                }
                i += 1;
                let parent = stack.pop().unwrap_or(Frame {
                    kind: K::Top,
                    text: String::new(),
                    start_type: 0,
                    list: Vec::new(),
                });
                let done = close(db, core::mem::replace(&mut frame, parent));
                frame.list.extend(done);
            }
            END => {
                if frame.kind != K::Top {
                    return Err(fail(i, "'end'"));
                }
                return db.apply(&frame.list);
            }
            _ => return Err(fail(i, "'NEWLINE', 'ACTOR', 'participant', 'note', 'end'")),
        }
    }
}
