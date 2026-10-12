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

static KEYWORDS: [&str; 168] = [
    "AppendStructuredBuffer",
    "asm",
    "asm_fragment",
    "BlendState",
    "bool",
    "break",
    "Buffer",
    "ByteAddressBuffer",
    "case",
    "cbuffer",
    "centroid",
    "class",
    "column_major",
    "compile",
    "compile_fragment",
    "CompileShader",
    "const",
    "continue",
    "ComputeShader",
    "ConsumeStructuredBuffer",
    "default",
    "DepthStencilState",
    "DepthStencilView",
    "discard",
    "do",
    "double",
    "DomainShader",
    "dword",
    "else",
    "export",
    "extern",
    "false",
    "float",
    "for",
    "fxgroup",
    "GeometryShader",
    "globallycoherent",
    "groupshared",
    "half",
    "HullShader",
    "if",
    "in",
    "inline",
    "inout",
    "InputPatch",
    "int",
    "interface",
    "line",
    "lineadj",
    "linear",
    "LineStream",
    "matrix",
    "min16float",
    "min10float",
    "min16int",
    "min12int",
    "min16uint",
    "namespace",
    "nointerpolation",
    "noperspective",
    "NULL",
    "out",
    "OutputPatch",
    "packoffset",
    "pass",
    "pixelfragment",
    "PixelShader",
    "point",
    "PointStream",
    "precise",
    "RasterizerState",
    "RasterizerOrderedBuffer",
    "RasterizerOrderedByteAddressBuffer",
    "RasterizerOrderedStructuredBuffer",
    "RasterizerOrderedTexture1D",
    "RasterizerOrderedTexture1DArray",
    "RasterizerOrderedTexture2D",
    "RasterizerOrderedTexture2DArray",
    "RasterizerOrderedTexture3D",
    "RenderTargetView",
    "return",
    "register",
    "row_major",
    "RWBuffer",
    "RWByteAddressBuffer",
    "RWStructuredBuffer",
    "RWTexture1D",
    "RWTexture1DArray",
    "RWTexture2D",
    "RWTexture2DArray",
    "RWTexture3D",
    "sample",
    "sampler",
    "SamplerState",
    "SamplerComparisonState",
    "sampler1D",
    "sampler2D",
    "sampler3D",
    "samplerCUBE",
    "sampler_state",
    "shared",
    "snorm",
    "stateblock",
    "stateblock_state",
    "static",
    "string",
    "struct",
    "switch",
    "StructuredBuffer",
    "tbuffer",
    "technique",
    "technique10",
    "technique11",
    "texture",
    "Texture1D",
    "Texture1DArray",
    "Texture2D",
    "Texture2DArray",
    "Texture2DMS",
    "Texture2DMSArray",
    "Texture3D",
    "TextureCube",
    "TextureCubeArray",
    "true",
    "typedef",
    "triangle",
    "triangleadj",
    "TriangleStream",
    "uint",
    "uniform",
    "unorm",
    "unsigned",
    "vector",
    "vertexfragment",
    "VertexShader",
    "void",
    "volatile",
    "while",
    "auto",
    "catch",
    "char",
    "const_cast",
    "delete",
    "dynamic_cast",
    "enum",
    "explicit",
    "friend",
    "goto",
    "long",
    "mutable",
    "new",
    "operator",
    "private",
    "protected",
    "public",
    "reinterpret_cast",
    "short",
    "signed",
    "sizeof",
    "static_cast",
    "template",
    "this",
    "throw",
    "try",
    "typename",
    "union",
    "using",
    "virtual",
];

pub struct Parser<'a, 'tokens> {
    src: &'a str,
    tokens: &'tokens [Token<'a>],
    curr: usize,
    output: Analysis<'a, 'tokens>,
}

impl<'a, 'tokens> Parser<'a, 'tokens> {
    #[must_use]
    pub fn new(src: &'a str, tokens: &'tokens [Token<'a>]) -> Self {
        Self {
            src,
            tokens,
            curr: 0,
            output: Analysis::default(),
        }
    }

    pub fn parse(mut self) -> Result<Analysis<'a, 'tokens>, Error> {
        while self.curr < self.tokens.len() {
            self.read()?;
        }

        Ok(self.output)
    }

    #[allow(clippy::too_many_lines)]
    pub(super) fn parse_declaration(src: &'a str, tokens: &[Token<'a>]) -> Result<Option<Declaration<'a>>, Error> {
        let mut curr = 0;
        let mut modifiers = Vec::new();

        while let Some(token) = tokens.get(curr) {
            let TokenKind::Modifier(modifier) = token.kind else {
                break;
            };
            modifiers.push(modifier);
            curr += 1;
        }

        let (ty, len) = Self::parse_type(src, &tokens[curr..]);
        let Some(token) = tokens.get(curr) else {
            return Ok(None);
        };

        let ty = ty.ok_or(Error {
            offset: token.span.st,
            message: "invalid declaration type",
        })?;
        let mut decl = Declaration {
            ty,
            span: Span {
                st: tokens[0].span.st,
                ed: tokens.last().unwrap().span.ed,
            },
            modifiers,
            members: Vec::new(),
        };

        let error = |curr: usize, message| Error {
            offset: tokens.get(curr).map_or(decl.span.ed, |token| token.span.st),
            message,
        };

