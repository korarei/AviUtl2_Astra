use super::{Error, Span, Token, TokenKind};

#[derive(Clone)]
pub(in crate::build) struct Lexer<'a> {
    pub(super) src: &'a str,
    bytes: &'a [u8],
    curr: usize,
}

impl<'a> Lexer<'a> {
    pub(in crate::build) fn new(src: &'a str) -> Self {
        Self {
            src,
            bytes: src.as_bytes(),
            curr: 0,
        }
    }

    fn read_token(&mut self) -> Result<Token<'a>, Error> {
        let st = self.curr;
        let byte = self.bytes[self.curr];
        let kind = if byte.is_ascii_whitespace() {
            while self.bytes.get(self.curr).is_some_and(u8::is_ascii_whitespace) {
                self.curr += 1;
            }
            TokenKind::Whitespace
        } else if matches!(byte, b'\'' | b'"') {
            self.curr += 1;
            loop {
                let Some(&curr) = self.bytes.get(self.curr) else {
                    return Err(Error {
                        offset: st,
                        message: "unclosed string".into(),
                    });
                };

                if curr == b'\\' {
                    self.curr += 1;

                    if self.curr < self.bytes.len() {
                        self.curr += 1;
                    }
                } else if curr == byte {
                    self.curr += 1;
                    break TokenKind::String;
                } else if curr == b'\n' {
                    return Err(Error {
                        offset: st,
                        message: "unclosed string".into(),
                    });
                } else {
                    self.curr += 1;
                }
            }
        } else if matches!(byte, b'a'..=b'z' | b'A'..=b'Z' | b'_' | 0x80..=0xff) {
            while self.curr < self.bytes.len()
                && matches!(self.bytes[self.curr], b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'_' | 0x80..=0xff)
            {
                self.curr += 1;
            }
            TokenKind::Ident(&self.src[st..self.curr])
        } else if byte.is_ascii_digit()
            || (byte == b'.' && self.bytes.get(self.curr + 1).is_some_and(u8::is_ascii_digit))
        {
            let exponent = if self.src[st..].starts_with("0x") || self.src[st..].starts_with("0X") {
                b'p'
            } else {
                b'e'
            };
            let mut prev = byte;
            self.curr += 1;
            while self.curr < self.bytes.len() {
                let curr = self.bytes[self.curr];
                if curr.is_ascii_alphanumeric()
                    || curr == b'_'
                    || (curr == b'.' && self.bytes.get(self.curr + 1) != Some(&b'.'))
                    || (matches!(curr, b'+' | b'-') && prev.to_ascii_lowercase() == exponent)
                {
                    prev = curr;
                    self.curr += 1;
                } else {
                    break;
                }
            }
            TokenKind::Number(&self.src[st..self.curr])
        } else if let Some(op) = ["&&", "||", "==", "~=", "!=", "<=", ">="]
            .into_iter()
            .find(|op| self.src[st..].starts_with(op))
        {
            self.curr += op.len();
            TokenKind::Operator(op)
        } else {
            self.curr += self.src[self.curr..].chars().next().unwrap().len_utf8();
            TokenKind::Other(char::from(byte))
        };

        Ok(Token {
            kind,
            span: Span { st, ed: self.curr },
        })
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
        for src in [
            "名前",
            "defined",
            "0x1p-2",
            "1_000",
            "'a\\'b'",
            "\"a\\\"b\"",
            " \t\n",
            "(",
            ")",
            "+",
        ] {
            let token = Lexer::new(src).next().unwrap().unwrap();
            assert_eq!((token.span.st, token.span.ed), (0, src.len()), "span: {src:?}");
            assert!(Lexer::new(src).nth(1).is_none(), "end: {src:?}");
        }
        for src in ["&&", "||", "==", "~=", "!=", "<=", ">="] {
            let token = Lexer::new(src).next().unwrap().unwrap();
            assert!(token.kind == TokenKind::Operator(src), "operator: {src}");
        }
        for src in ["'", "\"", "'a\n'", "'a\\"] {
            let mut lexer = Lexer::new(src);
            let err = lexer.next().unwrap().err().unwrap();
            assert_eq!((err.offset, err.message.as_ref()), (0, "unclosed string"), "{src:?}");
            assert!(lexer.next().is_none(), "error end: {src:?}");
        }
    }
}
