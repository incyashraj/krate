//! The modal engine, key by key: every motion, operator and mode the editor
//! promises, checked against what Vim does with the same keys.

use crate::buffer::{Buffer, Pos};
use crate::vim::{parse_keys, Ctx, Cursor, Effect, Input, Key, Mode, Vim};
use alloc::string::String;
use alloc::vec::Vec;

struct Ed {
    buf: Buffer,
    cur: Cursor,
    vim: Vim,
    fx: Vec<Effect>,
}

impl Ed {
    fn new(text: &str) -> Ed {
        Ed {
            buf: Buffer::from_text(text),
            cur: Cursor::default(),
            vim: Vim::new(),
            fx: Vec::new(),
        }
    }

    fn at(mut self, line: usize, col: usize) -> Ed {
        self.cur = Cursor::at(Pos::new(line, col));
        self.vim.sync_want(&self.buf, &mut self.cur);
        self
    }

    fn keys(mut self, keys: &str) -> Ed {
        for i in parse_keys(keys) {
            self.feed(i);
        }
        self
    }

    fn feed(&mut self, i: Input) {
        let mut ctx = Ctx {
            buf: &mut self.buf,
            cur: &mut self.cur,
            page: 20,
            top: 0,
        };
        let fx = self.vim.feed(&mut ctx, i);
        self.fx.extend(fx);
    }

    fn text(&self) -> String {
        self.buf.text()
    }

    fn pos(&self) -> (usize, usize) {
        (self.cur.pos.line, self.cur.pos.col)
    }
}

fn ed(text: &str) -> Ed {
    Ed::new(text)
}

const CODE: &str = "fn main() {\n    let total = add(2, 40);\n    println!(\"{total}\");\n}\n";

// ---- motions ----------------------------------------------------------------

#[test]
fn h_j_k_l_and_counts() {
    let e = ed("abcdef\nghijkl\nmn").at(0, 2).keys("l");
    assert_eq!(e.pos(), (0, 3));
    let e = e.keys("2h");
    assert_eq!(e.pos(), (0, 1));
    let e = e.keys("j");
    assert_eq!(e.pos(), (1, 1));
    let e = e.keys("$j");
    assert_eq!(e.pos(), (2, 1), "j after $ sticks to the line end");
    let e = e.keys("k");
    assert_eq!(e.pos(), (1, 5));
    let e = e.keys("10l");
    assert_eq!(e.pos(), (1, 5), "l stops at the last character");
    let e = e.keys("0kh");
    assert_eq!(e.pos(), (0, 0));
}

#[test]
fn j_and_k_keep_the_wanted_column_across_short_lines() {
    let e = ed("long line here\nab\nanother long one")
        .at(0, 10)
        .keys("j");
    assert_eq!(e.pos(), (1, 1));
    let e = e.keys("j");
    assert_eq!(e.pos(), (2, 10));
}

#[test]
fn word_motions_w_b_e_and_big_words() {
    let text = "let x = foo.bar(1);\n\n  next";
    let e = ed(text).at(0, 0).keys("w");
    assert_eq!(e.pos(), (0, 4));
    let e = e.keys("w");
    assert_eq!(e.pos(), (0, 6));
    let e = e.keys("ww");
    assert_eq!(e.pos(), (0, 11), "punctuation is its own word");
    let e = e.keys("e");
    assert_eq!(e.pos(), (0, 14));
    let e = e.keys("w");
    assert_eq!(e.pos(), (0, 15));
    let e = e.keys("www");
    assert_eq!(e.pos(), (1, 0), "an empty line is a word stop");
    let e = e.keys("w");
    assert_eq!(e.pos(), (2, 2));
    let e = e.keys("b");
    assert_eq!(e.pos(), (1, 0));
    let e = e.keys("b");
    assert_eq!(e.pos(), (0, 17));
    let e = ed(text).at(0, 8).keys("W");
    assert_eq!(
        e.pos(),
        (1, 0),
        "W skips the whole foo.bar(1); to the empty line"
    );
    let e = ed(text).at(0, 18).keys("B");
    assert_eq!(e.pos(), (0, 8));
    let e = ed(text).at(0, 8).keys("E");
    assert_eq!(e.pos(), (0, 18));
    let e = ed(text).at(0, 14).keys("ge");
    assert_eq!(e.pos(), (0, 11));
}