        curr += len;
        loop {
            let member = tokens
                .get(curr)
                .ok_or_else(|| error(curr, "missing declaration member"))?;
            let TokenKind::Ident(name) = member.kind else {
                return Err(error(curr, "invalid declaration member"));
            };

            curr += 1;
            let mut dims = Vec::new();

            while tokens.get(curr).is_some_and(|token| token.kind == TokenKind::LBracket) {
                let ed = Self::find_group(tokens, curr).ok_or_else(|| error(curr, "unclosed array dimension"))?;
                dims.push(match &tokens[curr + 1..ed] {
                    [
                        Token {
                            kind: TokenKind::Number(text),
                            ..
                        },
                    ] => text.parse::<usize>().ok(),
                    _ => None,
                });
                curr = ed + 1;
            }

            let annotation = if tokens.get(curr).is_some_and(|token| token.kind == TokenKind::Colon) {
                curr += 1;
                let token = tokens
                    .get(curr)
                    .ok_or_else(|| error(curr, "missing declaration annotation"))?;
                let (TokenKind::Ident(name) | TokenKind::Keyword(name @ ("packoffset" | "register"))) = token.kind
                else {
                    return Err(error(curr, "invalid declaration annotation"));
                };

                curr += 1;

                let args = if tokens.get(curr).is_some_and(|token| token.kind == TokenKind::LParen) {
                    let ed =
                        Self::find_group(tokens, curr).ok_or_else(|| error(curr, "unclosed declaration annotation"))?;
                    let args = &src[tokens[curr].span.ed..tokens[ed].span.st];
                    curr = ed + 1;
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
                if tokens.get(curr).is_none_or(|token| token.kind == TokenKind::Comma) {
                    return Err(error(curr, "missing declaration initializer"));
                }

                while let Some(token) = tokens.get(curr).filter(|token| token.kind != TokenKind::Comma) {
                    match token.kind {
                        TokenKind::LParen | TokenKind::LBracket | TokenKind::LBrace => {
                            let ed = Self::find_group(tokens, curr)
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
        }

        Ok(Some(decl))
    }

    pub(super) fn parse_stmt(&mut self, st: usize) -> Result<Span, Error> {
        self.curr = self.tokens.partition_point(|token| token.span.st < st);
        self.skip_trivia();
        let st = self.curr;
        self.stmt(0)?;
        Ok(Span {
            st: self.tokens[st].span.st,
            ed: self.tokens[self.curr - 1].span.ed,
        })
    }

    pub(super) fn parse_expr(&mut self, st: usize) -> Result<Span, Error> {
        self.curr = self.tokens.partition_point(|token| token.span.st < st);
        self.expr(0, 0, false)
    }

    #[must_use]
    pub(super) fn read_directive<'token>(token: &'token Token<'_>) -> Option<(&'token str, &'token str)> {
        let TokenKind::Directive(text) = &token.kind else {
            return None;
        };

        let text = text.strip_prefix('#')?.trim_start();
        let ed = text
            .find(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_')
            .unwrap_or(text.len());

        Some((&text[..ed], text[ed..].trim_start()))
    }

    #[must_use]
    pub(super) fn find_group(tokens: &[Token<'_>], at: usize) -> Option<usize> {
        let pair = |kind: &TokenKind<'_>| match kind {
            TokenKind::LParen => Some((b'(', true)),
            TokenKind::LBracket => Some((b'[', true)),
            TokenKind::LBrace => Some((b'{', true)),
            TokenKind::RParen => Some((b'(', false)),
            TokenKind::RBracket => Some((b'[', false)),
            TokenKind::RBrace => Some((b'{', false)),
            _ => None,
        };
        let (mut group, is_open) = pair(&tokens.get(at)?.kind)?;
        let mut expected = Vec::new();
        let mut curr = at;
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

    #[allow(clippy::too_many_lines)]
    fn expr(&mut self, depth: usize, min: u8, should_stop_colon: bool) -> Result<Span, Error> {
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

                while tokens.get(type_st).is_some_and(|token| {
                    matches!(
                        token.kind,
                        TokenKind::Whitespace | TokenKind::Newline | TokenKind::Comment(_)
                    )
                }) {
                    type_st += 1;
                }

                let (ty, len) = Self::parse_type(src, &tokens[type_st..]);
                let mut type_ed = type_st + len;

                while ty.is_some() && tokens.get(type_ed).is_some() {
                    match tokens[type_ed].kind {
                        TokenKind::Whitespace | TokenKind::Newline | TokenKind::Comment(_) => type_ed += 1,
                        TokenKind::LBracket => {
                            type_ed = Self::find_group(tokens, type_ed)
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
                            | TokenKind::Vector
                            | TokenKind::Matrix
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
                    .find(|token| {
                        !matches!(
                            token.kind,
                            TokenKind::Whitespace | TokenKind::Newline | TokenKind::Comment(_)
                        )
                    })
                    .is_none_or(|token| !can_start(token));
                is_cast = ty.is_some()
                    && tokens.get(type_ed).is_some_and(|token| token.kind == TokenKind::RParen)
                    && (!matches!(ty, Some(Type::Named(_))) || !is_postfix)
                    && tokens.get(self.curr).is_some_and(can_start);

                if !is_cast {
                    self.curr = group_st + 1;
                    self.expr_group(&TokenKind::RParen, depth, false)?;
                }
            }
            Some(TokenKind::LBrace) => {
                self.curr += 1;
                self.expr_group(&TokenKind::RBrace, depth, true)?;
            }
            Some(TokenKind::Vector | TokenKind::Matrix) => {
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
                if !Lexer::is_number(text) {
                    return Err(error(self.curr, "invalid numeric literal"));
                }

                self.curr += 1;
            }
            _ => return Err(error(self.curr, "invalid expression operand")),
        }

        let mut ed = tokens[self.curr - 1].span.ed;
        self.skip_trivia();

        if is_cast {
            ed = self.expr(depth + 1, 13, should_stop_colon)?.ed;
        }
        loop {
            self.skip_trivia();

            match tokens.get(self.curr).map(|token| &token.kind) {
                Some(TokenKind::LParen) => {
                    self.curr += 1;
                    self.expr_group(&TokenKind::RParen, depth, true)?;
                    ed = tokens[self.curr - 1].span.ed;
                }
                Some(TokenKind::LBracket) => {
                    self.curr += 1;
                    self.expr_group(&TokenKind::RBracket, depth, false)?;
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
                Some(TokenKind::Other('?')) if min <= 2 => {
                    self.curr += 1;
                    self.expr(depth + 1, 0, true)?;
                    self.skip_trivia();

                    if tokens.get(self.curr).map(|token| &token.kind) != Some(&TokenKind::Colon) {
                        return Err(error(self.curr, "missing conditional colon"));
                    }

                    self.curr += 1;
                    ed = self.expr(depth + 1, 2, should_stop_colon)?.ed;
                }
                Some(TokenKind::Colon) if should_stop_colon => break,
                Some(
                    TokenKind::Assign
                    | TokenKind::Other('+' | '-' | '*' | '/' | '%' | '&' | '|' | '^' | '<' | '>' | '!'),
                ) => {
                    let token_st = tokens[self.curr].span.st;
                    let op = [
                        "<<=", ">>=", "&&", "||", "==", "!=", "<=", ">=", "<<", ">>", "+=", "-=", "*=", "/=", "%=",
                        "&=", "|=", "^=", "=", "+", "-", "*", "/", "%", "&", "|", "^", "<", ">",
                    ]
                    .into_iter()
                    .find(|op| src[token_st..].starts_with(op))
                    .ok_or_else(|| error(self.curr, "invalid expression operator"))?;
                    let priority = match op {
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

                    if priority < min {
                        break;
                    }

                    let op_ed = token_st + op.len();
                    while tokens.get(self.curr).is_some_and(|token| token.span.st < op_ed) {
                        self.curr += 1;
                    }
                    ed = self
                        .expr(depth + 1, priority + u8::from(priority != 1), should_stop_colon)?
                        .ed;
                }
                _ => break,
            }
        }

        Ok(Span { st, ed })
    }

    fn expr_group(&mut self, closing: &TokenKind<'a>, depth: usize, can_be_empty: bool) -> Result<(), Error> {
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
            self.expr(depth + 1, 0, false)?;
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

    fn skip_trivia(&mut self) {
        while self.tokens.get(self.curr).is_some_and(|token| {
            matches!(
                token.kind,
                TokenKind::Whitespace | TokenKind::Newline | TokenKind::Comment(_)
            )
        }) {
            self.curr += 1;
        }
    }

    #[allow(clippy::too_many_lines)]
    fn stmt(&mut self, depth: usize) -> Result<(), Error> {
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
            self.curr = Self::find_group(tokens, self.curr)
                .ok_or_else(|| error(self.curr, "unclosed statement attribute"))?
                + 1;
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

                self.curr = Self::find_group(tokens, self.curr)
                    .ok_or_else(|| error(self.curr, "unclosed control condition"))?
                    + 1;
            }

            self.stmt(depth + 1)?;
            let body_ed = self.curr;
            self.skip_trivia();

            if is_if && tokens.get(self.curr).map(|token| &token.kind) == Some(&TokenKind::Keyword("else")) {
                self.curr += 1;
                self.stmt(depth + 1)?;
            } else if is_do {
                if tokens.get(self.curr).map(|token| &token.kind) != Some(&TokenKind::Keyword("while")) {
                    return Err(error(self.curr, "missing while after do"));
                }

                self.curr += 1;
                self.skip_trivia();

                if tokens.get(self.curr).map(|token| &token.kind) != Some(&TokenKind::LParen) {
                    return Err(error(self.curr, "missing control condition"));
                }

                self.curr = Self::find_group(tokens, self.curr)
                    .ok_or_else(|| error(self.curr, "unclosed control condition"))?
                    + 1;
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
                    self.curr = Self::find_group(tokens, self.curr)
                        .ok_or_else(|| error(self.curr, "unclosed statement group"))?
                        + 1;
                }
                TokenKind::LBrace => {
                    self.curr = Self::find_group(tokens, self.curr)
                        .ok_or_else(|| error(self.curr, "unclosed statement block"))?
                        + 1;
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
    fn read(&mut self) -> Result<(), Error> {
        let tokens = self.tokens;
        let kind = |i: usize| tokens.get(i).map(|token| &token.kind);
        let st = self.curr;

        if tokens
            .get(st)
            .is_some_and(|token| token.kind == TokenKind::Keyword("typedef"))
            && tokens
                .get(st + 1)
                .is_some_and(|token| token.kind == TokenKind::Keyword("struct"))
        {
            self.curr += 1;
            self.stmt(0)?;
            return Ok(());
        }

        let mut is_initializer = false;
        while let Some(token) = tokens.get(self.curr) {
            let curr = self.curr;
            if let Some((name, rest)) = Self::read_directive(token) {
                self.curr += 1;
                self.output.directives.push(Directive {
                    name,
                    rest,
                    span: token.span,
                });
                return Ok(());
            }

            if !is_initializer
                && let TokenKind::Ident(name) = token.kind
                && curr > 0
                && matches!(
                    kind(curr - 1),
                    Some(
                        TokenKind::Ident(_)
                            | TokenKind::Type(_)
                            | TokenKind::Vector
                            | TokenKind::Matrix
                            | TokenKind::Keyword("void")
                            | TokenKind::Other('>')
                    )
                )
                && kind(curr + 1) == Some(&TokenKind::LParen)
                && let Some(args_ed) = Self::find_group(tokens, curr + 1)
            {
                let mut ed = args_ed + 1;
                if kind(ed) == Some(&TokenKind::Colon) {
                    ed += 1;
                    if matches!(kind(ed), Some(TokenKind::Ident(_))) {
                        ed += 1;
                    }
                }

                if kind(ed) == Some(&TokenKind::LBrace)
                    && let Some(body_ed) = Self::find_group(tokens, ed)
                {
                    self.curr = body_ed + 1;
                    self.output.functions.push(Function {
                        name,
                        span: token.span,
                        is_void: kind(curr - 1) == Some(&TokenKind::Keyword("void")),
                        has_numthreads: Self::find_numthreads(tokens, curr - 1).is_some(),
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
                    self.curr = Self::find_group(tokens, curr).ok_or(Error {
                        offset: token.span.st,
                        message: "unclosed declaration initializer",
                    })? + 1;
                    continue;
                }
                TokenKind::LBrace => {
                    let ed = Self::find_group(tokens, curr).ok_or(Error {
                        offset: token.span.st,
                        message: "unclosed declaration block",
                    })?;
                    self.curr = ed + 1;
                    self.output.groups.push(Span {
                        st: token.span.st,
                        ed: tokens[ed].span.ed,
                    });
                    return Ok(());
                }
                TokenKind::LParen | TokenKind::LBracket => {
                    self.curr = Self::find_group(tokens, curr).ok_or(Error {
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

                    if let Some(declaration) =
                        Self::parse_declaration(self.src, &tokens[st + usize::from(is_alias)..curr])?
                        && !is_alias
                    {
                        self.output.declarations.push(declaration);
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

        if let Some(declaration) = Self::parse_declaration(self.src, &tokens[st..])? {
            self.output.declarations.push(declaration);
        }

        Ok(())
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
            while tokens.get(curr).is_some_and(|token| {
                matches!(
                    token.kind,
                    TokenKind::Whitespace | TokenKind::Newline | TokenKind::Comment(_)
                )
            }) {
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
                        let Some(ed) = Self::find_group(tokens, curr) else {
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

        let is_matrix = match token.kind {
            TokenKind::Vector => false,
            TokenKind::Matrix => true,
            _ => return (None, 1),
        };

        let mut args = [token; 8];
        let mut indices = [0; 8];
        let mut count = 0;

        for (index, token) in tokens
            .iter()
            .enumerate()
            .filter(|(_, token)| {
                !matches!(
                    token.kind,
                    TokenKind::Whitespace | TokenKind::Newline | TokenKind::Comment(_)
                )
            })
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
                Some(if is_matrix {
                    Type::Matrix {
                        scalar,
                        rows: 4,
                        cols: 4,
                    }
                } else {
                    Type::Vector { scalar, cols: 4 }
                }),
                1,
            );
        }

        let len = if is_matrix { 8 } else { 6 };

        if count < len {
            return (None, 1);
        }

        let TokenKind::Type(Type::Scalar(scalar)) = args[2].kind else {
            return (None, 1);
        };

        if args[3].kind != TokenKind::Comma
            || args[len - 1].kind != TokenKind::Other('>')
            || (is_matrix && args[5].kind != TokenKind::Comma)
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
            (if is_matrix { parse(args[4]) } else { Some(1) })
                .zip(parse(args[if is_matrix { 6 } else { 4 }]))
                .map(|(rows, cols)| {
                    if is_matrix {
                        Type::Matrix { scalar, rows, cols }
                    } else {
                        Type::Vector { scalar, cols }
                    }
                }),
            indices[len - 1] + 1,
        )
    }

    fn find_numthreads(tokens: &[Token<'_>], mut curr: usize) -> Option<Span> {
        let kind = |index: usize| tokens.get(index).map(|token| &token.kind);

        while curr > 0 && matches!(kind(curr - 1), Some(TokenKind::Ident(_) | TokenKind::Modifier(_))) {
            curr -= 1;
        }

        while curr > 0 && kind(curr - 1) == Some(&TokenKind::RBracket) {
            let group_st = Self::find_group(tokens, curr - 1)?;

            if kind(group_st + 1) == Some(&TokenKind::Ident("numthreads")) {
                return Some(tokens[group_st + 1].span);
            }

            curr = group_st;
        }

        None
    }
}

pub(super) struct Preprocessor {
    stack: Vec<Frame>,
    macros: std::collections::HashMap<String, Macro>,
    pub(super) is_row_major: Option<bool>,
    pub(super) has_packing: bool,
}

impl Preprocessor {
    pub(super) fn new() -> Self {
        Self {
            stack: Vec::new(),
            macros: std::collections::HashMap::new(),
            is_row_major: Some(false),
            has_packing: false,
        }
    }

    #[allow(clippy::too_many_lines)]
    pub(super) fn process(&mut self, token: &Token<'_>) -> Result<Option<Action>, Error> {
        let Some((name, rest)) = Parser::read_directive(token) else {
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
                let is_taken = if is_parent_active == Some(false) {
                    Some(false)
                } else {
                    is_met(self)
                };
                self.stack.push(Frame {
                    st: token.span.st,
                    is_parent_active,
                    is_taken,
                    is_active: combine(is_parent_active, is_taken),
                    has_else: false,
                });

                if is_parent_active != Some(false) && is_taken.is_none() {
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

                let is_taken = if frame.is_parent_active == Some(false) || frame.is_taken == Some(true) {
                    Some(false)
                } else {
                    is_met(self)
                };
                let frame = self.stack.last_mut().unwrap();
                frame.is_active = combine(
                    frame.is_parent_active,
                    combine(frame.is_taken.map(|val| !val), is_taken),
                );
                frame.is_taken = match (frame.is_taken, is_taken) {
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

                frame.is_active = combine(frame.is_parent_active, frame.is_taken.map(|val| !val));
                frame.is_taken = Some(true);
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
                if key
                    .bytes()
                    .next()
                    .is_some_and(|byte| byte.is_ascii_alphabetic() || byte == b'_')
                    && key.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
                {
                    let val = if self.is_active().is_none() {
                        Macro::Unknown
                    } else if name == "undef" {
                        Macro::Undefined
                    } else if rest[ed..].starts_with('(') {
                        Macro::Function
                    } else {
                        Macro::Value(rest[ed..].trim().to_owned())
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
                        .filter(|token| {
                            token.as_ref().map_or(true, |token| {
                                !matches!(
                                    token.kind,
                                    TokenKind::Whitespace | TokenKind::Newline | TokenKind::Comment(_)
                                )
                            })
                        })
                        .collect::<Result<Vec<_>, _>>()
                        .ok()
                });

                if let Some(args) = args {
                    if args
                        .first()
                        .is_some_and(|token| matches!(token.kind, TokenKind::Ident("pack_matrix")))
                    {
                        action = Action::MatrixPacking;
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
                                        kind: TokenKind::Modifier("row_major"),
                                        ..
                                    },
                                    Token {
                                        kind: TokenKind::RParen,
                                        ..
                                    },
                                ] => Some(true),
                                [
                                    _,
                                    Token {
                                        kind: TokenKind::LParen,
                                        ..
                                    },
                                    Token {
                                        kind: TokenKind::Modifier("column_major"),
                                        ..
                                    },
                                    Token {
                                        kind: TokenKind::RParen,
                                        ..
                                    },
                                ] => Some(false),
                                _ => None,
                            }
                        } else {
                            None
                        };
                    }
                } else {
                    action = Action::MatrixPacking;
                    self.is_row_major = None;
                }
            }
            _ => action = Action::Keep,
        }

        Ok(Some(action))
    }

    pub(super) fn expand(&self, src: &str) -> Option<String> {
        fn expand(preprocessor: &Preprocessor, src: &str, depth: usize) -> Option<String> {
            if depth >= 32 {
                return None;
            }

            let mut result = String::new();
            let mut curr = 0;

            for token in Lexer::new(src) {
                let token = token.ok()?;
                let name = &src[token.span.st..token.span.ed];
                if !name
                    .bytes()
                    .next()
                    .is_some_and(|byte| byte.is_ascii_alphabetic() || byte == b'_')
                    || !name.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
                {
                    continue;
                }

                match preprocessor.macros.get(name) {
                    Some(Macro::Value(val)) => {
                        result.push_str(&src[curr..token.span.st]);
                        result.push(' ');
                        result.push_str(&expand(preprocessor, val, depth + 1)?);
                        result.push(' ');
                        curr = token.span.ed;
                    }
                    Some(Macro::Function | Macro::Unknown) => return None,
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
    pub(super) fn evaluate(&self, src: &str) -> Option<i32> {
        #[allow(clippy::too_many_lines)]
        fn parse_expr(src: &str, tokens: &[Token<'_>], curr: &mut usize, min: u8, depth: usize) -> Option<i32> {
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

                    let text = text.strip_suffix(['l', 'L']).unwrap_or(text);
                    if let Some(text) = text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
                        i32::from_str_radix(text, 16).ok()?
                    } else if text.len() > 1 && text.starts_with('0') {
                        i32::from_str_radix(&text[1..], 8).ok()?
                    } else {
                        text.parse().ok()?
                    }
                }
                TokenKind::String => {
                    let text = &src[token.span.st..token.span.ed];
                    match text.as_bytes() {
                        [b'\'', val, b'\''] if val.is_ascii() => i32::from(*val),
                        _ => return None,
                    }
                }
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
                if token.kind == TokenKind::Other('?') && min <= 1 {
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

                let op = [
                    "||", "&&", "==", "!=", "<=", ">=", "<<", ">>", "|", "^", "&", "<", ">", "+", "-", "*", "/", "%",
                ]
                .into_iter()
                .find(|op| src[token.span.st..].starts_with(op));
                let Some(op) = op else { break };
                let priority = match op {
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

                if priority < min {
                    break;
                }

                let ed = token.span.st + op.len();
                while tokens.get(*curr).is_some_and(|token| token.span.st < ed) {
                    *curr += 1;
                }

                let right = parse_expr(src, tokens, curr, priority + 1, depth + 1)?;
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
            .filter(|token| {
                token.as_ref().map_or(true, |token| {
                    !matches!(
                        token.kind,
                        TokenKind::Whitespace | TokenKind::Newline | TokenKind::Comment(_)
                    )
                })
            })
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
            .filter(|token| {
                token.as_ref().map_or(true, |token| {
                    !matches!(
                        token.kind,
                        TokenKind::Whitespace | TokenKind::Newline | TokenKind::Comment(_)
                    )
                })
            })
            .collect::<Result<Vec<_>, _>>()
            .ok()?;

        let mut curr = 0;
        let val = parse_expr(&text, &tokens, &mut curr, 0, 0)?;
        (curr == tokens.len()).then_some(val)
    }

    pub(super) fn finish(&self) -> Result<(), Error> {
        if let Some(frame) = self.stack.last() {
            return Err(Error {
                offset: frame.st,
                message: "unclosed HLSL conditional directive",
            });
        }

        Ok(())
    }

    pub(super) fn is_active(&self) -> Option<bool> {
        self.stack.last().map_or(Some(true), |frame| frame.is_active)
    }

    fn is_defined(&self, name: &str) -> Option<bool> {
        match self.macros.get(name) {
            Some(Macro::Value(_) | Macro::Function) => Some(true),
            Some(Macro::Undefined) => Some(false),
            Some(Macro::Unknown) | None => None,
        }
    }
}

#[derive(Clone)]
pub struct Lexer<'a> {
    src: &'a str,
    bytes: &'a [u8],
    curr: usize,
    can_read_directive: bool,
}

impl<'a> Lexer<'a> {
    #[must_use]
    pub fn new(src: &'a str) -> Self {
        Self {
            src,
            bytes: src.as_bytes(),
            curr: 0,
            can_read_directive: true,
        }
    }

    #[allow(clippy::too_many_lines)]
    fn read(&mut self) -> Result<Token<'a>, Error> {
        if self.can_read_directive && self.bytes[self.curr] == b'#' {
            let st = self.curr;
            let body_st = st + 1;
            let mut lexer = Self::new(&self.src[body_st..]);
            let mut ed = self.src.len();

            loop {
                lexer.can_read_directive = false;
                let Some(token) = lexer.next() else {
                    break;
                };
                let token = token.map_err(|mut err| {
                    err.offset += body_st;
                    err
                })?;

                if token.kind == TokenKind::Newline {
                    let nl = body_st + token.span.st;
                    if self.bytes[nl - 1] == b'\\' {
                        continue;
                    }

                    ed = nl;
                    break;
                }
            }

            self.curr = ed + usize::from(ed < self.src.len());
            self.can_read_directive = true;
            let mut directive = None;
            let mut curr = st;

            for (rel, _) in self.src[st..ed].match_indices('\n') {
                let nl = st + rel;
                if nl <= st || self.bytes[nl - 1] != b'\\' {
                    continue;
                }

                let directive = directive.get_or_insert_with(|| String::with_capacity(ed - st));
                directive.push_str(&self.src[curr..nl - 1]);
                curr = nl + 1;
            }

            let directive = directive.map_or_else(
                || Cow::Borrowed(&self.src[st..ed]),
                |mut directive| {
                    directive.push_str(&self.src[curr..ed]);
                    Cow::Owned(directive)
                },
            );

            return Ok(Token {
                kind: TokenKind::Directive(directive),
                span: Span { st, ed },
            });
        }

        let st = self.curr;
        Ok(Token {
            kind: if self.src[st..].starts_with("/*") {
                self.curr += 2;
                let content_st = self.curr;
                let rel = self.src[self.curr..].find("*/").ok_or(Error {
                    offset: st,
                    message: "unfinished HLSL comment",
                })?;
                let content_ed = self.curr + rel;
                self.curr += rel + 2;

                if self.src[st..self.curr].contains('\n') {
                    self.can_read_directive = true;
                }

                TokenKind::Comment(Comment {
                    text: &self.src[st..self.curr],
                    content: &self.src[content_st..content_ed],
                    bracket: Some(Span {
                        st: content_st,
                        ed: content_ed,
                    }),
                })
            } else if self.src[st..].starts_with("//") {
                self.curr += 2;
                loop {
                    let ed = self.curr + self.src[self.curr..].find('\n').unwrap_or(self.bytes.len() - self.curr);
                    if ed < self.bytes.len() && self.bytes[ed - 1] == b'\\' {
                        self.curr = ed + 1;
                    } else {
                        self.curr = ed;
                        break;
                    }
                }

                TokenKind::Comment(Comment {
                    text: &self.src[st..self.curr],
                    content: &self.src[st + 2..self.curr],
                    bracket: None,
                })
            } else if self.bytes[st] == b'\n' {
                self.curr += 1;
                self.can_read_directive = true;
                TokenKind::Newline
            } else if self.bytes[st].is_ascii_whitespace() {
                while self.curr < self.bytes.len()
                    && self.bytes[self.curr].is_ascii_whitespace()
                    && self.bytes[self.curr] != b'\n'
                {
                    self.curr += 1;
                }

                TokenKind::Whitespace
            } else if matches!(self.bytes[st], b'\'' | b'"') {
                self.can_read_directive = false;
                self.curr += 1;
                loop {
                    if self.curr >= self.bytes.len() || self.bytes[self.curr] == b'\n' {
                        return Err(Error {
                            offset: st,
                            message: "unfinished HLSL string",
                        });
                    }

                    if self.bytes[self.curr] == b'\\' {
                        self.curr = (self.curr + 2).min(self.bytes.len());
                    } else if self.bytes[self.curr] == self.bytes[st] {
                        self.curr += 1;
                        break TokenKind::String;
                    } else {
                        self.curr += 1;
                    }
                }
            } else if self.bytes[st].is_ascii_digit()
                || (self.bytes[st] == b'.' && self.bytes.get(st + 1).is_some_and(u8::is_ascii_digit))
            {
                self.can_read_directive = false;
                let exponent = if self.src[st..].starts_with("0x") || self.src[st..].starts_with("0X") {
                    b'p'
                } else {
                    b'e'
                };

                self.curr += 1;
                while self.curr < self.bytes.len()
                    && (self.bytes[self.curr].is_ascii_alphanumeric()
                        || matches!(self.bytes[self.curr], b'_' | b'.')
                        || (matches!(self.bytes[self.curr], b'+' | b'-')
                            && self.bytes[self.curr - 1].to_ascii_lowercase() == exponent))
                {
                    self.curr += 1;
                }

                TokenKind::Number(&self.src[st..self.curr])
            } else if self.bytes[st].is_ascii_alphabetic() || self.bytes[st] == b'_' || self.bytes[st] >= 0x80 {
                self.can_read_directive = false;

                while self.curr < self.bytes.len()
                    && (self.bytes[self.curr].is_ascii_alphanumeric()
                        || self.bytes[self.curr] == b'_'
                        || self.bytes[self.curr] >= 0x80)
                {
                    self.curr += 1;
                }

                let text = &self.src[st..self.curr];
                match text {
                    "vector" => TokenKind::Vector,
                    "matrix" => TokenKind::Matrix,
                    "row_major" | "column_major" | "static" | "groupshared" | "const" | "uniform" | "volatile"
                    | "precise" | "extern" | "shared" | "nointerpolation" | "linear" | "centroid" | "noperspective"
                    | "sample" | "snorm" | "unorm" | "globallycoherent" | "in" | "out" | "inout" | "inline" => {
                        TokenKind::Modifier(text)
                    }
                    _ => {
                        let parse_dim = |text: &str| match *text.as_bytes() {
                            [ch @ b'1'..=b'4'] => Some(usize::from(ch - b'0')),
                            _ => None,
                        };
                        let ty = SCALARS.iter().find_map(|&(name, size)| {
                            let suffix = text.strip_prefix(name)?;
                            let scalar = Scalar { name, size };
                            let ty = match suffix.split_once('x') {
                                Some((rows, cols)) => Type::Matrix {
                                    scalar,
                                    rows: parse_dim(rows)?,
                                    cols: parse_dim(cols)?,
                                },
                                None if suffix.is_empty() => Type::Scalar(scalar),
                                None => Type::Vector {
                                    scalar,
                                    cols: parse_dim(suffix)?,
                                },
                            };
                            Some(ty)
                        });

                        if let Some(ty) = ty {
                            TokenKind::Type(ty)
                        } else if KEYWORDS.contains(&text) {
                            TokenKind::Keyword(text)
                        } else {
                            TokenKind::Ident(text)
                        }
                    }
                }
            } else {
                self.can_read_directive = false;
                self.curr += 1;
                match self.bytes[st] {
                    b'{' => TokenKind::LBrace,
                    b'}' => TokenKind::RBrace,
                    b'(' => TokenKind::LParen,
                    b')' => TokenKind::RParen,
                    b'[' => TokenKind::LBracket,
                    b']' => TokenKind::RBracket,
                    b'=' => TokenKind::Assign,
                    b';' => TokenKind::Semicolon,
                    b':' => TokenKind::Colon,
                    b',' => TokenKind::Comma,
                    b'.' => TokenKind::Dot,
                    byte => TokenKind::Other(char::from(byte)),
                }
            },
            span: Span { st, ed: self.curr },
        })
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

enum Macro {
    Value(String),
    Function,
    Undefined,
    Unknown,
}

struct Frame {
    st: usize,
    is_parent_active: Option<bool>,
    is_taken: Option<bool>,
    is_active: Option<bool>,
    has_else: bool,
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
            assert_eq!(output.declarations[0].ty, Type::Named(ty), "{src}");
            assert_eq!(output.declarations[0].members[0].name, "value");
            assert_eq!(output.declarations[0].members[0].annotation.unwrap().args, Some("u0"));
        }

        let src = "typedef float Real; typedef float4 Color; typedef Real Array[2]; \
            typedef struct Element { float value; } Item; Real value; Color main() : SV_Target { return 0; }";
        let tokens = Lexer::new(src)
            .map(Result::unwrap)
            .filter(|token| token.kind != TokenKind::Whitespace)
            .collect::<Vec<_>>();
        let output = Parser::new(src, &tokens).parse().unwrap();
        assert_eq!(output.declarations.len(), 1);
        assert_eq!(output.declarations[0].ty, Type::Named("Real"));
        assert_eq!(output.functions[0].name, "main");
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
            let span = Parser::new(&src, &tokens).parse_expr(0).unwrap();
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
                Parser::new(value, &tokens).parse_expr(0).unwrap().ed,
                value.len(),
                "{value}"
            );
        }

        for value in [
            "1e+", "1e-", "0x", "0xGG", "1.2.3", "1_000", "08", "0x1p2", "1.0u", "1ff",
        ] {
            let tokens = Lexer::new(value).collect::<Result<Vec<_>, _>>().unwrap();
            let err = Parser::new(value, &tokens).parse_expr(0).unwrap_err();
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
            let span = Parser::new(&src, &tokens).parse_expr(0).unwrap();
            assert_eq!(&src[span.st..span.ed], value);
        }

        let src = std::iter::repeat_n("1", 500).collect::<Vec<_>>().join("+");
        let tokens = Lexer::new(&src).collect::<Result<Vec<_>, _>>().unwrap();
        assert_eq!(Parser::new(&src, &tokens).parse_expr(0).unwrap().ed, src.len());

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
            assert!(Parser::new(src, &tokens).parse_expr(0).is_err(), "{src}");
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
            let span = Parser::new(&src, &tokens).parse_stmt(0).unwrap();
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
            assert!(Parser::new(src, &tokens).parse_stmt(0).is_err(), "{src}");
        }
    }

    #[test]
    fn find_groups() {
        let tokens = Lexer::new("({[]})").collect::<Result<Vec<_>, _>>().unwrap();
        assert_eq!(Parser::find_group(&tokens, 0), Some(5));
        assert_eq!(Parser::find_group(&tokens, 5), Some(0));
        assert_eq!(Parser::find_group(&tokens, 1), Some(4));
        assert_eq!(Parser::find_group(&tokens, 4), Some(1));
        let tokens = Lexer::new("([)]").collect::<Result<Vec<_>, _>>().unwrap();
        assert_eq!(Parser::find_group(&tokens, 0), None);
        assert_eq!(Parser::find_group(&tokens, 3), None);
    }

    fn tokenize(src: &str) -> Vec<Token<'_>> {
        Lexer::new(src)
            .filter(|token| {
                token.as_ref().map_or(true, |token| {
                    !matches!(
                        token.kind,
                        TokenKind::Whitespace | TokenKind::Newline | TokenKind::Comment(_)
                    )
                })
            })
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
            TokenKind::Type(Type::Vector {
                scalar: Scalar { name: "float", size: 4 },
                cols: 4,
            })
        );
        assert_eq!(tokens[2].kind, TokenKind::Ident("value"));
        assert_eq!(tokens[3].kind, TokenKind::Vector);
        assert_eq!(tokens[4].kind, TokenKind::Matrix);
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
        assert_eq!(output.declarations.len(), 1);
        assert_eq!(output.declarations[0].members[0].name, "value");
        assert_eq!(output.directives.len(), 1);
        assert_eq!(output.groups.len(), 1);
        let span = output.groups[0];
        assert_eq!(&src[span.st..span.ed], "{ float amount; }");
        assert_eq!(
            output
                .functions
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
        let declaration = &output.declarations[0];
        assert_eq!(
            declaration.ty,
            Type::Matrix {
                scalar: Scalar { name: "float", size: 4 },
                rows: 2,
                cols: 3
            }
        );
        assert_eq!(declaration.modifiers, ["row_major"]);
        assert_eq!(declaration.members.len(), 2);
        assert_eq!(declaration.members[0].name, "m");
        assert_eq!(declaration.members[0].dims, [Some(2)]);
        assert!(declaration.members[1].dims.is_empty());
        let src = "float a : TEXCOORD0, b : packoffset(c1.y), c : register(c2) = 0;";
        let tokens = tokenize(src);
        let output = Parser::new(src, &tokens).parse().unwrap();

        for (member, name, args) in [
            (&output.declarations[0].members[0], "TEXCOORD0", None),
            (&output.declarations[0].members[1], "packoffset", Some("c1.y")),
            (&output.declarations[0].members[2], "register", Some("c2")),
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
            assert_eq!(output.declarations[0].members[0].dims, dims);
        }
    }

    #[test]
    fn reject_declarations() {
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
            let err = Parser::new(&src, &tokens).parse_expr(0).unwrap_err();
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
                    TokenKind::Type(Type::Vector {
                        scalar: Scalar { name, size },
                        cols
                    })
                );

                for rows in 1..=4 {
                    let src = format!("{name}{rows}x{cols}");
                    assert_eq!(
                        Lexer::new(&src).next().unwrap().unwrap().kind,
                        TokenKind::Type(Type::Matrix {
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
            ("#pragma pack_matrix(row_major)", Some(Action::MatrixPacking)),
            ("#include \"external.hlsl\"", Some(Action::Include)),
        ] {
            let token = Lexer::new(src).next().unwrap().unwrap();
            assert_eq!(preprocessor.process(&token).unwrap(), expected, "{src}");
        }
    }
}
