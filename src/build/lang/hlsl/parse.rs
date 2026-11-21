use super::{
    Action, Analysis, Annotation, Decl, Directive, Error, Func, Lexer, Member, SCALARS, Scalar, Span, Token, TokenKind,
    Type,
};

enum Macro {
    Object(String),
    Func,
    Undefined,
    Unknown,
}

pub struct Parser<'a, 'tokens> {
    src: &'a str,
    tokens: &'tokens [Token<'a>],
    curr: usize,
}

impl<'a, 'tokens> Parser<'a, 'tokens> {
    #[must_use]
    pub fn new(src: &'a str, tokens: &'tokens [Token<'a>]) -> Self {
        Self { src, tokens, curr: 0 }
    }

    pub fn parse(mut self) -> Result<Analysis<'a, 'tokens>, Error> {
        self.curr = 0;
        let mut output = Analysis::default();

        while self.curr < self.tokens.len() {
            self.parse_item(&mut output)?;
        }

        Ok(output)
    }

    pub fn parse_stmt_at(&mut self, st: usize) -> Result<Span, Error> {
        self.curr = self.tokens.partition_point(|token| token.span.st < st);
        self.skip_trivia();

        let st = self.curr;
        self.parse_stmt(0)?;

        Ok(Span {
            st: self.tokens[st].span.st,
            ed: self.tokens[self.curr - 1].span.ed,
        })
    }

    pub fn parse_expr_at(&mut self, st: usize) -> Result<Span, Error> {
        self.curr = self.tokens.partition_point(|token| token.span.st < st);
        self.parse_expr(0, 0, false)
    }

    #[allow(clippy::too_many_lines)]
    pub(in crate::build) fn parse_decl(src: &'a str, tokens: &[Token<'a>]) -> Result<Option<Decl<'a>>, Error> {
        let mut curr = 0;
        let mut modifiers = Vec::new();
        let skip = |curr: &mut usize| {
            while tokens.get(*curr).is_some_and(|token| token.kind.is_trivia()) {
                *curr += 1;
            }
        };
        skip(&mut curr);
        let st = curr;

        while let Some(token) = tokens.get(curr) {
            let TokenKind::Modifier(modifier) = token.kind else {
                break;
            };
            modifiers.push(modifier);
            curr += 1;
            skip(&mut curr);
        }

        let Some(token) = tokens.get(curr) else {
            return if modifiers.is_empty() {
                Ok(None)
            } else {
                Err(Error {
                    offset: tokens.last().unwrap().span.ed,
                    message: "missing declaration type",
                })
            };
        };

        let (ty, len) = Self::parse_type(src, &tokens[curr..]);

        if ty.is_none()
            && modifiers.is_empty()
            && !matches!(
                token.kind,
                TokenKind::Ident(_) | TokenKind::Keyword(_) | TokenKind::Vec | TokenKind::Mat
            )
        {
            return Ok(None);
        }

        let ty = ty.ok_or(Error {
            offset: token.span.st,
            message: "invalid declaration type",
        })?;
        let mut decl = Decl {
            ty,
            span: Span {
                st: tokens[st].span.st,
                ed: tokens.iter().rfind(|token| !token.kind.is_trivia()).unwrap().span.ed,
            },
            modifiers,
            members: Vec::new(),
        };

        let error = |curr: usize, message| Error {
            offset: tokens.get(curr).map_or(decl.span.ed, |token| token.span.st),
            message,
        };

        curr += len;
        skip(&mut curr);
        loop {
            let member = tokens
                .get(curr)
                .ok_or_else(|| error(curr, "missing declaration member"))?;
            let TokenKind::Ident(name) = member.kind else {
                return Err(error(curr, "invalid declaration member"));
            };

            curr += 1;
            skip(&mut curr);
            let mut dims = Vec::new();

            while tokens.get(curr).is_some_and(|token| token.kind == TokenKind::LBracket) {
                let ed = find_pair(tokens, curr).ok_or_else(|| error(curr, "unclosed array dimension"))?;
                let mut size = tokens[curr + 1..ed].iter().filter(|token| !token.kind.is_trivia());
                dims.push(match (size.next().map(|token| &token.kind), size.next()) {
                    (Some(TokenKind::Number(text)), None) => text.parse::<usize>().ok(),
                    _ => None,
                });
                curr = ed + 1;
                skip(&mut curr);
            }

            let annotation = if tokens.get(curr).is_some_and(|token| token.kind == TokenKind::Colon) {
                curr += 1;
                skip(&mut curr);
                let token = tokens
                    .get(curr)
                    .ok_or_else(|| error(curr, "missing declaration annotation"))?;
                let (TokenKind::Ident(name) | TokenKind::Keyword(name @ ("packoffset" | "register"))) = token.kind
                else {
                    return Err(error(curr, "invalid declaration annotation"));
                };

                curr += 1;
                skip(&mut curr);

                let args = if tokens.get(curr).is_some_and(|token| token.kind == TokenKind::LParen) {
                    let ed = find_pair(tokens, curr).ok_or_else(|| error(curr, "unclosed declaration annotation"))?;
                    let args = &src[tokens[curr].span.ed..tokens[ed].span.st];
                    curr = ed + 1;
                    skip(&mut curr);
                    Some(args)
                } else {
                    None
                };

                Some(Annotation { name, args })
            } else {
                None
            };

            if tokens.get(curr).is_some_and(|token| token.kind == TokenKind::Assign) {
                curr += 1;
                skip(&mut curr);
                if tokens.get(curr).is_none_or(|token| token.kind == TokenKind::Comma) {
                    return Err(error(curr, "missing declaration initializer"));
                }

                while let Some(token) = tokens.get(curr).filter(|token| token.kind != TokenKind::Comma) {
                    match token.kind {
                        TokenKind::LParen | TokenKind::LBracket | TokenKind::LBrace => {
                            let ed = find_pair(tokens, curr)
                                .ok_or_else(|| error(curr, "unclosed declaration initializer"))?;
                            curr = ed + 1;
                        }
                        TokenKind::RParen | TokenKind::RBracket | TokenKind::RBrace => {
                            return Err(error(curr, "unexpected closing delimiter in declaration initializer"));
                        }
                        _ => curr += 1,
                    }
                }
            }

            decl.members.push(Member {
                name,
                span: member.span,
                dims,
                annotation,
            });

            let Some(token) = tokens.get(curr) else {
                break;
            };

            if token.kind != TokenKind::Comma {
                return Err(error(curr, "unexpected token after declaration member"));
            }

            curr += 1;
            skip(&mut curr);
        }

