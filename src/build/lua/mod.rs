mod lex;
mod purse;

pub use lex::Lexer;
pub use purse::Parser;

use std::borrow::Cow;
use std::fmt;

#[derive(Default)]
pub struct Analysis<'a> {
    pub assigns: Vec<Assignment>,
    pub calls: Vec<Call<'a>>,
}

pub struct Assignment {
    pub span: Span,
    pub targets: Vec<Span>,
    pub vals: Vec<Span>,
}

pub struct Call<'a> {
    pub stmt_st: Option<usize>,
    pub span: Span,
    pub reference: Reference<'a>,
    pub args: Vec<Span>,
}

type Reference<'a> = Option<Vec<&'a str>>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token<'a> {
    pub kind: TokenKind<'a>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenKind<'a> {
    Keyword(&'a str),
    Local,
    Ident(&'a str),
    String(Literal<'a>),
    Number(&'a str),
    Operator(&'a str),
    Assign,
    Eq,
    Ne,
    Le,
    Ge,
    Comma,
    Dot,
    Dots,
    Concat,
    Semicolon,
    Colon,
    LParen,
    RParen,
    LBracket,
    RBracket,
    LBrace,
    RBrace,
    Comment(Comment<'a>),
    Whitespace,
    Newline,
    Other(char),
}

impl<'a> TokenKind<'a> {
    pub(super) fn name(&self) -> Option<&'a str> {
        match self {
            Self::Ident(name) | Self::Keyword(name @ ("goto" | "const" | "continue")) => Some(name),
            _ => None,
        }
    }

    pub(super) fn is_trivia(&self) -> bool {
        matches!(self, Self::Whitespace | Self::Newline | Self::Comment(_))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Literal<'a> {
    pub val: Cow<'a, [u8]>,
    pub bracket: Option<Bracket>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Comment<'a> {
    pub content: &'a str,
    pub bracket: Option<Bracket>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bracket {
    pub span: Span,
    pub level: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub st: usize,
    pub ed: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Error {
    pub offset: usize,
    pub message: &'static str,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.message)
    }
}

impl std::error::Error for Error {}

pub fn is_ident(name: &str) -> Result<bool, Error> {
    let mut tokens = Lexer::new(name);
    Ok(matches!(
        tokens.next().transpose()?.map(|token| token.kind),
        Some(ref kind) if kind.name().is_some()
    ) && tokens.next().transpose()?.is_none())
}
