use super::{Analysis, Assign, Call, Error, Field, Key, Lexer, Literal, Reference, Span, Token, TokenKind};
use std::borrow::{Borrow, Cow};
use std::collections::{HashMap, HashSet};

const MAX_DEPTH: usize = 200;

enum TableKey<'a> {
    Name(&'a str),
    Expr(Span),
}

pub struct Parser<'a, 'tokens, T = Token<'a>> {
    src_ed: usize,
    tokens: &'tokens [T],
    curr: usize,
    prev: usize,
    depth: usize,
    error: Option<Error>,
    stmt_st: Option<usize>,
    ctx: Ctx<'a, 'tokens>,
    scopes: Vec<HashMap<&'a str, usize>>,
    bindings: Vec<Binding<'a>>,
    func_scopes: Vec<usize>,
    writes: Vec<usize>,
    output: Analysis<'a>,
}

impl<'a, 'tokens, T: Borrow<Token<'a>>> Parser<'a, 'tokens, T> {
    #[must_use]
    pub fn new(src: &str, tokens: &'tokens [T]) -> Self {
        Self {
            src_ed: src.len(),
            tokens,
            curr: 0,
            prev: 0,
            depth: 0,
            error: None,
            stmt_st: None,
            ctx: Ctx::default(),
            scopes: vec![HashMap::new()],
            bindings: Vec::new(),
            func_scopes: Vec::new(),
            writes: Vec::new(),
            output: Analysis::default(),
        }
    }

    pub fn parse(mut self) -> Result<Analysis<'a>, Error> {
        self.reset();
        self.curr = 0;
        self.skip_trivia();
        self.parse_block();

        if let Some(error) = self.error {
            return Err(error);
        }

        if let Some(token) = self.token(self.curr) {
            return Err(Error {
                offset: token.span.st,
                message: "unexpected block delimiter",
            });
        }

        Ok(self.output)
    }

    pub fn parse_stmt_at(&mut self, st: usize) -> Result<Span, Error> {
        self.reset();
        self.curr = self.tokens.partition_point(|token| token.borrow().span.st < st);
        self.skip_trivia();

        let st = self.curr;
        let token = self.token(st).ok_or(Error {
            offset: self.src_ed,
            message: "missing statement",
        })?;
        let span_st = token.span.st;

        self.parse_stmt();

        if let Some(error) = self.error {
            return Err(error);
        }

        if self.curr == st {
            return Err(Error {
                offset: span_st,
                message: "invalid statement",
            });
        }

        self.eat(&TokenKind::Semicolon);

        Ok(Span {
            st: span_st,
            ed: self.tokens[self.prev].borrow().span.ed,
        })
    }

    pub fn parse_expr_at(&mut self, st: usize) -> Result<Span, Error> {
        self.reset();
        self.curr = self.tokens.partition_point(|token| token.borrow().span.st < st);
        self.skip_trivia();

        if self.curr < self.ctx.ed {
            self.ctx.stack.clear();
            self.ctx.ed = 0;
        }

        if self.ctx.stack.is_empty() {
            self.ctx.stack.push((None, 0));
        }

        while self.ctx.ed < self.curr {
            let (kind, conditionals) = *self.ctx.stack.last().unwrap();
            let token = self.tokens[self.ctx.ed].borrow();

            match &token.kind {
                TokenKind::LParen
                | TokenKind::LBracket
                | TokenKind::LBrace
                | TokenKind::Keyword("function" | "if" | "while" | "for" | "repeat") => {
                    self.ctx.stack.push((Some(&token.kind), 0));
                }
                TokenKind::Keyword("do") if matches!(kind, Some(TokenKind::Keyword("while" | "for"))) => {
                    self.ctx.stack.last_mut().unwrap().0 = Some(&token.kind);
                }
                TokenKind::Keyword("do") => self.ctx.stack.push((Some(&token.kind), 0)),
                TokenKind::RParen | TokenKind::RBracket | TokenKind::RBrace | TokenKind::Keyword("end" | "until")
                    if self.ctx.stack.len() > 1 =>
                {
                    self.ctx.stack.pop();
                }
                TokenKind::Operator("?") => self.ctx.stack.last_mut().unwrap().1 += 1,
                TokenKind::Colon if conditionals != 0 => self.ctx.stack.last_mut().unwrap().1 -= 1,
                _ => {}
            }

            self.ctx.ed += 1;
        }

        let val = self.parse_expr(0, self.ctx.stack.last().unwrap().1 != 0);

        if let Some(error) = self.error {
            return Err(error);
        }

        val.map(|val| val.span).ok_or(Error {
            offset: self.token(self.curr).map_or(self.src_ed, |token| token.span.st),
            message: "missing expression",
        })
    }

    fn parse_block(&mut self) {
        if self.depth + 1 >= MAX_DEPTH {
            self.error.get_or_insert(Error {
                offset: self.token(self.curr).map_or(self.src_ed, |token| token.span.st),
                message: "chunk has too many syntax levels",
            });
            return;
        }

        self.depth += 1;

        while self.error.is_none()
            && self.kind().is_some()
            && !matches!(
                self.kind(),
                Some(TokenKind::Keyword("end" | "else" | "elseif" | "until"))
            )
        {
            let st = self.curr;
            self.parse_stmt();

            if self.curr == st {
                self.error.get_or_insert(Error {
                    offset: self.tokens[st].borrow().span.st,
                    message: "invalid statement",
                });
            }
        }

        self.depth -= 1;
    }

    fn parse_scoped_block(&mut self) {
        self.scopes.push(HashMap::new());
        self.parse_block();
        self.scopes.pop().expect("scope must exist");
    }

    #[allow(clippy::too_many_lines)]
    fn parse_stmt(&mut self) {
        let st = self.curr;
        let prev_stmt_st = self.stmt_st.replace(self.tokens[st].borrow().span.st);

        match self.kind() {
            Some(TokenKind::Local | TokenKind::Keyword("const"))
                if self.kind() == Some(&TokenKind::Local)
                    || self.peek(1).is_some_and(|token| {
                        token.kind.name().is_some() || token.kind == TokenKind::Keyword("function")
                    }) =>
            {
                self.bump();
                if self.eat(&TokenKind::Keyword("function")) {
                    if let Some(name) = self.require_name() {
                        self.declare(name, None);
                    }

                    self.parse_func(false);
                } else {
                    self.parse_assign(st, true);
                }
            }
            Some(TokenKind::Keyword("function")) => {
                self.bump();
                let mut is_method = false;

                if let Some(name) = self.require_name() {
                    let index = self.resolve(name);
                    self.assign(index, None);

                    while self.eat(&TokenKind::Dot) {
                        if self.require_name().is_none() {
                            break;
                        }
                    }

                    if self.eat(&TokenKind::Colon) {
                        is_method = true;
                        self.require_name();
                    }
                }

                self.parse_func(is_method);
            }
            Some(TokenKind::Keyword("if")) => {
                self.bump();
                self.require_expr(false);
                let writes_st = self.writes.len();
                let references = self
                    .bindings
                    .iter()
                    .map(|binding| binding.reference.clone())
                    .collect::<Vec<_>>();

                self.expect(&TokenKind::Keyword("then"));
                self.parse_scoped_block();

                while self.eat(&TokenKind::Keyword("elseif")) {
                    self.restore(&references);
                    self.require_expr(false);
                    self.expect(&TokenKind::Keyword("then"));
                    self.parse_scoped_block();
                }

                if self.eat(&TokenKind::Keyword("else")) {
                    self.restore(&references);
                    self.parse_scoped_block();
                }

                self.expect(&TokenKind::Keyword("end"));
                self.invalidate(&references, writes_st);
            }
            Some(TokenKind::Keyword("do")) => {
                self.bump();
                self.parse_scoped_block();
                self.expect(&TokenKind::Keyword("end"));
            }
            Some(TokenKind::Keyword(kind @ ("while" | "for" | "repeat"))) => {
                let kind = *kind;
                self.bump();
                let writes_st = self.writes.len();
                let calls_st = self.output.calls.len();
                let references = self
                    .bindings
                    .iter()
                    .map(|binding| binding.reference.clone())
                    .collect::<Vec<_>>();
                let mut names = Vec::new();

                if kind == "for" {
                    while let Some(name) = self.require_name() {
                        names.push(name);

                        if !self.eat(&TokenKind::Comma) {
                            break;
                        }
                    }

                    if names.is_empty() || self.kind() == Some(&TokenKind::Assign) && names.len() != 1 {
                        self.error.get_or_insert(Error {
                            offset: self
                                .token(self.curr)
                                .map_or(self.tokens[self.prev].borrow().span.ed, |token| token.span.st),
                            message: "invalid for variable list",
                        });
                    }

                    if self.eat(&TokenKind::Assign) {
                        self.require_expr(false);
                        self.expect(&TokenKind::Comma);
                        self.require_expr(false);

                        if self.eat(&TokenKind::Comma) {
                            self.require_expr(false);
                        }
                    } else {
                        self.expect(&TokenKind::Keyword("in"));
                        if self.parse_exprs().is_some_and(|vals| vals.is_empty()) {
                            self.error.get_or_insert(Error {
                                offset: self
                                    .token(self.curr)
                                    .map_or(self.tokens[self.prev].borrow().span.ed, |token| token.span.st),
                                message: "missing expression after 'in'",
                            });
                        }
                    }
                } else if kind == "while" {
                    self.require_expr(false);
                }

                if kind != "repeat" {
                    self.expect(&TokenKind::Keyword("do"));
                }

                self.scopes.push(HashMap::new());

                for name in names {
                    self.declare(name, None);
                }

                self.parse_block();

                if kind == "repeat" {
                    self.expect(&TokenKind::Keyword("until"));
                    self.require_expr(false);
                } else {
                    self.expect(&TokenKind::Keyword("end"));
                }

                self.scopes.pop().expect("scope must exist");

                let writes = self.writes[writes_st..]
                    .iter()
                    .filter_map(|&index| references.get(index).and_then(Option::as_deref))
                    .collect::<HashSet<_>>();

                for call in &mut self.output.calls[calls_st..] {
                    if call
                        .reference
                        .as_ref()
                        .is_some_and(|reference| (1..=reference.len()).any(|ed| writes.contains(&reference[..ed])))
                    {
                        call.reference = None;
                    }
                }

                self.invalidate(&references, writes_st);
            }
            Some(TokenKind::Keyword("return")) => {
                self.bump();
                let _ = self.parse_exprs();
            }
            Some(TokenKind::Keyword("break")) => {
                self.bump();
            }
            Some(TokenKind::Keyword("goto")) if self.peek(1).is_some_and(|token| token.kind.name().is_some()) => {
                self.bump();
                self.bump();
            }
            Some(TokenKind::Keyword("continue"))
                if self.peek(1).is_none_or(|token| {
                    matches!(
                        token.kind,
                        TokenKind::Semicolon | TokenKind::Keyword("end" | "else" | "elseif" | "until")
                    )
                }) =>
            {
                self.bump();
            }
            Some(TokenKind::Operator("::")) => {
                self.bump();

                self.require_name();
                self.expect(&TokenKind::Operator("::"));
            }
            Some(TokenKind::Semicolon) => self.bump(),
            _ => self.parse_assign(st, false),
        }

        self.stmt_st = prev_stmt_st;
    }

    #[allow(clippy::too_many_lines)]
    fn parse_assign(&mut self, st: usize, is_local: bool) {
        let mut targets = Vec::new();
        let mut bindings = Vec::new();

        loop {
            let target_st = self.curr;
            let name = self.kind().and_then(TokenKind::name);

            let expr = if is_local {
                if self.require_name().is_none() {
                    return;
                }

                Expr {
                    span: self.tokens[target_st].borrow().span,
                    reference: None,
                    binding: None,
                    captures: Vec::new(),
                    is_target: true,
                    is_call: false,
                }
            } else {
                let Some(expr) = self.parse_expr(15, false) else {
                    self.fail("missing assignment target");
                    return;
                };

                expr
            };

            if !is_local && !expr.is_target {
                if expr.is_call && targets.is_empty() && self.kind() != Some(&TokenKind::Assign) {
                    return;
                }

                self.fail("invalid assignment target");
                return;
            }

            bindings.push((name.filter(|_| self.prev == target_st), expr.binding));
            targets.push(expr.span);

            if !self.eat(&TokenKind::Comma) {
                break;
            }
        }

        let has_vals = self.eat(&TokenKind::Assign);
        let is_compound = if !is_local && !has_vals && targets.len() == 1 {
            if self.eat(&TokenKind::Ne) {
                true
            } else if matches!(
                self.kind(),
                Some(
                    TokenKind::Other('+' | '-' | '*' | '/' | '%')
                        | TokenKind::Operator("&" | "|" | "<<" | ">>" | "~>>")
                        | TokenKind::Concat
                )
            ) && self.peek(1).is_some_and(|token| {
                token.kind == TokenKind::Assign && self.tokens[self.curr].borrow().span.ed == token.span.st
            }) {
                self.bump();
                self.bump();
                true
            } else {
                false
            }
        } else {
            false
        };

        if !is_local && !has_vals && !is_compound {
            self.fail("expected assignment or function call");
            return;
        }

        let mut vals = if has_vals || is_compound {
            let Some(vals) = self.parse_exprs() else { return };

            if vals.is_empty() {
                self.error.get_or_insert(Error {
                    offset: self.tokens[self.prev].borrow().span.ed,
                    message: "missing assignment value",
                });
                return;
            }

            vals
        } else {
            Vec::new()
        };

        for (index, (name, binding)) in bindings.into_iter().enumerate() {
            let mut val = vals.get_mut(index);
            let reference = if is_compound {
                None
            } else {
                val.as_mut().and_then(|val| val.reference.take())
            };

            let binding = if is_local {
                name.map(|name| self.declare(name, reference))
            } else {
                if let Some(binding) = binding {
                    self.assign(binding, if name.is_some() { reference } else { None });
                }

                binding.filter(|_| name.is_some())
            };

            if let Some(binding) = binding {
                self.bindings[binding].captures = val.map_or_else(Vec::new, |val| std::mem::take(&mut val.captures));
            }
        }

        if is_compound {
            return;
        }

        self.output.assigns.push(Assign {
            span: Span {
                st: self.tokens[st].borrow().span.st,
                ed: self.tokens[self.prev].borrow().span.ed,
            },
            targets,
            vals: vals.into_iter().map(|val| val.span).collect(),
        });
    }

    fn parse_exprs(&mut self) -> Option<Vec<Expr<'a>>> {
        let mut vals = Vec::new();
        let Some(val) = self.parse_expr(0, false) else {
            return Some(vals);
        };

        vals.push(val);

        while self.eat(&TokenKind::Comma) {
            let Some(val) = self.parse_expr(0, false) else {
                self.error.get_or_insert(Error {
                    offset: self.tokens[self.prev].borrow().span.ed,
                    message: "missing expression after comma",
                });
                return None;
            };

            vals.push(val);
        }

        Some(vals)
    }

    #[allow(clippy::too_many_lines)]
    fn parse_expr(&mut self, min_prec: u8, should_stop_at_colon: bool) -> Option<Expr<'a>> {
        if self.error.is_some() {
            return None;
        }

        if self.depth + 1 >= MAX_DEPTH {
            self.error = Some(Error {
                offset: self.token(self.curr).map_or(self.src_ed, |token| token.span.st),
                message: "chunk has too many syntax levels",
            });
            return None;
        }

        self.depth += 1;
        let curr = self.curr;

        let val = (|| {
            let token = self.token(self.curr)?;
            let st = token.span.st;
            let mut reference = None;
            let mut binding = None;
            let mut captures = Vec::new();
            let mut is_prefix = false;
            let mut is_target = false;
            let mut is_call = false;
            let mut is_safe = false;

            match &token.kind {
                TokenKind::Ident(name) | TokenKind::Keyword(name @ ("goto" | "const" | "continue")) => {
                    let name = *name;
                    self.bump();

                    if self.kind() == Some(&TokenKind::Operator("->")) {
                        self.parse_short_func(Some(name), should_stop_at_colon);
                        return Some(Expr {
                            span: Span {
                                st,
                                ed: self.tokens[self.prev].borrow().span.ed,
                            },
                            reference: None,
                            binding: None,
                            captures: Vec::new(),
                            is_target: false,
                            is_call: false,
                        });
                    }

                    is_prefix = true;
                    is_target = true;
                    let index = self.resolve(name);
                    reference.clone_from(&self.bindings[index].reference);
                    binding = Some(index);
                    captures.clone_from(&self.bindings[index].captures);

                    if self
                        .func_scopes
                        .last()
                        .is_some_and(|&scope| self.bindings[index].scope < scope)
                    {
                        captures.push(index);
                    }
                }
                TokenKind::String(_)
                | TokenKind::Number(_)
                | TokenKind::Ellipsis
                | TokenKind::Keyword("nil" | "true" | "false") => self.bump(),
                TokenKind::Other('-' | '#') | TokenKind::Operator("~" | "!") | TokenKind::Keyword("not") => {
                    self.bump();
                    self.parse_expr(13, false)?;
                }
                TokenKind::LParen => {
                    is_prefix = true;
                    self.bump();
                    let val = self.parse_expr(0, false)?;
                    reference = val.reference;
                    binding = val.binding;
                    captures = val.captures;
                    if !self.expect(&TokenKind::RParen) {
                        return None;
                    }
                }
                TokenKind::LBrace => self.parse_table(None)?,
                TokenKind::Keyword("function") => {
                    self.bump();
                    self.parse_func(false);
                }
                TokenKind::Operator("|" | "||") => self.parse_short_func(None, should_stop_at_colon),
                _ => return None,
            }

            if is_prefix {
                loop {
                    match self.kind() {
                        Some(TokenKind::Dot) => {
                            self.bump();
                            is_target = !is_safe;
                            is_call = false;
                            let name = self.require_name()?;

                            if let Some(reference) = &mut reference {
                                reference.push(name);
                            }
                        }
                        Some(TokenKind::LBracket) => {
                            self.bump();
                            is_target = !is_safe;
                            is_call = false;
                            let key_st = self.curr;
                            self.parse_expr(0, false)?;

                            if self.prev == key_st
                                && let TokenKind::String(key) = &self.tokens[key_st].borrow().kind
                                && let Cow::Borrowed(key) = &key.val
                                && let Ok(key) = std::str::from_utf8(key)
                            {
                                if let Some(reference) = &mut reference {
                                    reference.push(key);
                                }
                            } else {
                                reference = None;
                            }

                            if !self.expect(&TokenKind::RBracket) {
                                return None;
                            }
                        }
                        Some(TokenKind::Operator("?.")) => {
                            self.bump();
                            is_safe = true;
                            is_target = false;
                            is_call = false;
                            if let Some(name) = self.kind().and_then(TokenKind::name) {
                                self.bump();
                                if let Some(reference) = &mut reference {
                                    reference.push(name);
                                }
                            } else if !matches!(
                                self.kind(),
                                Some(
                                    TokenKind::LBracket
                                        | TokenKind::Colon
                                        | TokenKind::LParen
                                        | TokenKind::LBrace
                                        | TokenKind::String(_)
                                )
                            ) {
                                self.fail("missing safe navigation suffix");
                                return None;
                            }
                        }
                        Some(TokenKind::Colon) if !should_stop_at_colon => {
                            self.bump();
                            self.require_name()?;
                            if self.eat(&TokenKind::Operator("?.")) {
                                is_safe = true;
                            }

                            if !matches!(
                                self.kind(),
                                Some(TokenKind::LParen | TokenKind::LBrace | TokenKind::String(_))
                            ) {
                                self.fail("missing method arguments");
                                return None;
                            }
                            reference = None;
                        }
                        Some(TokenKind::LParen | TokenKind::LBrace | TokenKind::String(_)) => {
                            is_target = false;
                            is_call = true;
                            let args = if self.eat(&TokenKind::LParen) {
                                let args = self.parse_exprs()?;

                                if !self.expect(&TokenKind::RParen) {
                                    return None;
                                }

                                args
                            } else {
                                vec![self.parse_expr(15, false)?]
                            };

                            if reference.is_some() {
                                for index in &captures {
                                    self.bindings[*index].calls.push(self.output.calls.len());
                                }
                            }

                            self.output.calls.push(Call {
                                stmt_st: self.stmt_st,
                                span: Span {
                                    st,
                                    ed: self.tokens[self.prev].borrow().span.ed,
                                },
                                reference,
                                args: args.into_iter().map(|arg| arg.span).collect(),
                            });
                            reference = None;
                            binding = None;
                            captures = Vec::new();
                        }
                        _ => break,
                    }
                }
            }

            loop {
                let (prec, is_right_associative) = match self.kind() {
                    Some(TokenKind::Keyword("or") | TokenKind::Operator("||" | "??")) => (1, false),
                    Some(TokenKind::Keyword("and") | TokenKind::Operator("&&")) => (2, false),
                    Some(
                        TokenKind::Eq
                        | TokenKind::Ne
                        | TokenKind::Le
                        | TokenKind::Ge
                        | TokenKind::Other('<' | '>')
                        | TokenKind::Operator("!="),
                    ) => (3, false),
                    Some(TokenKind::Operator("|")) => (4, false),
                    Some(TokenKind::Operator("~")) => (5, false),
                    Some(TokenKind::Operator("&")) => (6, false),
                    Some(TokenKind::Operator("<<" | ">>" | "~>>")) => (7, false),
                    Some(TokenKind::Concat) => (9, true),
                    Some(TokenKind::Other('+' | '-')) => (10, false),
                    Some(TokenKind::Other('*' | '/' | '%')) => (11, false),
                    Some(TokenKind::Other('^')) => (14, true),
                    _ => break,
                };

                if prec < min_prec {
                    break;
                }

                self.bump();
                self.parse_expr(prec + u8::from(!is_right_associative), should_stop_at_colon)?;
                is_target = false;
                is_call = false;
                reference = None;
                binding = None;
                captures.clear();
            }

            if min_prec == 0 && self.eat(&TokenKind::Operator("?")) {
                self.parse_expr(0, true)?;

                if !self.expect(&TokenKind::Colon) {
                    return None;
                }

                self.parse_expr(0, false)?;
                is_target = false;
                is_call = false;
                reference = None;
                binding = None;
                captures.clear();
            }

            Some(Expr {
                span: Span {
                    st,
                    ed: self.tokens[self.prev].borrow().span.ed,
                },
                reference,
                binding,
                captures,
                is_target,
                is_call,
            })
        })();

        self.depth -= 1;
        if val.is_none() && self.curr != curr {
            self.error.get_or_insert(Error {
                offset: self.token(self.curr).map_or(self.src_ed, |token| token.span.st),
                message: "incomplete expression",
            });
        }
        val
    }

    fn parse_func(&mut self, is_method: bool) {
        if !self.expect(&TokenKind::LParen) {
            return;
        }

        let mut params = self.parse_params(&TokenKind::RParen);

        if is_method {
            params.insert(0, "self");
        }

        self.parse_func_body(params, false, false);
    }

    fn parse_short_func(&mut self, name: Option<&'a str>, should_stop_at_colon: bool) {
        let params = if let Some(name) = name {
            vec![name]
        } else if self.eat(&TokenKind::Operator("||")) {
            Vec::new()
        } else if self.eat(&TokenKind::Operator("|")) {
            self.parse_params(&TokenKind::Operator("|"))
        } else {
            return;
        };

        if self.expect(&TokenKind::Operator("->")) {
            self.parse_func_body(params, true, should_stop_at_colon);
        }
    }

    fn parse_func_body(&mut self, params: Vec<&'a str>, is_short: bool, should_stop_at_colon: bool) {
        let prev_stmt_st = self.stmt_st.take();
        let references = self
            .bindings
            .iter()
            .map(|binding| binding.reference.clone())
            .collect::<Vec<_>>();
        let writes_st = self.writes.len();

        self.func_scopes.push(self.scopes.len());
        self.scopes.push(HashMap::new());

        for name in params {
            self.declare(name, None);
        }

        if is_short && !self.eat(&TokenKind::Keyword("do")) {
            self.require_expr(should_stop_at_colon);
        } else {
            self.parse_block();
            self.expect(&TokenKind::Keyword("end"));
        }

        self.scopes.pop().expect("scope must exist");
        self.func_scopes.pop().expect("function scope must exist");
        self.invalidate(&references, writes_st);
        self.stmt_st = prev_stmt_st;
    }

    fn parse_params(&mut self, closing: &TokenKind<'a>) -> Vec<&'a str> {
        let mut params = Vec::new();

        if self.eat(closing) {
            return params;
        }

        loop {
            if self.eat(&TokenKind::Ellipsis) {
                break;
            }

            let Some(name) = self.require_name() else { return params };
            params.push(name);

            if !self.eat(&TokenKind::Comma) {
                break;
            }
        }

        self.expect(closing);
        params
    }

    fn reset(&mut self) {
        self.prev = 0;
        self.depth = 0;
        self.error = None;
        self.stmt_st = None;
        self.scopes.truncate(1);
        self.scopes[0].clear();
        self.bindings.clear();
        self.func_scopes.clear();
        self.writes.clear();
        self.output.assigns.clear();
        self.output.calls.clear();
    }

    fn declare(&mut self, name: &'a str, reference: Reference<'a>) -> usize {
        let scope = self.scopes.len() - 1;
        let index = self.bindings.len();
        self.bindings.push(Binding {
            name,
            reference,
            scope,
            captures: Vec::new(),
            calls: Vec::new(),
        });
        self.scopes[scope].insert(name, index);
        index
    }

    fn resolve(&mut self, name: &'a str) -> usize {
        if let Some(index) = self.scopes.iter().rev().find_map(|scope| scope.get(name)) {
            return *index;
        }

        let index = self.bindings.len();
        self.bindings.push(Binding {
            name,
            reference: Some(vec![name]),
            scope: 0,
            captures: Vec::new(),
            calls: Vec::new(),
        });
        self.scopes[0].insert(name, index);
        index
    }

    fn assign(&mut self, binding: usize, reference: Reference<'a>) {
        if self.bindings[binding].reference != reference {
            for call in std::mem::take(&mut self.bindings[binding].calls) {
                self.output.calls[call].reference = None;
            }
        }

        self.bindings[binding].reference = reference;
        self.bindings[binding].captures.clear();
        self.writes.push(binding);
    }

    fn restore(&mut self, references: &[Reference<'a>]) {
        for (index, binding) in self.bindings.iter_mut().enumerate() {
            if binding.scope < self.scopes.len() {
                if let Some(reference) = references.get(index) {
                    binding.reference.clone_from(reference);
                } else {
                    binding.reference = Some(vec![binding.name]);
                }
            }
        }
    }

    fn invalidate(&mut self, references: &[Reference<'a>], writes_st: usize) {
        self.restore(references);

        let writes_ed = self.writes.len();

        for at in writes_st..writes_ed {
            let index = self.writes[at];
            if self.bindings[index].scope < self.scopes.len() {
                self.assign(index, None);
            }
        }
    }

    fn require_name(&mut self) -> Option<&'a str> {
        if let Some(name) = self.kind().and_then(TokenKind::name) {
            self.bump();
            return Some(name);
        }

        self.fail("missing identifier");
        None
    }

    fn require_expr(&mut self, should_stop_at_colon: bool) {
        if self.parse_expr(0, should_stop_at_colon).is_none() {
            self.fail("missing expression");
        }
    }

    fn expect(&mut self, kind: &TokenKind<'a>) -> bool {
        if self.eat(kind) {
            return true;
        }

        self.fail("missing syntax delimiter");

        false
    }

    fn eat(&mut self, kind: &TokenKind<'a>) -> bool {
        if self.kind() == Some(kind) {
            self.bump();
            true
        } else {
            false
        }
    }

    fn token(&self, pos: usize) -> Option<&'tokens Token<'a>> {
        self.tokens.get(pos).map(Borrow::borrow)
    }

    fn kind(&self) -> Option<&TokenKind<'a>> {
        self.token(self.curr).map(|token| &token.kind)
    }

    fn peek(&self, offset: usize) -> Option<&'tokens Token<'a>> {
        self.tokens[self.curr..]
            .iter()
            .map(Borrow::borrow)
            .filter(|token| !token.kind.is_trivia())
            .nth(offset)
    }

    fn bump(&mut self) {
        self.prev = self.curr;
        self.curr += 1;
        self.skip_trivia();
    }

    fn skip_trivia(&mut self) {
        while self.token(self.curr).is_some_and(|token| token.kind.is_trivia()) {
            self.curr += 1;
        }
    }

    fn fail(&mut self, message: &'static str) {
        self.error.get_or_insert(Error {
            offset: self.token(self.curr).map_or(self.src_ed, |token| token.span.st),
            message,
        });
    }

    fn parse_table(&mut self, mut fields: Option<&mut Vec<TableField<'a>>>) -> Option<()> {
        self.bump();

        while self.kind().is_some() && self.kind() != Some(&TokenKind::RBrace) {
            let key = if self.eat(&TokenKind::LBracket) {
                let Some(expr) = self.parse_expr(0, false) else {
                    self.fail("missing expression");
                    return None;
                };

                if !self.expect(&TokenKind::RBracket) || !self.expect(&TokenKind::Assign) {
                    return None;
                }

                Some(TableKey::Expr(expr.span))
            } else if let Some(name) = self.kind().and_then(TokenKind::name)
                && self.peek(1).is_some_and(|token| token.kind == TokenKind::Assign)
            {
                self.bump();
                self.bump();
                Some(TableKey::Name(name))
            } else {
                None
            };

            let Some(val) = self.parse_expr(0, false) else {
                self.fail("missing expression");
                return None;
            };

            if let Some(fields) = &mut fields {
                fields.push(TableField { key, val: val.span });
            }

            if self.kind() == Some(&TokenKind::RBrace) {
                break;
            }

            if !self.eat(&TokenKind::Comma) && !self.eat(&TokenKind::Semicolon) {
                self.expect(&TokenKind::Comma);
                return None;
            }
        }

        if !self.expect(&TokenKind::RBrace) {
            return None;
        }

        Some(())
    }
}

