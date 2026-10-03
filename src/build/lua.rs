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

        if matches!(b, b' ' | b'\t' | 0x0b | 0x0c) {
            while self.curr < self.bytes.len() && matches!(self.bytes[self.curr], b' ' | b'\t' | 0x0b | 0x0c) {
                self.curr += 1;
            }
            return Some(Token {
                kind: TokenKind::Whitespace,
                span: Span { st, ed: self.curr },
            });
        }

        if b == b'\n' {
            self.curr += 1;
            return Some(Token {
                kind: TokenKind::Newline,
                span: Span { st, ed: self.curr },
            });
        }

        if b == b'-' && self.bytes.get(self.curr + 1) == Some(&b'-') {
            self.curr += 2;
            if self.curr < self.bytes.len() && self.bytes[self.curr] == b'[' {
                self.curr += 1;
                if let Some((range, closed, raw_st)) = self.read_long() {
                    return Some(Token {
                        kind: if closed {
                            TokenKind::Comment {
                                content: &self.src[raw_st..range.ed],
                                is_block: true,
                            }
                        } else {
                            TokenKind::UnclosedComment(&self.src[raw_st..range.ed])
                        },
                        span: Span { st, ed: self.curr },
                    });
                }
            }

            while self.curr < self.bytes.len() && self.bytes[self.curr] != b'\n' {
                self.curr += 1;
            }
            return Some(Token {
                kind: TokenKind::Comment {
                    content: &self.src[st + 2..self.curr],
                    is_block: false,
                },
                span: Span { st, ed: self.curr },
            });
        }

        if b == b'"' || b == b'\'' {
            self.curr += 1;
            let raw_st = self.curr;
            let mut value = None;
            let mut copied = raw_st;
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
                    let value = value.get_or_insert_with(|| Vec::with_capacity(self.curr - raw_st + 8));
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
                        b'\\' => value.push(b'\\'),
                        b'"' => value.push(b'"'),
                        b'\'' => value.push(b'\''),
                        b'\n' => {
                            value.push(b'\n');
                        }
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
            return Some(Token {
                kind: if valid && closed {
                    TokenKind::String(match value {
                        Some(mut value) => {
                            value.extend_from_slice(&self.bytes[copied..self.curr - 1]);
                            Cow::Owned(value)
                        }
                        None => Cow::Borrowed(&self.bytes[raw_st..self.curr - 1]),
                    })
                } else {
                    TokenKind::Invalid
                },
                span: Span { st, ed: self.curr },
            });
        }

        if b == b'[' {
            self.curr += 1;
            if let Some((range, closed, _)) = self.read_long() {
                return Some(Token {
                    kind: if closed {
                        TokenKind::String(Cow::Borrowed(&self.bytes[range.st..range.ed]))
                    } else {
                        TokenKind::Invalid
                    },
                    span: Span { st, ed: self.curr },
                });
            }
            return Some(Token {
                kind: TokenKind::LBracket,
                span: Span { st, ed: self.curr },
            });
        }

        if matches!(b, b'a'..=b'z' | b'A'..=b'Z' | b'_' | 0x80..=0xff) {
            while self.curr < self.bytes.len()
                && matches!(self.bytes[self.curr], b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'_' | 0x80..=0xff)
            {
                self.curr += 1;
            }
            let word = &self.src[st..self.curr];
            return Some(Token {
                kind: match word {
                    "local" => TokenKind::Local,
                    "and" | "break" | "do" | "else" | "elseif" | "end" | "false" | "for" | "function" | "if"
                    | "const" | "continue" | "goto" | "in" | "nil" | "not" | "or" | "repeat" | "return" | "then"
                    | "true" | "until" | "while" => TokenKind::Keyword(word),
                    _ => TokenKind::Ident(word),
                },
                span: Span { st, ed: self.curr },
            });
        }

        if let Some(op) = ["~>>", "&&", "||", "!=", "?.", "??", "::", "->", "<<", ">>"]
            .into_iter()
            .find(|op| self.src[st..].starts_with(op))
        {
            self.curr += op.len();
            return Some(Token {
                kind: TokenKind::Operator(op),
                span: Span { st, ed: self.curr },
            });
        }

        if b.is_ascii_digit() || (b == b'.' && self.bytes.get(self.curr + 1).is_some_and(u8::is_ascii_digit)) {
            let mut prev = b'\0';
            if b == b'.' {
                self.curr += 1;
            }
            if b == b'0' {
                prev = b'0';
                self.curr += 1;
                while self.bytes.get(self.curr) == Some(&b'_') {
                    self.curr += 1;
                }
                if matches!(self.bytes.get(self.curr), Some(b'x' | b'X')) {
                    prev = self.bytes[self.curr];
                    self.curr += 1;
                }
            }
            while self.curr < self.bytes.len() {
                let c = self.bytes[self.curr];
                if matches!(c, b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'_' | 0x80..=0xff)
                    || c == b'.'
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
            return Some(Token {
                kind: TokenKind::Number(&self.src[st..self.curr]),
                span: Span { st, ed: self.curr },
            });
        }

        self.curr += 1;
        let kind = match b {
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
        };

        Some(Token {
            kind,
            span: Span { st, ed: self.curr },
        })
    }
}

