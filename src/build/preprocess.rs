use anyhow::bail;
use cel_interpreter::{Context, Program, Value};
use indexmap::IndexMap;
use std::borrow::Cow;
use std::collections::HashSet;
use std::fmt;
use std::path::Path;
use std::sync::Arc;

#[derive(Clone)]
pub(super) struct Position {
    pub(super) file: Arc<Path>,
    pub(super) line: usize,
    pub(super) col: usize,
}

impl fmt::Display for Position {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}:{}", self.file.display(), self.line, self.col)
    }
}

enum Directive<'a> {
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
    Define(&'a str, &'a str),
    Undef(&'a str),
    Include(Cow<'a, str>, bool),
    PragmaOnce,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind<'a> {
    Ident(&'a str),
    String,
    Number,
    Space,
    Operator(&'a str),
    Other(char),
}

#[derive(Clone, Copy)]
struct Token<'a> {
    kind: Kind<'a>,
    st: usize,
    ed: usize,
}

#[derive(Clone)]
struct Lexer<'a> {
    src: &'a str,
    bytes: &'a [u8],
    curr: usize,
}

impl<'a> Lexer<'a> {
    fn new(src: &'a str) -> Self {
        Self {
            src,
            bytes: src.as_bytes(),
            curr: 0,
        }
    }
}

impl<'a> Iterator for Lexer<'a> {
    type Item = Token<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.curr == self.bytes.len() {
            return None;
        }

        let st = self.curr;
        let byte = self.bytes[self.curr];
        let kind = if byte.is_ascii_whitespace() {
            while self.bytes.get(self.curr).is_some_and(u8::is_ascii_whitespace) {
                self.curr += 1;
            }
            Kind::Space
        } else if matches!(byte, b'\'' | b'"') {
            self.curr += 1;
            while self.curr < self.bytes.len() {
                if self.bytes[self.curr] == b'\\' {
                    self.curr += 1;
                    if self.curr < self.bytes.len() {
                        self.curr += 1;
                    }
                } else if self.bytes[self.curr] == byte {
                    self.curr += 1;
                    break;
                } else {
                    self.curr += 1;
                }
            }
            Kind::String
        } else if matches!(byte, b'a'..=b'z' | b'A'..=b'Z' | b'_' | 0x80..=0xff) {
            while self.curr < self.bytes.len()
                && matches!(self.bytes[self.curr], b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'_' | 0x80..=0xff)
            {
                self.curr += 1;
            }
            Kind::Ident(&self.src[st..self.curr])
        } else if byte.is_ascii_digit()
            || (byte == b'.' && self.bytes.get(self.curr + 1).is_some_and(u8::is_ascii_digit))
        {
            let mut prev = byte;
            self.curr += 1;
            while self.curr < self.bytes.len() {
                let curr = self.bytes[self.curr];
                if curr.is_ascii_alphanumeric()
                    || curr == b'_'
                    || (curr == b'.' && self.bytes.get(self.curr + 1) != Some(&b'.'))
                    || (matches!(curr, b'+' | b'-') && matches!(prev, b'e' | b'E' | b'p' | b'P'))
                {
                    prev = curr;
                    self.curr += 1;
                } else {
                    break;
                }
            }
            Kind::Number
        } else if let Some(op) = ["&&", "||", "==", "~=", "!=", "<=", ">="]
            .into_iter()
            .find(|op| self.src[st..].starts_with(op))
        {
            self.curr += op.len();
            Kind::Operator(op)
        } else {
            self.curr += self.src[self.curr..].chars().next()?.len_utf8();
            Kind::Other(char::from(byte))
        };

        Some(Token {
            kind,
            st,
            ed: self.curr,
        })
    }
}

