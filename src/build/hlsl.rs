#[derive(Clone)]
pub struct Lexer<'a> {
    src: &'a str,
    bytes: &'a [u8],
    curr: usize,
    is_line_start: bool,
    pub state: State,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Token<'a> {
    pub kind: TokenKind,
    pub text: &'a str,
    pub span: Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenKind {
    Ident,
    Number,
    String,
    Comment { is_block: bool },
    Directive,
    Continuation,
    Whitespace,
    Newline,
    Other(char),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub st: usize,
    pub ed: usize,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct State {
    pub is_comment: bool,
    pub is_directive: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Declaration<'a> {
    pub token: Token<'a>,
    pub ty: Option<Type<'a>>,
    pub is_row_major: Option<bool>,
    pub members: Vec<Member<'a>>,
    pub has_error: bool,
    pub is_manual: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Type<'a> {
    pub scalar: &'a str,
    pub rows: usize,
    pub cols: usize,
    pub is_matrix: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Member<'a> {
    pub name: &'a str,
    pub span: Span,
    pub dimensions: Vec<Option<usize>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Directive<'a> {
    pub name: &'a str,
    pub rest: &'a str,
    pub span: Span,
}

impl<'a> Lexer<'a> {
    #[must_use]
    pub fn new(src: &'a str) -> Self {
        Self {
            src,
            bytes: src.as_bytes(),
            curr: 0,
            is_line_start: true,
            state: State::default(),
        }
    }
}

impl<'a> Iterator for Lexer<'a> {
    type Item = Token<'a>;

    #[allow(clippy::too_many_lines)]
    fn next(&mut self) -> Option<Self::Item> {
        if self.curr == 0 && self.src.trim().is_empty() {
            self.state.is_directive = false;
        }
        if self.curr >= self.bytes.len() {
            return None;
        }
        if self.state.is_directive || (!self.state.is_comment && self.is_line_start && self.bytes[self.curr] == b'#') {
            let st = self.curr;
            let ed = st + self.src[st..].find('\n').unwrap_or(self.src.len() - st);
            let kind = if self.state.is_directive {
                TokenKind::Continuation
            } else {
                TokenKind::Directive
            };
            let mut lexer = Self::new(&self.src[st + usize::from(kind == TokenKind::Directive)..ed]);
            lexer.state.is_comment = self.state.is_comment;
            lexer.is_line_start = false;
            lexer.by_ref().for_each(drop);
            self.state.is_comment = lexer.state.is_comment;
            self.state.is_directive = self.src[st..ed].trim_end().ends_with('\\');
            self.curr = ed + usize::from(ed < self.src.len());
            self.is_line_start = true;
            return Some(Token {
                kind,
                text: &self.src[st..ed],
                span: Span { st, ed },
            });
        }

        let st = self.curr;
        Some(Token {
            kind: if self.state.is_comment || self.src[st..].starts_with("/*") {
                self.curr += if self.state.is_comment { 0 } else { 2 };
                if let Some(rel) = self.src[self.curr..].find("*/") {
                    self.curr += rel + 2;
                    self.state.is_comment = false;
                } else {
                    self.curr = self.bytes.len();
                    self.state.is_comment = true;
                }

                if self.src[st..self.curr].contains('\n') {
                    self.is_line_start = true;
                }

                TokenKind::Comment { is_block: true }
            } else if self.src[st..].starts_with("//") {
                self.curr += self.src[st..].find('\n').unwrap_or(self.bytes.len() - st);
                TokenKind::Comment { is_block: false }
            } else if self.bytes[st] == b'\n' {
                self.curr += 1;
                self.is_line_start = true;
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
                self.is_line_start = false;
                self.curr += 1;
                while self.curr < self.bytes.len() {
                    if self.bytes[self.curr] == b'\\' {
                        self.curr = (self.curr + 2).min(self.bytes.len());
                    } else if self.bytes[self.curr] == self.bytes[st] {
                        self.curr += 1;
                        break;
                    } else {
                        self.curr += 1;
                    }
                }

                TokenKind::String
            } else if self.bytes[st].is_ascii_digit()
                || (self.bytes[st] == b'.' && self.bytes.get(st + 1).is_some_and(u8::is_ascii_digit))
            {
                self.is_line_start = false;
                self.curr += 1;
                while self.curr < self.bytes.len()
                    && (self.bytes[self.curr].is_ascii_alphanumeric()
                        || matches!(self.bytes[self.curr], b'_' | b'.')
                        || (matches!(self.bytes[self.curr], b'+' | b'-')
                            && matches!(self.bytes[self.curr - 1], b'e' | b'E' | b'p' | b'P')))
                {
                    self.curr += 1;
                }

                TokenKind::Number
            } else if self.bytes[st].is_ascii_alphabetic() || self.bytes[st] == b'_' || self.bytes[st] >= 0x80 {
                self.is_line_start = false;
                while self.curr < self.bytes.len()
                    && (self.bytes[self.curr].is_ascii_alphanumeric()
                        || self.bytes[self.curr] == b'_'
                        || self.bytes[self.curr] >= 0x80)
                {
                    self.curr += 1;
                }

                TokenKind::Ident
            } else {
                self.is_line_start = false;
                self.curr += 1;
                TokenKind::Other(char::from(self.bytes[st]))
            },
            text: &self.src[st..self.curr],
            span: Span { st, ed: self.curr },
        })
    }
}

#[must_use]
pub fn parse_declaration<'a>(tokens: &[Token<'a>]) -> Option<Declaration<'a>> {
    let mut curr = 0;
    let mut is_row_major = None;
    while let Some(token) = tokens.get(curr) {
        match token.text {
            "row_major" => is_row_major = Some(true),
            "column_major" => is_row_major = Some(false),
            "static" | "groupshared" => return None,
            "const" | "uniform" | "volatile" | "precise" | "extern" | "shared" | "nointerpolation" | "linear"
            | "centroid" | "noperspective" | "sample" | "snorm" | "unorm" | "globallycoherent" => {}
            _ => break,
        }
        curr += 1;
    }

    let (ty, len) = parse_type(&tokens[curr..]);
    let mut decl = Declaration {
        token: *tokens.get(curr)?,
        ty,
        is_row_major,
        members: Vec::new(),
        has_error: ty.is_none() || curr + len == tokens.len(),
        is_manual: false,
    };

    curr += len;
    while curr < tokens.len() {
        let member = tokens[curr];
        if !member.text.starts_with(|ch: char| ch.is_alphabetic() || ch == '_')
            || !member.text.chars().all(|ch| ch.is_alphanumeric() || ch == '_')
        {
            decl.has_error = true;
            break;
        }

        curr += 1;
        let mut dims = Vec::new();
        while tokens.get(curr).is_some_and(|token| token.text == "[") {
            if tokens.get(curr + 2).is_none_or(|token| token.text != "]") {
                decl.has_error = true;
                break;
            }

            dims.push(tokens.get(curr + 1).and_then(|token| token.text.parse::<usize>().ok()));
            curr += 3;
        }

        if tokens.get(curr).is_some_and(|token| token.text == ":") {
            decl.is_manual = true;
            break;
        }

        if tokens.get(curr).is_some_and(|token| token.text == "=") {
            let mut depth = 0_usize;
            while let Some(token) = tokens.get(curr).filter(|token| depth > 0 || token.text != ",") {
                match token.text {
                    "(" | "[" | "{" => depth += 1,
                    ")" | "]" | "}" => depth = depth.saturating_sub(1),
                    _ => {}
                }
                curr += 1;
            }
            decl.has_error |= depth != 0;
        }

        if decl.has_error || tokens.get(curr).is_some_and(|token| token.text != ",") {
            decl.has_error = true;
            break;
        }

        decl.members.push(Member {
            name: member.text,
            span: member.span,
            dimensions: dims,
        });

        if curr < tokens.len() {
            curr += 1;
            decl.has_error = curr == tokens.len();
        }
    }
    Some(decl)
}

