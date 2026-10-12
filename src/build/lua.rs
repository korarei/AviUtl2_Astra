use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::fmt;

const MAX_DEPTH: usize = 200;

pub struct Parser<'a, 'tokens> {
    tokens: Vec<&'tokens Token<'a>>,
    curr: usize,
    depth: usize,
    error: Option<Error>,
    stmt_st: Option<usize>,
    context_curr: usize,
    context: Vec<(Option<&'tokens TokenKind<'a>>, usize)>,
    scopes: Vec<HashMap<&'a str, usize>>,
    bindings: Vec<Binding<'a>>,
    functions: Vec<usize>,
    writes: Vec<usize>,
    output: Analysis<'a>,
}

impl<'a, 'tokens> Parser<'a, 'tokens> {
    #[must_use]
    pub fn new(tokens: impl IntoIterator<Item = &'tokens Token<'a>>) -> Self {
        Self {
            tokens: tokens
                .into_iter()
                .filter(|token| {
                    !matches!(
                        token.kind,
                        TokenKind::Whitespace | TokenKind::Newline | TokenKind::Comment(_)
                    )
                })
                .collect(),
            curr: 0,
            depth: 0,
            error: None,
            stmt_st: None,
            context_curr: 0,
            context: Vec::new(),
            scopes: vec![HashMap::new()],
            bindings: Vec::new(),
            functions: Vec::new(),
            writes: Vec::new(),
            output: Analysis::default(),
        }
    }

    pub fn parse(mut self) -> Result<Analysis<'a>, Error> {
        self.block();

        if let Some(error) = self.error {
            return Err(error);
        }

        if let Some(token) = self.tokens.get(self.curr) {
            return Err(Error {
                offset: token.span.st,
                message: "unexpected block delimiter",
            });
        }