#[test]
fn line_motions_0_caret_dollar_and_g_underscore() {
    let e = ed("    let x = 1;   ").at(0, 8).keys("0");
    assert_eq!(e.pos(), (0, 0));
    let e = e.keys("^");
    assert_eq!(e.pos(), (0, 4));
    let e = e.keys("$");
    assert_eq!(e.pos(), (0, 16));
    let e = e.keys("g_");
    assert_eq!(e.pos(), (0, 13));
}

#[test]
fn gg_g_and_counted_line_jumps() {
    let e = ed("a\n  b\nc\nd").at(0, 0).keys("G");
    assert_eq!(e.pos(), (3, 0));
    let e = e.keys("gg");
    assert_eq!(e.pos(), (0, 0));
    let e = e.keys("2G");
    assert_eq!(e.pos(), (1, 2));
    let e = e.keys("3gg");
    assert_eq!(e.pos(), (2, 0));
    let e = e.keys(":2<CR>");
    assert_eq!(e.pos(), (1, 2));
    let e = e.keys(":$<CR>");
    assert_eq!(e.pos(), (3, 0));
}

#[test]
fn percent_matches_brackets_across_lines() {
    let e = ed(CODE).at(0, 0).keys("%");
    assert_eq!(
        e.pos(),
        (0, 8),
        "the first bracket on the line, then its match"
    );
    let e = ed(CODE).at(0, 10).keys("%");
    assert_eq!(e.pos(), (3, 0));
    let e = e.keys("%");
    assert_eq!(e.pos(), (0, 10));
}

#[test]
fn f_t_and_repeats() {
    let line = "a,b,c,d";
    let e = ed(line).at(0, 0).keys("f,");
    assert_eq!(e.pos(), (0, 1));
    let e = e.keys(";");
    assert_eq!(e.pos(), (0, 3));
    let e = e.keys(",");
    assert_eq!(e.pos(), (0, 1));
    let e = ed(line).at(0, 0).keys("t,");
    assert_eq!(e.pos(), (0, 0), "t stops before; already there");
    let e = ed(line).at(0, 0).keys("tc");
    assert_eq!(e.pos(), (0, 3));
    let e = e.keys(";");
    assert_eq!(e.pos(), (0, 3), "; after t does not stick on the same char");
    let e = ed(line).at(0, 6).keys("F,");
    assert_eq!(e.pos(), (0, 5));
    let e = ed(line).at(0, 6).keys("2F,");
    assert_eq!(e.pos(), (0, 3));
    let e = ed(line).at(0, 6).keys("Tb");
    assert_eq!(e.pos(), (0, 3));
}

#[test]
fn paragraphs_and_screen_lines() {
    let text = "a\nb\n\nc\nd\n\ne";
    let e = ed(text).at(0, 0).keys("}");
    assert_eq!(e.pos(), (2, 0));
    let e = e.keys("}");
    assert_eq!(e.pos(), (5, 0));
    let e = e.keys("{");
    assert_eq!(e.pos(), (2, 0));
    let e = ed(text).at(3, 0).keys("H");
    assert_eq!(e.pos(), (0, 0));
    let e = e.keys("L");
    assert_eq!(e.pos(), (6, 0));
    let e = e.keys("M");
    assert_eq!(e.pos(), (3, 0));
}

#[test]
fn search_forward_backward_n_and_star() {
    let text = "alpha beta\ngamma alpha\nbeta alpha";
    let e = ed(text).at(0, 0).keys("/alpha<CR>");
    assert_eq!(e.pos(), (1, 6));
    let e = e.keys("n");
    assert_eq!(e.pos(), (2, 5));
    let e = e.keys("n");
    assert_eq!(e.pos(), (0, 0), "wraps round to the top");
    assert!(e.vim.message.as_ref().unwrap().0.contains("BOTTOM"));
    let e = e.keys("N");
    assert_eq!(e.pos(), (2, 5));
    let e = ed(text).at(2, 9).keys("?beta<CR>");
    assert_eq!(e.pos(), (2, 0));
    let e = e.keys("n");
    assert_eq!(e.pos(), (0, 6));
    let e = ed(text).at(0, 6).keys("*");
    assert_eq!(e.pos(), (2, 0));
    let e = e.keys("#");
    assert_eq!(e.pos(), (0, 6));
    // Smartcase: lowercase matches either case; a capital is exact.
    let e = ed("Foo foo FOO").at(0, 0).keys("/foo<CR>");
    assert_eq!(e.pos(), (0, 4));
    let e = ed("foo Foo FOO").at(0, 0).keys("/FOO<CR>");
    assert_eq!(e.pos(), (0, 8));
    let e = ed("abc").at(0, 0).keys("/zzz<CR>");
    assert!(e.vim.message.as_ref().unwrap().1, "not found is an error");
}

