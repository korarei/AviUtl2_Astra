use super::source::{self, Position};
use crate::vars::is_ident;
use anyhow::bail;
use cel_interpreter::{Context, Program, Value};
use indexmap::IndexMap;
use std::borrow::Cow;
use std::fmt;
use std::ops::Range;
use std::path::{Path, PathBuf};

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

        let (body, width, close) = if let Some(body) = tail.strip_prefix("${{") {
            (body, 3, "}}")
        } else if let Some(body) = tail.strip_prefix("${") {
            (body, 2, "}")
        } else {
            return Ok(None);
        };

        let body = body.split('\n').next().unwrap();
        if st > 0 && src.as_bytes()[st - 1] == b'$' {
            let ed = body.find(close).map_or(body.len(), |ed| ed + close.len());
            return Ok(Some((Placeholder::Escaped, st + width + ed)));
        }

        let ed = body.find(close).ok_or_else(|| Error {
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

        Ok(Some((Placeholder::Variable(name), st + width + ed + close.len())))
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

pub(super) struct Parser<'a> {
    lexer: Lexer<'a>,
    stack: Vec<Frame>,
}

impl<'a> Parser<'a> {
    pub(super) fn new(lexer: Lexer<'a>) -> Self {
        Self {
            lexer,
            stack: Vec::new(),
        }
    }

    pub(super) fn parse(mut self, vars: &IndexMap<String, String>) -> Result<Cow<'a, str>, Error> {
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
    pub(super) fn process<'text>(
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

    pub(super) fn finish(&self) -> anyhow::Result<()> {
        if let Some(frame) = self.stack.last() {
            bail!("{}: unclosed conditional directive at end of file", frame.pos);
        }

        Ok(())
    }

    pub(super) fn parse_directive(src: &str, is_block: bool) -> Result<Option<Directive<'_>>, &'static str> {
        let Some(body) = src.strip_prefix('#') else {
            return Ok(None);
        };

        let body = body.trim_start();
        let ed = body.find(char::is_whitespace).unwrap_or(body.len());
        let (name, rest) = body.split_at(ed);

        if !is_ident(name) {
            return Ok(None);
        }

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

        let checked = Self::normalize_negation(expr)?;
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
            Program::compile(&checked)
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
        let find_ed = |mut curr: usize| {
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
                find_ed(ed)
            } else {
                ed + 1
            };

            while let Some(token) = tokens.get(ed) {
                match token.kind {
                    TokenKind::Other('(' | '[') => ed = find_ed(ed),
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

        let mut checked = String::with_capacity(expr.len());
        let mut curr = 0;

        for (st, ed, text) in edits {
            checked.push_str(&expr[curr..st]);
            checked.push_str(text);
            curr = ed;
        }

        checked.push_str(&expr[curr..]);
        Ok(Cow::Owned(checked))
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

#[derive(Clone)]
pub(super) struct Lexer<'a> {
    src: &'a str,
    bytes: &'a [u8],
    curr: usize,
}

impl<'a> Lexer<'a> {
    pub(super) fn new(src: &'a str) -> Self {
        Self {
            src,
            bytes: src.as_bytes(),
            curr: 0,
        }
    }

    fn read(&mut self) -> Result<Token<'a>, Error> {
        let st = self.curr;
        let byte = self.bytes[self.curr];
        let kind = if byte.is_ascii_whitespace() {
            while self.bytes.get(self.curr).is_some_and(u8::is_ascii_whitespace) {
                self.curr += 1;
            }
            TokenKind::Whitespace
        } else if matches!(byte, b'\'' | b'"') {
            self.curr += 1;
            loop {
                let Some(&curr) = self.bytes.get(self.curr) else {
                    return Err(Error {
                        offset: st,
                        message: "unclosed string".into(),
                    });
                };

                if curr == b'\\' {
                    self.curr += 1;

                    if self.curr < self.bytes.len() {
                        self.curr += 1;
                    }
                } else if curr == byte {
                    self.curr += 1;
                    break TokenKind::String;
                } else if curr == b'\n' {
                    return Err(Error {
                        offset: st,
                        message: "unclosed string".into(),
                    });
                } else {
                    self.curr += 1;
                }
            }
        } else if matches!(byte, b'a'..=b'z' | b'A'..=b'Z' | b'_' | 0x80..=0xff) {
            while self.curr < self.bytes.len()
                && matches!(self.bytes[self.curr], b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'_' | 0x80..=0xff)
            {
                self.curr += 1;
            }
            TokenKind::Ident(&self.src[st..self.curr])
        } else if byte.is_ascii_digit()
            || (byte == b'.' && self.bytes.get(self.curr + 1).is_some_and(u8::is_ascii_digit))
        {
            let exponent = if self.src[st..].starts_with("0x") || self.src[st..].starts_with("0X") {
                b'p'
            } else {
                b'e'
            };
            let mut prev = byte;
            self.curr += 1;
            while self.curr < self.bytes.len() {
                let curr = self.bytes[self.curr];
                if curr.is_ascii_alphanumeric()
                    || curr == b'_'
                    || (curr == b'.' && self.bytes.get(self.curr + 1) != Some(&b'.'))
                    || (matches!(curr, b'+' | b'-') && prev.to_ascii_lowercase() == exponent)
                {
                    prev = curr;
                    self.curr += 1;
                } else {
                    break;
                }
            }
            TokenKind::Number(&self.src[st..self.curr])
        } else if let Some(op) = ["&&", "||", "==", "~=", "!=", "<=", ">="]
            .into_iter()
            .find(|op| self.src[st..].starts_with(op))
        {
            self.curr += op.len();
            TokenKind::Operator(op)
        } else {
            self.curr += self.src[self.curr..].chars().next().unwrap().len_utf8();
            TokenKind::Other(char::from(byte))
        };

        Ok(Token {
            kind,
            span: Span { st, ed: self.curr },
        })
    }
}

impl<'a> Iterator for Lexer<'a> {
    type Item = Result<Token<'a>, Error>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.curr >= self.bytes.len() {
            return None;
        }

        let token = self.read();

        if token.is_err() {
            self.curr = self.bytes.len();
        }

        Some(token)
    }
}

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

#[cfg(test)]
mod tests {
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

        for src in ["ordinary", " #omit", "#unknown value", "#pragma message(\"x\")"] {
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
}