#[must_use]
pub fn parse_type<'a>(tokens: &[Token<'a>]) -> (Option<Type<'a>>, usize) {
    let Some(token) = tokens.first() else {
        return (None, 0);
    };

    if token.kind != TokenKind::Ident {
        return (None, 1);
    }

    if matches!(token.text, "vector" | "matrix") {
        let is_matrix = token.text == "matrix";
        if tokens.get(1).is_none_or(|token| token.text != "<") {
            return (
                Some(Type {
                    scalar: "float",
                    rows: if is_matrix { 4 } else { 1 },
                    cols: 4,
                    is_matrix,
                }),
                1,
            );
        }
        let len = if is_matrix { 8 } else { 6 };
        let Some(args) = tokens.get(..len) else {
            return (None, 1);
        };
        if args[2].kind != TokenKind::Ident
            || args[3].text != ","
            || args[len - 1].text != ">"
            || (is_matrix && args[5].text != ",")
        {
            return (None, 1);
        }

        let parse = |text: &str| text.parse::<usize>().ok().filter(|&val| (1..=4).contains(&val));

        return (
            (if is_matrix { parse(args[4].text) } else { Some(1) })
                .zip(parse(args[if is_matrix { 6 } else { 4 }].text))
                .map(|(rows, cols)| Type {
                    scalar: args[2].text,
                    rows,
                    cols,
                    is_matrix,
                }),
            len,
        );
    }

    (
        Some(parse_builtin(token.text).unwrap_or(Type {
            scalar: token.text,
            rows: 1,
            cols: 1,
            is_matrix: false,
        })),
        1,
    )
}

