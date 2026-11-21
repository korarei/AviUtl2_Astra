use super::lex::Lexer;
use super::{Action, Directive, DirectiveKind, Error, Escape, TokenKind, expand};
use crate::text::normalize_newlines;
use crate::vars::is_ident;
use cel::{Context, Program, Value};
use indexmap::IndexMap;
use std::borrow::Cow;
use std::ops::Range;

pub(in crate::build) type Verbatim = (Vec<Range<usize>>, usize);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Placeholder<'a> {
    Variable(&'a str),
    Escaped,
}

#[derive(Clone, Copy)]
enum State {
    ParentInactive,
    Pending,
    Active,
    Matched,
    ElseActive,
    ElseInactive,
}

pub(in crate::build) struct Preprocessor {
    stack: Vec<Frame>,
}

impl Preprocessor {
    pub(in crate::build) fn new() -> Self {
        Self { stack: Vec::new() }
    }

    #[allow(clippy::too_many_lines)]
    pub(in crate::build) fn process<'text>(
        &mut self,
        directive: Option<Directive<'text>>,
        vars: &IndexMap<String, String>,
        st: usize,
        body_st: usize,
    ) -> Result<Action<'text>, Error> {
        let Some(directive) = directive else {
            return Ok(if self.is_active() { Action::Keep } else { Action::Drop });
        };

        let is_active = self.is_active();
        let error = |message: &'static str| Error {
            offset: st,
            message: message.into(),
        };
        let locate_error = |text: &str, mut err: Error| {
            err.offset += body_st + text.as_ptr() as usize - directive.src.as_ptr() as usize;
            err
        };

        Ok(match directive.kind {
            DirectiveKind::If(expr) => {
                self.stack.push(Frame {
                    offset: st,
                    state: if is_active {
                        if is_condition_met(expr, vars).map_err(|err| locate_error(expr, err))? {
                            State::Active
                        } else {
                            State::Pending
                        }
                    } else {
                        State::ParentInactive
                    },
                });

                Action::Drop
            }
            DirectiveKind::Elif(expr) => {
                let frame = self
                    .stack
                    .last_mut()
                    .ok_or_else(|| error("#elif without matching #if/#ifdef/#ifndef"))?;
                frame.state = match frame.state {
                    State::ElseActive | State::ElseInactive => return Err(error("#elif after #else")),
                    State::ParentInactive | State::Matched => frame.state,
                    State::Active => State::Matched,
                    State::Pending => {
                        if is_condition_met(expr, vars).map_err(|err| locate_error(expr, err))? {
                            State::Active
                        } else {
                            State::Pending
                        }
                    }
                };

                Action::Drop
            }
            DirectiveKind::IfDef(key) | DirectiveKind::IfNDef(key) => {
                self.stack.push(Frame {
                    offset: st,
                    state: if is_active {
                        let key = expand(key, vars, Escape::None).map_err(|err| locate_error(key, err))?;

                        if vars.contains_key(key.text.as_ref()) == matches!(directive.kind, DirectiveKind::IfDef(_)) {
                            State::Active
                        } else {
                            State::Pending
                        }
                    } else {
                        State::ParentInactive
                    },
                });

                Action::Drop
            }
            DirectiveKind::Else => {
                let frame = self
                    .stack
                    .last_mut()
                    .ok_or_else(|| error("#else without matching #if/#ifdef/#ifndef"))?;
                frame.state = match frame.state {
                    State::ElseActive | State::ElseInactive => {
                        return Err(error("duplicate #else directive"));
                    }
                    State::Pending => State::ElseActive,
                    State::ParentInactive | State::Matched | State::Active => State::ElseInactive,
                };

                Action::Drop
            }
            DirectiveKind::EndIf => {
                if self.stack.pop().is_none() {
                    return Err(error("#endif without matching #if/#ifdef/#ifndef"));
                }

                Action::Drop
            }
            DirectiveKind::Define { key, val } if is_active => Action::Define { key, val },
            DirectiveKind::Undef(key) if is_active => Action::Undef(key),
            DirectiveKind::Include { path, is_quoted } if is_active => {
                let include = expand(path, vars, Escape::None).map_err(|err| locate_error(path, err))?;
                Action::Include {
                    path: include.text,
                    is_quoted,
                }
            }
            DirectiveKind::Replace(key) if is_active => {
                let val = vars.get(key).ok_or_else(|| Error {
                    offset: st,
                    message: format!("undefined variable '{key}' in #replace").into(),
                })?;
                let val = normalize_newlines(val);

                Action::Replace(
                    expand(&val, vars, Escape::None)
                        .map_err(|mut err| {
                            err.offset = st;
                            err
                        })?
                        .text
                        .into_owned(),
                )
            }
            DirectiveKind::Omit if is_active => Action::Omit,
            DirectiveKind::Define { .. }
            | DirectiveKind::Undef(_)
            | DirectiveKind::Replace(_)
            | DirectiveKind::Omit => Action::Drop,
            DirectiveKind::PragmaOnce if is_active => Action::PragmaOnce,
            _ if is_active => Action::Keep,
            _ => Action::Drop,
        })
    }

    pub(in crate::build) fn parse_verbatim(
        &self,
        src: &str,
        st: usize,
        extract: impl Fn(&str) -> Option<&str>,
    ) -> Result<Option<Verbatim>, Error> {
        let parse_marker = |line: &str| {
            let content = extract(line)?;
            let (name, rest) = split_directive(content)?;
            let args = rest.trim();
            match name {
                "verbatim" => {
                    if args.is_empty() || args.eq_ignore_ascii_case("on") || args.eq_ignore_ascii_case("start") {
                        Some(Ok(true))
                    } else if args.eq_ignore_ascii_case("off") || args.eq_ignore_ascii_case("end") {
                        Some(Ok(false))
                    } else {
                        Some(Err("expected 'on', 'off', 'start', or 'end' after #verbatim"))
                    }
                }
                "endverbatim" => {
                    if args.is_empty() {
                        Some(Ok(false))
                    } else {
                        Some(Err("unexpected argument after #endverbatim"))
                    }
                }
                _ => None,
            }
        };
        let is_active = self.is_active();
        let mut stack = Vec::new();
        let mut spans: Vec<Range<usize>> = Vec::new();
        let mut curr = st;

        for line in src[st..].split_inclusive('\n') {
            if let Some(res) = parse_marker(line) {
                let is_open = res.map_err(|message| Error {
                    offset: curr,
                    message: message.into(),
                })?;

                if is_open {
                    stack.push(curr);
                } else if stack.pop().is_none() {
                    let message = if line.contains("endverbatim") {
                        "#endverbatim without matching #verbatim"
                    } else if line.contains("end") {
                        "#verbatim end without matching #verbatim start"
                    } else {
                        "#verbatim off without matching #verbatim on"
                    };

                    return Err(Error {
                        offset: curr,
                        message: message.into(),
                    });
                }
            } else if curr == st {
                return Ok(None);
            } else if is_active {
                if let Some(span) = spans.last_mut().filter(|span| span.end == curr) {
                    span.end += line.len();
                } else {
                    spans.push(curr..curr + line.len());
                }
            }

            curr += line.len();

            if stack.is_empty() {
                return Ok(Some((spans, curr)));
            }
        }

        if let Some(&offset) = stack.last() {
            return Err(Error {
                offset,
                message: "unclosed #verbatim on at end of file".into(),
            });
        }

        Ok(None)
    }

    pub(in crate::build) fn finish(self) -> Result<(), Error> {
        if let Some(frame) = self.stack.last() {
            return Err(Error {
                offset: frame.offset,
                message: "unclosed conditional directive at end of file".into(),
            });
        }

        Ok(())
    }

    fn is_active(&self) -> bool {
        self.stack
            .last()
            .is_none_or(|frame| matches!(frame.state, State::Active | State::ElseActive))
    }
}

