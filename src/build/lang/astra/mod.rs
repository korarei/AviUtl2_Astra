mod expand;
mod lex;
mod parse;

pub(in crate::build) use expand::{Escape, expand};
pub(in crate::build) use parse::{Preprocessor, parse_directive, parse_replace, split_sigil};

use std::borrow::Cow;
use std::fmt;

pub(in crate::build) enum Action<'a> {
    Keep,
    Drop,
    Omit,
    Replace(String),
    Define { key: &'a str, val: &'a str },
    Undef(&'a str),
    Include { path: Cow<'a, str>, is_quoted: bool },
    PragmaOnce,
}

#[derive(Clone, Copy)]
enum DirectiveKind<'a> {
    Omit,
    Replace(&'a str),
    If(&'a str),
    Elif(&'a str),
    IfDef(&'a str),
    IfNDef(&'a str),
    Else,
    EndIf,
    Define { key: &'a str, val: &'a str },
    Undef(&'a str),
    Include { path: &'a str, is_quoted: bool },
    PragmaOnce,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TokenKind<'a> {
    Ident(&'a str),
    String,
    Number(&'a str),
    Whitespace,
    Operator(&'a str),
    Other(char),
}

#[derive(Clone, Copy)]
pub(in crate::build) struct Directive<'a> {
    src: &'a str,
    kind: DirectiveKind<'a>,
}

#[derive(Debug, PartialEq, Eq)]
pub(in crate::build) struct Error {
    pub(in crate::build) offset: usize,
    pub(in crate::build) message: Cow<'static, str>,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Error {}

#[derive(Clone, Copy)]
struct Token<'a> {
    kind: TokenKind<'a>,
    span: Span,
}

#[derive(Clone, Copy)]
struct Span {
    st: usize,
    ed: usize,
}