#[derive(Default)]
struct Ctx<'a, 'tokens> {
    ed: usize,
    stack: Vec<(Option<&'tokens TokenKind<'a>>, usize)>,
}

struct Binding<'a> {
    name: &'a str,
    reference: Reference<'a>,
    scope: usize,
    captures: Vec<usize>,
    calls: Vec<usize>,
}

struct Expr<'a> {
    span: Span,
    reference: Reference<'a>,
    binding: Option<usize>,
    captures: Vec<usize>,
    is_target: bool,
    is_call: bool,
}

struct TableField<'a> {
    key: Option<TableKey<'a>>,
    val: Span,
}

#[must_use]
pub fn parse_number(src: &str) -> Option<f64> {
    let src = src.trim_matches(|ch| matches!(ch, ' ' | '\t'..='\u{c}'));
    let is_neg = src.starts_with('-');
    let src = src.trim_prefix(['+', '-']);

    if src.eq_ignore_ascii_case("nan") {
        return Some(f64::NAN);
    }

    if src.eq_ignore_ascii_case("inf") || src.eq_ignore_ascii_case("infinity") {
        return Some(if is_neg { f64::NEG_INFINITY } else { f64::INFINITY });
    }

    if !matches!(src.as_bytes(), [b'0'..=b'9', ..] | [b'.', b'0'..=b'9', ..]) {
        return None;
    }

    let src = if src.contains('_') {
        Cow::Owned(src.replace('_', ""))
    } else {
        Cow::Borrowed(src)
    };
    let src = src.as_ref();

    let val = match src.as_bytes() {
        [b'0', b'x' | b'X', ..] => parse_hex(&src[2..])?,
        [b'0', b'b' | b'B', b'0'..=b'1', ..] => to_float(u64::from_str_radix(&src[2..], 2).ok()?, 0, false),
        _ => {
            split_number(src, ['e', 'E'])?;

            src.parse::<f64>().ok()?
        }
    };

    Some(if is_neg { -val } else { val })
}