struct Frame {
    offset: usize,
    state: State,
}

pub(in crate::build) fn parse_directive(src: &str, is_block: bool) -> Result<Option<Directive<'_>>, &'static str> {
    let Some((name, rest)) = split_directive(src) else {
        return Ok(None);
    };

    parse_args(src, name, rest, is_block)
}

pub(in crate::build) fn parse_replace(src: &str) -> Result<Option<Directive<'_>>, &'static str> {
    let Some(("replace", rest)) = split_directive(src) else {
        return Ok(None);
    };

    parse_args(src, "replace", rest, true)
}

pub(in crate::build) fn split_sigil(src: &str) -> Option<(&str, &str)> {
    let at_sigil = src.trim_start();
    let body = at_sigil.strip_prefix('#')?.trim_start();
    Some((at_sigil, body))
}

pub(super) fn parse_placeholder(src: &str, st: usize) -> Result<Option<(Placeholder<'_>, usize)>, Error> {
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

fn parse_args<'a>(
    src: &'a str,
    name: &str,
    rest: &'a str,
    is_block: bool,
) -> Result<Option<Directive<'a>>, &'static str> {
    Ok((match name {
        "replace" => {
            let key = rest.trim();
            if !is_ident(key) {
                return Err("#replace requires a single variable name");
            }
            Some(DirectiveKind::Replace(key))
        }
        "omit" if !is_block => {
            if !rest.trim().is_empty() {
                return Err("unexpected argument after #omit");
            }
            Some(DirectiveKind::Omit)
        }
        "if" | "elif" | "elseif" if !is_block && rest.trim().is_empty() => {
            return Err("empty conditional expression in #if/#elif");
        }
        "if" if !is_block => Some(DirectiveKind::If(rest.trim())),
        "elif" | "elseif" if !is_block => Some(DirectiveKind::Elif(rest.trim())),
        "ifdef" | "ifndef" if !is_block && rest.trim().is_empty() => {
            return Err("missing variable name in #ifdef/#ifndef");
        }
        "ifdef" if !is_block => Some(DirectiveKind::IfDef(rest.trim())),
        "ifndef" if !is_block => Some(DirectiveKind::IfNDef(rest.trim())),
        "else" if !is_block => Some(DirectiveKind::Else),
        "endif" if !is_block => Some(DirectiveKind::EndIf),
        "define" | "undef" | "undefine" => {
            let args = rest.trim_start();
            let ed = args.find(char::is_whitespace).unwrap_or(args.len());
            let key = &args[..ed];

            if !is_ident(key) {
                return Err("invalid variable name in #define/#undef");
            }

            let val = args[ed..].trim();
            if matches!(name, "undef" | "undefine") {
                if !val.is_empty() {
                    return Err("unexpected argument after variable name in #undef");
                }

                Some(DirectiveKind::Undef(key))
            } else {
                Some(DirectiveKind::Define { key, val })
            }
        }
        "include" if !is_block => {
            let args = rest.trim();
            if let Some(args) = args.strip_prefix('"') {
                let ed = args.find('"').ok_or("unclosed include path")?;
                if !args[ed + 1..].trim().is_empty() {
                    return Err("unexpected argument after include path");
                }

                if ed == 0 {
                    return Err("empty include path");
                }

                Some(DirectiveKind::Include {
                    path: &args[..ed],
                    is_quoted: true,
                })
            } else if let Some(path) = args.strip_prefix('<').and_then(|args| args.strip_suffix('>')) {
                if path.is_empty() {
                    return Err("empty include path");
                }

                Some(DirectiveKind::Include { path, is_quoted: false })
            } else {
                return Err("include path must be enclosed in quotes or angle brackets");
            }
        }
        "pragma" if !is_block => {
            let args = rest.trim();
            if args == "once"
                || args
                    .strip_prefix("once")
                    .is_some_and(|tail| tail.trim_start().starts_with(['-', ';']))
            {
                Some(DirectiveKind::PragmaOnce)
            } else {
                None
            }
        }
        _ => None,
    })
    .map(|kind| Directive { src, kind }))
}