impl Lexer<'_> {
    fn read_long(&mut self) -> Option<(Span, bool, usize)> {
        let p = self.curr;
        let mut eq = 0;
        while self.curr < self.bytes.len() && self.bytes[self.curr] == b'=' {
            eq += 1;
            self.curr += 1;
        }

        if self.curr >= self.bytes.len() || self.bytes[self.curr] != b'[' {
            self.curr = p;
            return None;
        }

        self.curr += 1;
        let raw_st = self.curr;

        if self.bytes.get(self.curr) == Some(&b'\n') {
            self.curr += 1;
        }

        let inner_st = self.curr;
        while self.curr < self.bytes.len() {
            if self.bytes[self.curr] == b']' {
                let mut ed = self.curr + 1;
                while self.bytes.get(ed) == Some(&b'=') {
                    ed += 1;
                }
                if ed - self.curr == eq + 1 && self.bytes.get(ed) == Some(&b']') {
                    let range = Span {
                        st: inner_st,
                        ed: self.curr,
                    };
                    self.curr = ed + 1;
                    return Some((range, true, raw_st));
                }
            }
            self.curr += 1;
        }

        Some((
            Span {
                st: inner_st,
                ed: self.curr,
            },
            false,
            raw_st,
        ))
    }
}

#[must_use]
pub fn find_require(src: &str, st: usize) -> Option<RequireMatch> {
    let src = src.get(st..)?;
    let mut tokens = Lexer::new(src).peekable();
    let skip = |tokens: &mut std::iter::Peekable<Lexer<'_>>| {
        while tokens
            .next_if(|token| {
                matches!(
                    &token.kind,
                    TokenKind::Whitespace
                        | TokenKind::Newline
                        | TokenKind::Comment { .. }
                        | TokenKind::UnclosedComment(_)
                )
            })
            .is_some()
        {}
    };
    let mut member = false;

    while let Some(token) = tokens.next() {
        if matches!(
            &token.kind,
            TokenKind::Whitespace | TokenKind::Newline | TokenKind::Comment { .. } | TokenKind::UnclosedComment(_)
        ) {
            continue;
        }

        if matches!(&token.kind, TokenKind::Ident("require")) && !member {
            let mut args = tokens.clone();
            skip(&mut args);
            if let Some(arg) = args.next() {
                let (name, ed) = if arg.kind == TokenKind::LParen {
                    skip(&mut args);
                    let Some(Token {
                        kind: TokenKind::String(name),
                        ..
                    }) = args.next()
                    else {
                        member = false;
                        continue;
                    };
                    skip(&mut args);
                    let Some(Token {
                        kind: TokenKind::RParen,
                        span,
                    }) = args.next()
                    else {
                        member = false;
                        continue;
                    };
                    (name.into_owned(), span.ed)
                } else if let TokenKind::String(name) = arg.kind {
                    (name.into_owned(), arg.span.ed)
                } else {
                    member = false;
                    continue;
                };

                return Some(RequireMatch {
                    name,
                    st: st + token.span.st,
                    ed: st + ed,
                });
            }
        }

        member = matches!(&token.kind, TokenKind::Dot | TokenKind::Colon);
    }

    None
}