        Ok(Some(decl))
    }

    #[allow(clippy::too_many_lines)]
    fn parse_item(&mut self, output: &mut Analysis<'a, 'tokens>) -> Result<(), Error> {
        self.skip_trivia();
        let tokens = self.tokens;
        let kind = |pos: usize| tokens.get(pos).map(|token| &token.kind);
        let next = |pos: usize| pos + tokens[pos..].iter().take_while(|token| token.kind.is_trivia()).count();
        let src = self.src;
        let parse_decl = |tokens: &[Token<'a>]| {
            let decl = Self::parse_decl(src, tokens)?;
            if decl.is_none()
                && let Some(token) = tokens.iter().find(|token| !token.kind.is_trivia())
            {
                return Err(Error {
                    offset: token.span.st,
                    message: "invalid declaration type",
                });
            }

            Ok(decl)
        };
        let st = self.curr;

        if tokens
            .get(st)
            .is_some_and(|token| token.kind == TokenKind::Keyword("typedef"))
            && tokens
                .get(next(st + 1))
                .is_some_and(|token| token.kind == TokenKind::Keyword("struct"))
        {
            self.curr = next(st + 1);
            self.parse_stmt(0)?;
            return Ok(());
        }

        let mut is_initializer = false;
        while let Some(token) = tokens.get(self.curr) {
            let curr = self.curr;
            if token.kind.is_trivia() {
                self.curr += 1;
                continue;
            }

            let prev = tokens[..curr].iter().rposition(|token| !token.kind.is_trivia());
            if let Some((name, rest)) = split_directive(token) {
                self.curr += 1;
                output.directives.push(Directive {
                    name,
                    rest,
                    span: token.span,
                });
                return Ok(());
            }

            if !is_initializer
                && let TokenKind::Ident(name) = token.kind
                && let Some(prev) = prev
                && matches!(
                    kind(prev),
                    Some(
                        TokenKind::Ident(_)
                            | TokenKind::Type(_)
                            | TokenKind::Vec
                            | TokenKind::Mat
                            | TokenKind::Keyword("void")
                            | TokenKind::Other('>')
                    )
                )
                && kind(next(curr + 1)) == Some(&TokenKind::LParen)
                && let Some(args_ed) = find_pair(tokens, next(curr + 1))
            {
                let mut ed = next(args_ed + 1);
                if kind(ed) == Some(&TokenKind::Colon) {
                    ed = next(ed + 1);
                    if matches!(kind(ed), Some(TokenKind::Ident(_))) {
                        ed = next(ed + 1);
                    }
                }

                if kind(ed) == Some(&TokenKind::LBrace)
                    && let Some(body_ed) = find_pair(tokens, ed)
                {
                    self.curr = body_ed + 1;
                    output.funcs.push(Func {
                        name,
                        span: token.span,
                        is_void: kind(prev) == Some(&TokenKind::Keyword("void")),
                        has_numthreads: Self::find_numthreads(tokens, prev).is_some(),
                    });
                    return Ok(());
                }

                if kind(ed) == Some(&TokenKind::Semicolon) {
                    self.curr = ed + 1;
                    return Ok(());
                }
            }

            match token.kind {
                TokenKind::LBrace if is_initializer => {
                    self.curr = find_pair(tokens, curr).ok_or(Error {
                        offset: token.span.st,
                        message: "unclosed declaration initializer",
                    })? + 1;
                    continue;
                }
                TokenKind::LBrace => {
                    let ed = find_pair(tokens, curr).ok_or(Error {
                        offset: token.span.st,
                        message: "unclosed declaration block",
                    })?;
                    self.curr = ed + 1;
                    output.groups.push(Span {
                        st: token.span.st,
                        ed: tokens[ed].span.ed,
                    });
                    return Ok(());
                }
                TokenKind::LParen | TokenKind::LBracket => {
                    self.curr = find_pair(tokens, curr).ok_or(Error {
                        offset: token.span.st,
                        message: if token.kind == TokenKind::LBracket {
                            "unclosed array dimension"
                        } else if tokens[st..curr].iter().any(|token| token.kind == TokenKind::Colon) {
                            "unclosed declaration annotation"
                        } else {
                            "unclosed declaration group"
                        },
                    })? + 1;
                    continue;
                }
                TokenKind::Assign => is_initializer = true,
                TokenKind::Comment(_) if st == curr => {
                    self.curr += 1;
                    return Ok(());
                }
                TokenKind::Comment(_) => {
                    self.curr += 1;
                    continue;
                }
                TokenKind::Semicolon => {
                    self.curr += 1;
                    let is_alias = tokens[st].kind == TokenKind::Keyword("typedef");

                    if let Some(decl) = parse_decl(&tokens[st + usize::from(is_alias)..curr])?
                        && !is_alias
                    {
                        output.decls.push(decl);
                    }

                    return Ok(());
                }
                _ => {}
            }

            self.curr += 1;
        }

        if tokens
            .get(st)
            .is_some_and(|token| token.kind == TokenKind::Keyword("typedef"))
        {
            return Err(Error {
                offset: tokens[st].span.st,
                message: "missing semicolon after typedef",
            });
        }

        if let Some(decl) = parse_decl(&tokens[st..])? {
            output.decls.push(decl);
        }

        Ok(())
    }

    #[allow(clippy::too_many_lines)]
    fn parse_stmt(&mut self, depth: usize) -> Result<(), Error> {
        self.skip_trivia();
        let tokens = self.tokens;
        let src_ed = self.src.len();
        let error = |curr: usize, message| Error {
            offset: tokens.get(curr).map_or(src_ed, |token| token.span.st),
            message,
        };

        if depth >= 200 {
            return Err(error(self.curr, "statement has too many syntax levels"));
        }

        let mut kind = &tokens
            .get(self.curr)
            .ok_or_else(|| error(self.curr, "missing statement"))?
            .kind;

        while kind == &TokenKind::LBracket {
            self.curr =
                find_pair(tokens, self.curr).ok_or_else(|| error(self.curr, "unclosed statement attribute"))? + 1;
            self.skip_trivia();
            kind = &tokens
                .get(self.curr)
                .ok_or_else(|| error(self.curr, "missing statement after attribute"))?
                .kind;
        }

        if matches!(kind, TokenKind::Keyword("if" | "for" | "while" | "switch" | "do")) {
            let is_if = kind == &TokenKind::Keyword("if");
            let is_do = kind == &TokenKind::Keyword("do");
            self.curr += 1;

            if !is_do {
                self.skip_trivia();

                if tokens.get(self.curr).map(|token| &token.kind) != Some(&TokenKind::LParen) {
                    return Err(error(self.curr, "missing control condition"));
                }

                self.curr =
                    find_pair(tokens, self.curr).ok_or_else(|| error(self.curr, "unclosed control condition"))? + 1;
            }

            self.parse_stmt(depth + 1)?;
            let body_ed = self.curr;
            self.skip_trivia();

            if is_if && tokens.get(self.curr).map(|token| &token.kind) == Some(&TokenKind::Keyword("else")) {
                self.curr += 1;
                self.parse_stmt(depth + 1)?;
            } else if is_do {
                if tokens.get(self.curr).map(|token| &token.kind) != Some(&TokenKind::Keyword("while")) {
                    return Err(error(self.curr, "missing while after do"));
                }

                self.curr += 1;
                self.skip_trivia();

                if tokens.get(self.curr).map(|token| &token.kind) != Some(&TokenKind::LParen) {
                    return Err(error(self.curr, "missing control condition"));
                }

                self.curr =
                    find_pair(tokens, self.curr).ok_or_else(|| error(self.curr, "unclosed control condition"))? + 1;
                self.skip_trivia();

                if tokens.get(self.curr).map(|token| &token.kind) != Some(&TokenKind::Semicolon) {
                    return Err(error(self.curr, "missing semicolon after do"));
                }

                self.curr += 1;
            } else {
                self.curr = body_ed;
            }

            return Ok(());
        }

        let is_type = matches!(kind, TokenKind::Keyword("struct" | "class" | "interface"));
        let mut is_initializer = false;

        while let Some(token) = tokens.get(self.curr) {
            match token.kind {
                TokenKind::LParen | TokenKind::LBracket => {
                    self.curr =
                        find_pair(tokens, self.curr).ok_or_else(|| error(self.curr, "unclosed statement group"))? + 1;
                }
                TokenKind::LBrace => {
                    self.curr =
                        find_pair(tokens, self.curr).ok_or_else(|| error(self.curr, "unclosed statement block"))? + 1;

                    if !is_type && !is_initializer {
                        return Ok(());
                    }
                }
                TokenKind::Semicolon => {
                    self.curr += 1;
                    return Ok(());
                }
                TokenKind::Assign => {
                    is_initializer = true;
                    self.curr += 1;
                }
                TokenKind::RParen | TokenKind::RBracket | TokenKind::RBrace | TokenKind::Directive(_) => {
                    return Err(error(self.curr, "invalid statement"));
                }
                _ => self.curr += 1,
            }
        }

        Err(error(self.curr, "unclosed statement"))
    }

    #[allow(clippy::too_many_lines)]
    fn parse_expr(&mut self, depth: usize, min_prec: u8, should_stop_at_colon: bool) -> Result<Span, Error> {
        self.skip_trivia();
        let tokens = self.tokens;
        let src = self.src;
        let error = |curr: usize, message| Error {
            offset: tokens.get(curr).map_or(src.len(), |token| token.span.st),
            message,
        };

        if depth >= 200 {
            return Err(error(self.curr, "expression has too many syntax levels"));
        }

        let st = tokens
            .get(self.curr)
            .ok_or_else(|| error(self.curr, "missing expression"))?
            .span
            .st;

        while tokens
            .get(self.curr)
            .is_some_and(|token| matches!(token.kind, TokenKind::Other('+' | '-' | '!' | '~')))
        {
            self.curr += 1;
            self.skip_trivia();
        }

        let mut is_cast = false;

        match tokens.get(self.curr).map(|token| &token.kind) {
            Some(TokenKind::LParen) => {
                let group_st = self.curr;
                let mut type_st = group_st + 1;

                while tokens.get(type_st).is_some_and(|token| token.kind.is_trivia()) {
                    type_st += 1;
                }

                let (ty, len) = Self::parse_type(src, &tokens[type_st..]);
                let mut type_ed = type_st + len;

                while ty.is_some() && tokens.get(type_ed).is_some() {
                    match tokens[type_ed].kind {
                        TokenKind::Whitespace | TokenKind::Newline | TokenKind::Comment(_) => type_ed += 1,
                        TokenKind::LBracket => {
                            type_ed = find_pair(tokens, type_ed)
                                .ok_or_else(|| error(type_ed, "unclosed cast array dimension"))?
                                + 1;
                        }
                        _ => break,
                    }
                }

                let can_start = |token: &Token<'_>| {
                    matches!(
                        token.kind,
                        TokenKind::Ident(_)
                            | TokenKind::Type(_)
                            | TokenKind::Vec
                            | TokenKind::Mat
                            | TokenKind::Number(_)
                            | TokenKind::String
                            | TokenKind::LParen
                            | TokenKind::LBrace
                            | TokenKind::Keyword("true" | "false" | "NULL")
                            | TokenKind::Other('+' | '-' | '!' | '~')
                    )
                };
                self.curr = (type_ed + 1).min(tokens.len());
                self.skip_trivia();
                let is_postfix = tokens.get(self.curr).is_some_and(|token| {
                    src[token.span.st..].starts_with("++") || src[token.span.st..].starts_with("--")
                }) && tokens
                    .iter()
                    .skip(self.curr + 2)
                    .find(|token| !token.kind.is_trivia())
                    .is_none_or(|token| !can_start(token));
                is_cast = ty.is_some()
                    && tokens.get(type_ed).is_some_and(|token| token.kind == TokenKind::RParen)
                    && (!matches!(ty, Some(Type::Named(_))) || !is_postfix)
                    && tokens.get(self.curr).is_some_and(can_start);

                if !is_cast {
                    self.curr = group_st + 1;
                    self.parse_group(&TokenKind::RParen, depth, false)?;
                }
            }
            Some(TokenKind::LBrace) => {
                self.curr += 1;
                self.parse_group(&TokenKind::RBrace, depth, true)?;
            }
            Some(TokenKind::Vec | TokenKind::Mat) => {
                let (ty, len) = Self::parse_type(src, &tokens[self.curr..]);
                if ty.is_none() {
                    return Err(error(self.curr, "invalid vector or matrix type"));
                }

                self.curr += len;
            }
            Some(
                TokenKind::Ident(_)
                | TokenKind::Type(_)
                | TokenKind::String
                | TokenKind::Keyword("true" | "false" | "NULL"),
            ) => self.curr += 1,
            Some(TokenKind::Number(text)) => {
                if !is_number(text) {
                    return Err(error(self.curr, "invalid numeric literal"));
                }

                self.curr += 1;
            }
            _ => return Err(error(self.curr, "invalid expression operand")),
        }

        let mut ed = tokens[self.curr - 1].span.ed;
        self.skip_trivia();

        if is_cast {
            ed = self.parse_expr(depth + 1, 13, should_stop_at_colon)?.ed;
        }

        loop {
            self.skip_trivia();

            match tokens.get(self.curr).map(|token| &token.kind) {
                Some(TokenKind::LParen) => {
                    self.curr += 1;
                    self.parse_group(&TokenKind::RParen, depth, true)?;
                    ed = tokens[self.curr - 1].span.ed;
                }
                Some(TokenKind::LBracket) => {
                    self.curr += 1;
                    self.parse_group(&TokenKind::RBracket, depth, false)?;
                    ed = tokens[self.curr - 1].span.ed;
                }
                Some(TokenKind::Dot) => {
                    self.curr += 1;
                    self.skip_trivia();

                    if !matches!(
                        tokens.get(self.curr).map(|token| &token.kind),
                        Some(TokenKind::Ident(_))
                    ) {
                        return Err(error(self.curr, "missing member name"));
                    }

                    ed = tokens[self.curr].span.ed;
                    self.curr += 1;
                }
                Some(TokenKind::Other('+' | '-'))
                    if src[tokens[self.curr].span.st..].starts_with("++")
                        || src[tokens[self.curr].span.st..].starts_with("--") =>
                {
                    ed = tokens[self.curr].span.st + 2;
                    self.curr += 2;
                }
                Some(TokenKind::Other('?')) if min_prec <= 2 => {
                    self.curr += 1;
                    self.parse_expr(depth + 1, 0, true)?;
                    self.skip_trivia();

                    if tokens.get(self.curr).map(|token| &token.kind) != Some(&TokenKind::Colon) {
                        return Err(error(self.curr, "missing conditional colon"));
                    }

                    self.curr += 1;
                    ed = self.parse_expr(depth + 1, 2, should_stop_at_colon)?.ed;
                }
                Some(TokenKind::Colon) if should_stop_at_colon => break,
                Some(
                    TokenKind::Assign
                    | TokenKind::Other('+' | '-' | '*' | '/' | '%' | '&' | '|' | '^' | '<' | '>' | '!'),
                ) => {
                    static OPERATORS: [&str; 29] = [
                        "<<=", ">>=", "&&", "||", "==", "!=", "<=", ">=", "<<", ">>", "+=", "-=", "*=", "/=", "%=",
                        "&=", "|=", "^=", "=", "+", "-", "*", "/", "%", "&", "|", "^", "<", ">",
                    ];

                    let token_st = tokens[self.curr].span.st;
                    let op = OPERATORS
                        .iter()
                        .copied()
                        .find(|op| src[token_st..].starts_with(op))
                        .ok_or_else(|| error(self.curr, "invalid expression operator"))?;
                    let prec = match op {
                        "=" | "+=" | "-=" | "*=" | "/=" | "%=" | "&=" | "|=" | "^=" | "<<=" | ">>=" => 1,
                        "||" => 3,
                        "&&" => 4,
                        "|" => 5,
                        "^" => 6,
                        "&" => 7,
                        "==" | "!=" => 8,
                        "<" | ">" | "<=" | ">=" => 9,
                        "<<" | ">>" => 10,
                        "+" | "-" => 11,
                        _ => 12,
                    };

                    if prec < min_prec {
                        break;
                    }

                    let op_ed = token_st + op.len();

                    while tokens.get(self.curr).is_some_and(|token| token.span.st < op_ed) {
                        self.curr += 1;
                    }

                    ed = self
                        .parse_expr(depth + 1, prec + u8::from(prec != 1), should_stop_at_colon)?
                        .ed;
                }
                _ => break,
            }
        }

        Ok(Span { st, ed })
    }

    fn parse_group(&mut self, closing: &TokenKind<'a>, depth: usize, can_be_empty: bool) -> Result<(), Error> {
        self.skip_trivia();
        let tokens = self.tokens;
        let src_ed = self.src.len();
        let error = |curr, message| Error {
            offset: tokens.get(curr).map_or(src_ed, |token: &Token<'_>| token.span.st),
            message,
        };

        if tokens.get(self.curr).map(|token| &token.kind) == Some(closing) {
            if !can_be_empty {
                return Err(error(self.curr, "missing expression"));
            }

            self.curr += 1;
            return Ok(());
        }

        loop {
            self.parse_expr(depth + 1, 0, false)?;
            self.skip_trivia();

            if tokens.get(self.curr).map(|token| &token.kind) == Some(closing) {
                self.curr += 1;
                return Ok(());
            }

            if tokens.get(self.curr).map(|token| &token.kind) != Some(&TokenKind::Comma) {
                return Err(error(self.curr, "missing expression delimiter"));
            }

            self.curr += 1;
            self.skip_trivia();

            if closing == &TokenKind::RBrace && tokens.get(self.curr).map(|token| &token.kind) == Some(closing) {
                self.curr += 1;
                return Ok(());
            }
        }
    }

    #[must_use]
    #[allow(clippy::too_many_lines)]
    fn parse_type(src: &'a str, tokens: &[Token<'a>]) -> (Option<Type<'a>>, usize) {
        let Some(token) = tokens.first() else {
            return (None, 0);
        };

        if let TokenKind::Type(ty) = token.kind {
            return (Some(ty), 1);
        }

        if matches!(
            token.kind,
            TokenKind::Ident(_)
                | TokenKind::Keyword(
                    "AppendStructuredBuffer"
                        | "Buffer"
                        | "ByteAddressBuffer"
                        | "ConsumeStructuredBuffer"
                        | "RasterizerOrderedBuffer"
                        | "RasterizerOrderedByteAddressBuffer"
                        | "RasterizerOrderedStructuredBuffer"
                        | "RasterizerOrderedTexture1D"
                        | "RasterizerOrderedTexture1DArray"
                        | "RasterizerOrderedTexture2D"
                        | "RasterizerOrderedTexture2DArray"
                        | "RasterizerOrderedTexture3D"
                        | "RWBuffer"
                        | "RWByteAddressBuffer"
                        | "RWStructuredBuffer"
                        | "RWTexture1D"
                        | "RWTexture1DArray"
                        | "RWTexture2D"
                        | "RWTexture2DArray"
                        | "RWTexture3D"
                        | "SamplerState"
                        | "SamplerComparisonState"
                        | "StructuredBuffer"
                        | "Texture1D"
                        | "Texture1DArray"
                        | "Texture2D"
                        | "Texture2DArray"
                        | "Texture2DMS"
                        | "Texture2DMSArray"
                        | "Texture3D"
                        | "TextureCube"
                        | "TextureCubeArray"
                        | "string"
                        | "texture"
                        | "sampler"
                        | "sampler1D"
                        | "sampler2D"
                        | "sampler3D"
                        | "samplerCUBE"
                )
        ) {
            let mut curr = 1;
            while tokens.get(curr).is_some_and(|token| token.kind.is_trivia()) {
                curr += 1;
            }

            if tokens.get(curr).is_none_or(|token| token.kind != TokenKind::Other('<')) {
                return (Some(Type::Named(&src[token.span.st..token.span.ed])), 1);
            }

            let mut depth = 1;
            curr += 1;
            while let Some(next) = tokens.get(curr) {
                match next.kind {
                    TokenKind::Other('<') => depth += 1,
                    TokenKind::Other('>') => {
                        depth -= 1;
                        if depth == 0 {
                            return (Some(Type::Named(&src[token.span.st..next.span.ed])), curr + 1);
                        }
                    }
                    TokenKind::LParen | TokenKind::LBracket => {
                        let Some(ed) = find_pair(tokens, curr) else {
                            return (None, 1);
                        };
                        curr = ed;
                    }
                    TokenKind::Semicolon
                    | TokenKind::LBrace
                    | TokenKind::RBrace
                    | TokenKind::RParen
                    | TokenKind::RBracket
                    | TokenKind::Directive(_) => break,
                    _ => {}
                }
                curr += 1;
            }

            return (None, 1);
        }

        let is_mat = match token.kind {
            TokenKind::Vec => false,
            TokenKind::Mat => true,
            _ => return (None, 1),
        };

        let mut args = [token; 8];
        let mut indices = [0; 8];
        let mut count = 0;

        for (index, token) in tokens
            .iter()
            .enumerate()
            .filter(|(_, token)| !token.kind.is_trivia())
            .take(args.len())
        {
            args[count] = token;
            indices[count] = index;
            count += 1;
        }

        if count < 2 || args[1].kind != TokenKind::Other('<') {
            let (name, size) = SCALARS[0];
            let scalar = Scalar { name, size };
            return (
                Some(if is_mat {
                    Type::Mat {
                        scalar,
                        rows: 4,
                        cols: 4,
                    }
                } else {
                    Type::Vec { scalar, cols: 4 }
                }),
                1,
            );
        }

        let len = if is_mat { 8 } else { 6 };

        if count < len {
            return (None, 1);
        }

        let TokenKind::Type(Type::Scalar(scalar)) = args[2].kind else {
            return (None, 1);
        };

        if args[3].kind != TokenKind::Comma
            || args[len - 1].kind != TokenKind::Other('>')
            || (is_mat && args[5].kind != TokenKind::Comma)
        {
            return (None, 1);
        }

        let parse = |token: &Token<'_>| {
            let TokenKind::Number(text) = token.kind else {
                return None;
            };
            text.parse::<usize>().ok().filter(|&val| (1..=4).contains(&val))
        };

        (
            (if is_mat { parse(args[4]) } else { Some(1) })
                .zip(parse(args[if is_mat { 6 } else { 4 }]))
                .map(|(rows, cols)| {
                    if is_mat {
                        Type::Mat { scalar, rows, cols }
                    } else {
                        Type::Vec { scalar, cols }
                    }
                }),
            indices[len - 1] + 1,
        )
    }

    fn find_numthreads(tokens: &[Token<'_>], mut curr: usize) -> Option<Span> {
        let kind = |pos: usize| tokens.get(pos).map(|token| &token.kind);

        while curr > 0
            && tokens.get(curr - 1).is_some_and(|token| {
                token.kind.is_trivia() || matches!(token.kind, TokenKind::Ident(_) | TokenKind::Modifier(_))
            })
        {
            curr -= 1;
        }

        while curr > 0 && kind(curr - 1) == Some(&TokenKind::RBracket) {
            let group_st = find_pair(tokens, curr - 1)?;

            if let Some(token) = tokens[group_st + 1..curr - 1]
                .iter()
                .find(|token| !token.kind.is_trivia())
                && token.kind == TokenKind::Ident("numthreads")
            {
                return Some(token.span);
            }

            curr = group_st;
            while curr > 0 && tokens[curr - 1].kind.is_trivia() {
                curr -= 1;
            }
        }

        None
    }

    fn skip_trivia(&mut self) {
        while self.tokens.get(self.curr).is_some_and(|token| token.kind.is_trivia()) {
            self.curr += 1;
        }
    }
}