fn split_directive(src: &str) -> Option<(&str, &str)> {
    let (name, rest) = src.split_at(src.find(char::is_whitespace).unwrap_or(src.len()));
    is_ident(name).then_some((name, rest))
}

fn is_condition_met(expr: &str, vars: &IndexMap<String, String>) -> Result<bool, Error> {
    let expansion = expand(expr, vars, Escape::None)?;
    let expr = translate(&expansion.text, vars).map_err(|mut err| {
        err.offset = expansion.to_src_offset(err.offset);
        err
    })?;
    let expr = expr.trim();

    if expr.is_empty() {
        return Err(Error {
            offset: 0,
            message: "empty conditional expression in #if/#elif".into(),
        });
    }

    let normalize = |mut message: String| {
        if let Some(first) = message.get_mut(..1) {
            first.make_ascii_lowercase();
        }

        message
    };

    let val = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        Program::compile(expr)
            .map_err(|err| Error {
                offset: 0,
                message: normalize(err.to_string()).into(),
            })?
            .execute(&Context::default())
            .map_err(|err| Error {
                offset: 0,
                message: format!("evaluation error: {}", normalize(err.to_string())).into(),
            })
    }))
    .map_err(|_| Error {
        offset: 0,
        message: "invalid conditional expression".into(),
    })??;

    Ok(match val {
        Value::Bool(val) => val,
        Value::Int(val) => val != 0,
        Value::UInt(val) => val != 0,
        Value::Float(val) => val != 0.0,
        Value::String(val) => !val.is_empty(),
        _ => false,
    })
}