fn parse(src: &str, block: bool) -> Option<Directive<'_>> {
    let body = src.strip_prefix('#')?.trim_start();
    let ed = body.find(char::is_whitespace).unwrap_or(body.len());
    let (name, rest) = body.split_at(ed);
    if !is_ident(name) {
        return None;
    }

    match name {
        "if" if !block => Some(Directive::If(rest.trim())),
        "elif" if !block => Some(Directive::Elif(rest.trim())),
        "ifdef" if !block => Some(Directive::IfDef(parse_key(rest.trim())?)),
        "ifndef" if !block => Some(Directive::IfNDef(parse_key(rest.trim())?)),
        "else" if !block => Some(Directive::Else),
        "endif" if !block => Some(Directive::EndIf),
        "define" | "undef" => {
            let args = rest.trim_start();
            let ed = args.find(char::is_whitespace).unwrap_or(args.len());
            let key = &args[..ed];
            if !is_ident(key) {
                return None;
            }

            let val = args[ed..].trim();
            if name == "undef" {
                val.is_empty().then_some(Directive::Undef(key))
            } else {
                Some(Directive::Define(key, val))
            }
        }
        "include" if !block => {
            let args = rest.trim();
            if let Some(quote) = args
                .as_bytes()
                .first()
                .copied()
                .filter(|byte| matches!(*byte, b'\'' | b'"'))
            {
                let mut path: Option<String> = None;
                let mut st = 1;
                let mut curr = 1;
                while curr < args.len() {
                    if args.as_bytes()[curr] == quote {
                        if !args[curr + 1..].trim().is_empty() {
                            return None;
                        }

                        return Some(Directive::Include(
                            match path {
                                Some(mut path) => {
                                    path.push_str(&args[st..curr]);
                                    Cow::Owned(path)
                                }
                                None => Cow::Borrowed(&args[1..curr]),
                            },
                            true,
                        ));
                    }

                    if args.as_bytes()[curr] == b'\\' {
                        let path = path.get_or_insert_with(String::new);
                        path.push_str(&args[st..curr]);
                        curr += 1;
                        let escape = args[curr..].chars().next()?;
                        match escape {
                            '\\' => path.push('\\'),
                            '"' => path.push('"'),
                            '\'' => path.push('\''),
                            'n' => path.push('\n'),
                            'r' => path.push('\r'),
                            't' => path.push('\t'),
                            _ => {
                                path.push('\\');
                                path.push(escape);
                            }
                        }
                        curr += escape.len_utf8();
                        st = curr;
                    } else {
                        curr += args[curr..].chars().next()?.len_utf8();
                    }
                }

                None
            } else if let Some(path) = args.strip_prefix('<').and_then(|args| args.strip_suffix('>')) {
                (!path.is_empty()).then_some(Directive::Include(Cow::Borrowed(path), false))
            } else {
                None
            }
        }
        "pragma" if !block => {
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
    }
}

pub(super) fn resolve<'a>(
    comment: Option<(&'a str, bool, bool)>,
    conditions: &mut Conditions,
    vars: &IndexMap<String, String>,
    pos: &Position,
) -> anyhow::Result<Action<'a>> {
    let Some((src, block, unclosed)) = comment else {
        return Ok(if conditions.is_active() {
            Action::Keep
        } else {
            Action::Drop
        });
    };
    let Some(directive) = parse(src, block) else {
        return Ok(if conditions.is_active() {
            Action::Keep
        } else {
            Action::Drop
        });
    };

    if conditions.apply(&directive, vars, pos)? {
        return Ok(Action::Drop);
    }

    let active = conditions.is_active();
    match directive {
        Directive::Define(key, val) => {
            if unclosed {
                bail!("{pos}: unclosed block #define/#undef directive");
            }

            Ok(if active { Action::Define(key, val) } else { Action::Drop })
        }
        Directive::Undef(key) => {
            if unclosed {
                bail!("{pos}: unclosed block #define/#undef directive");
            }

            Ok(if active { Action::Undef(key) } else { Action::Drop })
        }
        Directive::Include(path, quoted) if active => Ok(Action::Include(path, quoted)),
        Directive::PragmaOnce if active => Ok(Action::PragmaOnce),
        _ if active => Ok(Action::Keep),
        _ => Ok(Action::Drop),
    }
}

fn parse_key(src: &str) -> Option<&str> {
    let src = src.trim();
    if is_ident(src) {
        return Some(src);
    }

    let src = if src.len() >= 2
        && ((src.starts_with('"') && src.ends_with('"')) || (src.starts_with('\'') && src.ends_with('\'')))
    {
        &src[1..src.len() - 1]
    } else {
        src
    };

    let src = src.trim();
    let (name, ed) = placeholder(src, 0).ok().flatten()?;
    src[ed..].trim().is_empty().then_some(name)
}

fn is_ident(src: &str) -> bool {
    crate::vars::is_ident(src)
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum PlaceholderError<'a> {
    Unclosed,
    Empty,
    InvalidIdent(&'a str),
}

impl std::fmt::Display for PlaceholderError<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unclosed => write!(f, "unclosed placeholder"),
            Self::Empty => write!(f, "empty variable name in placeholder"),
            Self::InvalidIdent(name) => write!(f, "invalid variable name '{name}' in placeholder"),
        }
    }
}

