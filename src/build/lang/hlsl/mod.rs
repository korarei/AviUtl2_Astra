mod lex;
mod purse;

pub use lex::Lexer;
pub use purse::Parser;
pub(super) use purse::Preprocessor;

use std::borrow::Cow;
use std::fmt;

static SCALARS: [(&str, usize); 12] = [
    ("float", 4),
    ("half", 4),
    ("min16float", 4),
    ("min10float", 4),
    ("double", 8),
    ("uint", 4),
    ("int", 4),
    ("bool", 4),
    ("dword", 4),
    ("min16int", 4),
    ("min12int", 4),
    ("min16uint", 4),
];

#[derive(Debug, Default)]
pub struct Analysis<'a, 'tokens> {
    pub declarations: Vec<Declaration<'a>>,
    pub functions: Vec<Function<'a>>,
    pub directives: Vec<Directive<'tokens>>,
    pub groups: Vec<Span>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Declaration<'a> {
    pub ty: Type<'a>,
    pub span: Span,
    pub modifiers: Vec<&'a str>,
    pub members: Vec<Member<'a>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Function<'a> {
    pub name: &'a str,
    pub span: Span,
    pub is_void: bool,
    pub has_numthreads: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Directive<'a> {
    pub name: &'a str,
    pub rest: &'a str,
    pub span: Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Type<'a> {
    Scalar(Scalar),
    Vector { scalar: Scalar, cols: usize },
    Matrix { scalar: Scalar, rows: usize, cols: usize },
    Named(&'a str),
}

impl fmt::Display for Type<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Scalar(scalar) => f.write_str(scalar.name),
            Self::Vector { scalar, cols } => write!(f, "{}{cols}", scalar.name),
            Self::Matrix { scalar, rows, cols } => write!(f, "{}{rows}x{cols}", scalar.name),
            Self::Named(name) => f.write_str(name),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Scalar {
    pub name: &'static str,
    pub size: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Member<'a> {
    pub name: &'a str,
    pub span: Span,
    pub dims: Vec<Option<usize>>,
    pub annotation: Option<Annotation<'a>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Annotation<'a> {
    pub name: &'a str,
    pub args: Option<&'a str>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Action {
    Keep,
    Drop,
    Unknown,
    Include,
    MatrixPacking,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token<'a> {
    pub kind: TokenKind<'a>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenKind<'a> {
    Ident(&'a str),
    Keyword(&'a str),
    Modifier(&'a str),
    Type(Type<'a>),
    Vector,
    Matrix,
    Number(&'a str),
    String,
    Comment(Comment<'a>),
    Directive(Cow<'a, str>),
    Whitespace,
    Newline,
    LBrace,
    RBrace,
    LParen,
    RParen,
    LBracket,
    RBracket,
    Assign,
    Semicolon,
    Colon,
    Comma,
    Dot,
    Other(char),
}

impl TokenKind<'_> {
    pub(super) fn is_trivia(&self) -> bool {
        matches!(self, Self::Whitespace | Self::Newline | Self::Comment(_))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Comment<'a> {
    pub text: &'a str,
    pub content: &'a str,
    pub bracket: Option<Span>,
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
