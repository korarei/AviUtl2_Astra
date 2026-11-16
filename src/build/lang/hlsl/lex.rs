use super::{Comment, Error, SCALARS, Scalar, Span, Token, TokenKind, Type};
use std::borrow::Cow;

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
    fn read_token(&mut self) -> Result<Token<'a>, Error> {
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
                    let newline = body_st + token.span.st;
                    if self.bytes[newline - 1] == b'\\' {
                        continue;
                    }

                    ed = newline;
                    break;
                }
            }

            self.curr = ed + usize::from(ed < self.src.len());
            self.can_read_directive = true;
            let mut directive = None;
            let mut curr = st;

            for (offset, _) in self.src[st..ed].match_indices('\n') {
                let newline = st + offset;
                if newline <= st || self.bytes[newline - 1] != b'\\' {
                    continue;
                }

                let directive = directive.get_or_insert_with(|| String::with_capacity(ed - st));
                directive.push_str(&self.src[curr..newline - 1]);
                curr = newline + 1;
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
                let offset = self.src[self.curr..].find("*/").ok_or(Error {
                    offset: st,
                    message: "unfinished HLSL comment",
                })?;
                let content_ed = self.curr + offset;
                self.curr += offset + 2;

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

    pub(super) fn is_number(text: &str) -> bool {
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

        let token = self.read_token();

        if token.is_err() {
            self.curr = self.bytes.len();
        }

        Some(token)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_and_errors() {
        for (src, expected) in [
            ("名前", TokenKind::Ident("名前")),
            ("cbuffer", TokenKind::Keyword("cbuffer")),
            ("row_major", TokenKind::Modifier("row_major")),
            ("0x1u", TokenKind::Number("0x1u")),
            ("1.0e-2f", TokenKind::Number("1.0e-2f")),
            ("\"text\"", TokenKind::String),
            (" \t", TokenKind::Whitespace),
            ("\n", TokenKind::Newline),
        ] {
            let mut lexer = Lexer::new(src);
            let token = lexer.next().unwrap().unwrap();
            assert_eq!(token.kind, expected, "token: {src:?}");
            assert_eq!(token.span, Span { st: 0, ed: src.len() }, "span: {src:?}");
            assert!(lexer.next().is_none(), "end: {src:?}");
        }
        for src in ["// comment", "/* comment */", "/* multi\nline */"] {
            let token = Lexer::new(src).next().unwrap().unwrap();
            assert!(matches!(token.kind, TokenKind::Comment(_)), "comment: {src:?}");
            assert_eq!(token.span.ed, src.len(), "comment span: {src:?}");
        }
        for (src, expected) in [("/*", "unfinished HLSL comment"), ("\"", "unfinished HLSL string")] {
            let mut lexer = Lexer::new(src);
            let err = lexer.next().unwrap().unwrap_err();
            assert_eq!((err.offset, err.message), (0, expected), "{src:?}");
            assert!(lexer.next().is_none(), "error end: {src:?}");
        }
    }
}