pub(super) fn placeholder(src: &str, st: usize) -> Result<Option<(&str, usize)>, PlaceholderError<'_>> {
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

    let newline_pos = body.find('\n');
    let close_pos = body.find(close);

    let ed = match (close_pos, newline_pos) {
        (Some(c), Some(n)) if c < n => c,
        (Some(c), None) => c,
        _ => return Err(PlaceholderError::Unclosed),
    };

    let name = body[..ed].trim();
    if name.is_empty() {
        return Err(PlaceholderError::Empty);
    }
    if !is_ident(name) {
        return Err(PlaceholderError::InvalidIdent(name));
    }

    Ok(Some((name, st + width + ed + close.len())))
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

#[derive(Clone)]
struct Frame {
    pos: Position,
    state: State,
}

#[derive(Default)]
pub(super) struct Conditions {
    stack: Vec<Frame>,
}

impl Conditions {
    fn is_active(&self) -> bool {
        self.stack
            .last()
            .is_none_or(|frame| matches!(frame.state, State::Active | State::ElseActive))
    }

    fn apply(
        &mut self,
        directive: &Directive<'_>,
        vars: &IndexMap<String, String>,
        pos: &Position,
    ) -> anyhow::Result<bool> {
        match directive {
            Directive::If(expr) => {
                let state = if self.is_active() {
                    if eval(expr, vars).map_err(|err| anyhow::anyhow!("{pos}: {err}"))? {
                        State::Active
                    } else {
                        State::Pending
                    }
                } else {
                    State::ParentInactive
                };

                self.stack.push(Frame {
                    pos: pos.clone(),
                    state,
                });
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
                        if eval(expr, vars).map_err(|err| anyhow::anyhow!("{pos}: {err}"))? {
                            State::Active
                        } else {
                            State::Pending
                        }
                    }
                };
            }
            Directive::IfDef(key) | Directive::IfNDef(key) => {
                let parent = self.is_active();
                let state = if !parent {
                    State::ParentInactive
                } else if vars.contains_key(*key) == matches!(directive, Directive::IfDef(_)) {
                    State::Active
                } else {
                    State::Pending
                };
                self.stack.push(Frame {
                    pos: pos.clone(),
                    state,
                });
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
            }
            Directive::EndIf => {
                if self.stack.pop().is_none() {
                    bail!("{pos}: #endif without matching #if/#ifdef/#ifndef");
                }
            }
            _ => return Ok(false),
        }

        Ok(true)
    }

    pub(super) fn finish(&self) -> anyhow::Result<()> {
        if let Some(frame) = self.stack.last() {
            bail!("{}: unclosed conditional directive at end of file", frame.pos);
        }

        Ok(())
    }
}