#[must_use]
pub fn parse_directive(token: Token<'_>) -> Option<Directive<'_>> {
    if token.kind == TokenKind::Continuation {
        return Some(Directive {
            name: "",
            rest: token.text,
            span: token.span,
        });
    }

    if token.kind != TokenKind::Directive {
        return None;
    }

    let text = token.text.strip_prefix('#')?.trim_start();
    let ed = text
        .find(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_')
        .unwrap_or(text.len());

    Some(Directive {
        name: &text[..ed],
        rest: text[ed..].trim_start(),
        span: token.span,
    })
}

#[must_use]
pub fn skip_group(tokens: &[Token<'_>], st: usize, open: char, close: char) -> Option<usize> {
    let mut depth = 0_usize;
    for (i, token) in tokens.iter().enumerate().skip(st) {
        if token.kind == TokenKind::Other(open) {
            depth += 1;
        } else if token.kind == TokenKind::Other(close) {
            depth = depth.checked_sub(1)?;
            if depth == 0 {
                return Some(i + 1);
            }
        }
    }
    None
}

pub(super) fn parse_builtin(text: &str) -> Option<Type<'static>> {
    let dim = |text: &str| match *text.as_bytes() {
        [ch @ b'1'..=b'4'] => Some(usize::from(ch - b'0')),
        _ => None,
    };

    SCALARS.iter().find_map(|&(scalar, ..)| {
        let suffix = text.strip_prefix(scalar)?;
        let (rows, cols, is_matrix) = match suffix.split_once('x') {
            Some((rows, cols)) => (dim(rows)?, dim(cols)?, true),
            None if suffix.is_empty() => (1, 1, false),
            None => (1, dim(suffix)?, false),
        };
        Some(Type {
            scalar,
            rows,
            cols,
            is_matrix,
        })
    })
}

pub static SCALARS: [(&str, usize, bool); 12] = [
    ("float", 4, true),
    ("half", 4, true),
    ("min16float", 4, true),
    ("min10float", 4, true),
    ("double", 8, false),
    ("uint", 4, false),
    ("int", 4, false),
    ("bool", 4, false),
    ("dword", 4, false),
    ("min16int", 4, false),
    ("min12int", 4, false),
    ("min16uint", 4, false),
];

pub(super) static KEYWORDS: [&str; 168] = [
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