#[must_use]
pub fn parse_integer(src: &str) -> Option<i64> {
    let src = src.trim_matches(|ch| matches!(ch, ' ' | '\t'..='\u{c}'));
    let is_neg = src.starts_with('-');
    let src = src.trim_prefix(['+', '-']);

    if !matches!(src.as_bytes(), [b'0'..=b'9', ..]) {
        return None;
    }

    let src = if src.contains('_') {
        Cow::Owned(src.replace('_', ""))
    } else {
        Cow::Borrowed(src)
    };
    let (src, radix) = match src.as_bytes() {
        [b'0', b'x' | b'X', ..] => (&src[2..], 16),
        [b'0', b'b' | b'B', ..] => (&src[2..], 2),
        _ => (src.as_ref(), 10),
    };

    if src.starts_with('+') {
        return None;
    }

    let val = u64::from_str_radix(src, radix).ok()?;

    if is_neg {
        0i64.checked_sub_unsigned(val)
    } else {
        i64::try_from(val).ok()
    }
}

pub fn is_ident(name: &str) -> Result<bool, Error> {
    let mut tokens = Lexer::new(name);
    Ok(matches!(
        tokens.next().transpose()?.map(|token| token.kind),
        Some(ref kind) if kind.name().is_some()
    ) && tokens.next().transpose()?.is_none())
}

