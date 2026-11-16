use super::super::source::{self, Position};
use super::{Action, Directive, Error, Lexer, TokenKind, expand};
use crate::vars::is_ident;
use anyhow::bail;
use cel_interpreter::{Context, Program, Value};
use indexmap::IndexMap;
use std::borrow::Cow;
use std::path::{Path, PathBuf};

pub(in crate::build) struct Parser<'a> {
    lexer: Lexer<'a>,
    stack: Vec<Frame>,
}

impl<'a> Parser<'a> {
    pub(in crate::build) fn new(lexer: Lexer<'a>) -> Self {
        Self {
            lexer,
            stack: Vec::new(),
        }
    }

    pub(in crate::build) fn parse(mut self, vars: &IndexMap<String, String>) -> Result<Cow<'a, str>, Error> {
        let src = self.lexer.src;
        let mut dst = None;
        let mut curr = 0;

        while let Some(token) = self.lexer.next() {
            let token = token?;

            if token.kind == TokenKind::Ident("defined")
                && let Some((key, ed)) = self.parse_defined(token.span.ed)?
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

    #[allow(clippy::too_many_lines)]
    pub(in crate::build) fn process<'text>(
        &mut self,
        comment: Option<(&'text str, bool)>,
        vars: &IndexMap<String, String>,
        pos: &Position,
        include_dirs: &[PathBuf],
    ) -> anyhow::Result<Action<'text>> {
        let Some(directive) = comment
            .map(|(src, is_block)| Parser::parse_directive(src, is_block))
            .transpose()
            .map_err(|err| anyhow::anyhow!("{pos}: {err}"))?
            .flatten()
        else {
            return Ok(if self.is_active() { Action::Keep } else { Action::Drop });
        };

        let is_active = self.is_active();
        let locate_error = |text: &str, err: Error| {
            let src = comment.unwrap().0;
            let at = text.as_ptr() as usize - src.as_ptr() as usize + err.offset;
            let pos = source::Map::new(src, pos.clone()).locate(at);
            anyhow::anyhow!("{pos}: {err}")
        };

        Ok(match directive {
            Directive::If(expr) => {
                self.stack.push(Frame {
                    pos: pos.clone(),
                    state: if is_active {
                        if Parser::is_condition_met(expr, vars).map_err(|err| locate_error(expr, err))? {
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
            Directive::Elif(expr) => {
                let frame = self
                    .stack
                    .last_mut()
                    .ok_or_else(|| anyhow::anyhow!("{pos}: #elif without matching #if/#ifdef/#ifndef"))?;
                frame.state = match frame.state {
                    State::ElseActive | State::ElseInactive => bail!("{pos}: #elif after #else"),
                    State::ParentInactive | State::Matched => frame.state,
                    State::Active => State::Matched,
                    State::Pending => {
                        if Parser::is_condition_met(expr, vars).map_err(|err| locate_error(expr, err))? {
                            State::Active
                        } else {
                            State::Pending
                        }
                    }
                };

                Action::Drop
            }
            Directive::IfDef(key) | Directive::IfNDef(key) => {
                self.stack.push(Frame {
                    pos: pos.clone(),
                    state: if is_active {
                        let key = expand(key, vars).map_err(|err| locate_error(key, err))?;

                        if vars.contains_key(key.text.as_ref()) == matches!(directive, Directive::IfDef(_)) {
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
            Directive::Else => {
                let frame = self
                    .stack
                    .last_mut()
                    .ok_or_else(|| anyhow::anyhow!("{pos}: #else without matching #if/#ifdef/#ifndef"))?;
                frame.state = match frame.state {
                    State::ElseActive | State::ElseInactive => {
                        bail!("{pos}: duplicate #else directive");
                    }
                    State::Pending => State::ElseActive,
                    State::ParentInactive | State::Matched | State::Active => State::ElseInactive,
                };

                Action::Drop
            }
            Directive::EndIf => {
                if self.stack.pop().is_none() {
                    bail!("{pos}: #endif without matching #if/#ifdef/#ifndef");
                }

                Action::Drop
            }
            Directive::Define(key, val) if is_active => Action::Define(key, val),
            Directive::Undef(key) if is_active => Action::Undef(key),
            Directive::Include(path, is_quoted) if is_active => {
                let include = expand(&path, vars).map_err(|err| match &path {
                    Cow::Borrowed(text) => locate_error(text, err),
                    Cow::Owned(_) => anyhow::anyhow!("{pos}: {err}"),
                })?;
                let dir = pos.file.parent().unwrap_or(Path::new("."));
                let file = is_quoted
                    .then(|| dir.join(include.text.as_ref()))
                    .filter(|file| file.is_file())
                    .or_else(|| {
                        include_dirs
                            .iter()
                            .map(|dir| dir.join(include.text.as_ref()))
                            .find(|file| file.is_file())
                    })
                    .ok_or_else(|| {
                        anyhow::anyhow!(
                            "{pos}: include file '{}' not found from '{}'",
                            include.text,
                            dir.display()
                        )
                    })?;

                Action::Include(file)
            }
            Directive::Replace(key) if is_active => {
                let val = vars
                    .get(key)
                    .ok_or_else(|| anyhow::anyhow!("{pos}: undefined variable '{key}' in #replace"))?;

                Action::Replace(
                    expand(val, vars)
                        .map_err(|err| anyhow::anyhow!("{pos}: {err}"))?
                        .text
                        .into_owned(),
                )
            }
            Directive::Omit if is_active => Action::Omit,
            Directive::Define(..) | Directive::Undef(_) | Directive::Replace(_) | Directive::Omit => Action::Drop,
            Directive::PragmaOnce if is_active => Action::PragmaOnce,
            _ if is_active => Action::Keep,
            _ => Action::Drop,
        })
    }

    pub(in crate::build) fn finish(&self) -> anyhow::Result<()> {
        if let Some(frame) = self.stack.last() {
            bail!("{}: unclosed conditional directive at end of file", frame.pos);
        }

        Ok(())
    }

    pub(in crate::build) fn parse_directive(src: &str, is_block: bool) -> Result<Option<Directive<'_>>, &'static str> {
        let Some((name, rest)) = Self::read_directive(src) else {
            return Ok(None);
        };

        Ok(match name {
            "replace" => {
                let key = rest.trim();
                if !is_ident(key) {
                    return Err("#replace requires a single variable name");
                }
                Some(Directive::Replace(key))
            }
            "omit" if !is_block => {
                if !rest.trim().is_empty() {
                    return Err("unexpected argument after #omit");
                }
                Some(Directive::Omit)
            }
            "if" | "elif" if !is_block && rest.trim().is_empty() => {
                return Err("empty conditional expression in #if/#elif");
            }
            "if" if !is_block => Some(Directive::If(rest.trim())),
            "elif" if !is_block => Some(Directive::Elif(rest.trim())),
            "ifdef" | "ifndef" if !is_block && rest.trim().is_empty() => {
                return Err("missing variable name in #ifdef/#ifndef");
            }
            "ifdef" if !is_block => Some(Directive::IfDef(rest.trim())),
            "ifndef" if !is_block => Some(Directive::IfNDef(rest.trim())),
            "else" if !is_block => Some(Directive::Else),
            "endif" if !is_block => Some(Directive::EndIf),
            "define" | "undef" => {
                let args = rest.trim_start();
                let ed = args.find(char::is_whitespace).unwrap_or(args.len());
                let key = &args[..ed];

                if !is_ident(key) {
                    return Err("invalid variable name in #define/#undef");
                }

                let val = args[ed..].trim();
                if name == "undef" {
                    if !val.is_empty() {
                        return Err("unexpected argument after variable name in #undef");
                    }

                    Some(Directive::Undef(key))
                } else {
                    Some(Directive::Define(key, val))
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

                    Some(Directive::Include(Cow::Borrowed(&args[..ed]), true))
                } else if let Some(path) = args.strip_prefix('<').and_then(|args| args.strip_suffix('>')) {
                    if path.is_empty() {
                        return Err("empty include path");
                    }

                    Some(Directive::Include(Cow::Borrowed(path), false))
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
                    Some(Directive::PragmaOnce)
                } else {
                    None
                }
            }
            _ => None,
        })
    }

    pub(in crate::build) fn read_directive(src: &str) -> Option<(&str, &str)> {
        let body = src.trim_start().strip_prefix('#')?.trim_start();
        let (name, rest) = body.split_at(body.find(char::is_whitespace).unwrap_or(body.len()));
        is_ident(name).then_some((name, rest))
    }

    fn is_condition_met(expr: &str, vars: &IndexMap<String, String>) -> Result<bool, Error> {
        let text = expand(expr, vars)?;
        let expr = Parser::new(Lexer::new(&text.text)).parse(vars).map_err(|mut err| {
            err.offset = text.locate(err.offset);
            err
        })?;
        let expr = expr.trim();

        if expr.is_empty() {
            return Err(Error {
                offset: 0,
                message: "empty conditional expression in #if/#elif".into(),
            });
        }

        let normalized = Self::normalize_negation(expr)?;
        let mut context = Context::default();
        context.add_function("__astra_neg", |val: Value| -> cel_interpreter::ResolveResult {
            match val {
                Value::Int(val) => Value::Int(0) - Value::Int(val),
                Value::Float(val) => Ok(Value::Float(-val)),
                val => Err(cel_interpreter::ExecutionError::UnsupportedUnaryOperator("minus", val)),
            }
        });

        let normalize = |mut message: String| {
            if let Some(first) = message.get_mut(..1) {
                first.make_ascii_lowercase();
            }

            message
        };

        let val = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            Program::compile(&normalized)
                .map_err(|err| Error {
                    offset: 0,
                    message: normalize(err.to_string()).into(),
                })?
                .execute(&context)
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

    fn normalize_negation(expr: &str) -> Result<Cow<'_, str>, Error> {
        if !expr.contains('-') {
            return Ok(Cow::Borrowed(expr));
        }

        let tokens = Lexer::new(expr)
            .filter(|token| token.as_ref().map_or(true, |token| token.kind != TokenKind::Whitespace))
            .collect::<Result<Vec<_>, _>>()?;
        let find_group_ed = |mut curr: usize| {
            let mut depth = 0;
            while let Some(token) = tokens.get(curr) {
                match token.kind {
                    TokenKind::Other('(' | '[' | '{') => depth += 1,
                    TokenKind::Other(')' | ']' | '}') => {
                        depth -= 1;
                        if depth == 0 {
                            return curr + 1;
                        }
                    }
                    _ => {}
                }

                curr += 1;
            }

            curr
        };
        let mut edits = Vec::new();

        for (index, token) in tokens.iter().enumerate() {
            if token.kind != TokenKind::Other('-')
                || tokens
                    .get(index + 1)
                    .is_some_and(|token| matches!(token.kind, TokenKind::Number(_)))
                || index.checked_sub(1).is_some_and(|index| {
                    matches!(
                        tokens[index].kind,
                        TokenKind::Ident(_)
                            | TokenKind::Number(_)
                            | TokenKind::String
                            | TokenKind::Other(')' | ']' | '}')
                    )
                })
            {
                continue;
            }

            let mut ed = index + 1;

            while tokens
                .get(ed)
                .is_some_and(|token| matches!(token.kind, TokenKind::Other('-' | '!')))
            {
                ed += 1;
            }

            let Some(operand) = tokens.get(ed) else { continue };
            ed = if matches!(operand.kind, TokenKind::Other('(' | '[' | '{')) {
                find_group_ed(ed)
            } else {
                ed + 1
            };

            while let Some(token) = tokens.get(ed) {
                match token.kind {
                    TokenKind::Other('(' | '[') => ed = find_group_ed(ed),
                    TokenKind::Other('.')
                        if tokens
                            .get(ed + 1)
                            .is_some_and(|token| matches!(token.kind, TokenKind::Ident(_))) =>
                    {
                        ed += 2;
                    }
                    _ => break,
                }
            }

            edits.push((token.span.st, token.span.ed, "__astra_neg("));
            edits.push((tokens[ed - 1].span.ed, tokens[ed - 1].span.ed, ")"));
        }

        edits.sort_unstable_by_key(|&(st, ed, _)| (st, ed));

        if edits.is_empty() {
            return Ok(Cow::Borrowed(expr));
        }

        let mut normalized = String::with_capacity(expr.len());
        let mut curr = 0;

        for (st, ed, text) in edits {
            normalized.push_str(&expr[curr..st]);
            normalized.push_str(text);
            curr = ed;
        }

        normalized.push_str(&expr[curr..]);
        Ok(Cow::Owned(normalized))
    }

    fn parse_defined(&mut self, st: usize) -> Result<Option<(&'a str, usize)>, Error> {
        let mut tokens = self.lexer.clone();
        let next = loop {
            let Some(token) = tokens.next() else {
                return Ok(None);
            };
            let token = token?;

            if token.kind != TokenKind::Whitespace {
                break token;
            }
        };

        let src = self.lexer.src;
        if next.kind == TokenKind::Other('(') {
            let mut depth = 1;
            while let Some(token) = tokens.next() {
                let token = token?;
                match token.kind {
                    TokenKind::Other('(') => depth += 1,
                    TokenKind::Other(')') => {
                        depth -= 1;
                        if depth == 0 {
                            self.lexer = tokens;
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
            self.lexer = tokens;
            Ok(Some((&src[next.span.st..next.span.ed], next.span.ed)))
        } else {
            Ok(None)
        }
    }

    fn is_active(&self) -> bool {
        self.stack
            .last()
            .is_none_or(|frame| matches!(frame.state, State::Active | State::ElseActive))
    }
}

struct Frame {
    pos: Position,
    state: State,
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

#[cfg(test)]
mod tests {
    use super::super::{Expander, Placeholder};
    use super::*;

    #[test]
    fn preserve_cols() {
        let vars = IndexMap::from([("X".to_owned(), "日本語".to_owned())]);
        let expanded = expand("あ${X}後$${X}${X}", &vars).unwrap();
        assert_eq!(expanded.text, "あ日本語後${X}日本語");
        assert_eq!(
            expanded.edits,
            [(3..7, 3..12, false), (10..15, 15..19, true), (15..19, 19..28, false)]
        );

        for (at, offset) in [(0, 0), (3, 3), (9, 3), (12, 7), (15, 11), (18, 14), (19, 15), (28, 19)] {
            assert_eq!(expanded.locate(at), offset);
        }
    }

    #[test]
    fn preserve_origins() {
        let text = "あ${X}";
        let src = source::Map::new(
            text,
            Position {
                file: Path::new("test.lua").into(),
                line: 1,
                col: 1,
            },
        );
        let vars = IndexMap::from([("X".to_owned(), String::new())]);
        let expanded = expand(text, &vars).unwrap();
        assert_eq!(expanded.locate(expanded.text.len()), text.len());
        let mut map = source::Map::default();
        expanded.append(&mut map, &src, 0..src.len());
        assert_eq!(map.text(), "あ");
        assert_eq!(map.locate(map.len()).to_string(), "test.lua:1:6");

        let vars = IndexMap::from([("X".to_owned(), "日本語\nsecond".to_owned())]);
        let expanded = expand(text, &vars).unwrap();
        let mut map = source::Map::default();
        expanded.append(&mut map, &src, 0..src.len());
        assert_eq!(map.locate(13).to_string(), "test.lua:1:2");
        assert_eq!(map.original(13), (text, 3..7));
        assert_eq!(map.locate(map.len()).to_string(), "test.lua:1:6");
    }

    #[test]
    fn preserve_conditions() {
        let vars = IndexMap::new();
        let pos = Position {
            file: Path::new("test.lua").into(),
            line: 1,
            col: 1,
        };
        let mut parser = Parser::new(Lexer::new(""));

        for directive in ["#if false", "#if true", "#else", "#endif"] {
            assert!(matches!(
                parser.process(Some((directive, false)), &vars, &pos, &[]).unwrap(),
                Action::Drop
            ));
        }

        assert!(matches!(parser.process(None, &vars, &pos, &[]).unwrap(), Action::Drop));
        assert!(matches!(
            parser.process(Some(("#else", false)), &vars, &pos, &[]).unwrap(),
            Action::Drop
        ));
        assert!(matches!(parser.process(None, &vars, &pos, &[]).unwrap(), Action::Keep));
        assert!(matches!(
            parser.process(Some(("#endif", false)), &vars, &pos, &[]).unwrap(),
            Action::Drop
        ));
        parser.finish().unwrap();
        parser.process(Some(("#if true", false)), &vars, &pos, &[]).unwrap();
        assert!(parser.finish().unwrap_err().to_string().contains("test.lua:1:1"));
    }

    #[test]
    fn reject_multiline_placeholders() {
        assert_eq!(
            Expander::parse("${X\n}", 0).unwrap_err().message,
            "unclosed placeholder"
        );
        assert_eq!(
            Expander::parse("${{X}\n}", 0).unwrap_err().message,
            "unclosed placeholder"
        );
        assert_eq!(Expander::parse("${X}\n", 0), Ok(Some((Placeholder::Variable("X"), 4))));
    }

    #[test]
    fn preserve_paths() {
        for src in [r#"#include "x\n\\y""#, r#"#include <x\n\\y>"#] {
            let directive = Parser::parse_directive(src, false).unwrap();
            assert!(matches!(directive, Some(Directive::Include(path, _)) if path == r"x\n\\y"));
        }

        assert!(Parser::parse_directive(r#"#include "a\"b""#, false).is_err());
        assert!(Parser::parse_directive("#include 'file.h'", false).is_err());
        assert!(matches!(
            Parser::parse_directive(r#"#include "dir\""#, false).unwrap(),
            Some(Directive::Include(path, true)) if path == r#"dir\"#
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
            let mut parser = Parser::new(Lexer::new(src));
            let token = parser.lexer.next().unwrap().unwrap();
            let (key, ed) = parser.parse_defined(token.span.ed).unwrap().unwrap();
            assert_eq!(key, expected);
            assert_eq!(token.span.st, 0);
            assert_eq!(ed, src.len());
        }

        assert_eq!(
            Parser::new(Lexer::new("defined+A")).parse(&IndexMap::new()).unwrap(),
            "defined+A"
        );
        let err = Parser::new(Lexer::new("defined(A"))
            .parse(&IndexMap::new())
            .unwrap_err();
        assert_eq!(err.offset, 7);
        assert_eq!(err.message, "unclosed defined argument");
        assert!(Parser::new(Lexer::new("defined(\"A)")).parse(&IndexMap::new()).is_err());
    }

    #[test]
    fn parse_defined() {
        let src = "true defined(A) and defined(\"B\")";
        let mut lexer = Lexer::new(src);
        let _ = lexer.next();
        let parser = Parser::new(lexer);
        let vars = IndexMap::from([("A".to_owned(), String::new()), ("\"B\"".to_owned(), String::new())]);
        assert_eq!(parser.parse(&vars).unwrap(), "true true && true");
        let parser = Parser::new(Lexer::new("defined(\"unfinished"));
        assert!(parser.parse(&IndexMap::new()).is_err());
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
            Parser::parse_directive("#replace VALUE", false).unwrap(),
            Some(Directive::Replace("VALUE"))
        ));
        assert!(matches!(
            Parser::parse_directive("#replace\n VALUE", true).unwrap(),
            Some(Directive::Replace("VALUE"))
        ));
        assert!(matches!(
            Parser::parse_directive("#omit", false).unwrap(),
            Some(Directive::Omit)
        ));
        assert!(matches!(
            Parser::parse_directive("#define VALUE 1 + 2", true).unwrap(),
            Some(Directive::Define("VALUE", "1 + 2"))
        ));
        assert!(matches!(
            Parser::parse_directive("#undef VALUE", true).unwrap(),
            Some(Directive::Undef("VALUE"))
        ));

        for src in ["ordinary", "#unknown value", "#pragma message(\"x\")"] {
            assert!(Parser::parse_directive(src, false).unwrap().is_none(), "{src}");
        }

        for src in ["#omit", "#if true", "#include \"x\"", "#pragma once"] {
            assert!(Parser::parse_directive(src, true).unwrap().is_none(), "{src}");
        }
    }

    #[test]
    fn reject_directives() {
        for src in [
            "#replace",
            "#replace A B",
            "#replace 1",
            "#omit extra",
            "#if",
            "#elif",
            "#ifdef",
            "#ifndef",
            "#define",
            "#define 1 2",
            "#undef A extra",
            "#include",
            "#include \"\"",
            "#include <>",
            "#include \"x",
            "#include \"x\" extra",
        ] {
            assert!(Parser::parse_directive(src, false).is_err(), "{src}");
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
            ("-[1, 2][0] == -1", true),
            ("-int('2') == -2", true),
            ("'a-b' == 'a-b'", true),
        ] {
            assert_eq!(Parser::is_condition_met(expr, &vars).unwrap(), is_expected, "{expr}");
        }

        for expr in [
            "",
            "(",
            "1 +",
            "true ?",
            "[1",
            "${MISSING}",
            "-(-9223372036854775807 - 1)",
            "-int('-9223372036854775808')",
        ] {
            assert!(Parser::is_condition_met(expr, &vars).is_err(), "{expr}");
        }
    }

    #[test]
    fn reject_conditional_order() {
        let pos = Position {
            file: Path::new("input.lua").into(),
            line: 3,
            col: 2,
        };

        for directives in [
            vec!["#else"],
            vec!["#elif true"],
            vec!["#endif"],
            vec!["#if true", "#else", "#else"],
            vec!["#if false", "#else", "#elif true"],
        ] {
            let mut parser = Parser::new(Lexer::new(""));

            for directive in &directives[..directives.len() - 1] {
                parser
                    .process(Some((directive, false)), &IndexMap::new(), &pos, &[])
                    .unwrap();
            }

            let err = parser
                .process(Some((directives.last().unwrap(), false)), &IndexMap::new(), &pos, &[])
                .err()
                .unwrap();
            assert!(err.to_string().starts_with("input.lua:3:2:"), "{directives:?}: {err}");
        }
    }

    #[test]
    fn resolve_inactive_directives() {
        let pos = Position {
            file: Path::new("input.lua").into(),
            line: 1,
            col: 1,
        };
        let vars = IndexMap::new();
        let mut parser = Parser::new(Lexer::new(""));
        parser.process(Some(("#if false", false)), &vars, &pos, &[]).unwrap();

        for directive in [
            "#define VALUE 1",
            "#undef VALUE",
            "#include \"missing.lua\"",
            "#replace VALUE",
            "#omit",
            "#pragma once",
            "#if ${MISSING}",
        ] {
            assert!(
                matches!(
                    parser.process(Some((directive, false)), &vars, &pos, &[]).unwrap(),
                    Action::Drop
                ),
                "{directive}"
            );
        }
        parser.process(Some(("#endif", false)), &vars, &pos, &[]).unwrap();
        parser.process(Some(("#endif", false)), &vars, &pos, &[]).unwrap();
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
            assert_eq!(expand(src, &vars).unwrap().text, expected);
        }
        assert!(matches!(expand("ordinary", &vars).unwrap().text, Cow::Borrowed(_)));

        for src in ["${MISSING}", "${", "${A B}", "${{VALUE}"] {
            assert!(expand(src, &vars).is_err(), "{src}");
        }
    }

    #[test]
    fn directive_cases() {
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
            for prefix in ["#", " # ", "\t#\t"] {
                let src = format!("{prefix}{body}");
                assert!(Parser::parse_directive(&src, false).unwrap().is_some(), "line: {src:?}");
                assert_eq!(
                    Parser::parse_directive(&src, true).unwrap().is_some(),
                    is_block_allowed,
                    "block: {src:?}"
                );
            }
        }
        for (src, expected) in [
            ("# replace", "#replace requires a single variable name"),
            ("# omit extra", "unexpected argument after #omit"),
            ("# if", "empty conditional expression in #if/#elif"),
            ("# undef X Y", "unexpected argument after variable name in #undef"),
            ("# include \"\"", "empty include path"),
            ("# include \"file", "unclosed include path"),
        ] {
            assert_eq!(Parser::parse_directive(src, false).err(), Some(expected), "{src:?}");
        }
        for src in ["", "ordinary", "##include", "#unknown"] {
            assert!(
                Parser::parse_directive(src, false).unwrap().is_none(),
                "ordinary: {src:?}"
            );
        }
    }
}
