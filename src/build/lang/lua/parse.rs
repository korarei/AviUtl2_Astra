use super::{Analysis, Assignment, Call, Error, Reference, Span, Token, TokenKind};
use std::borrow::{Borrow, Cow};
use std::collections::{HashMap, HashSet};

const MAX_DEPTH: usize = 200;

pub struct Parser<'a, 'tokens, T = Token<'a>> {
    tokens: &'tokens [T],
    curr: usize,
    prev: usize,
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

impl<'a, 'tokens, T: Borrow<Token<'a>>> Parser<'a, 'tokens, T> {
    #[must_use]
    pub fn new(tokens: &'tokens [T]) -> Self {
        Self {
            tokens,
            curr: tokens
                .iter()
                .position(|token| !token.borrow().kind.is_trivia())
                .unwrap_or(tokens.len()),
            prev: 0,
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

    pub fn parse_expr(&mut self, st: usize) -> Result<Span, Error> {
        self.curr = self.tokens.partition_point(|token| token.borrow().span.st < st);
        self.skip_trivia();

        if self.curr < self.context_curr {
            self.context.clear();
            self.context_curr = 0;
        }

        if self.context.is_empty() {
            self.context.push((None, 0));
        }

        while self.context_curr < self.curr {
            let (kind, conditionals) = *self.context.last().unwrap();
            let token = self.tokens[self.context_curr].borrow();
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

        let val = self.read_expr(0, self.context.last().unwrap().1 != 0);
        if let Some(error) = self.error {
            return Err(error);
        }

        val.map(|val| val.span).ok_or(Error {
            offset: self.token(self.curr).map_or(st, |token| token.span.st),
            message: "missing expression",
        })
    }

    pub fn parse_stmt(&mut self, st: usize) -> Result<Span, Error> {
        self.curr = self.tokens.partition_point(|token| token.borrow().span.st < st);
        self.skip_trivia();
        let st = self.curr;
        let token = self.token(st).ok_or(Error {
            offset: self.tokens.last().map(Borrow::borrow).map_or(0, |token| token.span.ed),
            message: "missing statement",
        })?;
        let span_st = token.span.st;
        self.read_stmt();

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

    fn parse_block(&mut self) {
        if self.depth + 1 >= MAX_DEPTH {
            self.error.get_or_insert(Error {
                offset: self.token(self.curr).map_or_else(
                    || self.tokens.last().map(Borrow::borrow).map_or(0, |token| token.span.ed),
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
            self.read_stmt();

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
    fn read_stmt(&mut self) {
        let st = self.curr;
        let prev_stmt_st = self.stmt_st.replace(self.tokens[st].borrow().span.st);

        match self.kind().cloned() {
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

                    self.parse_function(false);
                } else {
                    self.parse_assignment(st, true);
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

                self.parse_function(is_method);
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
            Some(TokenKind::Keyword("while" | "for" | "repeat")) => {
                let kind = self.kind().cloned().unwrap();
                self.bump();
                let writes_st = self.writes.len();
                let calls_st = self.output.calls.len();
                let references = self
                    .bindings
                    .iter()
                    .map(|binding| binding.reference.clone())
                    .collect::<Vec<_>>();
                let mut names = Vec::new();

                if kind == TokenKind::Keyword("for") {
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
                } else if kind == TokenKind::Keyword("while") {
                    self.require_expr(false);
                }

                if kind != TokenKind::Keyword("repeat") {
                    self.expect(&TokenKind::Keyword("do"));
                }

                self.scopes.push(HashMap::new());

                for name in names {
                    self.declare(name, None);
                }

                self.parse_block();

                if kind == TokenKind::Keyword("repeat") {
                    self.expect(&TokenKind::Keyword("until"));
                    self.require_expr(false);
                } else {
                    self.expect(&TokenKind::Keyword("end"));
                }

                self.scopes.pop().expect("scope must exist");

                let changed = self.writes[writes_st..]
                    .iter()
                    .filter_map(|&index| references.get(index).and_then(Option::as_deref))
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
            _ => self.parse_assignment(st, false),
        }

        self.stmt_st = prev_stmt_st;
    }

    #[allow(clippy::too_many_lines)]
    fn parse_assignment(&mut self, st: usize, is_local: bool) {
        let mut targets = Vec::new();
        let mut names = Vec::new();
        let mut bindings = Vec::new();

        loop {
            let target_st = self.curr;
            let name = self.kind().and_then(TokenKind::name);

            let expression = if is_local {
                if self.require_name().is_none() {
                    return;
                }
                Expression {
                    span: self.tokens[target_st].borrow().span,
                    reference: None,
                    binding: None,
                    captures: Vec::new(),
                    is_target: true,
                    is_call: false,
                }
            } else {
                let Some(expression) = self.read_expr(15, false) else {
                    self.fail("missing assignment target");
                    return;
                };

                expression
            };

            if !is_local && !expression.is_target {
                if expression.is_call && targets.is_empty() && self.kind() != Some(&TokenKind::Assign) {
                    return;
                }

                self.fail("invalid assignment target");
                return;
            }

            names.push(name.filter(|_| self.prev == target_st));
            bindings.push(expression.binding);
            targets.push(expression.span);

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

        let vals = if has_vals || is_compound {
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

        let ed = self.tokens[self.prev].borrow().span.ed;

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
                st: self.tokens[st].borrow().span.st,
                ed,
            },
            targets,
            vals: vals.into_iter().map(|val| val.span).collect(),
        });
    }

    fn parse_exprs(&mut self) -> Option<Vec<Expression<'a>>> {
        let mut vals = Vec::new();
        let Some(val) = self.read_expr(0, false) else {
            return Some(vals);
        };

        vals.push(val);

        while self.eat(&TokenKind::Comma) {
            let Some(val) = self.read_expr(0, false) else {
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

    fn parse_parameters(&mut self, ed: &TokenKind<'a>) -> Vec<&'a str> {
        let mut names = Vec::new();

        if self.eat(ed) {
            return names;
        }

        loop {
            if self.eat(&TokenKind::Dots) {
                break;
            }

            let Some(name) = self.require_name() else { return names };
            names.push(name);

            if !self.eat(&TokenKind::Comma) {
                break;
            }
        }

        self.expect(ed);
        names
    }

    fn parse_function(&mut self, is_method: bool) {
        if !self.expect(&TokenKind::LParen) {
            return;
        }

        let mut names = self.parse_parameters(&TokenKind::RParen);
        if is_method {
            names.insert(0, "self");
        }

        self.parse_function_body(names, false, false);
    }

    fn parse_short_function(&mut self, name: Option<&'a str>, should_stop_at_colon: bool) {
        let names = if let Some(name) = name {
            vec![name]
        } else if self.eat(&TokenKind::Operator("||")) {
            Vec::new()
        } else if self.eat(&TokenKind::Operator("|")) {
            self.parse_parameters(&TokenKind::Operator("|"))
        } else {
            return;
        };

        if self.expect(&TokenKind::Operator("->")) {
            self.parse_function_body(names, true, should_stop_at_colon);
        }
    }

    fn parse_function_body(&mut self, names: Vec<&'a str>, is_short: bool, should_stop_at_colon: bool) {
        let prev_stmt_st = self.stmt_st.take();
        let references = self
            .bindings
            .iter()
            .map(|binding| binding.reference.clone())
            .collect::<Vec<_>>();
        let writes_st = self.writes.len();

        self.functions.push(self.scopes.len());
        self.scopes.push(HashMap::new());

        for name in names {
            self.declare(name, None);
        }

        if is_short && !self.eat(&TokenKind::Keyword("do")) {
            self.require_expr(should_stop_at_colon);
        } else {
            self.parse_block();
            self.expect(&TokenKind::Keyword("end"));
        }

        self.scopes.pop().expect("scope must exist");
        self.functions.pop().expect("function scope must exist");
        self.invalidate(&references, writes_st);
        self.stmt_st = prev_stmt_st;
    }

    #[allow(clippy::too_many_lines)]
    fn read_expr(&mut self, min_precedence: u8, should_stop_at_colon: bool) -> Option<Expression<'a>> {
        if self.error.is_some() {
            return None;
        }

        if self.depth + 1 >= MAX_DEPTH {
            self.error = Some(Error {
                offset: self.token(self.curr).map_or_else(
                    || self.tokens.last().map(Borrow::borrow).map_or(0, |token| token.span.ed),
                    |token| token.span.st,
                ),
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
            let mut has_safe_navigation = false;

            match &token.kind {
                TokenKind::Ident(name) | TokenKind::Keyword(name @ ("goto" | "const" | "continue")) => {
                    let name = *name;
                    self.bump();
                    if self.kind() == Some(&TokenKind::Operator("->")) {
                        self.parse_short_function(Some(name), should_stop_at_colon);
                        return Some(Expression {
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
                | TokenKind::Keyword("nil" | "true" | "false") => self.bump(),
                TokenKind::Other('-' | '#') | TokenKind::Operator("~" | "!") | TokenKind::Keyword("not") => {
                    self.bump();
                    self.read_expr(13, false)?;
                }
                TokenKind::LParen => {
                    is_prefix = true;
                    self.bump();
                    let val = self.read_expr(0, false)?;
                    reference = val.reference;
                    binding = val.binding;
                    captures = val.captures;
                    if !self.expect(&TokenKind::RParen) {
                        return None;
                    }
                }
                TokenKind::LBrace => {
                    self.bump();
                    while self.kind().is_some() && self.kind() != Some(&TokenKind::RBrace) {
                        if self.eat(&TokenKind::LBracket) {
                            self.read_expr(0, false)?;
                            if !self.expect(&TokenKind::RBracket) || !self.expect(&TokenKind::Assign) {
                                return None;
                            }
                        } else if self.kind().and_then(TokenKind::name).is_some()
                            && self.peek(1).is_some_and(|token| token.kind == TokenKind::Assign)
                        {
                            self.bump();
                            self.bump();
                        }

                        self.read_expr(0, false)?;
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
                }
                TokenKind::Keyword("function") => {
                    self.bump();
                    self.parse_function(false);
                }
                TokenKind::Operator("|" | "||") => self.parse_short_function(None, should_stop_at_colon),
                _ => return None,
            }

            if is_prefix {
                loop {
                    match self.kind().cloned() {
                        Some(TokenKind::Dot) => {
                            self.bump();
                            is_target = !has_safe_navigation;
                            is_call = false;
                            let name = self.require_name()?;
                            if let Some(reference) = &mut reference {
                                reference.push(name);
                            }
                        }
                        Some(TokenKind::LBracket) => {
                            self.bump();
                            is_target = !has_safe_navigation;
                            is_call = false;
                            let key_st = self.curr;
                            self.read_expr(0, false)?;

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
                            has_safe_navigation = true;
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
                                has_safe_navigation = true;
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
                                vec![self.read_expr(15, false)?]
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
                let (precedence, is_right_associative) = match self.kind() {
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

                if precedence < min_precedence {
                    break;
                }

                self.bump();
                self.read_expr(precedence + u8::from(!is_right_associative), should_stop_at_colon)?;
                is_target = false;
                is_call = false;
                reference = None;
                binding = None;
                captures.clear();
            }

            if min_precedence == 0 && self.eat(&TokenKind::Operator("?")) {
                self.read_expr(0, true)?;

                if !self.expect(&TokenKind::Colon) {
                    return None;
                }

                self.read_expr(0, false)?;
                is_target = false;
                is_call = false;
                reference = None;
                binding = None;
                captures.clear();
            }

            Some(Expression {
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
                offset: self.token(self.curr).map_or_else(
                    || self.tokens.last().map(Borrow::borrow).map_or(0, |token| token.span.ed),
                    |token| token.span.st,
                ),
                message: "incomplete expression",
            });
        }
        val
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

    fn assign(&mut self, index: usize, reference: Reference<'a>) {
        if self.bindings[index].reference != reference {
            for call in std::mem::take(&mut self.bindings[index].calls) {
                self.output.calls[call].reference = None;
            }
        }

        self.bindings[index].reference = reference;
        self.bindings[index].captures.clear();
        self.writes.push(index);
    }

    fn restore(&mut self, references: &[Reference<'a>]) {
        for (index, binding) in self.bindings.iter_mut().enumerate() {
            if binding.scope < self.scopes.len() {
                binding.reference = references
                    .get(index)
                    .cloned()
                    .unwrap_or_else(|| Some(vec![binding.name]));
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
        if self.read_expr(0, should_stop_at_colon).is_none() {
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

    fn token(&self, at: usize) -> Option<&'tokens Token<'a>> {
        self.tokens.get(at).map(Borrow::borrow)
    }

    fn kind(&self) -> Option<&TokenKind<'a>> {
        self.token(self.curr).map(|token| &token.kind)
    }

    fn fail(&mut self, message: &'static str) {
        self.error.get_or_insert(Error {
            offset: self.token(self.curr).map_or_else(
                || self.tokens.last().map(Borrow::borrow).map_or(0, |token| token.span.ed),
                |token| token.span.st,
            ),
            message,
        });
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
}

struct Binding<'a> {
    name: &'a str,
    reference: Reference<'a>,
    scope: usize,
    captures: Vec<usize>,
    calls: Vec<usize>,
}

struct Expression<'a> {
    span: Span,
    reference: Reference<'a>,
    binding: Option<usize>,
    captures: Vec<usize>,
    is_target: bool,
    is_call: bool,
}

#[cfg(test)]
mod tests {
    use super::super::{Lexer, is_ident};
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

    #[test]
    fn syntax_cases() {
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
            if let Err(err) = Parser::new(&tokens).parse() {
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
            let err = Parser::new(&tokens)
                .parse()
                .err()
                .unwrap_or_else(|| panic!("{name}: {src:?}"));
            assert!(err.offset <= src.len(), "{name}: {err} at {}", err.offset);
        }
    }

    #[test]
    fn fragment_spans() {
        let src = "-- first\nlocal value = --[[inline]] first + second; -- tail\ncall(value);";
        let tokens = Lexer::new(src).collect::<Result<Vec<_>, _>>().unwrap();
        let mut parser = Parser::new(&tokens);
        for (st, expected) in [
            (src.find("first +").unwrap(), "first + second"),
            (src.find("call").unwrap(), "call(value)"),
        ] {
            let span = parser.parse_expr(st).unwrap();
            assert_eq!(&src[span.st..span.ed], expected, "expression at {st}");
        }
        let span = parser.parse_stmt(0).unwrap();
        assert_eq!(
            &src[span.st..span.ed],
            "local value = --[[inline]] first + second;",
            "statement"
        );
        assert!(Parser::new(&tokens).parse_expr(src.len()).is_err(), "expression at end");
        assert!(Parser::new(&tokens).parse_stmt(src.len()).is_err(), "statement at end");
    }
}