pub fn parse_byte_string(src: &str) -> Result<Option<Cow<'_, [u8]>>, Error> {
    Ok(parse_literal(src)?.map(|(literal, _)| literal.val))
}

pub fn parse_string(src: &str) -> Result<Option<Cow<'_, str>>, Error> {
    let Some((literal, span)) = parse_literal(src)? else {
        return Ok(None);
    };

    let error = Error {
        offset: span.st,
        message: "string is not valid UTF-8",
    };

    let val = match literal.val {
        Cow::Borrowed(bytes) => Cow::Borrowed(std::str::from_utf8(bytes).map_err(|_| error)?),
        Cow::Owned(bytes) => Cow::Owned(String::from_utf8(bytes).map_err(|_| error)?),
    };

    Ok(Some(val))
}

pub fn parse_table(src: &str) -> Result<Option<Vec<Field<'_>>>, Error> {
    let tokens: Vec<_> = Lexer::new(src).collect::<Result<_, _>>()?;
    let mut parser = Parser::new(src, &tokens);
    parser.skip_trivia();

    if parser.kind() != Some(&TokenKind::LBrace) {
        return Ok(None);
    }

    let mut fields = Vec::new();
    parser.parse_table(Some(&mut fields));

    if let Some(error) = parser.error {
        return Err(error);
    }

    if parser.kind().is_some() {
        return Ok(None);
    }

    Ok(Some(
        fields
            .into_iter()
            .map(|field| Field {
                key: field.key.map(|key| match key {
                    TableKey::Name(name) => Key::Name(name),
                    TableKey::Expr(span) => Key::Expr(&src[span.st..span.ed]),
                }),
                val: &src[field.val.st..field.val.ed],
            })
            .collect(),
    ))
}

fn parse_hex(src: &str) -> Option<f64> {
    let (src, exp) = split_number(src, ['p', 'P'])?;
    let mut mantissa = 0u64;
    let mut shift = 0i64;
    let mut has_remainder = false;
    let mut has_digits = false;
    let mut has_dot = false;

    for byte in src.bytes() {
        let digit = match byte {
            b'0'..=b'9' => byte - b'0',
            b'a'..=b'f' => byte - b'a' + 10,
            b'A'..=b'F' => byte - b'A' + 10,
            b'.' if !has_dot => {
                has_dot = true;
                continue;
            }
            _ => return None,
        };

        has_digits = true;

        if 64 - mantissa.leading_zeros() <= 56 {
            mantissa = (mantissa << 4) | u64::from(digit);

            if has_dot {
                shift = shift.saturating_sub(4);
            }
        } else {
            if !has_dot {
                shift = shift.saturating_add(4);
            }

            has_remainder |= digit != 0;
        }
    }

    if !has_digits {
        return None;
    }

    Some(to_float(mantissa, exp.saturating_add(shift), has_remainder))
}

