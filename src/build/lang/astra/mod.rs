mod lex;
mod purse;

pub(super) use lex::Lexer;
pub(super) use purse::Parser;

use super::source;
use crate::vars::is_ident;
use std::borrow::Cow;
use std::fmt;
use std::ops::Range;
use std::path::PathBuf;

pub(super) struct Expander<'a> {
    src: &'a str,
    dst: Option<String>,
    edits: Vec<Edit>,
    curr: usize,
}

impl<'a> Expander<'a> {
    pub(super) fn new(src: &'a str) -> Self {
        Self {
            src,
            dst: None,
            edits: Vec::new(),
            curr: 0,
        }
    }

    pub(super) fn expand(&mut self, src_st: usize, src_ed: usize, vars: &impl crate::vars::Vars) -> Result<(), Error> {
        let mut rest = &self.src[src_st..src_ed];
        while let Some(st) = rest.find("${") {
            let rest_st = src_ed - rest.len();
            let (placeholder, ed) = Self::parse(rest, st)
                .map_err(|mut err| {
                    err.offset += rest_st;
                    err
                })?
                .expect("placeholder prefix has already been found");
            self.replace(rest_st + st, rest_st + ed, placeholder, vars)?;
            rest = &rest[ed..];
        }

        Ok(())
    }

    pub(super) fn finish(self) -> Expanded<'a> {
        Expanded {
            text: if let Some(mut dst) = self.dst {
                dst.push_str(&self.src[self.curr..]);
                Cow::Owned(dst)
            } else {
                Cow::Borrowed(self.src)
            },
            edits: self.edits,
        }
    }

    pub(super) fn parse(src: &str, st: usize) -> Result<Option<(Placeholder<'_>, usize)>, Error> {
        let Some(tail) = src.get(st..) else {
            return Ok(None);
        };

        let (body, prefix_len, closing) = if let Some(body) = tail.strip_prefix("${{") {
            (body, 3, "}}")
        } else if let Some(body) = tail.strip_prefix("${") {
            (body, 2, "}")
        } else {
            return Ok(None);
        };

        let body = body.split('\n').next().unwrap();
        if st > 0 && src.as_bytes()[st - 1] == b'$' {
            let ed = body.find(closing).map_or(body.len(), |ed| ed + closing.len());
            return Ok(Some((Placeholder::Escaped, st + prefix_len + ed)));
        }

        let ed = body.find(closing).ok_or_else(|| Error {
            offset: st,
            message: "unclosed placeholder".into(),
        })?;

        let name = body[..ed].trim();
        if name.is_empty() {
            return Err(Error {
                offset: st,
                message: "empty variable name in placeholder".into(),
            });
        }

        if !is_ident(name) {
            return Err(Error {
                offset: st,
                message: format!("invalid variable name '{name}' in placeholder").into(),
            });
        }

        Ok(Some((
            Placeholder::Variable(name),
            st + prefix_len + ed + closing.len(),
        )))
    }

    pub(super) fn replace(
        &mut self,
        st: usize,
        ed: usize,
        placeholder: Placeholder<'_>,
        vars: &impl crate::vars::Vars,
    ) -> Result<(), Error> {
        let (at, val, is_escaped) = match placeholder {
            Placeholder::Variable(key) => (
                st,
                vars.get(key).ok_or_else(|| Error {
                    offset: st,
                    message: format!("variable '{key}' not found").into(),
                })?,
                false,
            ),
            Placeholder::Escaped => (st - 1, &self.src[st..ed], true),
        };

        let dst = self.dst.get_or_insert_with(|| String::with_capacity(self.src.len()));
        dst.push_str(&self.src[self.curr..at]);

        let st = dst.len();
        dst.push_str(val);
        self.edits.push((at..ed, st..dst.len(), is_escaped));
        self.curr = ed;
        Ok(())
    }
}

#[derive(Debug)]
pub(super) struct Expanded<'a> {
    pub(super) text: Cow<'a, str>,
    edits: Vec<Edit>,
}

impl Expanded<'_> {
    pub(super) fn append(&self, map: &mut source::Map, src: &source::Map, span: Range<usize>) {
        let st = span.start;
        let mut curr = 0;

        for (origin, output, is_escaped) in &self.edits {
            map.append(src, st + curr..st + origin.start);

            if *is_escaped {
                map.append(src, st + origin.start + 1..st + origin.end);
            } else {
                map.generate(&self.text[output.clone()], src, st + origin.start..st + origin.end);
            }
            curr = origin.end;
        }

        map.append(src, st + curr..span.end);
    }

    fn locate(&self, at: usize) -> usize {
        let Some(i) = self
            .edits
            .partition_point(|(_, output, _)| output.start <= at)
            .checked_sub(1)
        else {
            return at;
        };

        let (origin, output, is_escaped) = &self.edits[i];
        if at < output.end {
            origin.start + if *is_escaped { 1 + at - output.start } else { 0 }
        } else {
            origin.end + at - output.end
        }
    }
}

type Edit = (Range<usize>, Range<usize>, bool);

pub(super) enum Directive<'a> {
    Omit,
    Replace(&'a str),
    If(&'a str),
    Elif(&'a str),
    IfDef(&'a str),
    IfNDef(&'a str),
    Else,
    EndIf,
    Define(&'a str, &'a str),
    Undef(&'a str),
    Include(Cow<'a, str>, bool),
    PragmaOnce,
}

pub(super) enum Action<'a> {
    Keep,
    Drop,
    Omit,
    Replace(String),
    Define(&'a str, &'a str),
    Undef(&'a str),
    Include(PathBuf),
    PragmaOnce,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Placeholder<'a> {
    Variable(&'a str),
    Escaped,
}

#[derive(Clone, Copy)]
pub(super) struct Token<'a> {
    kind: TokenKind<'a>,
    span: Span,
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
pub(super) struct Span {
    st: usize,
    ed: usize,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct Error {
    pub(super) offset: usize,
    pub(super) message: Cow<'static, str>,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Error {}

pub(super) fn expand<'text>(src: &'text str, vars: &impl crate::vars::Vars) -> Result<Expanded<'text>, Error> {
    let mut expander = Expander::new(src);
    expander.expand(0, src.len(), vars)?;
    Ok(expander.finish())
}