#[test]
fn gd_jumps_to_the_definition_in_the_file() {
    let text = "fn helper() {}\nfn main() {\n    let count = 1;\n    helper();\n    count;\n}";
    let e = ed(text).at(3, 6).keys("gd");
    assert_eq!(e.pos(), (0, 3));
    let e = ed(text).at(4, 4).keys("gd");
    assert_eq!(e.pos(), (2, 8));
}

// ---- operators ----------------------------------------------------------------

#[test]
fn d_with_motions_and_counts() {
    assert_eq!(ed("one two three").at(0, 0).keys("dw").text(), "two three");
    assert_eq!(ed("one two three").at(0, 0).keys("2dw").text(), "three");
    assert_eq!(ed("one two three").at(0, 0).keys("d2w").text(), "three");
    assert_eq!(ed("one two three").at(0, 4).keys("de").text(), "one  three");
    assert_eq!(ed("one two three").at(0, 8).keys("db").text(), "one three");
    assert_eq!(ed("one two three").at(0, 4).keys("d$").text(), "one ");
    assert_eq!(ed("one two three").at(0, 4).keys("D").text(), "one ");
    assert_eq!(ed("    one two").at(0, 8).keys("d^").text(), "    two");
    assert_eq!(ed("one two three").at(0, 4).keys("d0").text(), "two three");
    assert_eq!(ed("a(b, c)d").at(0, 0).keys("dt)").text(), ")d");
    assert_eq!(ed("a(b, c)d").at(0, 0).keys("df,").text(), " c)d");
    assert_eq!(ed("x(a(b)c)y").at(0, 1).keys("d%").text(), "xy");
}

#[test]
fn dw_on_the_last_word_stays_on_its_line() {
    assert_eq!(
        ed("one two\nthree").at(0, 4).keys("dw").text(),
        "one \nthree"
    );
}

#[test]
fn dd_and_counted_dd_and_linewise_motions() {
    let text = "a\nb\nc\nd\ne";
    assert_eq!(ed(text).at(1, 0).keys("dd").text(), "a\nc\nd\ne");
    assert_eq!(ed(text).at(1, 0).keys("2dd").text(), "a\nd\ne");
    assert_eq!(ed(text).at(1, 0).keys("dj").text(), "a\nd\ne");
    assert_eq!(ed(text).at(2, 0).keys("dk").text(), "a\nd\ne");
    assert_eq!(ed(text).at(2, 0).keys("dG").text(), "a\nb");
    assert_eq!(ed(text).at(2, 0).keys("dgg").text(), "d\ne");
    assert_eq!(
        ed(text).at(4, 0).keys("dd").text(),
        "a\nb\nc\nd",
        "the last line takes the line end before it"
    );
    let e = ed("x\n    y\nz").at(0, 0).keys("dd");
    assert_eq!(e.pos(), (0, 4), "the cursor lands on the first non-blank");
    let r = e.vim.register('"').unwrap();
    assert!(r.linewise);
    assert_eq!(r.text, "x\n");
    assert_eq!(ed("only").at(0, 0).keys("dd").text(), "");
}