#[allow(clippy::too_many_lines)]
fn eval(expr: &str, vars: &IndexMap<String, String>) -> Result<bool, String> {
    let mut expanded = None;
    let mut curr = 0;
    let mut tokens = Lexer::new(expr).peekable();

    while let Some(token) = tokens.next() {
        match token.kind {
            Kind::Other('$') => {
                if let Some((key, ed)) = placeholder(expr, token.st).map_err(|err| err.to_string())? {
                    expanded
                        .get_or_insert_with(|| String::with_capacity(expr.len()))
                        .push_str(&expr[curr..token.st]);
                    expanded.as_mut().unwrap().push_str(key);
                    curr = ed;
                    while tokens.peek().is_some_and(|next| next.st < ed) {
                        let _ = tokens.next();
                    }
                }
            }
            Kind::String => {
                let raw = &expr[token.st..token.ed];
                if let Some(key) = parse_key(raw) {
                    expanded
                        .get_or_insert_with(|| String::with_capacity(expr.len()))
                        .push_str(&expr[curr..token.st]);
                    expanded.as_mut().unwrap().push_str(key);
                    curr = token.ed;
                } else {
                    let mut scan = 0;
                    let mut st = 0;
                    let mut replaced = false;
                    while let Some(rel) = raw[scan..].find('$') {
                        let at = scan + rel;
                        if let Some((key, ed)) = placeholder(raw, at).map_err(|err| err.to_string())? {
                            let expanded = expanded.get_or_insert_with(|| String::with_capacity(expr.len()));
                            if !replaced {
                                expanded.push_str(&expr[curr..token.st]);
                                replaced = true;
                            }
                            expanded.push_str(&raw[st..at]);
                            expanded.push_str(key);
                            st = ed;
                            scan = ed;
                        } else {
                            scan = at + 1;
                        }
                    }
                    if replaced {
                        expanded.as_mut().unwrap().push_str(&raw[st..]);
                        curr = token.ed;
                    }
                }
            }
            _ => {}
        }
    }
    let text = if let Some(mut expanded) = expanded {
        expanded.push_str(&expr[curr..]);
        Cow::Owned(expanded)
    } else {
        Cow::Borrowed(expr)
    };

    let mut result = None;
    let mut curr = 0;
    let mut tokens = Lexer::new(text.as_ref()).peekable();

    while let Some(token) = tokens.next() {
        if token.kind == Kind::Ident("defined") {
            let mut look = tokens.clone();
            let mut separated = false;
            while look.peek().is_some_and(|next| next.kind == Kind::Space) {
                let _ = look.next();
                separated = true;
            }

            let paren = look.peek().is_some_and(|next| next.kind == Kind::Other('('));
            if paren {
                let _ = look.next();
                while look.peek().is_some_and(|next| next.kind == Kind::Space) {
                    let _ = look.next();
                }
            }

            if let Some(Token {
                kind: Kind::Ident(key),
                ed,
                ..
            }) = look.next()
            {
                let ed = if paren {
                    while look.peek().is_some_and(|next| next.kind == Kind::Space) {
                        let _ = look.next();
                    }
                    look.next_if(|next| next.kind == Kind::Other(')')).map(|token| token.ed)
                } else if separated {
                    Some(ed)
                } else {
                    None
                };

                if let Some(ed) = ed {
                    let result = result.get_or_insert_with(|| String::with_capacity(text.len()));
                    result.push_str(&text[curr..token.st]);
                    result.push_str(if vars.contains_key(key) { "true" } else { "false" });
                    curr = ed;
                    while tokens.peek().is_some_and(|next| next.st < ed) {
                        let _ = tokens.next();
                    }
                    continue;
                }
            }
        }

        let replacement = match token.kind {
            Kind::Ident("and") => Some("&&"),
            Kind::Ident("or") => Some("||"),
            Kind::Ident("not") => Some("!"),
            Kind::Ident("nil") => Some("null"),
            Kind::Operator("~=") => Some("!="),
            _ => None,
        };
        if let Some(replacement) = replacement {
            let result = result.get_or_insert_with(|| String::with_capacity(text.len()));
            result.push_str(&text[curr..token.st]);
            result.push_str(replacement);
            curr = token.ed;
        }
    }

    let expr = if let Some(mut result) = result {
        result.push_str(&text[curr..]);
        Cow::Owned(result)
    } else {
        text
    };

    let expr = expr.trim();
    if expr.is_empty() {
        return Err("empty conditional expression in #if/#elif".into());
    }

    let mut context = Context::default();
    let mut names = HashSet::new();
    for token in Lexer::new(expr) {
        let Kind::Ident(name) = token.kind else {
            continue;
        };
        if matches!(name, "true" | "false" | "null") || !names.insert(name) {
            continue;
        }

        let val = if let Some(val) = vars.get(name) {
            let val = val.trim();
            if let Ok(num) = val.parse::<i64>() {
                Value::Int(num)
            } else if let Some(hex) = val.strip_prefix("0x").or_else(|| val.strip_prefix("0X"))
                && let Ok(num) = i64::from_str_radix(hex, 16)
            {
                Value::Int(num)
            } else if val.eq_ignore_ascii_case("true") {
                Value::Bool(true)
            } else if val.eq_ignore_ascii_case("false") {
                Value::Bool(false)
            } else {
                Value::String(val.to_owned().into())
            }
        } else {
            Value::Int(0)
        };

        context
            .add_variable(name, val)
            .map_err(|err| format!("failed to bind variable '{name}': {err}"))?;
    }

    Ok(
        match Program::compile(expr)
            .map_err(|err| err.to_string())?
            .execute(&context)
            .map_err(|err| format!("evaluation error: {err}"))?
        {
            Value::Bool(value) => value,
            Value::Int(value) => value != 0,
            Value::UInt(value) => value != 0,
            Value::Float(value) => value.abs().to_bits() != 0,
            Value::String(value) => !value.is_empty(),
            _ => false,
        },
    )
}