        Ok(self.output)
    }

    pub(super) fn parse_expr(&mut self, st: usize) -> Result<Span, Error> {
        self.curr = self.tokens.partition_point(|token| token.span.st < st);

        if self.curr < self.context_curr {
            self.context.clear();
            self.context_curr = 0;
        }

        if self.context.is_empty() {
            self.context.push((None, 0));
        }

        while self.context_curr < self.curr {
            let (kind, conditionals) = *self.context.last().unwrap();
            let token = self.tokens[self.context_curr];
            match &token.kind {
                TokenKind::LParen
                | TokenKind::LBracket
                | TokenKind::LBrace
                | TokenKind::Keyword("function" | "if" | "while" | "for" | "repeat") => {
                    self.context.push((Some(&token.kind), 0));
                }
                TokenKind::Keyword("do") if matches!(kind, Some(TokenKind::Keyword("while" | "for"))) => {
                    self.context.last_mut().unwrap().0 = Some(&token.kind);
                }
                TokenKind::Keyword("do") => self.context.push((Some(&token.kind), 0)),
                TokenKind::RParen | TokenKind::RBracket | TokenKind::RBrace | TokenKind::Keyword("end" | "until")
                    if self.context.len() > 1 =>
                {
                    self.context.pop();
                }
                TokenKind::Operator("?") => self.context.last_mut().unwrap().1 += 1,
                TokenKind::Colon if conditionals != 0 => self.context.last_mut().unwrap().1 -= 1,
                _ => {}
            }

            self.context_curr += 1;
        }

        let val = self.expr(0, self.context.last().unwrap().1 != 0);
        if let Some(error) = self.error {
            return Err(error);
        }

        val.map(|val| val.span).ok_or(Error {
            offset: self.tokens.get(self.curr).map_or(st, |token| token.span.st),
            message: "missing expression",
        })
    }

    pub(super) fn parse_stmt(&mut self, st: usize) -> Result<Span, Error> {
        self.curr = self.tokens.partition_point(|token| token.span.st < st);
        let st = self.curr;
        let token = self.tokens.get(st).ok_or(Error {
            offset: self.tokens.last().map_or(0, |token| token.span.ed),
            message: "missing statement",
        })?;
        let span_st = token.span.st;
        self.stmt();

        if let Some(error) = self.error {
            return Err(error);
        }

        if self.curr == st {
            return Err(Error {
                offset: span_st,
                message: "invalid statement",
            });
        }

        self.take(&TokenKind::Semicolon);
        Ok(Span {
            st: span_st,
            ed: self.tokens[self.curr - 1].span.ed,
        })
    }

    fn block(&mut self) {
        if self.depth + 1 >= MAX_DEPTH {
            self.error.get_or_insert(Error {
                offset: self.tokens.get(self.curr).map_or_else(
                    || self.tokens.last().map_or(0, |token| token.span.ed),
                    |token| token.span.st,
                ),
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
            self.stmt();

            if self.curr == st {
                self.error.get_or_insert(Error {
                    offset: self.tokens[st].span.st,
                    message: "invalid statement",
                });
            }
        }

        self.depth -= 1;
    }

    fn scoped_block(&mut self) {
        self.scopes.push(HashMap::new());
        self.block();
        self.scopes.pop().expect("scope must exist");
    }

    #[allow(clippy::too_many_lines)]
    fn stmt(&mut self) {
        let st = self.curr;
        let prev_stmt_st = self.stmt_st.replace(self.tokens[st].span.st);

        match self.kind().cloned() {
            Some(TokenKind::Local) => {
                self.curr += 1;
                if self.take(&TokenKind::Keyword("function")) {
                    if let Some(name) = self.require_name() {
                        self.declare(name, None);
                    }

                    self.function(false);
                } else {
                    self.assignment(st, true);
                }
            }
            Some(TokenKind::Keyword("const"))
                if self.tokens.get(self.curr + 1).is_some_and(|token| {
                    token.kind.name().is_some() || token.kind == TokenKind::Keyword("function")
                }) =>
            {
                self.curr += 1;
                if self.take(&TokenKind::Keyword("function")) {
                    if let Some(name) = self.require_name() {
                        self.declare(name, None);
                    }

                    self.function(false);
                } else {
                    self.assignment(st, true);
                }
            }
            Some(TokenKind::Keyword("function")) => {
                self.curr += 1;
                let mut is_method = false;

                if let Some(name) = self.require_name() {
                    let index = self.resolve(name);
                    self.assign(index, None);

                    while matches!(self.kind(), Some(TokenKind::Dot | TokenKind::Colon)) {
                        is_method |= self.kind() == Some(&TokenKind::Colon);
                        self.curr += 1;

                        if self.require_name().is_none() {
                            break;
                        }
                    }
                }

                self.function(is_method);
            }
            Some(TokenKind::Keyword("if")) => {
                self.curr += 1;
                self.require_expr(false);
                let writes_st = self.writes.len();
                let vals = self
                    .bindings
                    .iter()
                    .map(|binding| binding.val.clone())
                    .collect::<Vec<_>>();

                self.require(&TokenKind::Keyword("then"));
                self.scoped_block();

                while self.take(&TokenKind::Keyword("elseif")) {
                    self.restore(&vals);
                    self.require_expr(false);
                    self.require(&TokenKind::Keyword("then"));
                    self.scoped_block();
                }

                if self.take(&TokenKind::Keyword("else")) {
                    self.restore(&vals);
                    self.scoped_block();
                }

                self.require(&TokenKind::Keyword("end"));
                self.invalidate(&vals, writes_st);
            }
            Some(TokenKind::Keyword("do")) => {
                self.curr += 1;
                self.scoped_block();
                self.require(&TokenKind::Keyword("end"));
            }
            Some(TokenKind::Keyword("while" | "for" | "repeat")) => {
                let kind = self.kind().cloned().unwrap();
                self.curr += 1;
                let writes_st = self.writes.len();
                let calls_st = self.output.calls.len();
                let vals = self
                    .bindings
                    .iter()
                    .map(|binding| binding.val.clone())
                    .collect::<Vec<_>>();
                let mut names = Vec::new();

                if kind == TokenKind::Keyword("for") {
                    while let Some(name) = self.require_name() {
                        names.push(name);

                        if !self.take(&TokenKind::Comma) {
                            break;
                        }
                    }

                    if names.is_empty() || self.kind() == Some(&TokenKind::Assign) && names.len() != 1 {
                        self.error.get_or_insert(Error {
                            offset: self
                                .tokens
                                .get(self.curr)
                                .map_or(self.tokens[self.curr - 1].span.ed, |token| token.span.st),
                            message: "invalid for variable list",
                        });
                    }

                    if self.take(&TokenKind::Assign) {
                        self.require_expr(false);
                        self.require(&TokenKind::Comma);
                        self.require_expr(false);

                        if self.take(&TokenKind::Comma) {
                            self.require_expr(false);
                        }
                    } else {
                        self.require(&TokenKind::Keyword("in"));
                        if self.exprs().is_some_and(|vals| vals.is_empty()) {
                            self.error.get_or_insert(Error {
                                offset: self
                                    .tokens
                                    .get(self.curr)
                                    .map_or(self.tokens[self.curr - 1].span.ed, |token| token.span.st),
                                message: "missing expression after 'in'",
                            });
                        }
                    }
                } else if kind == TokenKind::Keyword("while") {
                    self.require_expr(false);
                }

                if kind != TokenKind::Keyword("repeat") {
                    self.require(&TokenKind::Keyword("do"));
                }

                self.scopes.push(HashMap::new());

                for name in names {
                    self.declare(name, None);
                }

                self.block();

                if kind == TokenKind::Keyword("repeat") {
                    self.require(&TokenKind::Keyword("until"));
                    self.require_expr(false);
                } else {
                    self.require(&TokenKind::Keyword("end"));
                }

                self.scopes.pop().expect("scope must exist");

                let changed = self.writes[writes_st..]
                    .iter()
                    .filter_map(|&index| vals.get(index).and_then(Option::as_deref))
                    .collect::<HashSet<_>>();

                for call in &mut self.output.calls[calls_st..] {
                    if call
                        .reference
                        .as_ref()
                        .is_some_and(|reference| (1..=reference.len()).any(|ed| changed.contains(&reference[..ed])))
                    {
                        call.reference = None;
                    }
                }

                self.invalidate(&vals, writes_st);
            }
            Some(TokenKind::Keyword("return")) => {
                self.curr += 1;
                let _ = self.exprs();
            }
            Some(TokenKind::Keyword("break")) => {
                self.curr += 1;
            }
            Some(TokenKind::Keyword("goto"))
                if self
                    .tokens
                    .get(self.curr + 1)
                    .is_some_and(|token| token.kind.name().is_some()) =>
            {
                self.curr += 2;
            }
            Some(TokenKind::Keyword("continue"))
                if self.tokens.get(self.curr + 1).is_none_or(|token| {
                    matches!(token.kind, TokenKind::Keyword("end" | "else" | "elseif" | "until"))
                }) =>
            {
                self.curr += 1;
            }
            Some(TokenKind::Operator("::")) => {
                self.curr += 1;

                if self.kind().and_then(TokenKind::name).is_some() {
                    self.curr += 1;
                }

                self.take(&TokenKind::Operator("::"));
            }
            Some(TokenKind::Semicolon) => self.curr += 1,
            _ => self.assignment(st, false),
        }

        self.stmt_st = prev_stmt_st;
    }

    #[allow(clippy::too_many_lines)]
    fn assignment(&mut self, st: usize, is_local: bool) {
        let mut targets = Vec::new();
        let mut names = Vec::new();
        let mut bindings = Vec::new();

        loop {
            let target_st = self.curr;
            let Some(name) = self.kind().and_then(TokenKind::name) else {
                if is_local {
                    self.error.get_or_insert(Error {
                        offset: self.tokens[self.curr - 1].span.ed,
                        message: "missing local name",
                    });
                } else {
                    let _ = self.expr(0, false);
                }

                return;
            };

            let expression = if is_local {
                self.curr += 1;
                Expression {
                    span: self.tokens[target_st].span,
                    reference: None,
                    binding: None,
                    captures: Vec::new(),
                }
            } else {
                let Some(expression) = self.expr(15, false) else {
                    return;
                };

                expression
            };

            names.push((self.curr == target_st + 1).then_some(name));
            bindings.push(expression.binding);
            targets.push(expression.span);

            if !self.take(&TokenKind::Comma) {
                break;
            }
        }

        let has_vals = self.take(&TokenKind::Assign);
        let is_compound =
            if !is_local && !has_vals && targets.len() == 1 {
                if self.take(&TokenKind::Ne) {
                    true
                } else if matches!(
                    self.kind(),
                    Some(
                        TokenKind::Other('+' | '-' | '*' | '/' | '%')
                            | TokenKind::Operator("&" | "|" | "<<" | ">>" | "~>>")
                            | TokenKind::Concat
                    )
                ) && self.tokens.get(self.curr + 1).is_some_and(|token| {
                    token.kind == TokenKind::Assign && self.tokens[self.curr].span.ed == token.span.st
                }) {
                    self.curr += 2;
                    true
                } else {
                    false
                }
            } else {
                false
            };

        if !is_local && !has_vals && !is_compound {
            return;
        }

        let vals = if has_vals || is_compound {
            let Some(vals) = self.exprs() else { return };
            if vals.is_empty() {
                self.error.get_or_insert(Error {
                    offset: self.tokens[self.curr - 1].span.ed,
                    message: "missing assignment value",
                });
                return;
            }

            vals
        } else {
            Vec::new()
        };

        let ed = self.tokens[self.curr - 1].span.ed;

        for (index, name) in names.iter().enumerate() {
            let val = if is_compound {
                None
            } else {
                vals.get(index).and_then(|val| val.reference.clone())
            };

            if is_local {
                if let Some(name) = name {
                    let binding = self.declare(name, val);
                    self.bindings[binding].captures = vals.get(index).map_or_else(Vec::new, |val| val.captures.clone());
                }
            } else if let Some(binding) = bindings[index] {
                self.assign(binding, if name.is_some() { val } else { None });
                if name.is_some() {
                    self.bindings[binding].captures = vals.get(index).map_or_else(Vec::new, |val| val.captures.clone());
                }
            }
        }

        if is_compound {
            return;
        }

        self.output.assigns.push(Assignment {
            span: Span {
                st: self.tokens[st].span.st,
                ed,
            },
            targets,
            vals: vals.into_iter().map(|val| val.span).collect(),
        });
    }

    fn exprs(&mut self) -> Option<Vec<Expression<'a>>> {
        let mut vals = Vec::new();
        let Some(val) = self.expr(0, false) else {
            return Some(vals);
        };

        vals.push(val);

        while self.take(&TokenKind::Comma) {
            let Some(val) = self.expr(0, false) else {
                self.error.get_or_insert(Error {
                    offset: self.tokens[self.curr - 1].span.ed,
                    message: "missing expression after comma",
                });
                return None;
            };
            vals.push(val);
        }

        Some(vals)
    }

    fn parameters(&mut self, ed: &TokenKind<'a>) -> Vec<&'a str> {
        let mut names = Vec::new();

        if self.take(ed) {
            return names;
        }

        loop {
            if self.take(&TokenKind::Dots) {
                break;
            }

            let Some(name) = self.require_name() else { return names };
            names.push(name);

            if !self.take(&TokenKind::Comma) {
                break;
            }
        }

        self.require(ed);
        names
    }

    fn function(&mut self, is_method: bool) {
        if !self.require(&TokenKind::LParen) {
            return;
        }

        let mut names = self.parameters(&TokenKind::RParen);
        if is_method {
            names.insert(0, "self");
        }

        self.function_body(names, false, false);
    }

    fn short_function(&mut self, name: Option<&'a str>, should_stop_colon: bool) {
        let names = if let Some(name) = name {
            vec![name]
        } else if self.take(&TokenKind::Operator("||")) {
            Vec::new()
        } else if self.take(&TokenKind::Operator("|")) {
            self.parameters(&TokenKind::Operator("|"))
        } else {
            return;
        };

        if self.take(&TokenKind::Operator("->")) {
            self.function_body(names, true, should_stop_colon);
        }
    }

    fn function_body(&mut self, names: Vec<&'a str>, is_short: bool, should_stop_colon: bool) {
        let prev_stmt_st = self.stmt_st.take();
        let vals = self
            .bindings
            .iter()
            .map(|binding| binding.val.clone())
            .collect::<Vec<_>>();
        let writes_st = self.writes.len();

        self.functions.push(self.scopes.len());
        self.scopes.push(HashMap::new());

        for name in names {
            self.declare(name, None);
        }

        if is_short && !self.take(&TokenKind::Keyword("do")) {
            self.require_expr(should_stop_colon);
        } else {
            self.block();
            self.require(&TokenKind::Keyword("end"));
        }

        self.scopes.pop().expect("scope must exist");
        self.functions.pop().expect("function scope must exist");
        self.invalidate(&vals, writes_st);
        self.stmt_st = prev_stmt_st;
    }

    #[allow(clippy::too_many_lines)]
    fn expr(&mut self, min: u8, should_stop_colon: bool) -> Option<Expression<'a>> {
        if self.error.is_some() {
            return None;
        }

        if self.depth + 1 >= MAX_DEPTH {
            self.error = Some(Error {
                offset: self.tokens.get(self.curr).map_or_else(
                    || self.tokens.last().map_or(0, |token| token.span.ed),
                    |token| token.span.st,
                ),
                message: "chunk has too many syntax levels",
            });
            return None;
        }

        self.depth += 1;
        let curr = self.curr;

        let val = (|| {
            let token = self.tokens.get(self.curr)?;
            let st = token.span.st;
            let mut reference = None;
            let mut binding = None;
            let mut captures = Vec::new();
            let mut is_prefix = false;

            match &token.kind {
                TokenKind::Ident(name) => {
                    let name = *name;
                    self.curr += 1;
                    if self.kind() == Some(&TokenKind::Operator("->")) {
                        self.short_function(Some(name), should_stop_colon);
                        return Some(Expression {
                            span: Span {
                                st,
                                ed: self.tokens[self.curr - 1].span.ed,
                            },
                            reference: None,
                            binding: None,
                            captures: Vec::new(),
                        });
                    }
                    is_prefix = true;
                    let index = self.resolve(name);
                    reference.clone_from(&self.bindings[index].val);
                    binding = Some(index);
                    captures.clone_from(&self.bindings[index].captures);

                    if self
                        .functions
                        .last()
                        .is_some_and(|&scope| self.bindings[index].scope < scope)
                    {
                        captures.push(index);
                    }
                }
                TokenKind::String(_)
                | TokenKind::Number(_)
                | TokenKind::Dots
                | TokenKind::Keyword("nil" | "true" | "false") => self.curr += 1,
                TokenKind::Other('-' | '#') | TokenKind::Operator("~" | "!") | TokenKind::Keyword("not") => {
                    self.curr += 1;
                    self.expr(13, false)?;
                }
                TokenKind::LParen => {
                    is_prefix = true;
                    self.curr += 1;
                    let val = self.expr(0, false)?;
                    reference = val.reference;
                    binding = val.binding;
                    captures = val.captures;
                    if !self.require(&TokenKind::RParen) {
                        return None;
                    }
                }
                TokenKind::LBrace => {
                    self.curr += 1;
                    while self.kind().is_some() && self.kind() != Some(&TokenKind::RBrace) {
                        if self.take(&TokenKind::LBracket) {
                            self.expr(0, false)?;
                            if !self.require(&TokenKind::RBracket) || !self.require(&TokenKind::Assign) {
                                return None;
                            }
                        } else if self.kind().and_then(TokenKind::name).is_some()
                            && self
                                .tokens
                                .get(self.curr + 1)
                                .is_some_and(|token| token.kind == TokenKind::Assign)
                        {
                            self.curr += 2;
                        }

                        self.expr(0, false)?;
                        if self.kind() == Some(&TokenKind::RBrace) {
                            break;
                        }

                        if !self.take(&TokenKind::Comma) && !self.take(&TokenKind::Semicolon) {
                            self.require(&TokenKind::Comma);
                            return None;
                        }
                    }

                    if !self.require(&TokenKind::RBrace) {
                        return None;
                    }
                }
                TokenKind::Keyword("function") => {
                    self.curr += 1;
                    self.function(false);
                }
                TokenKind::Operator("|" | "||") => self.short_function(None, should_stop_colon),
                _ => return None,
            }

            if is_prefix {
                loop {
                    match self.kind().cloned() {
                        Some(TokenKind::Dot) => {
                            self.curr += 1;
                            let name = self.kind().and_then(TokenKind::name)?;
                            self.curr += 1;
                            if let Some(reference) = &mut reference {
                                reference.push(name);
                            }
                        }
                        Some(TokenKind::LBracket) => {
                            self.curr += 1;
                            let key_st = self.curr;
                            self.expr(0, false)?;

                            if self.curr == key_st + 1
                                && let TokenKind::String(key) = &self.tokens[key_st].kind
                                && let Cow::Borrowed(key) = &key.val
                                && let Ok(key) = std::str::from_utf8(key)
                            {
                                if let Some(reference) = &mut reference {
                                    reference.push(key);
                                }
                            } else {
                                reference = None;
                            }

                            if !self.require(&TokenKind::RBracket) {
                                return None;
                            }
                        }
                        Some(TokenKind::Operator("?.")) => {
                            self.curr += 1;
                            if let Some(name) = self.kind().and_then(TokenKind::name) {
                                self.curr += 1;
                                if let Some(reference) = &mut reference {
                                    reference.push(name);
                                }
                            }
                        }
                        Some(TokenKind::Colon) if !should_stop_colon => {
                            self.curr += 1;
                            if matches!(self.kind(), Some(TokenKind::Ident(_))) {
                                self.curr += 1;
                            }
                            reference = None;
                        }
                        Some(TokenKind::LParen | TokenKind::LBrace | TokenKind::String(_)) => {
                            let args = if self.take(&TokenKind::LParen) {
                                let args = self.exprs()?;

                                if !self.require(&TokenKind::RParen) {
                                    return None;
                                }

                                args
                            } else {
                                vec![self.expr(15, false)?]
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
                                    ed: self.tokens[self.curr - 1].span.ed,
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
                let (priority, is_right) = match self.kind() {
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

                if priority < min {
                    break;
                }

                self.curr += 1;
                self.expr(priority + u8::from(!is_right), should_stop_colon)?;
                reference = None;
                binding = None;
                captures.clear();
            }

            if min == 0 && self.take(&TokenKind::Operator("?")) {
                self.expr(0, true)?;

                if !self.require(&TokenKind::Colon) {
                    return None;
                }

                self.expr(0, false)?;
                reference = None;
                binding = None;
                captures.clear();
            }

            Some(Expression {
                span: Span {
                    st,
                    ed: self.tokens[self.curr - 1].span.ed,
                },
                reference,
                binding,
                captures,
            })
        })();

        self.depth -= 1;
        if val.is_none() && self.curr != curr {
            self.error.get_or_insert(Error {
                offset: self.tokens.get(self.curr).map_or_else(
                    || self.tokens.last().map_or(0, |token| token.span.ed),
                    |token| token.span.st,
                ),
                message: "incomplete expression",
            });
        }
        val
    }

    fn declare(&mut self, name: &'a str, val: Reference<'a>) -> usize {
        let scope = self.scopes.len() - 1;
        let index = self.bindings.len();
        self.bindings.push(Binding {
            name,
            val,
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
            val: Some(vec![name]),
            scope: 0,
            captures: Vec::new(),
            calls: Vec::new(),
        });
        self.scopes[0].insert(name, index);
        index
    }

    fn assign(&mut self, index: usize, val: Reference<'a>) {
        if self.bindings[index].val != val {
            for call in std::mem::take(&mut self.bindings[index].calls) {
                self.output.calls[call].reference = None;
            }
        }

        self.bindings[index].val = val;
        self.bindings[index].captures.clear();
        self.writes.push(index);
    }

    fn restore(&mut self, vals: &[Reference<'a>]) {
        for (index, binding) in self.bindings.iter_mut().enumerate() {
            if binding.scope < self.scopes.len() {
                binding.val = vals.get(index).cloned().unwrap_or_else(|| Some(vec![binding.name]));
            }
        }
    }

    fn invalidate(&mut self, vals: &[Reference<'a>], writes_st: usize) {
        self.restore(vals);

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
            self.curr += 1;
            return Some(name);
        }

        self.error.get_or_insert(Error {
            offset: self.tokens.get(self.curr).map_or_else(
                || self.tokens.last().map_or(0, |token| token.span.ed),
                |token| token.span.st,
            ),
            message: "missing identifier",
        });
        None
    }

    fn require_expr(&mut self, should_stop_colon: bool) {
        if self.expr(0, should_stop_colon).is_none() {
            self.error.get_or_insert(Error {
                offset: self.tokens.get(self.curr).map_or_else(
                    || self.tokens.last().map_or(0, |token| token.span.ed),
                    |token| token.span.st,
                ),
                message: "missing expression",
            });
        }
    }

    fn require(&mut self, kind: &TokenKind<'a>) -> bool {
        if self.take(kind) {
            return true;
        }

        self.error.get_or_insert(Error {
            offset: self.tokens.get(self.curr).map_or_else(
                || self.tokens.last().map_or(0, |token| token.span.ed),
                |token| token.span.st,
            ),
            message: "missing syntax delimiter",
        });

        false
    }

    fn take(&mut self, kind: &TokenKind<'a>) -> bool {
        if self.kind() == Some(kind) {
            self.curr += 1;
            true
        } else {
            false
        }
    }

    fn kind(&self) -> Option<&TokenKind<'a>> {
        self.tokens.get(self.curr).map(|token| &token.kind)
    }
}

#[derive(Clone)]
pub struct Lexer<'a> {
    src: &'a str,
    bytes: &'a [u8],
    pub(super) curr: usize,
}

impl<'a> Lexer<'a> {
    #[must_use]
    pub fn new(src: &'a str) -> Self {
        Self {
            src,
            bytes: src.as_bytes(),
            curr: 0,
        }
    }

    #[allow(clippy::too_many_lines)]
    fn read(&mut self) -> Result<Token<'a>, Error> {
        let st = self.curr;
        let byte = self.bytes[self.curr];

        Ok(Token {
            kind: if matches!(byte, b' ' | b'\t' | 0x0b | 0x0c) {
                while self.curr < self.bytes.len() && matches!(self.bytes[self.curr], b' ' | b'\t' | 0x0b | 0x0c) {
                    self.curr += 1;
                }
                TokenKind::Whitespace
            } else if byte == b'\n' {
                self.curr += 1;
                TokenKind::Newline
            } else if self.src[st..].starts_with("--") {
                self.curr += 2;

                if let Some(bracket) = self.read_bracket().map_err(|_| Error {
                    offset: st,
                    message: "unfinished long comment",
                })? {
                    TokenKind::Comment(Comment {
                        content: &self.src[bracket.span.st..bracket.span.ed],
                        bracket: Some(bracket),
                    })
                } else {
                    self.curr += self.src[self.curr..].find('\n').unwrap_or(self.bytes.len() - self.curr);
                    TokenKind::Comment(Comment {
                        content: &self.src[st + 2..self.curr],
                        bracket: None,
                    })
                }
            } else if matches!(byte, b'"' | b'\'') {
                TokenKind::String(self.read_string()?)
            } else if byte == b'[' {
                if let Some(bracket) = self.read_bracket()? {
                    let st = bracket.span.st + usize::from(self.bytes.get(bracket.span.st) == Some(&b'\n'));
                    TokenKind::String(Literal {
                        val: Cow::Borrowed(&self.bytes[st..bracket.span.ed]),
                        bracket: Some(bracket),
                    })
                } else {
                    if self.bytes.get(st + 1) == Some(&b'=') {
                        return Err(Error {
                            offset: st,
                            message: "invalid long string delimiter",
                        });
                    }

                    self.curr += 1;
                    TokenKind::LBracket
                }
            } else if byte.is_ascii_alphabetic() || byte == b'_' || byte >= 0x80 {
                while self.curr < self.bytes.len()
                    && (self.bytes[self.curr].is_ascii_alphanumeric()
                        || self.bytes[self.curr] == b'_'
                        || self.bytes[self.curr] >= 0x80)
                {
                    self.curr += 1;
                }

                match &self.src[st..self.curr] {
                    "local" => TokenKind::Local,
                    word @ ("and" | "break" | "do" | "else" | "elseif" | "end" | "false" | "for" | "function"
                    | "if" | "const" | "continue" | "goto" | "in" | "nil" | "not" | "or" | "repeat"
                    | "return" | "then" | "true" | "until" | "while") => TokenKind::Keyword(word),
                    word => TokenKind::Ident(word),
                }
            } else if let Some(op) = ["~>>", "&&", "||", "!=", "?.", "??", "::", "->", "<<", ">>"]
                .into_iter()
                .find(|op| self.src[st..].starts_with(op))
            {
                self.curr += op.len();
                TokenKind::Operator(op)
            } else if byte.is_ascii_digit()
                || (byte == b'.' && self.bytes.get(self.curr + 1).is_some_and(u8::is_ascii_digit))
            {
                TokenKind::Number(self.read_number()?)
            } else {
                self.curr += 1;

                match byte {
                    b'@' => {
                        return Err(Error {
                            offset: st,
                            message: "unexpected '@'",
                        });
                    }
                    b'=' => {
                        if self.curr < self.bytes.len() && self.bytes[self.curr] == b'=' {
                            self.curr += 1;
                            TokenKind::Eq
                        } else {
                            TokenKind::Assign
                        }
                    }
                    b'~' if self.curr < self.bytes.len() && self.bytes[self.curr] == b'=' => {
                        self.curr += 1;
                        TokenKind::Ne
                    }
                    b'<' if self.curr < self.bytes.len() && self.bytes[self.curr] == b'=' => {
                        self.curr += 1;
                        TokenKind::Le
                    }
                    b'>' if self.curr < self.bytes.len() && self.bytes[self.curr] == b'=' => {
                        self.curr += 1;
                        TokenKind::Ge
                    }
                    b',' => TokenKind::Comma,
                    b'.' => {
                        if self.curr < self.bytes.len() && self.bytes[self.curr] == b'.' {
                            self.curr += 1;
                            if self.curr < self.bytes.len() && self.bytes[self.curr] == b'.' {
                                self.curr += 1;
                                TokenKind::Dots
                            } else {
                                TokenKind::Concat
                            }
                        } else {
                            TokenKind::Dot
                        }
                    }
                    b';' => TokenKind::Semicolon,
                    b':' => TokenKind::Colon,
                    b'~' => TokenKind::Operator("~"),
                    b'&' => TokenKind::Operator("&"),
                    b'|' => TokenKind::Operator("|"),
                    b'!' => TokenKind::Operator("!"),
                    b'?' => TokenKind::Operator("?"),
                    b'(' => TokenKind::LParen,
                    b')' => TokenKind::RParen,
                    b']' => TokenKind::RBracket,
                    b'{' => TokenKind::LBrace,
                    b'}' => TokenKind::RBrace,
                    _ => TokenKind::Other(char::from(byte)),
                }
            },
            span: Span { st, ed: self.curr },
        })
    }

    #[allow(clippy::too_many_lines)]
    fn read_number(&mut self) -> Result<&'a str, Error> {
        let st = self.curr;
        let mut prefix = self.src[st..].bytes().filter(|&byte| byte != b'_');
        let base = match (prefix.next(), prefix.next()) {
            (Some(b'0'), Some(b'x' | b'X')) => 16,
            (Some(b'0'), Some(b'b' | b'B')) => 2,
            _ => 10,
        };
        let exponent = if base == 16 { b'p' } else { b'e' };
        let mut prev = self.bytes[self.curr];
        self.curr += 1;

        while self.curr < self.bytes.len() {
            let byte = self.bytes[self.curr];
            if byte.is_ascii_alphanumeric()
                || matches!(byte, b'_' | b'.')
                || byte >= 0x80
                || (matches!(byte, b'+' | b'-') && prev.to_ascii_lowercase() == exponent)
            {
                if byte != b'_' {
                    prev = byte;
                }

                self.curr += 1;
            } else {
                break;
            }
        }

        let number = &self.src[st..self.curr];
        let is_valid = (|| {
            let mut bytes = number
                .bytes()
                .filter(|&byte| byte != b'_')
                .map(|byte| byte.to_ascii_lowercase())
                .peekable();
            if base != 10 {
                bytes.next();
                bytes.next();
            }

            let mut has_digits = false;
            let mut has_dot = false;
            let mut is_float = false;
            let mut val = Some(0u64);
            let mut fraction = 0usize;
            let mut fraction_ed = 0;

            while let Some(&byte) = bytes.peek() {
                let digit = match byte {
                    b'0'..=b'9' => Some(byte - b'0'),
                    b'a'..=b'f' if base == 16 => Some(byte - b'a' + 10),
                    _ => None,
                };

                if let Some(digit) = digit {
                    if digit >= base {
                        return false;
                    }
                    has_digits = true;
                    val = val.and_then(|val| val.checked_mul(u64::from(base))?.checked_add(u64::from(digit)));
                    if has_dot {
                        fraction += 1;
                        if digit != 0 {
                            fraction_ed = fraction;
                        }
                    }
                } else if byte == b'.' && !has_dot && base != 2 {
                    has_dot = true;
                    is_float = true;
                } else {
                    break;
                }
                bytes.next();
            }

            if !has_digits || fraction_ed >= 1 << 20 || (base == 2 && val.is_none()) {
                return false;
            }

            if base != 2 && bytes.peek() == Some(&exponent) {
                is_float = true;
                bytes.next();
                if matches!(bytes.peek(), Some(b'+' | b'-')) {
                    bytes.next();
                }

                let mut has_exponent = false;
                let mut exponent = 0u32;

                while let Some(&byte @ b'0'..=b'9') = bytes.peek() {
                    has_exponent = true;
                    exponent = exponent * 10 + u32::from(byte - b'0');
                    if exponent >= 1 << 20 {
                        return false;
                    }
                    bytes.next();
                }

                if !has_exponent {
                    return false;
                }
            }

            match [bytes.next(), bytes.next(), bytes.next(), bytes.next()] {
                [None | Some(b'i'), None, None, None] => true,
                [Some(b'l'), Some(b'l'), None | Some(b'u'), None] | [Some(b'u'), Some(b'l'), Some(b'l'), None] => {
                    !is_float && val.is_some()
                }
                _ => false,
            }
        })();

        if !is_valid {
            return Err(Error {
                offset: st,
                message: "malformed number",
            });
        }

        Ok(number)
    }

    #[allow(clippy::too_many_lines)]
    fn read_string(&mut self) -> Result<Literal<'a>, Error> {
        let st = self.curr;
        let quote = self.bytes[st];
        self.curr += 1;
        let content_st = self.curr;
        let mut val = None;
        let mut copy_st = content_st;
        let mut is_valid = true;
        let mut is_closed = false;
        let hex = |digit| match digit {
            b'0'..=b'9' => Some(digit - b'0'),
            b'a'..=b'f' => Some(digit - b'a' + 10),
            b'A'..=b'F' => Some(digit - b'A' + 10),
            _ => None,
        };

        while self.curr < self.bytes.len() {
            let byte = self.bytes[self.curr];
            if byte == quote {
                self.curr += 1;
                is_closed = true;
                break;
            }

            if byte == b'\n' {
                is_valid = false;
                break;
            }

            if byte == b'\\' {
                let val = val.get_or_insert_with(|| Vec::with_capacity(self.curr - content_st + 8));
                val.extend_from_slice(&self.bytes[copy_st..self.curr]);
                self.curr += 1;

                if self.curr >= self.bytes.len() {
                    is_valid = false;
                    break;
                }

                let escape = self.bytes[self.curr];
                self.curr += 1;

                match escape {
                    b'a' => val.push(b'\x07'),
                    b'b' => val.push(b'\x08'),
                    b'f' => val.push(b'\x0C'),
                    b'n' => val.push(b'\n'),
                    b'r' => val.push(b'\r'),
                    b't' => val.push(b'\t'),
                    b'v' => val.push(b'\x0B'),
                    b'\\' | b'"' | b'\'' | b'\n' => val.push(escape),
                    b'x' => {
                        let Some(high) = self.bytes.get(self.curr).copied().and_then(hex) else {
                            is_valid = false;
                            continue;
                        };
                        self.curr += 1;
                        let Some(low) = self.bytes.get(self.curr).copied().and_then(hex) else {
                            is_valid = false;
                            continue;
                        };
                        self.curr += 1;
                        val.push((high << 4) | low);
                    }
                    b'u' => {
                        if self.bytes.get(self.curr) != Some(&b'{') {
                            is_valid = false;
                            continue;
                        }

                        self.curr += 1;
                        let mut code = 0u32;
                        let mut digits = 0;

                        while let Some(digit) = self.bytes.get(self.curr).copied().and_then(hex) {
                            if let Some(next) = code.checked_mul(16).and_then(|v| v.checked_add(u32::from(digit))) {
                                code = next;
                            } else {
                                is_valid = false;
                            }

                            digits += 1;
                            self.curr += 1;
                        }

                        if self.bytes.get(self.curr) == Some(&b'}') {
                            self.curr += 1;
                        } else {
                            is_valid = false;
                        }

                        if digits == 0 {
                            is_valid = false;
                        }

                        if let Some(ch) = char::from_u32(code) {
                            let mut buf = [0; 4];
                            val.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
                        } else {
                            is_valid = false;
                        }
                    }
                    b'z' => {
                        while self.curr < self.bytes.len()
                            && matches!(self.bytes[self.curr], b' ' | b'\t' | b'\n' | 0x0b | 0x0c)
                        {
                            self.curr += 1;
                        }
                    }
                    digit @ b'0'..=b'9' => {
                        let mut num = u32::from(digit - b'0');

                        for _ in 0..2 {
                            if self.curr < self.bytes.len() && self.bytes[self.curr].is_ascii_digit() {
                                num = num * 10 + u32::from(self.bytes[self.curr] - b'0');
                                self.curr += 1;
                            } else {
                                break;
                            }
                        }

                        if let Ok(byte) = u8::try_from(num) {
                            val.push(byte);
                        } else {
                            is_valid = false;
                        }
                    }
                    _ => {
                        is_valid = false;
                        while self.bytes.get(self.curr).is_some_and(|byte| byte & 0xC0 == 0x80) {
                            self.curr += 1;
                        }
                    }
                }

                copy_st = self.curr;
            } else {
                self.curr += 1;
            }
        }

        if is_valid && is_closed {
            Ok(Literal {
                val: match val {
                    Some(mut val) => {
                        val.extend_from_slice(&self.bytes[copy_st..self.curr - 1]);
                        Cow::Owned(val)
                    }
                    None => Cow::Borrowed(&self.bytes[content_st..self.curr - 1]),
                },
                bracket: None,
            })
        } else {
            Err(Error {
                offset: st,
                message: if is_closed {
                    "invalid string escape"
                } else {
                    "unfinished string"
                },
            })
        }
    }

    fn read_bracket(&mut self) -> Result<Option<Bracket>, Error> {
        let st = self.curr;

        if self.bytes.get(st) != Some(&b'[') {
            return Ok(None);
        }

        let mut curr = st + 1;

        while self.bytes.get(curr) == Some(&b'=') {
            curr += 1;
        }

        if self.bytes.get(curr) != Some(&b'[') {
            return Ok(None);
        }

        let level = curr - st - 1;
        let content_st = curr + 1;
        let mut ed = content_st;
        while ed < self.bytes.len() {
            if self.bytes[ed] == b']' {
                let mut curr = ed + 1;

                while self.bytes.get(curr) == Some(&b'=') {
                    curr += 1;
                }

                if curr - ed - 1 == level && self.bytes.get(curr) == Some(&b']') {
                    self.curr = curr + 1;
                    return Ok(Some(Bracket {
                        span: Span { st: content_st, ed },
                        level,
                    }));
                }
            }

            ed += 1;
        }

        Err(Error {
            offset: st,
            message: "unfinished long string",
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

struct Binding<'a> {
    name: &'a str,
    val: Reference<'a>,
    scope: usize,
    captures: Vec<usize>,
    calls: Vec<usize>,
}

struct Expression<'a> {
    span: Span,
    reference: Reference<'a>,
    binding: Option<usize>,
    captures: Vec<usize>,
}

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
            Self::Ident(name) => Some(name),
            _ => None,
        }
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

#[cfg(test)]
mod tests {
    use super::*;

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
            let span = Parser::new(&tokens).parse_expr(0).unwrap();
            assert_eq!(&src[span.st..span.ed], value);
        }

        for src in ["", "1 +", "{1 2}", "{key =}", "{[key] 1}", "(1", "local a = 1"] {
            let tokens = Lexer::new(src).collect::<Result<Vec<_>, _>>().unwrap();
            assert!(Parser::new(&tokens).parse_expr(0).is_err(), "{src}");
        }
    }

    #[test]
    fn preserve_context() {
        let src = "flag ? function() return object:method() end : other:method(), flag ? first + second : fallback";
        let tokens = Lexer::new(src).collect::<Result<Vec<_>, _>>().unwrap();
        let mut parser = Parser::new(&tokens);

        for value in ["object:method()", "other:method()", "first + second", "other:method()"] {
            let span = parser.parse_expr(src.find(value).unwrap()).unwrap();
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
            let span = Parser::new(&tokens).parse_stmt(0).unwrap();
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
            assert!(Parser::new(&tokens).parse_stmt(0).is_err(), "{src}");
        }
    }

    #[test]
    fn resolve_references() {
        let src = "local api = object.operation; do local api = other end; \
            function helper(api) api('local') end; api('name')";
        let tokens = Lexer::new(src).collect::<Result<Vec<_>, _>>().unwrap();
        let output = Parser::new(&tokens).parse().unwrap();
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
            let output = Parser::new(&tokens).parse().unwrap();
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
            let error = Parser::new(&tokens).parse().err().unwrap();
            assert!(error.offset < src.len());
            assert_eq!(error.message, "chunk has too many syntax levels");
        }

        let src = format!("local value = {}1{}", "(".repeat(20), ")".repeat(20));
        let tokens = Lexer::new(&src).collect::<Result<Vec<_>, _>>().unwrap();
        assert!(Parser::new(&tokens).parse().is_ok());
    }

    #[test]
    fn validate_ident() {
        assert!(is_ident("abc").unwrap());
        assert!(is_ident("_").unwrap());
        assert!(is_ident("_foo123").unwrap());
        assert!(!is_ident("").unwrap());
        assert!(!is_ident("123").unwrap());
        assert!(!is_ident("local").unwrap());
        assert!(!is_ident("goto").unwrap());
        assert!(!is_ident("const").unwrap());
        assert!(!is_ident("continue").unwrap());
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
        let analysis = Parser::new(&tokens).parse()?;
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
            let output = Parser::new(&tokens).parse().unwrap();
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
            let output = Parser::new(&tokens).parse().unwrap();
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
        let output = Parser::new(&tokens).parse().unwrap();
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
            assert!(Parser::new(&tokens).parse().is_err(), "{src}");
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
            assert!(Parser::new(&tokens).parse().is_ok(), "{src}");
        }
    }

    #[test]
    fn parse_parameters() {
        for src in [
            "function f() end",
            "function f(a, b) end",
            "function f(...) end",
            "function f(a, ...) end",
            "local f = function(a, ...) return a end",
            "local f = |a, b| -> a + b",
        ] {
            let tokens = Lexer::new(src).collect::<Result<Vec<_>, _>>().unwrap();
            assert!(Parser::new(&tokens).parse().is_ok(), "{src}");
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
}