#[test]
fn c_with_motions_and_cc_keep_indent() {
    let e = ed("one two three").at(0, 0).keys("cwONE<Esc>");
    assert_eq!(e.text(), "ONE two three", "cw is ce on a word");
    assert_eq!(e.vim.mode, Mode::Normal);
    assert_eq!(
        ed("one two three").at(0, 4).keys("c$end<Esc>").text(),
        "one end"
    );
    assert_eq!(
        ed("one two three").at(0, 4).keys("Cend<Esc>").text(),
        "one end"
    );
    assert_eq!(
        ed("    let x = 1;\nnext")
            .at(0, 6)
            .keys("ccreturn;<Esc>")
            .text(),
        "    return;\nnext"
    );
    assert_eq!(ed("a\nb\nc").at(0, 0).keys("2ccX<Esc>").text(), "X\nc");
    assert_eq!(ed("abc").at(0, 1).keys("sZ<Esc>").text(), "aZc");
    assert_eq!(ed("  abc\nd").at(0, 3).keys("Snew<Esc>").text(), "  new\nd");
}

#[test]
fn y_and_p_charwise_and_linewise() {
    let e = ed("one two").at(0, 0).keys("yw$p");
    assert_eq!(e.text(), "one twoone ");
    let e = ed("one two").at(0, 4).keys("yiw0P");
    assert_eq!(e.text(), "twoone two");
    let e = ed("a\nb\nc").at(0, 0).keys("yyjp");
    assert_eq!(e.text(), "a\nb\na\nc");
    assert_eq!(e.pos(), (2, 0));
    let e = ed("a\nb\nc").at(1, 0).keys("yyP");
    assert_eq!(e.text(), "a\nb\nb\nc");
    let e = ed("a\nb").at(1, 0).keys("yyp");
    assert_eq!(e.text(), "a\nb\nb", "put below the last line");
    let e = ed("a\nb").at(0, 0).keys("Y2p");
    assert_eq!(e.text(), "a\na\na\nb");
    let e = ed("one two").at(0, 0).keys("dwP");
    assert_eq!(e.text(), "one two", "a delete fills the register");
    let e = ed("x").at(0, 0).keys("p");
    assert!(e.vim.message.is_some(), "putting nothing says so");
}

#[test]
fn named_registers_keep_their_own_text() {
    let e = ed("first\nsecond").at(0, 0).keys("\"ayyj\"byy\"ap");
    assert_eq!(e.text(), "first\nsecond\nfirst");
    let e = e.keys("\"bP");
    assert_eq!(e.text(), "first\nsecond\nsecond\nfirst");
    let e = ed("keep\ngone").at(1, 0).keys("yyk\"_ddp");
    assert_eq!(
        e.text(),
        "gone\ngone",
        "the black hole register keeps the yank"
    );
}

#[test]
fn x_x_r_tilde_and_join() {
    assert_eq!(ed("abcd").at(0, 1).keys("x").text(), "acd");
    assert_eq!(ed("abcd").at(0, 1).keys("2x").text(), "ad");
    assert_eq!(ed("abcd").at(0, 2).keys("X").text(), "acd");
    assert_eq!(ed("abcd").at(0, 1).keys("rZ").text(), "aZcd");
    assert_eq!(ed("abcd").at(0, 1).keys("3r-").text(), "a---");
    assert_eq!(
        ed("abcd").at(0, 1).keys("5r-").text(),
        "abcd",
        "r past the end does nothing"
    );
    assert_eq!(ed("abcd").at(0, 0).keys("~~").text(), "ABcd");
    assert_eq!(
        ed("let x =\n    1;").at(0, 0).keys("J").text(),
        "let x = 1;"
    );
    assert_eq!(ed("a\nb\nc").at(0, 0).keys("3J").text(), "a b c");
    assert_eq!(ed("f(\n    x\n)").at(0, 0).keys("3J").text(), "f( x)");
}

#[test]
fn indent_and_dedent_lines() {
    assert_eq!(ed("a\nb").at(0, 0).keys(">>").text(), "    a\nb");
    assert_eq!(ed("a\nb").at(0, 0).keys("2>>").text(), "    a\n    b");
    assert_eq!(ed("      a").at(0, 0).keys("<<").text(), "    a");
    assert_eq!(ed("a\nb\nc").at(0, 0).keys(">j").text(), "    a\n    b\nc");
    assert_eq!(ed("a\nb\nc").at(0, 0).keys("Vj>").text(), "    a\n    b\nc");
}

#[test]
fn case_operators() {
    assert_eq!(
        ed("hello world").at(0, 0).keys("gUiw").text(),
        "HELLO world"
    );
    assert_eq!(ed("HELLO").at(0, 0).keys("guu").text(), "hello");
    assert_eq!(ed("Hello").at(0, 0).keys("g~~").text(), "hELLO");
}