pub(in crate::build) struct Preprocessor {
    stack: Vec<Frame>,
    macros: std::collections::HashMap<String, Macro>,
    pub(in crate::build) is_row_major: Option<bool>,
    pub(in crate::build) has_packing: bool,
}

impl Preprocessor {
    pub(in crate::build) fn new() -> Self {
        Self {
            stack: Vec::new(),
            macros: std::collections::HashMap::new(),
            is_row_major: Some(false),
            has_packing: false,
        }
    }

    #[allow(clippy::too_many_lines)]
    pub(in crate::build) fn process(&mut self, token: &Token<'_>) -> Result<Option<Action>, Error> {
        let Some((name, rest)) = split_directive(token) else {
            return Ok(None);
        };
        let combine = |left, right| match (left, right) {
            (Some(false), _) | (_, Some(false)) => Some(false),
            (Some(true), Some(true)) => Some(true),
            _ => None,
        };
        let is_met = |preprocessor: &Self| match name {
            "ifdef" => preprocessor.is_defined(rest.trim()),
            "ifndef" => preprocessor.is_defined(rest.trim()).map(|val| !val),
            _ => preprocessor.evaluate(rest).map(|val| val != 0),
        };

        let mut action = Action::Drop;

        match name {
            "if" | "ifdef" | "ifndef" => {
                let is_parent_active = self.is_active();
                let has_matched = if is_parent_active == Some(false) {
                    Some(false)
                } else {
                    is_met(self)
                };
                self.stack.push(Frame {
                    st: token.span.st,
                    is_parent_active,
                    has_matched,
                    is_active: combine(is_parent_active, has_matched),
                    has_else: false,
                });

                if is_parent_active != Some(false) && has_matched.is_none() {
                    action = Action::Unknown;
                }
            }
            "elif" => {
                let frame = self.stack.last().ok_or(Error {
                    offset: token.span.st,
                    message: "unexpected #elif",
                })?;
                if frame.has_else {
                    return Err(Error {
                        offset: token.span.st,
                        message: "#elif after #else",
                    });
                }

                let has_matched = if frame.is_parent_active == Some(false) || frame.has_matched == Some(true) {
                    Some(false)
                } else {
                    is_met(self)
                };
                let frame = self.stack.last_mut().unwrap();
                frame.is_active = combine(
                    frame.is_parent_active,
                    combine(frame.has_matched.map(|val| !val), has_matched),
                );
                frame.has_matched = match (frame.has_matched, has_matched) {
                    (Some(true), _) | (_, Some(true)) => Some(true),
                    (Some(false), Some(false)) => Some(false),
                    _ => None,
                };
            }
            "else" => {
                let frame = self.stack.last_mut().ok_or(Error {
                    offset: token.span.st,
                    message: "unexpected #else",
                })?;
                if frame.has_else {
                    return Err(Error {
                        offset: token.span.st,
                        message: "duplicate #else",
                    });
                }

                frame.is_active = combine(frame.is_parent_active, frame.has_matched.map(|val| !val));
                frame.has_matched = Some(true);
                frame.has_else = true;
            }
            "endif" => {
                self.stack.pop().ok_or(Error {
                    offset: token.span.st,
                    message: "unexpected #endif",
                })?;
            }
            _ if self.is_active() == Some(false) => {}
            "define" | "undef" => {
                let ed = rest
                    .find(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_')
                    .unwrap_or(rest.len());
                let key = &rest[..ed];
                if crate::vars::is_ident(key) {
                    let val = if self.is_active().is_none() {
                        Macro::Unknown
                    } else if name == "undef" {
                        Macro::Undefined
                    } else if rest[ed..].starts_with('(') {
                        Macro::Func
                    } else {
                        Macro::Object(rest[ed..].trim().to_owned())
                    };

                    self.macros.insert(key.to_owned(), val);
                }
            }
            "include" => {
                self.is_row_major = None;
                self.has_packing = true;

                for val in self.macros.values_mut() {
                    *val = Macro::Unknown;
                }

                action = Action::Include;
            }
            "pragma" => {
                action = Action::Keep;
                let text = self.expand(rest);
                let args = text.as_deref().and_then(|text| {
                    Lexer::new(text)
                        .filter(|token| token.as_ref().map_or(true, |token| !token.kind.is_trivia()))
                        .collect::<Result<Vec<_>, _>>()
                        .ok()
                });

                if let Some(args) = args {
                    if args
                        .first()
                        .is_some_and(|token| matches!(token.kind, TokenKind::Ident("pack_matrix")))
                    {
                        action = Action::MatPacking;
                        self.has_packing = true;
                        self.is_row_major = if self.is_active() == Some(true) {
                            match args.as_slice() {
                                [
                                    _,
                                    Token {
                                        kind: TokenKind::LParen,
                                        ..
                                    },
                                    Token {
                                        kind: TokenKind::Modifier(major @ ("row_major" | "column_major")),
                                        ..
                                    },
                                    Token {
                                        kind: TokenKind::RParen,
                                        ..
                                    },
                                ] => Some(*major == "row_major"),
                                _ => None,
                            }
                        } else {
                            None
                        };
                    }
                } else {
                    action = Action::MatPacking;
                    self.is_row_major = None;
                }
            }
            _ => action = Action::Keep,
        }

        Ok(Some(action))
    }

    pub(in crate::build) fn expand(&self, src: &str) -> Option<String> {
        fn expand(preprocessor: &Preprocessor, src: &str, depth: usize) -> Option<String> {
            if depth >= 32 {
                return None;
            }

            let mut result = String::new();
            let mut curr = 0;

            for token in Lexer::new(src) {
                let token = token.ok()?;
                let name = &src[token.span.st..token.span.ed];
                if !crate::vars::is_ident(name) {
                    continue;
                }

                match preprocessor.macros.get(name) {
                    Some(Macro::Object(val)) => {
                        result.push_str(&src[curr..token.span.st]);
                        result.push(' ');
                        result.push_str(&expand(preprocessor, val, depth + 1)?);
                        result.push(' ');
                        curr = token.span.ed;
                    }
                    Some(Macro::Func | Macro::Unknown) => return None,
                    Some(Macro::Undefined) | None => {}
                }

                if result.len() > 65_536 {
                    return None;
                }
            }

            result.push_str(&src[curr..]);
            (result.len() <= 65_536).then_some(result)
        }

        expand(self, src, 0)
    }

    #[allow(clippy::too_many_lines)]
    pub(in crate::build) fn evaluate(&self, src: &str) -> Option<i32> {
        #[allow(clippy::too_many_lines)]
        fn parse_expr(src: &str, tokens: &[Token<'_>], curr: &mut usize, min_prec: u8, depth: usize) -> Option<i32> {
            if depth >= 200 {
                return None;
            }

            let token = tokens.get(*curr)?;
            *curr += 1;
            let mut val = match token.kind {
                TokenKind::Number(text) => {
                    if text.ends_with(['u', 'U']) {
                        return None;
                    }

                    let text = text.trim_suffix(['l', 'L']);
                    if let Some(text) = text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
                        i32::from_str_radix(text, 16).ok()?
                    } else if text.len() > 1 && text.starts_with('0') {
                        i32::from_str_radix(&text[1..], 8).ok()?
                    } else {
                        text.parse().ok()?
                    }
                }
                TokenKind::String => match &src.as_bytes()[token.span.st..token.span.ed] {
                    [b'\'', val, b'\''] if val.is_ascii() => i32::from(*val),
                    _ => return None,
                },
                TokenKind::LParen => {
                    let val = parse_expr(src, tokens, curr, 0, depth + 1)?;
                    if tokens.get(*curr)?.kind != TokenKind::RParen {
                        return None;
                    }

                    *curr += 1;
                    val
                }
                TokenKind::Other(op @ ('+' | '-' | '!' | '~')) => {
                    if src[token.span.st..].starts_with("++") || src[token.span.st..].starts_with("--") {
                        return None;
                    }

                    let val = parse_expr(src, tokens, curr, 13, depth + 1)?;
                    match op {
                        '+' => val,
                        '-' => val.checked_neg()?,
                        '!' => i32::from(val == 0),
                        _ => !val,
                    }
                }
                _ => return None,
            };

            while let Some(token) = tokens.get(*curr) {
                if token.kind == TokenKind::Other('?') && min_prec <= 1 {
                    *curr += 1;
                    let first = parse_expr(src, tokens, curr, 0, depth + 1)?;
                    if tokens.get(*curr)?.kind != TokenKind::Colon {
                        return None;
                    }

                    *curr += 1;
                    let second = parse_expr(src, tokens, curr, 1, depth + 1)?;
                    val = if val != 0 { first } else { second };
                    continue;
                }

                let Some(op) = [
                    "||", "&&", "==", "!=", "<=", ">=", "<<", ">>", "|", "^", "&", "<", ">", "+", "-", "*", "/", "%",
                ]
                .into_iter()
                .find(|op| src[token.span.st..].starts_with(op)) else {
                    break;
                };
                let prec = match op {
                    "||" => 2,
                    "&&" => 3,
                    "|" => 4,
                    "^" => 5,
                    "&" => 6,
                    "==" | "!=" => 7,
                    "<" | ">" | "<=" | ">=" => 8,
                    "<<" | ">>" => 9,
                    "+" | "-" => 10,
                    _ => 11,
                };

                if prec < min_prec {
                    break;
                }

                let ed = token.span.st + op.len();
                while tokens.get(*curr).is_some_and(|token| token.span.st < ed) {
                    *curr += 1;
                }

                let right = parse_expr(src, tokens, curr, prec + 1, depth + 1)?;
                val = match op {
                    "+" => val.checked_add(right)?,
                    "-" => val.checked_sub(right)?,
                    "*" => val.checked_mul(right)?,
                    "/" => val.checked_div(right)?,
                    "%" => val.checked_rem(right)?,
                    "<<" if val >= 0 && (0..32).contains(&right) => {
                        i32::try_from(i64::from(val).checked_shl(u32::try_from(right).ok()?)?).ok()?
                    }
                    ">>" if val >= 0 && (0..32).contains(&right) => val >> right,
                    "&" => val & right,
                    "|" => val | right,
                    "^" => val ^ right,
                    "==" => i32::from(val == right),
                    "!=" => i32::from(val != right),
                    "<" => i32::from(val < right),
                    ">" => i32::from(val > right),
                    "<=" => i32::from(val <= right),
                    ">=" => i32::from(val >= right),
                    "&&" => i32::from(val != 0 && right != 0),
                    "||" => i32::from(val != 0 || right != 0),
                    _ => return None,
                };
            }

            Some(val)
        }

        let tokens = Lexer::new(src)
            .filter(|token| token.as_ref().map_or(true, |token| !token.kind.is_trivia()))
            .collect::<Result<Vec<_>, _>>()
            .ok()?;
        let mut text = String::new();
        let mut curr = 0;
        let mut index = 0;
        while index < tokens.len() {
            let token = &tokens[index];
            if token.kind == TokenKind::Ident("defined") {
                text.push_str(&src[curr..token.span.st]);
                index += 1;
                let has_parens = tokens.get(index).is_some_and(|token| token.kind == TokenKind::LParen);
                index += usize::from(has_parens);
                let key = tokens.get(index)?;
                let is_defined = self.is_defined(&src[key.span.st..key.span.ed])?;
                text.push_str(if is_defined { "1" } else { "0" });
                index += 1;

                if has_parens {
                    if tokens.get(index)?.kind != TokenKind::RParen {
                        return None;
                    }

                    curr = tokens[index].span.ed;
                    index += 1;
                } else {
                    curr = key.span.ed;
                }
            } else if matches!(token.kind, TokenKind::Ident(_))
                && matches!(
                    self.macros.get(&src[token.span.st..token.span.ed]),
                    Some(Macro::Undefined)
                )
            {
                text.push_str(&src[curr..token.span.st]);
                text.push('0');
                curr = token.span.ed;
                index += 1;
            } else {
                index += 1;
            }
        }

        text.push_str(&src[curr..]);
        let text = self.expand(&text)?;
        let tokens = Lexer::new(&text)
            .filter(|token| token.as_ref().map_or(true, |token| !token.kind.is_trivia()))
            .collect::<Result<Vec<_>, _>>()
            .ok()?;

        let mut curr = 0;
        let val = parse_expr(&text, &tokens, &mut curr, 0, 0)?;
        (curr == tokens.len()).then_some(val)
    }

    pub(in crate::build) fn finish(&self) -> Result<(), Error> {
        if let Some(frame) = self.stack.last() {
            return Err(Error {
                offset: frame.st,
                message: "unclosed HLSL conditional directive",
            });
        }

        Ok(())
    }

    pub(in crate::build) fn is_active(&self) -> Option<bool> {
        self.stack.last().map_or(Some(true), |frame| frame.is_active)
    }

    fn is_defined(&self, name: &str) -> Option<bool> {
        match self.macros.get(name) {
            Some(Macro::Object(_) | Macro::Func) => Some(true),
            Some(Macro::Undefined) => Some(false),
            Some(Macro::Unknown) | None => None,
        }
    }
}

struct Frame {
    st: usize,
    is_parent_active: Option<bool>,
    has_matched: Option<bool>,
    is_active: Option<bool>,
    has_else: bool,
}

#[must_use]
pub(in crate::build) fn find_pair(tokens: &[Token<'_>], pos: usize) -> Option<usize> {
    let pair = |kind: &TokenKind<'_>| match kind {
        TokenKind::LParen => Some((b'(', true)),
        TokenKind::LBracket => Some((b'[', true)),
        TokenKind::LBrace => Some((b'{', true)),
        TokenKind::RParen => Some((b'(', false)),
        TokenKind::RBracket => Some((b'[', false)),
        TokenKind::RBrace => Some((b'{', false)),
        _ => None,
    };
    let (mut group, is_open) = pair(&tokens.get(pos)?.kind)?;
    let mut expected = Vec::new();
    let mut curr = pos;
    loop {
        curr = if is_open {
            curr.checked_add(1)?
        } else {
            curr.checked_sub(1)?
        };
        let token = tokens.get(curr)?;
        if let Some((next_group, is_next_open)) = pair(&token.kind) {
            if is_next_open == is_open {
                expected.push(group);
                group = next_group;
            } else {
                if group != next_group {
                    return None;
                }

                if let Some(prev) = expected.pop() {
                    group = prev;
                } else {
                    return Some(curr);
                }
            }
        }
    }
}

#[must_use]
fn split_directive<'token>(token: &'token Token<'_>) -> Option<(&'token str, &'token str)> {
    let TokenKind::Directive(text) = &token.kind else {
        return None;
    };