fn translate<'a>(src: &'a str, vars: &IndexMap<String, String>) -> Result<Cow<'a, str>, Error> {
    let mut lexer = Lexer::new(src);
    let mut dst = None;
    let mut curr = 0;

    while let Some(token) = lexer.next() {
        let token = token?;

        if token.kind == TokenKind::Ident("defined")
            && let Some((key, ed)) = parse_defined(src, &mut lexer, token.span.ed)?
        {
            let dst = dst.get_or_insert_with(|| String::with_capacity(src.len()));
            dst.push_str(&src[curr..token.span.st]);
            dst.push_str(if vars.contains_key(key) { "true" } else { "false" });
            curr = ed;
            continue;
        }

        let repl = match token.kind {
            TokenKind::Ident("and") => Some("&&"),
            TokenKind::Ident("or") => Some("||"),
            TokenKind::Ident("not") => Some("!"),
            TokenKind::Ident("nil") => Some("null"),
            TokenKind::Operator("~=") => Some("!="),
            _ => None,
        };

        if let Some(repl) = repl {
            let dst = dst.get_or_insert_with(|| String::with_capacity(src.len()));
            dst.push_str(&src[curr..token.span.st]);
            dst.push_str(repl);
            curr = token.span.ed;
        }
    }

    Ok(if let Some(mut dst) = dst {
        dst.push_str(&src[curr..]);
        Cow::Owned(dst)
    } else {
        Cow::Borrowed(src)
    })
}