#[test]
fn text_objects_words_brackets_quotes() {
    assert_eq!(ed("call(one, two)").at(0, 6).keys("di(").text(), "call()");
    assert_eq!(ed("call(one, two)").at(0, 6).keys("da(").text(), "call");
    assert_eq!(
        ed("let s = \"hi there\";")
            .at(0, 10)
            .keys("ci\"yo<Esc>")
            .text(),
        "let s = \"yo\";"
    );
    assert_eq!(
        ed("let s = \"hi there\";").at(0, 10).keys("da\"").text(),
        "let s = ;"
    );
    assert_eq!(ed("one two three").at(0, 5).keys("daw").text(), "one three");
    assert_eq!(
        ed("one two three").at(0, 5).keys("diw").text(),
        "one  three"
    );
    assert_eq!(ed("v[1, [2, 3]]").at(0, 7).keys("di[").text(), "v[1, []]");
    assert_eq!(ed("v[1, [2, 3]]").at(0, 7).keys("d2i[").text(), "v[]");
    let block = "fn x() {\n    a;\n    b;\n}";
    assert_eq!(ed(block).at(1, 4).keys("di{").text(), "fn x() {\n}");
    assert_eq!(
        ed(block)
            .at(1, 4)
            .keys("ya{")
            .vim
            .register('"')
            .unwrap()
            .text,
        "{\n    a;\n    b;\n}"
    );
}

// ---- insert mode ------------------------------------------------------------

#[test]
fn i_a_i_a_o_o_enter_insert_where_vim_does() {
    assert_eq!(ed("abc").at(0, 1).keys("iX<Esc>").text(), "aXbc");
    assert_eq!(ed("abc").at(0, 1).keys("aX<Esc>").text(), "abXc");
    assert_eq!(ed("  abc").at(0, 4).keys("IX<Esc>").text(), "  Xabc");
    assert_eq!(ed("abc").at(0, 0).keys("AX<Esc>").text(), "abcX");
    assert_eq!(ed("a\nb").at(0, 0).keys("oX<Esc>").text(), "a\nX\nb");
    assert_eq!(ed("a\nb").at(1, 0).keys("OX<Esc>").text(), "a\nX\nb");
    let e = ed("abc").at(0, 1).keys("iXY<Esc>");
    assert_eq!(
        e.pos(),
        (0, 2),
        "Escape steps back onto the last typed char"
    );
}

#[test]
fn counted_insert_repeats_the_text() {
    assert_eq!(ed("").keys("3ihi <Esc>").text(), "hi hi hi ");
    assert_eq!(ed("x").at(0, 0).keys("2a-<Esc>").text(), "x--");
}

#[test]
fn enter_and_o_indent_like_rust() {
    let e = ed("fn main() {").at(0, 0).keys("A<CR>let x = 1;<Esc>");
    assert_eq!(e.text(), "fn main() {\n    let x = 1;");
    let e = ed("fn main() {}").at(0, 11).keys("i<CR>body<Esc>");
    assert_eq!(
        e.text(),
        "fn main() {\n    body\n}",
        "Enter between braces opens a block"
    );
    let e = ed("    if x {").at(0, 0).keys("oy();<CR>}<Esc>");
    assert_eq!(
        e.text(),
        "    if x {\n        y();\n    }",
        "a typed brace steps back a level"
    );
    let e = ed("    a").at(0, 0).keys("ob<Esc>");
    assert_eq!(e.text(), "    a\n    b");
}

#[test]
fn backspace_tab_and_insert_deletes() {
    assert_eq!(ed("abc").at(0, 2).keys("i<BS><Esc>").text(), "ac");
    assert_eq!(ed("ab\ncd").at(1, 0).keys("i<BS><Esc>").text(), "abcd");
    assert_eq!(ed("x").at(0, 0).keys("i<Tab><Esc>").text(), "    x");
    assert_eq!(
        ed("        x").at(0, 8).keys("i<BS><Esc>").text(),
        "    x",
        "backspace in an indent steps a level"
    );
    assert_eq!(ed("one two").at(0, 7).keys("a<C-w><Esc>").text(), "one ");
    assert_eq!(ed("one two").at(0, 6).keys("a<C-u><Esc>").text(), "");
}

