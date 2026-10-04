use std::borrow::Cow;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub st: usize,
    pub ed: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequireMatch {
    pub name: Vec<u8>,
    pub st: usize,
    pub ed: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenKind<'a> {
    Keyword(&'a str),
    Local,
    Ident(&'a str),
    String(Cow<'a, [u8]>),
    Number(&'a str),
    Operator(&'a str),
    Invalid,
    UnclosedComment(&'a str),
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
    Comment { content: &'a str, is_block: bool },
    Whitespace,
    Newline,
    Other(char),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token<'a> {
    pub kind: TokenKind<'a>,
    pub span: Span,
}

#[derive(Clone)]
pub struct Lexer<'a> {
    src: &'a str,
    bytes: &'a [u8],
    curr: usize,
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
}

impl<'a> Iterator for Lexer<'a> {
    type Item = Token<'a>;

    #[allow(clippy::too_many_lines)]
    fn next(&mut self) -> Option<Self::Item> {
        if self.curr >= self.bytes.len() {
            return None;
        }

        let st = self.curr;
        let b = self.bytes[self.curr];
        Some(Token {
            kind: if matches!(b, b' ' | b'\t' | 0x0b | 0x0c) {
                while self.curr < self.bytes.len() && matches!(self.bytes[self.curr], b' ' | b'\t' | 0x0b | 0x0c) {
                    self.curr += 1;
                }
                TokenKind::Whitespace
            } else if b == b'\n' {
                self.curr += 1;
                TokenKind::Newline
            } else if self.src[st..].starts_with("--") {
                self.curr += 2;
                if self.bytes.get(self.curr) == Some(&b'[')
                    && let Some((span, closed, raw)) = {
                        self.curr += 1;
                        self.read_long()
                    }
                {
                    if closed {
                        TokenKind::Comment {
                            content: &self.src[raw..span.ed],
                            is_block: true,
                        }
                    } else {
                        TokenKind::UnclosedComment(&self.src[raw..span.ed])
                    }
                } else {
                    self.curr += self.src[self.curr..].find('\n').unwrap_or(self.bytes.len() - self.curr);
                    TokenKind::Comment {
                        content: &self.src[st + 2..self.curr],
                        is_block: false,
                    }
                }
            } else if matches!(b, b'"' | b'\'') {
                self.curr += 1;
                let raw = self.curr;
                let mut value = None;
                let mut copied = raw;
                let mut valid = true;
                let mut closed = false;
                let hex = |digit| match digit {
                    b'0'..=b'9' => Some(digit - b'0'),
                    b'a'..=b'f' => Some(digit - b'a' + 10),
                    b'A'..=b'F' => Some(digit - b'A' + 10),
                    _ => None,
                };
                while self.curr < self.bytes.len() {
                    let c = self.bytes[self.curr];
                    if c == b {
                        self.curr += 1;
                        closed = true;
                        break;
                    }

                    if c == b'\n' {
                        valid = false;
                        break;
                    }
                    if c == b'\\' {
                        let value = value.get_or_insert_with(|| Vec::with_capacity(self.curr - raw + 8));
                        value.extend_from_slice(&self.bytes[copied..self.curr]);
                        self.curr += 1;
                        if self.curr >= self.bytes.len() {
                            valid = false;
                            break;
                        }

                        let esc = self.bytes[self.curr];
                        self.curr += 1;
                        match esc {
                            b'a' => value.push(b'\x07'),
                            b'b' => value.push(b'\x08'),
                            b'f' => value.push(b'\x0C'),
                            b'n' => value.push(b'\n'),
                            b'r' => value.push(b'\r'),
                            b't' => value.push(b'\t'),
                            b'v' => value.push(b'\x0B'),
                            b'\\' | b'"' | b'\'' | b'\n' => value.push(esc),
                            b'x' => {
                                let Some(hi) = self.bytes.get(self.curr).copied().and_then(hex) else {
                                    valid = false;
                                    continue;
                                };
                                self.curr += 1;
                                let Some(lo) = self.bytes.get(self.curr).copied().and_then(hex) else {
                                    valid = false;
                                    continue;
                                };
                                self.curr += 1;
                                value.push((hi << 4) | lo);
                            }
                            b'u' => {
                                if self.bytes.get(self.curr) != Some(&b'{') {
                                    valid = false;
                                    continue;
                                }
                                self.curr += 1;
                                let mut val = 0u32;
                                let mut digits = 0;
                                while let Some(digit) = self.bytes.get(self.curr).copied().and_then(hex) {
                                    if let Some(next) =
                                        val.checked_mul(16).and_then(|val| val.checked_add(u32::from(digit)))
                                    {
                                        val = next;
                                    } else {
                                        valid = false;
                                    }
                                    digits += 1;
                                    self.curr += 1;
                                }
                                if self.bytes.get(self.curr) == Some(&b'}') {
                                    self.curr += 1;
                                } else {
                                    valid = false;
                                }
                                if digits == 0 {
                                    valid = false;
                                }
                                if let Some(c) = char::from_u32(val) {
                                    let mut buf = [0; 4];
                                    value.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
                                } else {
                                    valid = false;
                                }
                            }
                            b'z' => {
                                while self.curr < self.bytes.len()
                                    && matches!(self.bytes[self.curr], b' ' | b'\t' | b'\n' | 0x0b | 0x0c)
                                {
                                    self.curr += 1;
                                }
                            }
                            d @ b'0'..=b'9' => {
                                let mut val = u32::from(d - b'0');
                                for _ in 0..2 {
                                    if self.curr < self.bytes.len() && self.bytes[self.curr].is_ascii_digit() {
                                        val = val * 10 + u32::from(self.bytes[self.curr] - b'0');
                                        self.curr += 1;
                                    } else {
                                        break;
                                    }
                                }
                                if let Ok(val) = u8::try_from(val) {
                                    value.push(val);
                                } else {
                                    valid = false;
                                }
                            }
                            _ => {
                                valid = false;
                                while self.bytes.get(self.curr).is_some_and(|byte| byte & 0xC0 == 0x80) {
                                    self.curr += 1;
                                }
                            }
                        }
                        copied = self.curr;
                    } else {
                        self.curr += 1;
                    }
                }

                if valid && closed {
                    TokenKind::String(match value {
                        Some(mut value) => {
                            value.extend_from_slice(&self.bytes[copied..self.curr - 1]);
                            Cow::Owned(value)
                        }
                        None => Cow::Borrowed(&self.bytes[raw..self.curr - 1]),
                    })
                } else {
                    TokenKind::Invalid
                }
            } else if b == b'[' {
                self.curr += 1;
                if let Some((span, closed, _)) = self.read_long() {
                    if closed {
                        TokenKind::String(Cow::Borrowed(&self.bytes[span.st..span.ed]))
                    } else {
                        TokenKind::Invalid
                    }
                } else {
                    TokenKind::LBracket
                }
            } else if b.is_ascii_alphabetic() || b == b'_' || b >= 0x80 {
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
            } else if b.is_ascii_digit() || (b == b'.' && self.bytes.get(self.curr + 1).is_some_and(u8::is_ascii_digit))
            {
                let mut prev = b;
                self.curr += 1;
                while self.curr < self.bytes.len() {
                    let c = self.bytes[self.curr];
                    if c.is_ascii_alphanumeric()
                        || matches!(c, b'_' | b'.')
                        || c >= 0x80
                        || (matches!(c, b'+' | b'-') && matches!(prev, b'e' | b'E' | b'p' | b'P'))
                    {
                        if c != b'_' {
                            prev = c;
                        }
                        self.curr += 1;
                    } else {
                        break;
                    }
                }
                TokenKind::Number(&self.src[st..self.curr])
            } else {
                self.curr += 1;
                match b {
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
                    _ => TokenKind::Other(char::from(b)),
                }
            },
            span: Span { st, ed: self.curr },
        })
    }
}

impl Lexer<'_> {
    fn read_long(&mut self) -> Option<(Span, bool, usize)> {
        let st = self.curr;
        while self.bytes.get(self.curr) == Some(&b'=') {
            self.curr += 1;
        }

        if self.bytes.get(self.curr) != Some(&b'[') {
            self.curr = st;
            return None;
        }

        self.curr += 1;
        let raw = self.curr;

        if self.bytes.get(self.curr) == Some(&b'\n') {
            self.curr += 1;
        }

        let inner = self.curr;
        let mut ed = self.curr;
        while ed < self.bytes.len() {
            if self.bytes[ed] == b']' {
                let mut curr = ed + 1;
                while self.bytes.get(curr) == Some(&b'=') {
                    curr += 1;
                }
                if curr - ed == raw - st && self.bytes.get(curr) == Some(&b']') {
                    self.curr = curr + 1;
                    return Some((Span { st: inner, ed }, true, raw));
                }
            }
            ed += 1;
        }

        self.curr = ed;
        Some((Span { st: inner, ed }, false, raw))
    }
}

#[must_use]
pub fn find_require(src: &str, st: usize) -> Option<RequireMatch> {
    let mut tokens = Lexer::new(src.get(st..)?).filter(|token| {
        !matches!(
            &token.kind,
            TokenKind::Whitespace | TokenKind::Newline | TokenKind::Comment { .. } | TokenKind::UnclosedComment(_)
        )
    });
    let mut member = false;

    while let Some(token) = tokens.next() {
        if member || !matches!(&token.kind, TokenKind::Ident("require")) {
            member = matches!(&token.kind, TokenKind::Dot | TokenKind::Colon);
            continue;
        }
        member = false;
        let mut args = tokens.clone();
        let Some(arg) = args.next() else {
            continue;
        };
        let (name, ed) = if arg.kind == TokenKind::LParen {
            let Some(Token {
                kind: TokenKind::String(name),
                ..
            }) = args.next()
            else {
                continue;
            };
            let Some(Token {
                kind: TokenKind::RParen,
                span,
            }) = args.next()
            else {
                continue;
            };
            (name, span.ed)
        } else if let TokenKind::String(name) = arg.kind {
            (name, arg.span.ed)
        } else {
            continue;
        };
        return Some(RequireMatch {
            name: name.into_owned(),
            st: st + token.span.st,
            ed: st + ed,
        });
    }

    None
}