fn split_number(src: &str, marker: [char; 2]) -> Option<(&str, i64)> {
    let (src, exp) = if let Some((src, exp)) = src.split_once(marker) {
        let exp = exp.parse::<i32>().ok()?;

        if exp.unsigned_abs() >= 1 << 20 {
            return None;
        }

        (src, i64::from(exp))
    } else {
        (src, 0)
    };

    if let Some((_, fraction)) = src.split_once('.')
        && fraction.trim_end_matches('0').len() >= 1 << 20
    {
        return None;
    }

    Some((src, exp))
}

fn to_float(mantissa: u64, exp: i64, has_remainder: bool) -> f64 {
    if mantissa == 0 {
        return 0.0;
    }

    let bits = 64 - mantissa.leading_zeros();
    let mut exponent = exp.saturating_add(i64::from(bits) - 1);

    if exponent > 1023 {
        return f64::INFINITY;
    }

    if exponent < -1075 {
        return 0.0;
    }

    let round_right = |val: u64, drop: u32| -> u64 {
        let half = 1u64 << (drop - 1);
        let rem = val & ((1u64 << drop) - 1);
        let val = val >> drop;
        let should_round_up = rem > half || (rem == half && (has_remainder || (val & 1) != 0));

        val + u64::from(should_round_up)
    };

    if exponent >= -1022 {
        let sig = if bits <= 53 {
            mantissa << (53 - bits)
        } else {
            round_right(mantissa, bits - 53)
        };

        let sig = if sig == (1u64 << 53) {
            exponent += 1;

            if exponent > 1023 {
                return f64::INFINITY;
            }

            sig >> 1
        } else {
            sig
        };

        let biased_exp = u64::try_from(exponent + 1023).unwrap();

        f64::from_bits((biased_exp << 52) | (sig & ((1u64 << 52) - 1)))
    } else {
        let sig = if exp >= -1074 {
            mantissa << u32::try_from(exp + 1074).unwrap()
        } else {
            round_right(mantissa, u32::try_from(-1074 - exp).unwrap())
        };

        f64::from_bits(sig)
    }
}