#[test]
fn arrows_in_insert_mode_move_and_split_undo() {
    let e = ed("abc").at(0, 0).keys("iX<Right><Right>Y<Esc>");
    assert_eq!(e.text(), "XabYc");
    let e = e.keys("u");
    assert_eq!(e.text(), "Xabc", "the arrow ended the first undo step");
}

// ---- undo, redo, repeat -------------------------------------------------------

#[test]
fn u_and_ctrl_r_undo_whole_commands() {
    let e = ed("one two three").at(0, 0).keys("dwdw");
    assert_eq!(e.text(), "three");
    let e = e.keys("u");
    assert_eq!(e.text(), "two three");
    let e = e.keys("u");
    assert_eq!(e.text(), "one two three");
    assert_eq!(e.pos(), (0, 0));
    let e = e.keys("<C-r>");
    assert_eq!(e.text(), "two three");
    let e = e.keys("2u");
    assert_eq!(e.text(), "one two three");
    let e = e.keys("2<C-r>");
    assert_eq!(e.text(), "three");
    let e = ed("abc").at(0, 0).keys("ixyz<Esc>u");
    assert_eq!(e.text(), "abc", "one insert is one undo step");
    let e = ed("a").keys("u");
    assert!(e.vim.message.as_ref().unwrap().0.contains("oldest"));
}

#[test]
fn dot_repeats_the_last_change() {
    assert_eq!(ed("a b c d").at(0, 0).keys("dw..").text(), "d");
    assert_eq!(ed("a\nb\nc\nd\ne").at(0, 0).keys("dd.").text(), "c\nd\ne");
    assert_eq!(ed("a\nb\nc\nd\ne").at(0, 0).keys("2dd.").text(), "e");
    assert_eq!(
        ed("a\nb\nc\nd\ne").at(0, 0).keys("dd3.").text(),
        "e",
        "a count replaces the old one"
    );
    assert_eq!(ed("x\ny").at(0, 0).keys("A;<Esc>j.").text(), "x;\ny;");
    assert_eq!(
        ed("one two").at(0, 0).keys("cwbig<Esc>w.").text(),
        "big big"
    );
    assert_eq!(ed("abcdef").at(0, 0).keys("x..").text(), "def");
    assert_eq!(ed("a\nb").at(0, 0).keys(">>j.").text(), "    a\n    b");
    // Motions and yanks are not changes: `.` keeps the change before them.
    assert_eq!(ed("a b c d").at(0, 0).keys("dwwyw.").text(), "b d");
}

// ---- visual and select ----------------------------------------------------------

#[test]
fn visual_char_and_line_operators() {
    let e = ed("hello world").at(0, 0).keys("ve");
    assert_eq!(e.vim.mode, Mode::Visual);
    let (a, b, lw) = e.vim.selection(&e.buf, &e.cur).unwrap();
    assert_eq!((a, b, lw), (Pos::new(0, 0), Pos::new(0, 5), false));
    assert_eq!(e.keys("d").text(), " world");
    assert_eq!(
        ed("hello world").at(0, 6).keys("vey0P").text(),
        "worldhello world"
    );
    assert_eq!(ed("a\nb\nc").at(0, 0).keys("Vjd").text(), "c");
    assert_eq!(
        ed("a\nb\nc")
            .at(0, 0)
            .keys("Vjy")
            .vim
            .register('"')
            .unwrap()
            .text,
        "a\nb\n"
    );
    assert_eq!(ed("hello").at(0, 0).keys("vlcY<Esc>").text(), "Yllo");
    assert_eq!(ed("hello").at(0, 0).keys("v$U").text(), "HELLO");
    assert_eq!(ed("a\nb\nc").at(0, 0).keys("VjJ").text(), "a b\nc");
    let e = ed("abc def").at(0, 0).keys("vlo");
    assert_eq!(e.pos(), (0, 0), "o swaps the ends");
    let e = e.keys("<Esc>");
    assert_eq!(e.vim.mode, Mode::Normal);
    assert_eq!(ed("abc def").at(0, 5).keys("viwd").text(), "abc ");
    assert_eq!(ed("x\ny").at(0, 0).keys("yyjVp").text(), "x\nx");
    assert_eq!(ed("one\ntwo\nthree").at(0, 1).keys("vjD").text(), "three");
}