    let text = text.strip_prefix('#')?.trim_start();
    let ed = text
        .find(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_')
        .unwrap_or(text.len());

    Some((&text[..ed], text[ed..].trim_start()))
}

fn is_number(text: &str) -> bool {
    let bytes = text.as_bytes();

    if text.starts_with("0x") || text.starts_with("0X") {
        let ed = 2 + bytes[2..].iter().take_while(|byte| byte.is_ascii_hexdigit()).count();
        return ed > 2 && matches!(&text[ed..], "" | "u" | "U" | "l" | "L");
    }

    let digits_ed = bytes.iter().take_while(|byte| byte.is_ascii_digit()).count();
    let mut curr = digits_ed;
    let mut is_float = false;

    if bytes.get(curr) == Some(&b'.') {
        is_float = true;
        curr += 1;
        let st = curr;
        curr += bytes[curr..].iter().take_while(|byte| byte.is_ascii_digit()).count();
        if digits_ed == 0 && curr == st {
            return false;
        }
    } else if digits_ed == 0 {
        return false;
    }

    if bytes.get(curr).is_some_and(|byte| matches!(*byte, b'e' | b'E')) {
        is_float = true;
        curr += 1;
        if bytes.get(curr).is_some_and(|byte| matches!(*byte, b'+' | b'-')) {
            curr += 1;
        }

        let st = curr;
        curr += bytes[curr..].iter().take_while(|byte| byte.is_ascii_digit()).count();
        if curr == st {
            return false;
        }
    }

    let suffix = &text[curr..];
    if is_float || matches!(suffix, "f" | "F" | "h" | "H") {
        matches!(suffix, "" | "f" | "F" | "h" | "H" | "l" | "L")
    } else {
        matches!(suffix, "" | "u" | "U" | "l" | "L")
            && (digits_ed < 2 || bytes[0] != b'0' || bytes[..digits_ed].iter().all(|byte| *byte <= b'7'))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_resources() {
        for ty in [
            "RasterizerOrderedBuffer<float4>",
            "RasterizerOrderedByteAddressBuffer",
            "RasterizerOrderedStructuredBuffer<Element>",
            "RasterizerOrderedTexture1D<float4>",
            "RasterizerOrderedTexture1DArray<float4>",
            "RasterizerOrderedTexture2D<float4>",
            "RasterizerOrderedTexture2DArray<float4>",
            "RasterizerOrderedTexture3D<float4>",
            "Texture2D<float4>",
            "Texture2DMS<float4, 4>",
            "RWTexture2D<float4>",
            "StructuredBuffer<Element>",
            "RWStructuredBuffer<Element>",
            "SamplerState",
            "SamplerComparisonState",
        ] {
            let src = format!("{ty} value : register(u0);");
            let tokens = Lexer::new(&src)
                .map(Result::unwrap)
                .filter(|token| token.kind != TokenKind::Whitespace)
                .collect::<Vec<_>>();
            let output = Parser::new(&src, &tokens).parse().unwrap();
            assert_eq!(output.decls[0].ty, Type::Named(ty), "{src}");
            assert_eq!(output.decls[0].members[0].name, "value");
            assert_eq!(output.decls[0].members[0].annotation.unwrap().args, Some("u0"));
        }

        let src = "typedef float Real; typedef float4 Color; typedef Real Array[2]; \
            typedef struct Element { float value; } Item; Real value; Color main() : SV_Target { return 0; }";
        let tokens = Lexer::new(src)
            .map(Result::unwrap)
            .filter(|token| token.kind != TokenKind::Whitespace)
            .collect::<Vec<_>>();
        let output = Parser::new(src, &tokens).parse().unwrap();
        assert_eq!(output.decls.len(), 1);
        assert_eq!(output.decls[0].ty, Type::Named("Real"));
        assert_eq!(output.funcs[0].name, "main");
    }

    #[test]
    fn parse_casts() {
        for value in [
            "(value)++",
            "(value)--",
            "(value)++ /* retained */",
            "(value)++ * other",
            "(float)++value",
            "(Real)++value",
            "(vector<float, 2>)value",
            "(matrix<float, 2, 2>)value",
            "( float /* type */ ) value + 1",
            "(vector < float, 2 >) float2(1, 2)",
            "(float[2])values",
            "(float4)(first + second)",
        ] {
            let src = format!("{value}, retained");
            let tokens = Lexer::new(&src).collect::<Result<Vec<_>, _>>().unwrap();
            let span = Parser::new(&src, &tokens).parse_expr_at(0).unwrap();
            assert_eq!(
                &src[span.st..span.ed],
                value.trim_end_matches(" /* retained */"),
                "{value}"
            );
        }
    }

    #[test]
    fn parse_numbers() {
        for value in [
            "0", "07", "0xFF", "0XABC", "42u", "0xffU", "1f", ".5", "1.", "1.e-2", "2E+3", "1.0h", "0.5L",
        ] {
            let tokens = Lexer::new(value).collect::<Result<Vec<_>, _>>().unwrap();
            assert_eq!(
                Parser::new(value, &tokens).parse_expr_at(0).unwrap().ed,
                value.len(),
                "{value}"
            );
        }

        for value in [
            "1e+", "1e-", "0x", "0xGG", "1.2.3", "1_000", "08", "0x1p2", "1.0u", "1ff",
        ] {
            let tokens = Lexer::new(value).collect::<Result<Vec<_>, _>>().unwrap();
            let err = Parser::new(value, &tokens).parse_expr_at(0).unwrap_err();
            assert_eq!(err.message, "invalid numeric literal", "{value}");
        }
        assert!(Lexer::new("#define MERGE(a,b) a##b\nMERGE(float, 4x4)").all(|token| token.is_ok()));
    }

    #[test]
    fn reject_groups() {
        for src in [
            "struct Element { float value;",
            "cbuffer Values { float value;",
            "float4 main() : SV_Target { return 0;",
            "float values[] = {1, 2;",
            "float value[2;",
            "float helper(float value;",
            "typedef struct Element { float value;",
            "typedef float Real",
            "Texture2D<float4 value;",
        ] {
            let tokens = Lexer::new(src)
                .map(Result::unwrap)
                .filter(|token| token.kind != TokenKind::Whitespace)
                .collect::<Vec<_>>();
            assert!(Parser::new(src, &tokens).parse().is_err(), "{src}");
        }
    }

    #[test]
    fn parse_expr() {
        for value in [
            "first + second",
            "(first + second)",
            "{{1, 2}, {3, 4}}",
            "float4(1, 2, 3, 4).xy",
            "values[index + 1]",
            "( float ) value + 1",
            "flag ? first : second",
            "vector<float, 2>(1, 2)",
            "++value + other--",
        ] {
            let src = format!("/* annotation */ {value}, retained");
            let tokens = Lexer::new(&src).collect::<Result<Vec<_>, _>>().unwrap();
            let span = Parser::new(&src, &tokens).parse_expr_at(0).unwrap();
            assert_eq!(&src[span.st..span.ed], value);
        }

        let src = std::iter::repeat_n("1", 500).collect::<Vec<_>>().join("+");
        let tokens = Lexer::new(&src).collect::<Result<Vec<_>, _>>().unwrap();
        assert_eq!(Parser::new(&src, &tokens).parse_expr_at(0).unwrap().ed, src.len());

        for src in [
            "",
            "1 +",
            "{1 2}",
            "{1; 2}",
            "float4(1,)",
            "(1",
            "flag ? first",
            "values[",
        ] {
            let tokens = Lexer::new(src).collect::<Result<Vec<_>, _>>().unwrap();
            assert!(Parser::new(src, &tokens).parse_expr_at(0).is_err(), "{src}");
        }
    }

    #[test]
    fn parse_stmt() {
        for statement in [
            "float first, second;",
            "float values[] = {1, 2};",
            "float foo(float value) { return value; }",
            "struct Value { float first; } value;",
            "cbuffer Values { float first; }",
            "if (flag) foo(); else if (other) bar(); else baz();",
            "for (int i = 0; i < 10; ++i) while (flag) foo();",
            "do { foo(); } while (flag);",
            "[unroll] for (int i = 0; i < 10; ++i) { foo(); }",
        ] {
            let src = format!("// annotation\n{statement}\nfloat retained;");
            let tokens = Lexer::new(&src).collect::<Result<Vec<_>, _>>().unwrap();
            let span = Parser::new(&src, &tokens).parse_stmt_at(0).unwrap();
            assert_eq!(&src[span.st..span.ed], statement);
        }

        for src in [
            "",
            "// annotation",
            "float value",
            "if (flag)",
            "if (flag) { foo();",
            "do foo();",
            "do foo(); while (flag)",
            "foo(1;",
        ] {
            let tokens = Lexer::new(src).collect::<Result<Vec<_>, _>>().unwrap();
            assert!(Parser::new(src, &tokens).parse_stmt_at(0).is_err(), "{src}");
        }
    }

    #[test]
    fn find_pairs() {
        let tokens = Lexer::new("({[]})").collect::<Result<Vec<_>, _>>().unwrap();
        assert_eq!(find_pair(&tokens, 0), Some(5));
        assert_eq!(find_pair(&tokens, 5), Some(0));
        assert_eq!(find_pair(&tokens, 1), Some(4));
        assert_eq!(find_pair(&tokens, 4), Some(1));
        let tokens = Lexer::new("([)]").collect::<Result<Vec<_>, _>>().unwrap();
        assert_eq!(find_pair(&tokens, 0), None);
        assert_eq!(find_pair(&tokens, 3), None);
    }

    fn tokenize(src: &str) -> Vec<Token<'_>> {
        Lexer::new(src)
            .filter(|token| token.as_ref().map_or(true, |token| !token.kind.is_trivia()))
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    }

    #[test]
    fn classify_tokens() {
        let src = "volatile float4 value vector matrix cbuffer row_major Custom {}()[];:,.=";
        let tokens = tokenize(src);
        assert_eq!(tokens[0].kind, TokenKind::Modifier("volatile"));
        assert_eq!(
            tokens[1].kind,
            TokenKind::Type(Type::Vec {
                scalar: Scalar { name: "float", size: 4 },
                cols: 4,
            })
        );
        assert_eq!(tokens[2].kind, TokenKind::Ident("value"));
        assert_eq!(tokens[3].kind, TokenKind::Vec);
        assert_eq!(tokens[4].kind, TokenKind::Mat);
        assert_eq!(tokens[5].kind, TokenKind::Keyword("cbuffer"));
        assert_eq!(tokens[6].kind, TokenKind::Modifier("row_major"));
        assert_eq!(tokens[7].kind, TokenKind::Ident("Custom"));
        assert_eq!(
            tokens[8..].iter().map(|token| token.kind.clone()).collect::<Vec<_>>(),
            [
                TokenKind::LBrace,
                TokenKind::RBrace,
                TokenKind::LParen,
                TokenKind::RParen,
                TokenKind::LBracket,
                TokenKind::RBracket,
                TokenKind::Semicolon,
                TokenKind::Colon,
                TokenKind::Comma,
                TokenKind::Dot,
                TokenKind::Assign
            ]
        );
    }

    #[test]
    fn distinguish_directives() {
        let src = "//#define VALUE 1\n\
            /* #if FLAG */\n\
            \"#pragma pack_matrix(row_major)\"\n\
            float x; #endif\n \
            #pragma pack_matrix(column_major)\n";
        let tokens = Lexer::new(src).collect::<Result<Vec<_>, _>>().unwrap();
        assert_eq!(
            tokens
                .iter()
                .filter(|token| matches!(token.kind, TokenKind::Directive(_)))
                .map(|token| &src[token.span.st..token.span.ed])
                .collect::<Vec<_>>(),
            ["#pragma pack_matrix(column_major)"]
        );
        assert!(tokens.iter().any(|token| token.kind == TokenKind::Other('#')));
        let tokens = tokenize("# pragma pack_matrix(row_major)");
        let output = Parser::new("# pragma pack_matrix(row_major)", &tokens).parse().unwrap();
        assert_eq!(output.directives[0].name, "pragma");
        assert_eq!(output.directives[0].rest, "pack_matrix(row_major)");
    }

    #[test]
    fn preserve_comments() {
        for (src, content, is_block) in [("// text", " text", false), ("/*\ntext\n*/", "\ntext\n", true)] {
            let token = Lexer::new(src).next().unwrap().unwrap();
            let TokenKind::Comment(comment) = token.kind else {
                panic!("{src}");
            };
            assert_eq!(comment.text, src);
            assert_eq!(comment.content, content);
            assert_eq!(comment.bracket.is_some(), is_block);
            if let Some(span) = comment.bracket {
                assert_eq!(&src[span.st..span.ed], content);
            }
        }

        let src = "// comment \\\n//#define HIDDEN 1\nfloat value;";
        let tokens = Lexer::new(src).collect::<Result<Vec<_>, _>>().unwrap();
        let TokenKind::Comment(comment) = tokens[0].kind else {
            panic!("{src}");
        };
        assert_eq!(comment.content, " comment \\\n//#define HIDDEN 1");
        assert!(comment.bracket.is_none());
        assert_eq!(tokens[1].kind, TokenKind::Newline);
        assert_eq!(&src[tokens[2].span.st..tokens[2].span.ed], "float");
    }

    #[test]
    fn preserve_continuations() {
        for (src, expected) in [
            ("#define VALUE \\\n2\nfloat value;", "#define VALUE 2"),
            ("#define VALUE \\\n\nfloat value;", "#define VALUE "),
            (
                "#define VALUE /*\n//#define HIDDEN 1\n*/ 2\nfloat value;",
                "#define VALUE /*\n//#define HIDDEN 1\n*/ 2",
            ),
        ] {
            let tokens = Lexer::new(src).collect::<Result<Vec<_>, _>>().unwrap();
            let TokenKind::Directive(text) = &tokens[0].kind else {
                panic!("{src}");
            };
            assert_eq!(text.as_ref(), expected);
            assert_eq!(
                tokens
                    .iter()
                    .filter(|token| matches!(token.kind, TokenKind::Directive(_)))
                    .count(),
                1
            );
            assert!(tokens.iter().any(|token| token.kind == TokenKind::Ident("value")));
        }
    }

    #[test]
    fn preserve_spans() {
        let src = "cbuffer 色 { float4 値; }; \"日本語\\\"/* #if */\" // コメント\n0xff 1.5e-2 .5f";
        let tokens = Lexer::new(src).collect::<Result<Vec<_>, _>>().unwrap();
        let mut ed = 0;

        for token in &tokens {
            assert_eq!(token.span.st, ed);
            assert!(src.is_char_boundary(token.span.ed));
            ed = token.span.ed;
        }
        assert_eq!(ed, src.len());
        assert!(tokens.iter().any(|token| token.kind == TokenKind::Ident("値")));
        assert_eq!(
            tokens
                .iter()
                .filter_map(|token| match token.kind {
                    TokenKind::Number(value) => Some(value),
                    _ => None,
                })
                .collect::<Vec<_>>(),
            ["0xff", "1.5e-2", ".5f"]
        );
        let src = "0x1e+2 0X1E-2 1e+2 0x1p-2";
        assert_eq!(
            Lexer::new(src)
                .map(Result::unwrap)
                .filter_map(|token| match token.kind {
                    TokenKind::Number(value) => Some(value),
                    _ => None,
                })
                .collect::<Vec<_>>(),
            ["0x1e", "2", "0X1E", "2", "1e+2", "0x1p-2"]
        );
    }

    #[test]
    fn reject_lexical_errors() {
        for (src, offset, message) in [
            ("  /* unfinished", 2, "unfinished HLSL comment"),
            ("#define VALUE /* unfinished", 14, "unfinished HLSL comment"),
            ("\"unfinished\n", 0, "unfinished HLSL string"),
        ] {
            let mut lexer = Lexer::new(src);
            let err = lexer.by_ref().collect::<Result<Vec<_>, _>>().unwrap_err();
            assert_eq!(err.offset, offset, "{src}");
            assert_eq!(err.message, message, "{src}");
            assert!(lexer.next().is_none());
        }
        assert_eq!(
            Lexer::new("\"continued\\\nstring\"").next().unwrap().unwrap().kind,
            TokenKind::String
        );
    }

    #[test]
    fn collect_analysis() {
        let src = "#pragma pack_matrix(row_major)\n\
            float value; cbuffer Params { float amount; }; \
            [numthreads(1,1,1)] void cs() {} void ps(out float4 color) {} float4 other() { return 0; }";
        let tokens = tokenize(src);
        let output = Parser::new(src, &tokens).parse().unwrap();
        assert_eq!(output.decls.len(), 1);
        assert_eq!(output.decls[0].members[0].name, "value");
        assert_eq!(output.directives.len(), 1);
        assert_eq!(output.groups.len(), 1);
        let span = output.groups[0];
        assert_eq!(&src[span.st..span.ed], "{ float amount; }");
        assert_eq!(
            output
                .funcs
                .iter()
                .map(|func| {
                    assert_eq!(&src[func.span.st..func.span.ed], func.name);
                    (func.name, func.is_void, func.has_numthreads)
                })
                .collect::<Vec<_>>(),
            [("cs", true, true), ("ps", true, false), ("other", false, false)]
        );
    }

    #[test]
    fn parse_members() {
        let src = "row_major matrix<float, 2, 3> m[2], n;";
        let tokens = tokenize(src);
        let output = Parser::new(src, &tokens).parse().unwrap();
        let decl = &output.decls[0];
        assert_eq!(
            decl.ty,
            Type::Mat {
                scalar: Scalar { name: "float", size: 4 },
                rows: 2,
                cols: 3
            }
        );
        assert_eq!(decl.modifiers, ["row_major"]);
        assert_eq!(decl.members.len(), 2);
        assert_eq!(decl.members[0].name, "m");
        assert_eq!(decl.members[0].dims, [Some(2)]);
        assert!(decl.members[1].dims.is_empty());
        let src = "float a : TEXCOORD0, b : packoffset(c1.y), c : register(c2) = 0;";
        let tokens = tokenize(src);
        let output = Parser::new(src, &tokens).parse().unwrap();

        for (member, name, args) in [
            (&output.decls[0].members[0], "TEXCOORD0", None),
            (&output.decls[0].members[1], "packoffset", Some("c1.y")),
            (&output.decls[0].members[2], "register", Some("c2")),
        ] {
            assert_eq!(member.annotation, Some(Annotation { name, args }));
            assert_eq!(&src[member.span.st..member.span.ed], member.name);
        }

        for (src, dims) in [
            ("Custom value[COUNT + 1];", vec![None]),
            ("float value[][2];", vec![None, Some(2)]),
        ] {
            let tokens = tokenize(src);
            let output = Parser::new(src, &tokens).parse().unwrap();
            assert_eq!(output.decls[0].members[0].dims, dims);
        }
    }

    #[test]
    fn reject_decls() {
        for (src, offset, message) in [
            ("float good, broken[2", 18, "unclosed array dimension"),
            ("float value,", 12, "missing declaration member"),
            ("float value :", 13, "missing declaration annotation"),
            ("float value : packoffset(c0", 24, "unclosed declaration annotation"),
            ("float value =", 13, "missing declaration initializer"),
            ("float value = {1, 2", 14, "unclosed declaration initializer"),
            ("vector<float, 5> value;", 0, "invalid declaration type"),
            ("matrix<float, 2> value;", 0, "invalid declaration type"),
            ("vector<Custom, 2> value;", 0, "invalid declaration type"),
        ] {
            let tokens = tokenize(src);
            let err = Parser::new(src, &tokens).parse().unwrap_err();
            assert_eq!(err.offset, offset, "{src}");
            assert_eq!(err.message, message, "{src}");
        }
    }

    #[test]
    fn reject_depth() {
        for src in [
            format!("{}1{}", "(".repeat(250), ")".repeat(250)),
            format!("{}1", "flag ? 0 : ".repeat(250)),
        ] {
            let tokens = Lexer::new(&src).collect::<Result<Vec<_>, _>>().unwrap();
            let err = Parser::new(&src, &tokens).parse_expr_at(0).unwrap_err();
            assert_eq!(err.message, "expression has too many syntax levels");
            assert!(err.offset < src.len());
        }
    }

    #[test]
    fn classify_scalar_types() {
        for (name, size) in [
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
        ] {
            let token = Lexer::new(name).next().unwrap().unwrap();
            assert_eq!(token.kind, TokenKind::Type(Type::Scalar(Scalar { name, size })));

            for cols in 1..=4 {
                let src = format!("{name}{cols}");
                assert_eq!(
                    Lexer::new(&src).next().unwrap().unwrap().kind,
                    TokenKind::Type(Type::Vec {
                        scalar: Scalar { name, size },
                        cols
                    })
                );

                for rows in 1..=4 {
                    let src = format!("{name}{rows}x{cols}");
                    assert_eq!(
                        Lexer::new(&src).next().unwrap().unwrap().kind,
                        TokenKind::Type(Type::Mat {
                            scalar: Scalar { name, size },
                            rows,
                            cols
                        })
                    );
                }
            }
        }
    }

    fn preprocess(src: &str) -> Preprocessor {
        let mut preprocessor = Preprocessor::new();

        for token in Lexer::new(src) {
            preprocessor.process(&token.unwrap()).unwrap();
        }
        preprocessor
    }

    #[test]
    fn evaluate_conditions() {
        let preprocessor = Preprocessor::new();

        for (expr, expected) in [
            ("1 + 2 * 3 == 7", 1),
            ("(1 << 3) | 2", 10),
            ("!0 && (7 % 3 == 1)", 1),
            ("~0 == -1", 1),
            ("0 || 3 > 1", 1),
            ("0 ? 1 : 0", 0),
            ("1 ? 0 : 1", 0),
            ("010 == 8 && 0x10 == 16", 1),
            ("'A' == 65", 1),
        ] {
            assert_eq!(preprocessor.evaluate(expr), Some(expected), "{expr}");
        }
    }

    #[test]
    fn preserve_unknown_conditions() {
        for expr in ["2147483647 + 1", "1 << 32", "1 / 0", "UNKNOWN", "0xffffffffu"] {
            let mut preprocessor = Preprocessor::new();
            assert_eq!(preprocessor.evaluate(expr), None, "{expr}");
            let src = format!("#if {expr}");
            let token = Lexer::new(&src).next().unwrap().unwrap();
            assert_eq!(preprocessor.process(&token).unwrap(), Some(Action::Unknown));
            assert_eq!(preprocessor.is_active(), None);
        }

        for src in ["#define COUNT(x) (x)", "#define COUNT OTHER\n#define OTHER COUNT"] {
            assert!(preprocess(src).expand("COUNT(2)").is_none(), "{src}");
        }
    }

    #[test]
    fn reject_conditionals() {
        for (src, message) in [
            ("#else", "unexpected #else"),
            ("#elif 1", "unexpected #elif"),
            ("#endif", "unexpected #endif"),
            ("#if 1\n#else\n#else\n#endif", "duplicate #else"),
            ("#if 1\n#else\n#elif 1\n#endif", "#elif after #else"),
            ("#if 1", "unclosed HLSL conditional directive"),
        ] {
            let mut preprocessor = Preprocessor::new();
            let result = (|| {
                for token in Lexer::new(src) {
                    preprocessor.process(&token?)?;
                }
                preprocessor.finish()
            })();
            let err = result.unwrap_err();
            assert_eq!(err.message, message, "{src}");
            assert!(err.offset <= src.len());
        }

        let preprocessor = preprocess("\n#if 1\n#if 1");
        assert_eq!(preprocessor.finish().unwrap_err().offset, 7);
    }

    #[test]
    fn expand_macros() {
        let preprocessor = preprocess("#define VALUE (1 + 2)\n#define OTHER VALUE * 2\n#define float half");
        assert_eq!(preprocessor.evaluate("OTHER"), Some(6));
        assert_eq!(
            preprocessor
                .expand("float value = OTHER;")
                .unwrap()
                .split_whitespace()
                .collect::<Vec<_>>(),
            ["half", "value", "=", "(1", "+", "2)", "*", "2", ";"]
        );
        assert_eq!(
            preprocessor.expand("\"VALUE\" /* OTHER */"),
            Some("\"VALUE\" /* OTHER */".to_owned())
        );
        let preprocessor = preprocess("#define VALUE 1\n#undef VALUE");
        assert_eq!(preprocessor.evaluate("defined(VALUE)"), Some(0));
        assert_eq!(preprocessor.evaluate("VALUE"), Some(0));
        assert_eq!(preprocessor.expand("VALUE"), Some("VALUE".to_owned()));
    }

    #[test]
    fn preserve_conditional_state() {
        let mut preprocessor = Preprocessor::new();

        for (directive, is_expected) in [
            ("#if 0", Some(false)),
            ("#if UNKNOWN", Some(false)),
            ("#else", Some(false)),
            ("#endif", Some(false)),
            ("#elif 1", Some(true)),
            ("#else", Some(false)),
            ("#endif", Some(true)),
        ] {
            let token = Lexer::new(directive).next().unwrap().unwrap();
            preprocessor.process(&token).unwrap();
            assert_eq!(preprocessor.is_active(), is_expected, "{directive}");
        }
        preprocessor.finish().unwrap();
        let preprocessor = preprocess("#if 0\n#define VALUE 1\n#endif");
        assert_eq!(preprocessor.evaluate("defined(VALUE)"), None);
        let preprocessor = preprocess("#if UNKNOWN\n#define VALUE 1\n#endif");
        assert_eq!(preprocessor.expand("VALUE"), None);
    }

    #[test]
    fn preserve_packing_state() {
        let preprocessor = Preprocessor::new();
        assert_eq!(preprocessor.is_row_major, Some(false));
        assert!(!preprocessor.has_packing);

        for (src, is_expected) in [
            ("#pragma pack_matrix(row_major)", Some(true)),
            ("#pragma pack_matrix(column_major)", Some(false)),
            ("#if UNKNOWN\n#pragma pack_matrix(row_major)\n#endif", None),
            ("#include \"external.hlsl\"", None),
        ] {
            let preprocessor = preprocess(src);
            assert_eq!(preprocessor.is_row_major, is_expected, "{src}");
            assert!(preprocessor.has_packing);
        }

        let preprocessor = preprocess(
            "#pragma pack_matrix(row_major)\n\
             #if 0\n\
             #include \"missing.hlsl\"\n\
             #pragma pack_matrix(column_major)\n\
             #endif",
        );
        assert_eq!(preprocessor.is_row_major, Some(true));
    }

    #[test]
    fn invalidate_external_definitions() {
        let preprocessor = preprocess("#define VALUE 1\n#undef REMOVED\n#include \"external.hlsl\"");
        assert_eq!(preprocessor.evaluate("VALUE"), None);
        assert_eq!(preprocessor.evaluate("defined(REMOVED)"), None);
        let preprocessor = preprocess("#define VALUE 1\n#include \"external.hlsl\"\n#define VALUE 2\n#undef REMOVED");
        assert_eq!(preprocessor.evaluate("VALUE"), Some(2));
        assert_eq!(preprocessor.evaluate("defined(REMOVED)"), Some(0));
    }

    #[test]
    fn return_directive_actions() {
        let mut preprocessor = Preprocessor::new();

        for (src, expected) in [
            ("float", None),
            ("#define VALUE 1", Some(Action::Drop)),
            ("#pragma message(\"value\")", Some(Action::Keep)),
            ("#pragma pack_matrix(row_major)", Some(Action::MatPacking)),
            ("#include \"external.hlsl\"", Some(Action::Include)),
        ] {
            let token = Lexer::new(src).next().unwrap().unwrap();
            assert_eq!(preprocessor.process(&token).unwrap(), expected, "{src}");
        }
    }

    #[test]
    fn distinguish_decls() {
        for src in ["", " /* comment */ ", "1 + 2"] {
            let tokens = Lexer::new(src).collect::<Result<Vec<_>, _>>().unwrap();
            assert!(Parser::parse_decl(src, &tokens).unwrap().is_none(), "{src}");
        }
        let src = "const ";
        let tokens = Lexer::new(src).collect::<Result<Vec<_>, _>>().unwrap();
        assert!(Parser::parse_decl(src, &tokens).is_err());
    }

    #[test]
    fn parse_decl_cases() {
        for (name, src, members, dims) in [
            ("scalar", " float value ", 1, vec![]),
            ("matrix", "row_major matrix < float, 2, 3 > value", 1, vec![]),
            ("arrays", "float4 value [ 2 ] [ 3 ], other", 2, vec![Some(2), Some(3)]),
            ("annotation", "float4 value : packoffset ( c1 . y )", 1, vec![]),
            ("alias", "Real value", 1, vec![]),
            ("initializer", "float4 value = {1, 2, 3, 4}, other = 0", 2, vec![]),
            ("trivia", "/*type*/ float /*name*/ value /*tail*/", 1, vec![]),
        ] {
            let tokens = Lexer::new(src).collect::<Result<Vec<_>, _>>().unwrap();
            let decl = Parser::parse_decl(src, &tokens)
                .unwrap_or_else(|err| panic!("{name}: {src:?}: {err} at {}", err.offset))
                .unwrap();
            assert_eq!(decl.members.len(), members, "{name}: members");
            assert_eq!(decl.members[0].dims, dims, "{name}: dimensions");
            assert_eq!(decl.members[0].name, "value", "{name}: name");
        }
        for (name, src) in [
            ("name", "float"),
            ("member", "float 1"),
            ("trailing comma", "float value,"),
            ("array close", "float value[2"),
            ("annotation", "float value:"),
            ("annotation close", "float value:packoffset(c0"),
            ("initializer", "float value ="),
            ("type arguments", "matrix<float, 2> value"),
        ] {
            let tokens = Lexer::new(src).collect::<Result<Vec<_>, _>>().unwrap();
            assert!(Parser::parse_decl(src, &tokens).is_err(), "{name}: {src:?}");
        }
    }

    #[test]
    fn parse_fragments() {
        let src = concat!(
            "cbuffer Params : register(b0) { float4 color : packoffset(c0); }; ",
            "[numthreads(1, 1, 1)] void main() { unused(); }"
        );
        let tokens = Lexer::new(src).collect::<Result<Vec<_>, _>>().unwrap();
        let output = Parser::new(src, &tokens).parse().unwrap();
        assert_eq!(output.groups.len(), 1, "cbuffer");
        assert_eq!(output.funcs.len(), 1, "function");
        assert!(output.funcs[0].is_void, "return type");
        assert!(output.funcs[0].has_numthreads, "attribute");

        let src = " /* prefix */ float value = first + second; // tail";
        let tokens = Lexer::new(src).collect::<Result<Vec<_>, _>>().unwrap();
        let span = Parser::new(src, &tokens)
            .parse_expr_at(src.find("first").unwrap())
            .unwrap();
        assert_eq!(&src[span.st..span.ed], "first + second", "expression");
        let span = Parser::new(src, &tokens).parse_stmt_at(0).unwrap();
        assert_eq!(&src[span.st..span.ed], "float value = first + second;", "statement");
        assert!(
            Parser::new(src, &tokens).parse_expr_at(src.len()).is_err(),
            "expression at end"
        );
        assert!(
            Parser::new(src, &tokens).parse_stmt_at(src.len()).is_err(),
            "statement at end"
        );
    }
}
