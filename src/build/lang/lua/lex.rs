use super::{Bracket, Comment, Error, Literal, Span, Token, TokenKind};
use std::borrow::Cow;

#[derive(Clone)]
pub struct Lexer<'a> {
    src: &'a str,
    curr: usize,
}

impl<'a> Lexer<'a> {
    #[must_use]
    pub fn new(src: &'a str) -> Self {
        Self { src, curr: 0 }
    }

    pub(in crate::build) fn new_at(src: &'a str, st: usize) -> Result<Self, Error> {
        if !src.is_char_boundary(st) {
            return Err(Error {
                offset: st.min(src.len()),
                message: "invalid lexer offset",
            });
        }

        Ok(Self { src, curr: st })
    }

    #[allow(clippy::too_many_lines)]
    fn read_token(&mut self) -> Result<Token<'a>, Error> {
        let bytes = self.src.as_bytes();
        let st = self.curr;
        let byte = bytes[self.curr];

        Ok(Token {
            kind: if matches!(byte, b' ' | b'\t' | 0x0b | 0x0c) {
                while self.curr < bytes.len() && matches!(bytes[self.curr], b' ' | b'\t' | 0x0b | 0x0c) {
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
                    self.curr += self.src[self.curr..].find('\n').unwrap_or(bytes.len() - self.curr);
                    TokenKind::Comment(Comment {
                        content: &self.src[st + 2..self.curr],
                        bracket: None,
                    })
                }
            } else if matches!(byte, b'"' | b'\'') {
                TokenKind::String(self.read_string()?)
            } else if byte == b'[' {
                if let Some(bracket) = self.read_bracket()? {
                    let st = bracket.span.st + usize::from(bytes.get(bracket.span.st) == Some(&b'\n'));
                    TokenKind::String(Literal {
                        val: Cow::Borrowed(&bytes[st..bracket.span.ed]),
                        bracket: Some(bracket),
                    })
                } else {
                    if bytes.get(st + 1) == Some(&b'=') {
                        return Err(Error {
                            offset: st,
                            message: "invalid long string delimiter",
                        });
                    }

                    self.curr += 1;
                    TokenKind::LBracket
                }
            } else if byte.is_ascii_alphabetic() || byte == b'_' || byte >= 0x80 {
                while self.curr < bytes.len()
                    && (bytes[self.curr].is_ascii_alphanumeric()
                        || bytes[self.curr] == b'_'
                        || bytes[self.curr] >= 0x80)
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
                || (byte == b'.' && bytes.get(self.curr + 1).is_some_and(u8::is_ascii_digit))
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
                        if self.curr < bytes.len() && bytes[self.curr] == b'=' {
                            self.curr += 1;
                            TokenKind::Eq
                        } else {
                            TokenKind::Assign
                        }
                    }
                    b'~' if self.curr < bytes.len() && bytes[self.curr] == b'=' => {
                        self.curr += 1;
                        TokenKind::Ne
                    }
                    b'<' if self.curr < bytes.len() && bytes[self.curr] == b'=' => {
                        self.curr += 1;
                        TokenKind::Le
                    }
                    b'>' if self.curr < bytes.len() && bytes[self.curr] == b'=' => {
                        self.curr += 1;
                        TokenKind::Ge
                    }
                    b',' => TokenKind::Comma,
                    b'.' => {
                        if self.curr < bytes.len() && bytes[self.curr] == b'.' {
                            self.curr += 1;
                            if self.curr < bytes.len() && bytes[self.curr] == b'.' {
                                self.curr += 1;
                                TokenKind::Ellipsis
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
        let bytes = self.src.as_bytes();
        let st = self.curr;
        let mut prefix = self.src[st..].bytes().filter(|&byte| byte != b'_');
        let base = match (prefix.next(), prefix.next()) {
            (Some(b'0'), Some(b'x' | b'X')) => 16,
            (Some(b'0'), Some(b'b' | b'B')) => 2,
            _ => 10,
        };
        let exponent = if base == 16 { b'p' } else { b'e' };
        let mut prev = bytes[self.curr];
        self.curr += 1;

        while self.curr < bytes.len() {
            let byte = bytes[self.curr];
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
            let mut fraction_digits = 0usize;
            let mut significant_fraction_digits = 0;

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
                        fraction_digits += 1;
                        if digit != 0 {
                            significant_fraction_digits = fraction_digits;
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

            if !has_digits || significant_fraction_digits >= 1 << 20 || (base == 2 && val.is_none()) {
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
        let bytes = self.src.as_bytes();
        let st = self.curr;
        let quote = bytes[st];
        self.curr += 1;
        let content_st = self.curr;
        let mut val = None;
        let mut copy_st = content_st;
        let mut is_valid = true;
        let mut is_closed = false;
        let decode_hex = |byte| match byte {
            b'0'..=b'9' => Some(byte - b'0'),
            b'a'..=b'f' => Some(byte - b'a' + 10),
            b'A'..=b'F' => Some(byte - b'A' + 10),
            _ => None,
        };

        while self.curr < bytes.len() {
            let byte = bytes[self.curr];
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
                val.extend_from_slice(&bytes[copy_st..self.curr]);
                self.curr += 1;

                if self.curr >= bytes.len() {
                    is_valid = false;
                    break;
                }

                let escape = bytes[self.curr];
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
                        let Some(high) = bytes.get(self.curr).copied().and_then(decode_hex) else {
                            is_valid = false;
                            continue;
                        };
                        self.curr += 1;
                        let Some(low) = bytes.get(self.curr).copied().and_then(decode_hex) else {
                            is_valid = false;
                            continue;
                        };
                        self.curr += 1;
                        val.push((high << 4) | low);
                    }
                    b'u' => {
                        if bytes.get(self.curr) != Some(&b'{') {
                            is_valid = false;
                            continue;
                        }

                        self.curr += 1;
                        let mut code = 0u32;
                        let mut digits = 0;

                        while let Some(digit) = bytes.get(self.curr).copied().and_then(decode_hex) {
                            if let Some(next) = code.checked_mul(16).and_then(|v| v.checked_add(u32::from(digit))) {
                                code = next;
                            } else {
                                is_valid = false;
                            }

                            digits += 1;
                            self.curr += 1;
                        }

                        if bytes.get(self.curr) == Some(&b'}') {
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
                        while self.curr < bytes.len() && matches!(bytes[self.curr], b' ' | b'\t' | b'\n' | 0x0b | 0x0c)
                        {
                            self.curr += 1;
                        }
                    }
                    digit @ b'0'..=b'9' => {
                        let mut code = u32::from(digit - b'0');

                        for _ in 0..2 {
                            if self.curr < bytes.len() && bytes[self.curr].is_ascii_digit() {
                                code = code * 10 + u32::from(bytes[self.curr] - b'0');
                                self.curr += 1;
                            } else {
                                break;
                            }
                        }

                        if let Ok(byte) = u8::try_from(code) {
                            val.push(byte);
                        } else {
                            is_valid = false;
                        }
                    }
                    _ => {
                        is_valid = false;
                        while bytes.get(self.curr).is_some_and(|byte| byte & 0xC0 == 0x80) {
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
                        val.extend_from_slice(&bytes[copy_st..self.curr - 1]);
                        Cow::Owned(val)
                    }
                    None => Cow::Borrowed(&bytes[content_st..self.curr - 1]),
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
        let bytes = self.src.as_bytes();
        let st = self.curr;

        if bytes.get(st) != Some(&b'[') {
            return Ok(None);
        }

        let mut curr = st + 1;

        while bytes.get(curr) == Some(&b'=') {
            curr += 1;
        }

        if bytes.get(curr) != Some(&b'[') {
            return Ok(None);
        }

        let level = curr - st - 1;
        let content_st = curr + 1;
        let mut ed = content_st;
        while ed < bytes.len() {
            if bytes[ed] == b']' {
                let mut curr = ed + 1;

                while bytes.get(curr) == Some(&b'=') {
                    curr += 1;
                }

                if curr - ed - 1 == level && bytes.get(curr) == Some(&b']') {
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
        if self.curr >= self.src.len() {
            return None;
        }

        let token = self.read_token();

        if token.is_err() {
            self.curr = self.src.len();
        }

        Some(token)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_at_offset() {
        let src = "--\u{3042}\ncall()";
        let st = src.find("call").unwrap();
        let token = Lexer::new_at(src, st).unwrap().next().unwrap().unwrap();
        assert_eq!(token.span, Span { st, ed: st + 4 });
        assert!(matches!(token.kind, TokenKind::Ident("call")));
        assert!(Lexer::new_at(src, 3).is_err());
        assert!(Lexer::new_at(src, src.len() + 1).is_err());
        assert!(Lexer::new_at(src, src.len()).unwrap().next().is_none());
    }

    #[test]
    fn read_tokens() {
        for (src, expected) in [
            ("local", TokenKind::Local),
            ("const", TokenKind::Keyword("const")),
            ("continue", TokenKind::Keyword("continue")),
            ("goto", TokenKind::Keyword("goto")),
            ("名前", TokenKind::Ident("名前")),
            ("0x1p-2", TokenKind::Number("0x1p-2")),
            ("1_000ULL", TokenKind::Number("1_000ULL")),
            ("\n", TokenKind::Newline),
            (" \t", TokenKind::Whitespace),
            ("..", TokenKind::Concat),
            ("...", TokenKind::Ellipsis),
            ("~=", TokenKind::Ne),
        ] {
            let mut lexer = Lexer::new(src);
            let token = lexer.next().unwrap().unwrap();
            assert_eq!(token.kind, expected, "token: {src:?}");
            assert_eq!(token.span, Span { st: 0, ed: src.len() }, "span: {src:?}");
            assert!(lexer.next().is_none(), "end: {src:?}");
        }

        for src in [
            "~>>", "<<", ">>", "?.", "??", "->", "&&", "||", "!=", "::", "~", "&", "|",
        ] {
            assert_eq!(
                Lexer::new(src).next().unwrap().unwrap().kind,
                TokenKind::Operator(src),
                "{src}"
            );
        }
    }

    #[test]
    fn read_literals() {
        for (src, expected) in [(r#""\x41\u{42}\067""#, "ABC"), ("[=[\n名前]=]", "名前")] {
            let token = Lexer::new(src).next().unwrap().unwrap();
            let TokenKind::String(literal) = token.kind else {
                panic!("string: {src:?}")
            };
            assert_eq!(literal.val.as_ref(), expected.as_bytes(), "value: {src:?}");
        }

        for (src, message) in [
            ("'", "unfinished string"),
            (r#""\q""#, "invalid string escape"),
            ("[=[", "unfinished long string"),
            ("--[=[", "unfinished long comment"),
            ("0x", "malformed number"),
            ("@", "unexpected '@'"),
        ] {
            let mut lexer = Lexer::new(src);
            let err = lexer.next().unwrap().unwrap_err();
            assert_eq!((err.offset, err.message), (0, message), "{src:?}");
            assert!(lexer.next().is_none(), "error end: {src:?}");
        }
    }
}