#[test]
fn visual_colon_runs_on_the_selected_lines() {
    let e = ed("a a\na a\na a").at(0, 0).keys("Vj:s/a/b/g<CR>");
    assert_eq!(e.text(), "b b\nb b\na a");
}

#[test]
fn select_mode_typing_replaces_the_selection() {
    let mut e = ed("hello world").at(0, 0);
    // A mouse drag over "hello": anchor at 0, cursor at 5.
    e.cur.pos = Pos::new(0, 5);
    {
        let anchor = Pos::new(0, 0);
        e.vim.begin_select(&mut e.buf, &mut e.cur, anchor);
    }
    assert_eq!(e.vim.mode, Mode::Select);
    let e = e.keys("howdy");
    assert_eq!(e.text(), "howdy world");
    assert_eq!(e.vim.mode, Mode::Insert);
    let e = e.keys("<Esc>u");
    assert_eq!(e.text(), "hello world");
}

#[test]
fn shift_arrows_select_and_backspace_deletes_it() {
    let e = ed("abcdef").at(0, 0).keys("i<S-Right><S-Right>");
    assert_eq!(e.vim.mode, Mode::Select);
    let e = e.keys("<BS>");
    assert_eq!(e.text(), "cdef");
    assert_eq!(e.vim.mode, Mode::Insert);
    let e = ed("abcdef").at(0, 0).keys("i<S-Right><Left>");
    assert_eq!(
        e.vim.mode,
        Mode::Insert,
        "an arrow drops the selection back to typing"
    );
}

#[test]
fn escape_always_returns_to_normal() {
    for keys in [
        "i", "v", "V", ":", "/", "d", "2d", "\"a", "f", "c", "Vj", "g", "z",
    ] {
        let e = ed("abc\ndef").at(0, 0).keys(keys).keys("<Esc>");
        assert_eq!(e.vim.mode, Mode::Normal, "after {keys}");
        let e = e.keys("<Esc>");
        assert_eq!(e.vim.mode, Mode::Normal);
        // And nothing was left half-typed.
        assert_eq!(
            e.keys("x").text().len(),
            "abc\ndef".len() - 1,
            "after {keys}"
        );
    }
}

// ---- the command line -------------------------------------------------------------

#[test]
fn substitute_on_the_line_and_the_file() {
    let text = "foo foo\nfoo bar\nbaz";
    assert_eq!(
        ed(text).at(0, 0).keys(":s/foo/x/<CR>").text(),
        "x foo\nfoo bar\nbaz"
    );
    assert_eq!(
        ed(text).at(0, 0).keys(":s/foo/x/g<CR>").text(),
        "x x\nfoo bar\nbaz"
    );
    let e = ed(text).at(0, 0).keys(":%s/foo/x/g<CR>");
    assert_eq!(e.text(), "x x\nx bar\nbaz");
    assert_eq!(
        e.vim.message.as_ref().unwrap().0,
        "3 substitutions on 2 lines"
    );
    assert_eq!(
        ed(text).at(0, 0).keys(":2,3s/ba/BA/<CR>").text(),
        "foo foo\nfoo BAr\nBAz"
    );
    assert_eq!(
        ed("a/b").at(0, 0).keys(":s#/#\\\\#<CR>").text(),
        "a\\\\b".replace("\\\\", "\\")
    );
    assert_eq!(ed("a.b").at(0, 0).keys(":s/./[&]/<CR>").text(), "a[.]b");
    assert_eq!(ed("ab").at(0, 0).keys(":s/a/1\\n2/<CR>").text(), "1\n2b");
    let e = ed(text).at(0, 0).keys(":%s/nope/x/<CR>");
    assert!(e.vim.message.as_ref().unwrap().1);
    // One substitution is one undo step.
    assert_eq!(ed(text).at(0, 0).keys(":%s/foo/x/g<CR>u").text(), text);
}