fn parse_defined<'a>(src: &'a str, lexer: &mut Lexer<'a>, st: usize) -> Result<Option<(&'a str, usize)>, Error> {
    let mut tokens = lexer.clone();
    let next = loop {
        let Some(token) = tokens.next() else {
            return Ok(None);
        };
        let token = token?;

        if token.kind != TokenKind::Whitespace {
            break token;
        }
    };

    if next.kind == TokenKind::Other('(') {
        let mut depth = 1;
        while let Some(token) = tokens.next() {
            let token = token?;
            match token.kind {
                TokenKind::Other('(') => depth += 1,
                TokenKind::Other(')') => {
                    depth -= 1;
                    if depth == 0 {
                        *lexer = tokens;
                        return Ok(Some((&src[next.span.ed..token.span.st], token.span.ed)));
                    }
                }
                _ => {}
            }
        }

        Err(Error {
            offset: next.span.st,
            message: "unclosed defined argument".into(),
        })
    } else if next.span.st > st {
        *lexer = tokens;
        Ok(Some((&src[next.span.st..next.span.ed], next.span.ed)))
    } else {
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_verbatim_ranges() {
        let vars = IndexMap::new();
        fn extract<'a>(line: &'a str, prefix: &str) -> Option<&'a str> {
            line.trim()
                .strip_prefix(prefix)?
                .trim_start()
                .strip_prefix('#')
                .map(str::trim_start)
        }

        for prefix in ["--", "//", ";"] {
            let src = format!(
                "{prefix}#verbatim\na ${{MISSING}}\n{prefix}#verbatim\n{prefix}#if invalid(\n{prefix}#endverbatim\nb\n{prefix}#endverbatim\nafter\n"
            );
            let mut parser = Preprocessor::new();
            let (spans, ed) = parser
                .parse_verbatim(&src, 0, |line| extract(line, prefix))
                .unwrap()
                .unwrap();
            let text: String = spans.into_iter().map(|span| &src[span]).collect();
            assert_eq!(text, format!("a ${{MISSING}}\n{prefix}#if invalid(\nb\n"));
            assert_eq!(&src[ed..], "after\n");
            parser
                .process(parse_directive("if false", false).unwrap(), &vars, 0, 2)
                .unwrap();
            let (spans, _) = parser
                .parse_verbatim(&src, 0, |line| extract(line, prefix))
                .unwrap()
                .unwrap();
            assert!(spans.is_empty());
        }
        let src = "--#verbatim\nfirst\nsecond\n--#endverbatim";
        let (spans, ed) = Preprocessor::new()
            .parse_verbatim(src, 0, |line| extract(line, "--"))
            .unwrap()
            .unwrap();
        assert_eq!(spans, [12..25]);
        assert_eq!(&src[spans[0].clone()], "first\nsecond\n");
        assert_eq!(ed, src.len());

        let src = "--#verbatim on\nfirst\nsecond\n--#verbatim off";
        let (spans, ed) = Preprocessor::new()
            .parse_verbatim(src, 0, |line| extract(line, "--"))
            .unwrap()
            .unwrap();
        assert_eq!(spans, [15..28]);
        assert_eq!(&src[spans[0].clone()], "first\nsecond\n");
        assert_eq!(ed, src.len());

        let src = "--#verbatim start\nfirst\nsecond\n--#verbatim end";
        let (spans, ed) = Preprocessor::new()
            .parse_verbatim(src, 0, |line| extract(line, "--"))
            .unwrap()
            .unwrap();
        assert_eq!(spans, [18..31]);
        assert_eq!(&src[spans[0].clone()], "first\nsecond\n");
        assert_eq!(ed, src.len());

        for (src, offset, expected) in [
            ("--#endverbatim\n", 0, "#endverbatim without matching #verbatim"),
            ("--#verbatim off\n", 0, "#verbatim off without matching #verbatim on"),
            ("--#verbatim end\n", 0, "#verbatim end without matching #verbatim start"),
            (
                "--#verbatim arg\n",
                0,
                "expected 'on', 'off', 'start', or 'end' after #verbatim",
            ),
            ("--#endverbatim on\n", 0, "unexpected argument after #endverbatim"),
            ("--#verbatim\n--#verbatim\n", 12, "unclosed #verbatim on at end of file"),
        ] {
            let err = Preprocessor::new()
                .parse_verbatim(src, 0, |line| extract(line, "--"))
                .unwrap_err();
            assert_eq!(err.offset, offset);
            assert_eq!(err.message, expected);
        }
    }

    #[test]
    fn parse_inline_directives() {
        for text in ["define", "undef A B", "include", "ordinary"] {
            assert!(parse_replace(text).unwrap().is_none());
        }
        assert!(parse_replace("replace A B").is_err());
        assert!(parse_replace("replace VALUE").unwrap().is_some());
    }

    #[test]
    fn parse_includes() {
        let mut preprocessor = Preprocessor::new();
        let vars = IndexMap::from([("DIR".to_owned(), "nested".to_owned())]);

        for (text, expected, is_borrowed) in [
            ("include <missing.hlsl>", "missing.hlsl", true),
            ("include <${DIR}/missing.hlsl>", "nested/missing.hlsl", false),
        ] {
            let directive = parse_directive(text, false).unwrap();
            let Action::Include { path, is_quoted } = preprocessor.process(directive, &vars, 0, 2).unwrap() else {
                panic!("include action expected");
            };
            assert_eq!(path, expected);
            assert!(!is_quoted);
            assert_eq!(matches!(path, Cow::Borrowed(_)), is_borrowed);
        }
    }

    #[test]
    fn preserve_conditions() {
        let vars = IndexMap::new();
        let offset = 12;
        let mut parser = Preprocessor::new();

        for directive in ["if false", "if true", "else", "endif"] {
            assert!(matches!(
                parser
                    .process(parse_directive(directive, false).unwrap(), &vars, offset, offset + 2)
                    .unwrap(),
                Action::Drop
            ));
        }

        assert!(matches!(
            parser.process(None, &vars, offset, offset + 2).unwrap(),
            Action::Drop
        ));
        assert!(matches!(
            parser
                .process(parse_directive("else", false).unwrap(), &vars, offset, offset + 2)
                .unwrap(),
            Action::Drop
        ));
        assert!(matches!(
            parser.process(None, &vars, offset, offset + 2).unwrap(),
            Action::Keep
        ));
        assert!(matches!(
            parser
                .process(parse_directive("endif", false).unwrap(), &vars, offset, offset + 2)
                .unwrap(),
            Action::Drop
        ));
        parser.finish().unwrap();
        let mut parser = Preprocessor::new();
        parser
            .process(parse_directive("if true", false).unwrap(), &vars, offset, offset + 2)
            .unwrap();
        assert_eq!(parser.finish().unwrap_err().offset, offset);
    }

    #[test]
    fn reject_multiline_placeholders() {
        assert_eq!(
            parse_placeholder("${X\n}", 0).unwrap_err().message,
            "unclosed placeholder"
        );
        assert_eq!(
            parse_placeholder("${{X}\n}", 0).unwrap_err().message,
            "unclosed placeholder"
        );
        assert_eq!(
            parse_placeholder("${X}\n", 0),
            Ok(Some((Placeholder::Variable("X"), 4)))
        );
    }

    #[test]
    fn preserve_paths() {
        for src in [r#"include "x\n\\y""#, r#"include <x\n\\y>"#] {
            let directive = parse_directive(src, false).unwrap();
            assert!(
                matches!(directive, Some(Directive { kind: DirectiveKind::Include { path, is_quoted: _ }, .. }) if path == r"x\n\\y")
            );
        }

        assert!(parse_directive(r#"include "a\"b""#, false).is_err());
        assert!(parse_directive("include 'file.h'", false).is_err());
        assert!(matches!(
            parse_directive(r#"include "dir\""#, false).unwrap(),
            Some(Directive { kind: DirectiveKind::Include { path, is_quoted: true }, .. }) if path == r#"dir\"#
        ));
    }

    #[test]
    fn reject_strings() {
        for src in ["\"abc", "'abc\n", "\"abc\\"] {
            let mut lexer = Lexer::new(src);
            let err = lexer.next().unwrap().err().unwrap();
            assert_eq!(err.offset, 0);
            assert_eq!(err.message, "unclosed string");
            assert!(lexer.next().is_none());
        }
    }

    #[test]
    fn parse_defined_arguments() {
        for (src, expected) in [
            ("defined(\"A\")", "\"A\""),
            ("defined A", "A"),
            ("defined(A(B))", "A(B)"),
        ] {
            let mut lexer = Lexer::new(src);
            let token = lexer.next().unwrap().unwrap();
            let (key, ed) = super::parse_defined(src, &mut lexer, token.span.ed).unwrap().unwrap();
            assert_eq!(key, expected);
            assert_eq!(token.span.st, 0);
            assert_eq!(ed, src.len());
        }

        assert_eq!(translate("defined+A", &IndexMap::new()).unwrap(), "defined+A");
        let err = translate("defined(A", &IndexMap::new()).unwrap_err();
        assert_eq!(err.offset, 7);
        assert_eq!(err.message, "unclosed defined argument");
        assert!(translate("defined(\"A)", &IndexMap::new()).is_err());
    }

    #[test]
    fn parse_defined() {
        let src = "true defined(A) and defined(\"B\")";
        let vars = IndexMap::from([("A".to_owned(), String::new()), ("\"B\"".to_owned(), String::new())]);
        assert_eq!(translate(src, &vars).unwrap(), "true true && true");
        assert!(translate("defined(\"unfinished", &IndexMap::new()).is_err());
    }

    #[test]
    fn lex_numbers() {
        let src = "0x1e+2 1e+2 0x1p-2 1..x";
        let numbers = Lexer::new(src)
            .map(Result::unwrap)
            .filter(|token| matches!(token.kind, TokenKind::Number(_)))
            .map(|token| &src[token.span.st..token.span.ed])
            .collect::<Vec<_>>();
        assert_eq!(numbers, ["0x1e", "2", "1e+2", "0x1p-2", "1"]);
    }

    #[test]
    fn parse_directives() {
        assert!(matches!(
            parse_directive("replace VALUE", false).unwrap(),
            Some(Directive {
                kind: DirectiveKind::Replace("VALUE"),
                ..
            })
        ));
        assert!(matches!(
            parse_directive("replace\n VALUE", true).unwrap(),
            Some(Directive {
                kind: DirectiveKind::Replace("VALUE"),
                ..
            })
        ));
        assert!(matches!(
            parse_directive("omit", false).unwrap(),
            Some(Directive {
                kind: DirectiveKind::Omit,
                ..
            })
        ));
        assert!(matches!(
            parse_directive("define VALUE 1 + 2", true).unwrap(),
            Some(Directive {
                kind: DirectiveKind::Define {
                    key: "VALUE",
                    val: "1 + 2"
                },
                ..
            })
        ));
        assert!(matches!(
            parse_directive("undef VALUE", true).unwrap(),
            Some(Directive {
                kind: DirectiveKind::Undef("VALUE"),
                ..
            })
        ));
        assert!(matches!(
            parse_directive("elseif true", false).unwrap(),
            Some(Directive {
                kind: DirectiveKind::Elif("true"),
                ..
            })
        ));
        assert!(matches!(
            parse_directive("undefine VALUE", true).unwrap(),
            Some(Directive {
                kind: DirectiveKind::Undef("VALUE"),
                ..
            })
        ));

        for src in ["ordinary", "unknown value", "pragma message(\"x\")"] {
            assert!(parse_directive(src, false).unwrap().is_none(), "{src}");
        }

        for src in ["omit", "if true", "include \"x\"", "pragma once"] {
            assert!(parse_directive(src, true).unwrap().is_none(), "{src}");
        }
    }

    #[test]
    fn reject_directives() {
        for src in [
            "replace",
            "replace A B",
            "replace 1",
            "omit extra",
            "if",
            "elif",
            "elseif",
            "ifdef",
            "ifndef",
            "define",
            "define 1 2",
            "undef A extra",
            "undefine A extra",
            "include",
            "include \"\"",
            "include <>",
            "include \"x",
            "include \"x\" extra",
        ] {
            assert!(parse_directive(src, false).is_err(), "{src}");
        }
    }

    #[test]
    fn resolve_conditions() {
        let vars = IndexMap::from([("FLAG".to_owned(), "2".to_owned())]);

        for (expr, is_expected) in [
            ("true", true),
            ("false", false),
            ("0", false),
            ("2", true),
            ("0.0", false),
            ("0.5", true),
            ("''", false),
            ("'x'", true),
            ("nil", false),
            ("defined(FLAG) and not defined(MISSING)", true),
            ("${FLAG} == 2", true),
            ("1 ~= 2", true),
            ("-1 == 0 - 1", true),
            ("1.0 / -2.0 == -0.5", true),
            ("-1.5 == 0.0 - 1.5", true),
            ("-(1 + 2) == -3", true),
            ("-(-(-int('2'))) == -2", true),
            ("-([1, 2][int('0')]) == -1", true),
            ("-[1, 2][0] == -1", true),
            ("-int('2') == -2", true),
            ("--1 == 1", true),
            ("---1 == -1", true),
            ("-9223372036854775808 == -9223372036854775807 - 1", true),
            ("'a-b' == 'a-b'", true),
        ] {
            assert_eq!(is_condition_met(expr, &vars).unwrap(), is_expected, "{expr}");
        }

        for expr in [
            "",
            "(",
            "1 +",
            "true ?",
            "[1",
            "-(1 + 2",
            "${MISSING}",
            "-(-9223372036854775808)",
            "-(-9223372036854775807 - 1)",
            "-int('-9223372036854775808')",
        ] {
            assert!(is_condition_met(expr, &vars).is_err(), "{expr}");
        }
    }

    #[test]
    fn reject_conditional_order() {
        let offset = 12;

        for directives in [
            vec!["else"],
            vec!["elif true"],
            vec!["endif"],
            vec!["if true", "else", "else"],
            vec!["if false", "else", "elif true"],
        ] {
            let mut parser = Preprocessor::new();

            for directive in &directives[..directives.len() - 1] {
                parser
                    .process(
                        parse_directive(directive, false).unwrap(),
                        &IndexMap::new(),
                        offset,
                        offset + 2,
                    )
                    .unwrap();
            }

            let err = parser
                .process(
                    parse_directive(directives.last().unwrap(), false).unwrap(),
                    &IndexMap::new(),
                    offset,
                    offset + 2,
                )
                .err()
                .unwrap();
            assert_eq!(err.offset, offset, "{directives:?}: {err}");
        }
    }

    #[test]
    fn resolve_inactive_directives() {
        let offset = 12;
        let vars = IndexMap::new();
        let mut parser = Preprocessor::new();
        parser
            .process(parse_directive("if false", false).unwrap(), &vars, offset, offset + 2)
            .unwrap();

        for directive in [
            "define VALUE 1",
            "undef VALUE",
            "include \"missing.lua\"",
            "replace VALUE",
            "omit",
            "pragma once",
            "if ${MISSING}",
        ] {
            assert!(
                matches!(
                    parser
                        .process(parse_directive(directive, false).unwrap(), &vars, offset, offset + 2)
                        .unwrap(),
                    Action::Drop
                ),
                "{directive}"
            );
        }
        parser
            .process(parse_directive("endif", false).unwrap(), &vars, offset, offset + 2)
            .unwrap();
        parser
            .process(parse_directive("endif", false).unwrap(), &vars, offset, offset + 2)
            .unwrap();
        parser.finish().unwrap();
    }

    #[test]
    fn preserve_literal_expansions() {
        let vars = IndexMap::from([("VALUE".to_owned(), "${OTHER}".to_owned())]);

        for (src, expected) in [
            ("ordinary", "ordinary"),
            ("${VALUE}", "${OTHER}"),
            ("$${VALUE}", "${VALUE}"),
        ] {
            assert_eq!(expand(src, &vars, Escape::None).unwrap().text, expected);
        }
        assert!(matches!(
            expand("ordinary", &vars, Escape::None).unwrap().text,
            Cow::Borrowed(_)
        ));

        for src in ["${MISSING}", "${", "${A B}", "${{VALUE}"] {
            assert!(expand(src, &vars, Escape::None).is_err(), "{src}");
        }
    }

    #[test]
    fn parse_pragma_once() {
        for space in [" ", "  ", "\t", " \t ", "\u{3000}"] {
            let src = format!("pragma{space}once");
            assert!(
                matches!(
                    parse_directive(&src, false).unwrap(),
                    Some(Directive {
                        kind: DirectiveKind::PragmaOnce,
                        ..
                    })
                ),
                "{src:?}"
            );
        }
        for src in ["pragmaonce", "pragma", "pragma once_more"] {
            assert!(parse_directive(src, false).unwrap().is_none(), "{src:?}");
        }
    }

    #[test]
    fn classify_directives() {
        for (body, is_block_allowed) in [
            ("omit", false),
            ("replace VALUE", true),
            ("if true", false),
            ("elif false", false),
            ("ifdef VALUE", false),
            ("ifndef VALUE", false),
            ("else", false),
            ("endif", false),
            ("define VALUE 1", true),
            ("undef VALUE", true),
            ("include \"file.lua\"", false),
            ("include <file.lua>", false),
            ("pragma once", false),
        ] {
            assert!(parse_directive(body, false).unwrap().is_some(), "line: {body:?}");
            assert_eq!(
                parse_directive(body, true).unwrap().is_some(),
                is_block_allowed,
                "block: {body:?}"
            );
        }
        for (src, expected) in [
            ("replace", "#replace requires a single variable name"),
            ("omit extra", "unexpected argument after #omit"),
            ("if", "empty conditional expression in #if/#elif"),
            ("undef X Y", "unexpected argument after variable name in #undef"),
            ("include \"\"", "empty include path"),
            ("include \"file", "unclosed include path"),
        ] {
            assert_eq!(parse_directive(src, false).err(), Some(expected), "{src:?}");
        }
        for src in ["", "ordinary", " if true", "#if true", "unknown"] {
            assert!(parse_directive(src, false).unwrap().is_none(), "ordinary: {src:?}");
        }
    }
}