fn parse_literal(src: &str) -> Result<Option<(Literal<'_>, Span)>, Error> {
    let mut tokens = Lexer::new(src).filter(|token| token.as_ref().map_or(true, |token| !token.kind.is_trivia()));
    let Some(Token {
        kind: TokenKind::String(literal),
        span,
    }) = tokens.next().transpose()?
    else {
        return Ok(None);
    };

    if tokens.next().transpose()?.is_some() {
        return Ok(None);
    }

    Ok(Some((literal, span)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_integers() {
        for (src, expected) in [
            ("0", 0),
            ("+0", 0),
            ("-0", 0),
            ("42", 42),
            ("+42", 42),
            ("-42", -42),
            (" \t-1_000\n", -1000),
            ("1__0_", 10),
            ("0x2a", 42),
            ("+0X2A", 42),
            ("-0_xF_F", -255),
            ("0b101010", 42),
            ("-0_B10_10", -10),
            ("9007199254740993", 9_007_199_254_740_993),
            ("-9007199254740993", -9_007_199_254_740_993),
            ("9223372036854775807", i64::MAX),
            ("-9223372036854775808", i64::MIN),
            ("0x7fff_ffff_ffff_ffff", i64::MAX),
            ("-0x8000_0000_0000_0000", i64::MIN),
        ] {
            assert_eq!(parse_integer(src), Some(expected), "{src}");
        }

        assert_eq!(parse_integer(&format!("0b{}", "1".repeat(63))), Some(i64::MAX));
        assert_eq!(parse_integer(&format!("-0b1{}", "0".repeat(63))), Some(i64::MIN));
        assert_eq!(parse_integer(&format!("0b{}1", "0".repeat(100))), Some(1));
    }

    #[test]
    fn rejects_integers() {
        for src in [
            "",
            " ",
            "+",
            "-",
            "++1",
            "--1",
            "+-1",
            "- 1",
            "_1",
            "+_1",
            "1.0",
            "1.5",
            ".5",
            "1e3",
            "0x1p3",
            "0x1.0",
            "0x",
            "0b",
            "0x+1",
            "0b+1",
            "0b2",
            "0xg",
            "NaN",
            "inf",
            "-infinity",
            "1LL",
            "1i",
            "9223372036854775808",
            "-9223372036854775809",
            "18446744073709551616",
            "0x8000000000000000",
            "-0x8000000000000001",
            "0x10000000000000000",
            "\u{a0}1",
            "1\u{a0}",
        ] {
            assert_eq!(parse_integer(src), None, "{src}");
        }

        assert_eq!(parse_integer(&format!("0b1{}", "0".repeat(63))), None);
        assert_eq!(parse_integer(&format!("-0b{}", "1".repeat(64))), None);
    }

    #[test]
    fn parses_luajit_numbers() {
        for (src, expected) in [
            ("0", 0.0),
            (" 1", 1.0),
            ("1 ", 1.0),
            ("\t\n\u{b}\u{c} -0x1p2 \t", -4.0),
            ("+0", 0.0),
            ("-0", -0.0),
            ("42", 42.0),
            ("1_000", 1000.0),
            ("1__0_", 10.0),
            ("-1_2.5_0e+1", -125.0),
            ("1e_+_3", 1000.0),
            ("+.1_25", 0.125),
            ("+5", 5.0),
            ("-3.5", -3.5),
            (".5", 0.5),
            ("+.5", 0.5),
            ("-.5", -0.5),
            ("5.", 5.0),
            ("1e3", 1000.0),
            ("2.5E-2", 0.025),
            ("0x0", 0.0),
            ("+0x2", 2.0),
            ("-0XFF", -255.0),
            ("0_xF_F", 255.0),
            ("0x1_.8_p+1_0", 1536.0),
            ("0x1.fp3", 15.5),
            ("-0x.8p0", -0.5),
            ("+0x1.P-2", 0.25),
            ("0x1.8", 1.5),
            ("0x10000000000000000", 18_446_744_073_709_551_616.0),
            ("0x1.fffffffffffffp1023", f64::MAX),
            ("0x1p-1022", f64::MIN_POSITIVE),
            ("0x0.fffffffffffffp-1022", f64::from_bits((1 << 52) - 1)),
            ("0x0.fffffffffffff8p-1022", f64::MIN_POSITIVE),
            ("0x0p999999", 0.0),
            ("0x1p-999999", 0.0),
            ("-0x0p1048575", -0.0),
            ("0x1p-1048575", 0.0),
            ("0x0000.0001P+16", 1.0),
            ("0b0", 0.0),
            ("+0b101", 5.0),
            ("-0B1111", -15.0),
            ("0_b10_10", 10.0),
            (
                "0b1111111111111111111111111111111111111111111111111111111111111111",
                18_446_744_073_709_551_616.0,
            ),
        ] {
            let actual = parse_number(src).unwrap();
            assert_eq!(actual.to_bits(), expected.to_bits(), "{src}");
        }
    }

    #[test]
    fn parses_nonfinite_numbers() {
        for src in [
            "inf",
            "INF",
            "infinity",
            "InFiNiTy",
            "1e400",
            "0x1p1024",
            "0x1.fffffffffffff8p1023",
        ] {
            for (sign, expected) in [("", f64::INFINITY), ("+", f64::INFINITY), ("-", f64::NEG_INFINITY)] {
                assert_eq!(parse_number(&format!("{sign}{src}")), Some(expected), "{sign}{src}");
            }
        }

        for src in ["nan", "NaN", "+NAN", "-nan", " \tnan\n"] {
            assert!(parse_number(src).unwrap().is_nan(), "{src}");
        }
    }

    #[test]
    fn limits_numbers() {
        assert_eq!(parse_number(&format!("0b{}1", "0".repeat(100))), Some(1.0));
        assert_eq!(parse_number(&format!("0b1{}", "0".repeat(64))), None);

        for prefix in ["", "0x"] {
            assert_eq!(
                parse_number(&format!("{prefix}0.{}1", "0".repeat((1 << 20) - 2))),
                Some(0.0)
            );
            assert_eq!(parse_number(&format!("{prefix}0.{}1", "0".repeat((1 << 20) - 1))), None);
            assert_eq!(
                parse_number(&format!("{prefix}0.1{}", "0".repeat(1 << 20))),
                Some(if prefix.is_empty() { 0.1 } else { 0.0625 })
            );
        }
    }

    #[test]
    fn rounds_numbers() {
        for (src, expected) in [
            (format!("0b1{}1", "0".repeat(52)), 9_007_199_254_740_992.0f64),
            (format!("0b1{}11", "0".repeat(51)), 9_007_199_254_740_996.0f64),
        ] {
            assert_eq!(parse_number(&src).unwrap().to_bits(), expected.to_bits(), "{src}");
        }

        assert_eq!(
            parse_number("0x1.00000000000008p0").unwrap().to_bits(),
            1.0f64.to_bits()
        );
        assert_eq!(
            parse_number("0x1.00000000000018p0").unwrap().to_bits(),
            (1.0 + 2.0 * f64::EPSILON).to_bits()
        );
        assert_eq!(
            parse_number("0x1.00000000000008000000001p0").unwrap().to_bits(),
            (1.0 + f64::EPSILON).to_bits()
        );
        assert_eq!(
            parse_number("0x1.fffffffffffff8p0").unwrap().to_bits(),
            2.0f64.to_bits()
        );
        assert_eq!(parse_number("0x1p-1074").unwrap().to_bits(), 1);
        assert_eq!(parse_number("0x0.8p-1074").unwrap().to_bits(), 0);
        assert_eq!(parse_number("0x0.8000000000000001p-1074").unwrap().to_bits(), 1);
    }

    #[test]
    fn rejects_numbers() {
        for src in [
            "",
            "\u{a0}1",
            "1\u{a0}",
            "- 1",
            "+",
            "-",
            ".",
            "++1",
            "+-1",
            "_1",
            "+_1",
            "._5",
            "i_nf",
            "n_an",
            "infinite",
            "nan(1)",
            "inf1",
            "1e",
            "1e+",
            "1e1048576",
            "1e-1048576",
            "0x",
            "0x.",
            "0x1p",
            "0x1p+",
            "0x1p1048576",
            "0x1p-1048576",
            "0x1p999999999999999999999999",
            "0x1p+-1",
            "0x1p--1",
            "0x1p1p1",
            "0x1.2.3",
            "0x.p1",
            "0x1e+2",
            "0xg",
            "0b",
            "0b2",
            "0b1.0",
            "0b1p2",
            "1LL",
            "1ULL",
            "1i",
        ] {
            assert_eq!(parse_number(src), None, "{src}");
        }
    }

    #[test]
    fn parses_strings() {
        for (src, expected) in [
            (r#""""#, ""),
            (r#"'hello'"#, "hello"),
            (r#"  "hello"  "#, "hello"),
            ("[[raw\\nline]]", r"raw\nline"),
            ("[=[nested [[br]]]=]", "nested [[br]]"),
            (r#""a\nb\tc""#, "a\nb\tc"),
            (r#""\xc3\xa9""#, "\u{e9}"),
            ("-- comment\n \"ok\" -- trailing\n", "ok"),
        ] {
            assert_eq!(parse_string(src).unwrap().as_deref(), Some(expected), "{src}");
        }

        assert_eq!(
            parse_byte_string(r#""\xff\x00""#).unwrap().as_deref(),
            Some(&b"\xff\x00"[..])
        );
    }

    #[test]
    fn rejects_strings() {
        for src in [
            "",
            "   ",
            "-- comment",
            "123",
            "nil",
            "ident",
            r#""a" .. "b""#,
            r#"123 "unterminated"#,
        ] {
            assert_eq!(parse_string(src).unwrap(), None, "{src}");
            assert_eq!(parse_byte_string(src).unwrap(), None, "{src}");
        }

        for src in [r#""unterminated"#, "[[unterminated", r#""ok" "unterminated"#] {
            assert!(parse_string(src).is_err(), "{src}");
            assert!(parse_byte_string(src).is_err(), "{src}");
        }
    }

    #[test]
    fn rejects_invalid_utf8() {
        for src in [r#""\xff""#, "-- comment\n  \"\\xff\"", r#""\xc3\x28""#] {
            let error = parse_string(src).unwrap_err();

            assert_eq!(error.offset, src.find('"').unwrap(), "{src}");
            assert_eq!(error.message, "string is not valid UTF-8", "{src}");
            assert!(parse_byte_string(src).unwrap().is_some(), "{src}");
        }
    }

    #[test]
    fn parses_tables() {
        assert_eq!(parse_table("{}").unwrap(), Some(Vec::new()));
        assert_eq!(
            parse_table(" { 1, 2; { col = 0xffffff }, } ").unwrap(),
            Some(vec![
                Field { key: None, val: "1" },
                Field { key: None, val: "2" },
                Field {
                    key: None,
                    val: "{ col = 0xffffff }",
                },
            ])
        );
        assert_eq!(
            parse_table("{ pos = 0, [\"col\"] = 0xffffff; [1] = a, tail }").unwrap(),
            Some(vec![
                Field {
                    key: Some(Key::Name("pos")),
                    val: "0",
                },
                Field {
                    key: Some(Key::Expr("\"col\"")),
                    val: "0xffffff",
                },
                Field {
                    key: Some(Key::Expr("1")),
                    val: "a",
                },
                Field { key: None, val: "tail" },
            ])
        );

        for src in ["", "   ", "123", "nil", "{} + {}", "{}.x", "{} extra"] {
            assert_eq!(parse_table(src).unwrap(), None, "{src}");
        }

        for src in [
            "{",
            "{ , }",
            "{ a = }",
            "{ [1] = }",
            "{ 1 2 }",
            "{ \"unterminated\"\" }",
        ] {
            assert!(parse_table(src).is_err(), "{src}");
        }
    }

    #[test]
    fn isolate_fragments() {
        let src = "local value = 1; call(value)";
        let tokens = Lexer::new(src).collect::<Result<Vec<_>, _>>().unwrap();
        let mut parser = Parser::new(src, &tokens);
        assert!(parser.parse_expr_at(0).is_err());
        let span = parser.parse_expr_at(src.find("call").unwrap()).unwrap();
        assert_eq!(&src[span.st..span.ed], "call(value)");
        parser.parse_stmt_at(0).unwrap();
        let analysis = parser.parse().unwrap();
        assert_eq!(analysis.assigns.len(), 1);
        assert_eq!(analysis.calls.len(), 1);
    }

    #[test]
    fn parse_expr() {
        for value in [
            "first + second",
            "(first + second)",
            "{first = 1, second = {2, 3}}",
            "get_values()",
            "function() return 1, 2 end",
            "flag ? first : second",
        ] {
            let src = format!("-- annotation\n{value}, retained");
            let tokens = Lexer::new(&src).collect::<Result<Vec<_>, _>>().unwrap();
            let span = Parser::new(&src, &tokens).parse_expr_at(0).unwrap();
            assert_eq!(&src[span.st..span.ed], value);
        }

        for src in ["", "1 +", "{1 2}", "{key =}", "{[key] 1}", "(1", "local a = 1"] {
            let tokens = Lexer::new(src).collect::<Result<Vec<_>, _>>().unwrap();
            assert!(Parser::new(&src, &tokens).parse_expr_at(0).is_err(), "{src}");
        }
    }

    #[test]
    fn preserve_ctx() {
        let src = "flag ? function() return object:method() end : other:method(), flag ? first + second : fallback";
        let tokens = Lexer::new(src).collect::<Result<Vec<_>, _>>().unwrap();
        let mut parser = Parser::new(&src, &tokens);

        for value in ["object:method()", "other:method()", "first + second", "other:method()"] {
            let span = parser.parse_expr_at(src.find(value).unwrap()).unwrap();
            assert_eq!(&src[span.st..span.ed], value);
        }
    }

    #[test]
    fn parse_stmt() {
        for statement in [
            "local aaa, bbb, ccc",
            "local value = function() return {1, 2} end",
            "if flag then foo() elseif other then bar() else baz() end",
            "for i = 1, 10 do while flag do foo() end end",
            "for key, value in pairs(values) do foo(key, value) end",
            "repeat foo() until not flag or check()",
            "do local value = 1; foo(value) end",
            "function foo(value) return value end",
            "foo({1, 2}, 'end')",
        ] {
            let src = format!("-- annotation\n{statement}\nlocal retained = 1");
            let tokens = Lexer::new(&src).collect::<Result<Vec<_>, _>>().unwrap();
            let span = Parser::new(&src, &tokens).parse_stmt_at(0).unwrap();
            assert_eq!(&src[span.st..span.ed], statement);
        }

        for src in [
            "",
            "-- annotation",
            "if flag then foo()",
            "while flag do foo()",
            "local value =",
            "local value = 1 +",
            "local value = {1",
            "foo(1",
        ] {
            let tokens = Lexer::new(src).collect::<Result<Vec<_>, _>>().unwrap();
            assert!(Parser::new(&src, &tokens).parse_stmt_at(0).is_err(), "{src}");
        }
    }

    #[test]
    fn resolve_references() {
        let src = "local api = object.operation; do local api = other end; \
            function helper(api) api('local') end; api('name')";
        let tokens = Lexer::new(src).collect::<Result<Vec<_>, _>>().unwrap();
        let output = Parser::new(&src, &tokens).parse().unwrap();
        assert_eq!(output.calls.len(), 2);
        assert!(output.calls[0].reference.is_none());
        assert_eq!(
            output.calls[1].reference.as_deref(),
            Some(["object", "operation"].as_slice())
        );
        let arg = output.calls[1].args[0];
        assert_eq!(&src[arg.st..arg.ed], "'name'");
    }

    #[test]
    fn parse_extensions() {
        for value in [
            "-2 ^ 3 ^ 4",
            "'a' .. 'b' .. 'c'",
            "1 << 1 | ~2 & 3 ~ 4 >> 1 ~>> 2",
            "!true && false || nil",
            "value ?? fallback",
            "flag ? first : second",
            "flag ? value -> value : nil",
            "flag ? |value| -> value : nil",
            "flag ? || -> value : nil",
            "flag ? value -> (value:operation()) : nil",
            "flag ? value -> do return value:operation() end : nil",
            "object?.operation",
            "name -> name",
            "|first, second| -> first + second",
            "|| -> 1",
            "|value| -> do local x = value; return x end",
            "function(object) return object.operation() end",
            "{ first = 1, [key] = 2, 3 }",
        ] {
            let src = format!("local result = {value}; object.operation('name')");
            let tokens = Lexer::new(&src).collect::<Result<Vec<_>, _>>().unwrap();
            let output = Parser::new(&src, &tokens).parse().unwrap();
            let assign = output.assigns.last().unwrap();
            assert_eq!(assign.targets.len(), 1, "{value}");
            assert_eq!(assign.vals.len(), 1, "{value}");
            let span = assign.vals[0];
            assert_eq!(&src[span.st..span.ed], value);
            assert_eq!(
                output.calls.last().unwrap().reference.as_deref(),
                Some(["object", "operation"].as_slice()),
                "{value}"
            );
        }
    }

    #[test]
    fn lex_numbers() {
        for src in [
            "0",
            "01",
            ".5",
            "1.",
            "1e999",
            "1_e_+_2",
            "0_x_1e",
            "0x1.fp3",
            "0_x_f.f_p_+_4",
            "0b101",
            "0_b_101",
            "1i",
            "0x1i",
            "0b1i",
            "1.5e2i",
            "1LL",
            "1uLl",
            "1LlU",
            "18446744073709551615ULL",
            "0xffffffffffffffffLL",
        ] {
            let token = Lexer::new(src).next().unwrap().unwrap();
            assert_eq!(token.kind, TokenKind::Number(src), "{src}");
        }

        for src in [
            "1e",
            "1e+",
            "0x",
            "0b",
            "0b2",
            "0b1.0",
            "0x1p",
            "1.2.3",
            "1..x",
            "1foo",
            "1u",
            "1l",
            "1f",
            "1.0LL",
            "1e2LL",
            "18446744073709551616ULL",
            "0x10000000000000000LL",
            "1e1048576",
        ] {
            let error = Lexer::new(src).next().unwrap().unwrap_err();
            assert_eq!(error.offset, 0, "{src}");
            assert_eq!(error.message, "malformed number", "{src}");
        }

        let tokens = Lexer::new("0_x1e+2 0_x1p-2").collect::<Result<Vec<_>, _>>().unwrap();
        assert_eq!(tokens[0].kind, TokenKind::Number("0_x1e"));
        assert_eq!(tokens[1].kind, TokenKind::Other('+'));
        assert_eq!(tokens[2].kind, TokenKind::Number("2"));
        assert_eq!(tokens[4].kind, TokenKind::Number("0_x1p-2"));
    }

    #[test]
    fn reject_depth() {
        for src in [
            format!("local value = {}1{}", "(".repeat(500), ")".repeat(500)),
            format!("local value = {}1", "not ".repeat(500)),
            format!("local value = {}1", "1 ^ ".repeat(500)),
            format!("local value = {}1{}", "{".repeat(500), "}".repeat(500)),
            format!("{}local value = 1{}", "do ".repeat(500), " end".repeat(500)),
            format!("local value = {}1", "flag ? nil : ".repeat(500)),
        ] {
            let tokens = Lexer::new(&src).collect::<Result<Vec<_>, _>>().unwrap();
            let error = Parser::new(&src, &tokens).parse().err().unwrap();
            assert!(error.offset < src.len());
            assert_eq!(error.message, "chunk has too many syntax levels");
        }

        let src = format!("local value = {}1{}", "(".repeat(20), ")".repeat(20));
        let tokens = Lexer::new(&src).collect::<Result<Vec<_>, _>>().unwrap();
        assert!(Parser::new(&src, &tokens).parse().is_ok());
    }

    #[test]
    fn validate_ident() {
        assert!(is_ident("abc").unwrap());
        assert!(is_ident("_").unwrap());
        assert!(is_ident("_foo123").unwrap());
        assert!(!is_ident("").unwrap());
        assert!(!is_ident("123").unwrap());
        assert!(!is_ident("local").unwrap());
        assert!(is_ident("goto").unwrap());
        assert!(is_ident("const").unwrap());
        assert!(is_ident("continue").unwrap());
        assert!(!is_ident("a.b").unwrap());
        assert!(!is_ident("a b").unwrap());
    }
    #[test]
    fn lex_exponents() -> anyhow::Result<()> {
        let src = "0x1e+2 0X1E-2 1e+2 0x1p-2 1 .. x ~>>";
        let tokens = Lexer::new(src).collect::<Result<Vec<_>, _>>()?;
        let numbers = tokens
            .iter()
            .filter_map(|token| match &token.kind {
                TokenKind::Number(number) => Some(*number),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(numbers, ["0x1e", "2", "0X1E", "2", "1e+2", "0x1p-2", "1"]);
        assert!(tokens.iter().any(|token| token.kind == TokenKind::Concat));
        assert_eq!(tokens.last().unwrap().kind, TokenKind::Operator("~>>"));
        Ok(())
    }

    #[test]
    fn reject_concatenation() {
        for src in ["1..x", "1..2", "1...x", "0x1..x"] {
            let mut lexer = Lexer::new(src);
            let err = lexer.next().unwrap().unwrap_err();
            assert_eq!(err.offset, 0);
            assert_eq!(err.message, "malformed number");
            assert!(lexer.next().is_none());
        }
    }

    #[test]
    fn preserve_delimiters() -> anyhow::Result<()> {
        for (src, content, level) in [
            ("-- aaaaa", " aaaaa", None),
            ("--[[aaaaa]]", "aaaaa", Some(0)),
            ("--[=[\naaaaa]=]", "\naaaaa", Some(1)),
            ("--[==[aaaaa ]=] b]==]", "aaaaa ]=] b", Some(2)),
            ("--[=not a block", "[=not a block", None),
        ] {
            let token = Lexer::new(src).next().unwrap()?;
            let TokenKind::Comment(comment) = token.kind else {
                panic!("comment expected: {src}");
            };
            assert_eq!(&src[token.span.st..token.span.ed], src);
            assert_eq!(comment.content, content);
            assert_eq!(comment.bracket.map(|bracket| bracket.level), level);
            if let Some(bracket) = comment.bracket {
                assert_eq!(&src[bracket.span.st..bracket.span.ed], content);
            }
        }

        for (src, value, level) in [
            ("\"aaaaa\"", "aaaaa", None),
            ("[[aaaaa]]", "aaaaa", Some(0)),
            ("[=[\naaaaa]=]", "aaaaa", Some(1)),
            ("[==[aaaaa ]=] b]==]", "aaaaa ]=] b", Some(2)),
        ] {
            let token = Lexer::new(src).next().unwrap()?;
            let TokenKind::String(literal) = token.kind else {
                panic!("string expected: {src}");
            };
            assert_eq!(literal.val.as_ref(), value.as_bytes());
            assert_eq!(literal.bracket.map(|bracket| bracket.level), level);
            if let Some(bracket) = literal.bracket {
                assert_eq!(src[bracket.span.st..bracket.span.ed].trim_start_matches('\n'), value);
            }
        }

        Ok(())
    }

    #[test]
    fn parse_calls() -> anyhow::Result<()> {
        let src = "local value = 1; obj.require('ignored'); require('first'); require [[second]]";
        let tokens = Lexer::new(src).collect::<Result<Vec<_>, _>>()?;
        let analysis = Parser::new(&src, &tokens).parse()?;
        let names = analysis
            .calls
            .iter()
            .filter(|call| call.reference.as_deref() == Some(&["require"][..]))
            .map(|call| {
                assert!(src[call.span.st..call.span.ed].starts_with("require"));
                let arg = call.args[0];
                let token = Lexer::new(&src[arg.st..arg.ed]).next().unwrap().unwrap();
                let TokenKind::String(name) = token.kind else {
                    panic!("string expected")
                };
                name.val.into_owned()
            })
            .collect::<Vec<_>>();

        assert_eq!(names, [b"first".to_vec(), b"second".to_vec()]);
        Ok(())
    }

    #[test]
    fn preserve_spans() {
        let src = "local 日本語 = '値'\n-- コメント\nreturn 日本語";
        let tokens = Lexer::new(src).collect::<Result<Vec<_>, _>>().unwrap();
        let mut ed = 0;

        for token in &tokens {
            assert_eq!(token.span.st, ed);
            assert!(src.is_char_boundary(token.span.ed));
            ed = token.span.ed;
        }
        assert_eq!(ed, src.len());
    }

    #[test]
    fn decode_strings() {
        for (src, expected) in [
            (r#""\a\b\f\n\r\t\v\\\"\'""#, vec![7, 8, 12, 10, 13, 9, 11, 92, 34, 39]),
            (r#""\065\x42\u{43}""#, b"ABC".to_vec()),
            ("\"a\\z \n \tb\"", b"ab".to_vec()),
        ] {
            let token = Lexer::new(src).next().unwrap().unwrap();
            let TokenKind::String(literal) = token.kind else {
                panic!("{src}");
            };
            assert_eq!(literal.val.as_ref(), expected.as_slice(), "{src}");
        }

        for src in [r#""\256""#, r#""\x0""#, r#""\u{110000}""#, r#""\q""#] {
            assert!(Lexer::new(src).next().unwrap().is_err(), "{src}");
        }
    }

    #[test]
    fn resolve_scopes() {
        for src in [
            "local api = object.operation; do local api = other end; api('name')",
            "local api = object.operation; for api = 1, 2 do api() end; api('name')",
            "local api = object.operation; for api in pairs(items) do api() end; api('name')",
            "local api = object.operation; function helper(api) api() end; api('name')",
            "local api = object.operation; repeat local api = other until api; api('name')",
            "local api = object.operation; local function helper() api('name') end",
        ] {
            let tokens = Lexer::new(src).collect::<Result<Vec<_>, _>>().unwrap();
            let output = Parser::new(&src, &tokens).parse().unwrap();
            assert_eq!(
                output.calls.last().unwrap().reference.as_deref(),
                Some(["object", "operation"].as_slice()),
                "{src}"
            );
        }

        for src in [
            "local api = object.operation; api = other; api('name')",
            "local api = object.operation; local function api() end; api('name')",
            "local api = object.operation; function api() end; api('name')",
        ] {
            let tokens = Lexer::new(src).collect::<Result<Vec<_>, _>>().unwrap();
            let output = Parser::new(&src, &tokens).parse().unwrap();
            assert_ne!(
                output.calls.last().unwrap().reference.as_deref(),
                Some(["object", "operation"].as_slice()),
                "{src}"
            );
        }
    }

    #[test]
    fn preserve_assignments() {
        let src = "local first, second = 1, get_values(); table.value = 2; table[key] = 3";
        let tokens = Lexer::new(src).collect::<Result<Vec<_>, _>>().unwrap();
        let output = Parser::new(&src, &tokens).parse().unwrap();
        assert_eq!(output.assigns.len(), 3);
        let first = &output.assigns[0];
        assert_eq!(
            first
                .targets
                .iter()
                .map(|span| &src[span.st..span.ed])
                .collect::<Vec<_>>(),
            ["first", "second"]
        );
        assert_eq!(
            first.vals.iter().map(|span| &src[span.st..span.ed]).collect::<Vec<_>>(),
            ["1", "get_values()"]
        );
        assert_eq!(
            &src[output.assigns[1].targets[0].st..output.assigns[1].targets[0].ed],
            "table.value"
        );
        assert_eq!(
            &src[output.assigns[2].targets[0].st..output.assigns[2].targets[0].ed],
            "table[key]"
        );
    }

    #[test]
    fn reject_statements() {
        for src in [
            "local",
            "local a =",
            "if true",
            "for i = 1 do end",
            "for i = , 2 do end",
            "for i = 1, do end",
            "for i = 1, 2, do end",
            "for i, j = 1, 2 do end",
            "for i = 1, 2, 3, 4 do end",
            "for in values do end",
            "for i, in values do end",
            "for i in",
            "for i in do end",
            "for i in values, do end",
            "function f(",
            "function () end",
            "local function () end",
            "function f.() end",
            "function f(a b) end",
            "function f(a,) end",
            "function f(1) end",
            "function f(..., a) end",
            "repeat f()",
            "repeat f() until",
            "while true f() end",
            "while do end",
            "if then end",
            "if true then elseif then end",
            "a =",
            "end",
            "else",
            "elseif true then",
            "until true",
        ] {
            let tokens = Lexer::new(src).collect::<Result<Vec<_>, _>>().unwrap();
            assert!(Parser::new(&src, &tokens).parse().is_err(), "{src}");
        }
    }

    #[test]
    fn parse_for() {
        for src in [
            "for i = 1, 10 do end",
            "for i = 1, 10, 2 do end",
            "for i = first(), last(), step() do end",
            "for item in values() do end",
            "for key, item in pairs(values) do end",
        ] {
            let tokens = Lexer::new(src).collect::<Result<Vec<_>, _>>().unwrap();
            assert!(Parser::new(&src, &tokens).parse().is_ok(), "{src}");
        }
    }

    #[test]
    fn parse_params() {
        for src in [
            "function f() end",
            "function f(a, b) end",
            "function f(...) end",
            "function f(a, ...) end",
            "local f = function(a, ...) return a end",
            "local f = |a, b| -> a + b",
        ] {
            let tokens = Lexer::new(src).collect::<Result<Vec<_>, _>>().unwrap();
            assert!(Parser::new(&src, &tokens).parse().is_ok(), "{src}");
        }
    }

    #[test]
    fn preserve_long_newlines() {
        for (src, expected) in [
            ("[=[\nfirst\nsecond]=]", "first\nsecond"),
            ("[=[\n\nfirst]=]", "\nfirst"),
            ("[[first\nsecond]]", "first\nsecond"),
            ("[[\n]]", ""),
            ("[[]]", ""),
        ] {
            let TokenKind::String(literal) = Lexer::new(src).next().unwrap().unwrap().kind else {
                panic!("{src}");
            };
            assert_eq!(literal.val.as_ref(), expected.as_bytes(), "{src}");
            assert!(matches!(literal.val, Cow::Borrowed(_)), "{src}");
        }
    }

    #[test]
    fn decode_escaped_newlines() {
        let src = "\"a\\\nb\"";
        let TokenKind::String(literal) = Lexer::new(src).next().unwrap().unwrap().kind else {
            panic!("{src}");
        };
        assert_eq!(literal.val.as_ref(), b"a\nb");
    }

    #[test]
    fn skip_escaped_whitespace() {
        let src = "\"a\\z \n \tb\"";
        let TokenKind::String(literal) = Lexer::new(src).next().unwrap().unwrap().kind else {
            panic!("{src}");
        };
        assert_eq!(literal.val.as_ref(), b"ab");
    }

    #[test]
    fn parse_syntax() {
        for (name, src) in [
            ("empty", " -- comment\n ; ; "),
            (
                "soft names",
                "local const, continue, goto = 1, 2, 3; const = continue + goto",
            ),
            ("soft members", "local t = {const=1}; t.continue = t.const; t:goto()"),
            ("continue", "while true do continue; end"),
            ("trivia", "local --[[name]] value --[[assign]] = --[[value]] 1 ;"),
            ("navigation", "local x = t?.[key]?.(1); local y = t?.:method?.(x)"),
            ("labels", "::again:: goto again"),
            ("group target", "(object).field = 1; (object)[1] += 2"),
            ("calls", "(factory())(); factory().field()"),
            (
                "short functions",
                "local f = |const, continue| -> do return const + continue; end",
            ),
            ("operators", "local x = a ?? b || c && d; x ~>>= 1; x ..= 'x'"),
        ] {
            let tokens = Lexer::new(src).collect::<Result<Vec<_>, _>>().unwrap();
            if let Err(err) = Parser::new(&src, &tokens).parse() {
                panic!("{name}: {src:?}: {err} at {}", err.offset);
            }
        }

        for (name, src) in [
            ("label name", "::::"),
            ("label close", "::again"),
            ("bare name", "value"),
            ("bare literal", "1"),
            ("literal target", "1 = 2"),
            ("call target", "f() = 2"),
            ("group target", "(value) = 2"),
            ("navigation target", "t?.field.other = 2"),
            ("method arguments", "local x = t:method"),
            ("navigation suffix", "local x = t?."),
            ("short arrow", "local f = |x| x"),
            ("method declaration", "function t:method.field() end"),
            ("value list", "local x = 1,"),
        ] {
            let tokens = Lexer::new(src).collect::<Result<Vec<_>, _>>().unwrap();
            let err = Parser::new(&src, &tokens)
                .parse()
                .err()
                .unwrap_or_else(|| panic!("{name}: {src:?}"));
            assert!(err.offset <= src.len(), "{name}: {err} at {}", err.offset);
        }
    }

    #[test]
    fn parse_fragments() {
        let src = "-- first\nlocal value = --[[inline]] first + second; -- tail\ncall(value);";
        let tokens = Lexer::new(src).collect::<Result<Vec<_>, _>>().unwrap();
        let mut parser = Parser::new(&src, &tokens);
        for (st, expected) in [
            (src.find("first +").unwrap(), "first + second"),
            (src.find("call").unwrap(), "call(value)"),
        ] {
            let span = parser.parse_expr_at(st).unwrap();
            assert_eq!(&src[span.st..span.ed], expected, "expression at {st}");
        }
        let span = parser.parse_stmt_at(0).unwrap();
        assert_eq!(
            &src[span.st..span.ed],
            "local value = --[[inline]] first + second;",
            "statement"
        );
        assert!(
            Parser::new(&src, &tokens).parse_expr_at(src.len()).is_err(),
            "expression at end"
        );
        assert!(
            Parser::new(&src, &tokens).parse_stmt_at(src.len()).is_err(),
            "statement at end"
        );
    }
}