#[test]
fn ex_commands_come_back_as_effects() {
    let fx = ed("x").keys(":w<CR>").fx;
    assert_eq!(fx, [Effect::Write { path: None }]);
    let fx = ed("x").keys(":w src/other.rs<CR>").fx;
    assert_eq!(
        fx,
        [Effect::Write {
            path: Some("src/other.rs".into())
        }]
    );
    assert_eq!(ed("x").keys(":q<CR>").fx, [Effect::Quit { force: false }]);
    assert_eq!(ed("x").keys(":q!<CR>").fx, [Effect::Quit { force: true }]);
    assert_eq!(ed("x").keys(":wq<CR>").fx, [Effect::WriteQuit]);
    assert_eq!(ed("x").keys(":x<CR>").fx, [Effect::WriteQuit]);
    assert_eq!(ed("x").keys("ZZ").fx, [Effect::WriteQuit]);
    assert_eq!(
        ed("x").keys(":e src/lib.rs<CR>").fx,
        [Effect::Edit {
            path: Some("src/lib.rs".into()),
            force: false
        }]
    );
    assert_eq!(ed("x").keys(":vsp<CR>").fx, [Effect::Split { path: None }]);
    assert_eq!(ed("x").keys("<C-w>v").fx, [Effect::Split { path: None }]);
    assert_eq!(
        ed("x").keys("<C-w>w").fx,
        [Effect::FocusPane { forward: true }]
    );
    assert_eq!(
        ed("x").keys(":theme light<CR>").fx,
        [Effect::Command("theme light".into())]
    );
    assert_eq!(
        ed("x").keys("zz").fx,
        [Effect::Scroll(crate::vim::ScrollTo::Center)]
    );
}

#[test]
fn set_options_and_history() {
    let e = ed("x").keys(":set rnu ts=8 sw=2<CR>");
    assert!(e.vim.relative_numbers);
    assert_eq!((e.vim.tab, e.vim.shiftwidth), (8, 2));
    let e = e.keys(":set nornu<CR>");
    assert!(!e.vim.relative_numbers);
    let e = e.keys(":<Up>");
    assert_eq!(e.vim.cmdline, "set nornu");
    let e = e.keys("<Up>");
    assert_eq!(e.vim.cmdline, "set rnu ts=8 sw=2");
    let e = e.keys("<Esc>");
    assert_eq!(e.vim.mode, Mode::Normal);
    let e = ed("x").keys(":set bogus<CR>");
    assert!(e.vim.message.as_ref().unwrap().1);
}

#[test]
fn ex_line_ranges_delete_and_indent() {
    assert_eq!(ed("a\nb\nc\nd").at(0, 0).keys(":2,3d<CR>").text(), "a\nd");
    assert_eq!(
        ed("a\nb\nc").at(0, 0).keys(":%><CR>").text(),
        "    a\n    b\n    c"
    );
    assert_eq!(ed("a\nb\nc\nd").at(1, 0).keys(":.,.+1d<CR>").text(), "a\nd");
}

#[test]
fn paste_inserts_literally_in_any_mode() {
    let mut e = ed("ab").at(0, 1);
    e.feed(Input::Paste("dd\nx".into()));
    assert_eq!(e.text(), "add\nxb", "pasted text is not run as commands");
    assert_eq!(e.vim.mode, Mode::Normal);
    let mut e = ed("ab").at(0, 1).keys("i");
    e.feed(Input::Paste("XY".into()));
    assert_eq!(e.text(), "aXYb");
}

#[test]
fn ctrl_d_and_u_move_half_a_page() {
    let mut text = String::new();
    for i in 0..100 {
        text.push_str(&alloc::format!("line {i}\n"));
    }
    let e = ed(&text).at(0, 0).keys("<C-d>");
    assert_eq!(e.pos().0, 10);
    assert_eq!(e.fx, [Effect::ScrollLines(10)]);
    let e = e.keys("<C-u>");
    assert_eq!(e.pos().0, 0);
    let mut e2 = ed(&text).at(0, 0);
    e2.feed(Input::Key(Key::PageDown));
    assert_eq!(e2.pos().0, 18);
}

#[test]
fn showcmd_shows_a_half_typed_command() {
    let e = ed("abc").keys("2d");
    assert_eq!(e.vim.pending_keys(), "2d");
    let e = e.keys("<Esc>");
    assert_eq!(e.vim.pending_keys(), "");
}
