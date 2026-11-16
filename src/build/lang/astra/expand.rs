use super::{Error, parse};
use crate::build::source;
use crate::text::normalize_newlines;
use std::borrow::Cow;
use std::fmt::Write;
use std::ops::Range;
use std::sync::Arc;

type Edit = (Range<usize>, Range<usize>, bool);

#[derive(Clone, Copy)]
pub(in crate::build) enum Escape {
    None,
    Comment,
    Lua,
    Hlsl,
}

impl Escape {
    fn encode(self, src: &str) -> Cow<'_, str> {
        if matches!(self, Self::None) {
            return Cow::Borrowed(src);
        }

        let Some(first) = src.find(char::is_control) else {
            return Cow::Borrowed(src);
        };

        let mut dst = String::with_capacity(src.len() + 8);
        dst.push_str(&src[..first]);

        for ch in src[first..].chars() {
            match ch {
                '\x07' => dst.push_str("\\a"),
                '\x08' => dst.push_str("\\b"),
                '\t' => dst.push_str("\\t"),
                '\n' => dst.push_str("\\n"),
                '\x0B' => dst.push_str("\\v"),
                '\x0C' => dst.push_str("\\f"),
                _ if !ch.is_control() => dst.push(ch),
                _ if matches!(self, Self::Comment) => {
                    if ch.is_ascii() {
                        write!(dst, "\\x{:02x}", ch as u32).unwrap();
                    } else {
                        write!(dst, "\\u{{{:04x}}}", ch as u32).unwrap();
                    }
                }
                _ => {
                    let mut buf = [0; 4];

                    for byte in ch.encode_utf8(&mut buf).bytes() {
                        if matches!(self, Self::Hlsl) {
                            write!(dst, "\\{byte:03o}").unwrap();
                        } else {
                            write!(dst, "\\{byte:03}").unwrap();
                        }
                    }
                }
            }
        }

        Cow::Owned(dst)
    }
}

#[derive(Debug)]
pub(in crate::build) struct Expanded<'a> {
    pub(in crate::build) text: Cow<'a, str>,
    edits: Vec<Edit>,
}

impl Expanded<'_> {
    pub(in crate::build) fn to_edits(&self, st: usize) -> impl Iterator<Item = source::Edit> {
        self.edits.iter().map(move |(src, dst, is_escaped)| {
            if *is_escaped {
                source::Edit {
                    span: st + src.start..st + src.start + 1,
                    text: None,
                }
            } else {
                source::Edit {
                    span: st + src.start..st + src.end,
                    text: Some(Arc::from(&self.text[dst.clone()])),
                }
            }
        })
    }

    pub(super) fn to_src_offset(&self, at: usize) -> usize {
        let Some(i) = self.edits.partition_point(|(_, dst, _)| dst.start <= at).checked_sub(1) else {
            return at;
        };

        let (src, dst, is_escaped) = &self.edits[i];

        if at < dst.end {
            src.start + if *is_escaped { 1 + at - dst.start } else { 0 }
        } else {
            src.end + at - dst.end
        }
    }
}

pub(in crate::build) fn expand<'text>(
    src: &'text str,
    vars: &impl crate::vars::Vars,
    escape: Escape,
) -> Result<Expanded<'text>, Error> {
    let mut dst = None;
    let mut edits = Vec::new();
    let mut curr = 0;

    while let Some(st) = src[curr..].find("${") {
        let st = curr + st;
        let (placeholder, ed) = parse::parse_placeholder(src, st)?.expect("placeholder prefix has already been found");
        let (at, val, is_escaped) = match placeholder {
            parse::Placeholder::Variable(key) => (
                st,
                vars.get(key).ok_or_else(|| Error {
                    offset: st,
                    message: format!("variable '{key}' not found").into(),
                })?,
                false,
            ),
            parse::Placeholder::Escaped => (st - 1, &src[st..ed], true),
        };

        let dst = dst.get_or_insert_with(|| String::with_capacity(src.len()));
        dst.push_str(&src[curr..at]);
        let dst_st = dst.len();

        if is_escaped {
            dst.push_str(val);
        } else {
            dst.push_str(&escape.encode(&normalize_newlines(val)));
        }

        edits.push((at..ed, dst_st..dst.len(), is_escaped));
        curr = ed;
    }

    Ok(Expanded {
        text: if let Some(mut dst) = dst {
            dst.push_str(&src[curr..]);
            Cow::Owned(dst)
        } else {
            Cow::Borrowed(src)
        },
        edits,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use indexmap::IndexMap;

    #[test]
    fn normalizes_newlines_and_escapes_controls() {
        let vars = IndexMap::from([
            ("CRLF".to_owned(), "a\r\nb\rc\r\r\nd".to_owned()),
            ("CTRL".to_owned(), "\x07\x08\t\r\n\x0B\x0C\x1b\u{0085}x".to_owned()),
        ]);

        assert_eq!(expand("${CRLF}", &vars, Escape::None).unwrap().text, "a\nb\nc\n\nd");
        assert_eq!(
            expand("${CTRL}", &vars, Escape::Comment).unwrap().text,
            "\\a\\b\\t\\n\\v\\f\\x1b\\u{0085}x"
        );
        assert_eq!(
            expand("${CTRL}", &vars, Escape::Lua).unwrap().text,
            "\\a\\b\\t\\n\\v\\f\\027\\194\\133x"
        );
        assert_eq!(
            expand("${CTRL}", &vars, Escape::Hlsl).unwrap().text,
            "\\a\\b\\t\\n\\v\\f\\033\\302\\205x"
        );
        assert_eq!(expand("$${CRLF}", &vars, Escape::Lua).unwrap().text, "${CRLF}");
    }

    #[test]
    fn preserve_cols() {
        let vars = IndexMap::from([("X".to_owned(), "日本語".to_owned())]);
        let expansion = expand("あ${X}後$${X}${X}", &vars, Escape::None).unwrap();
        assert_eq!(expansion.text, "あ日本語後${X}日本語");
        assert_eq!(
            expansion.edits,
            [(3..7, 3..12, false), (10..15, 15..19, true), (15..19, 19..28, false)]
        );

        for (at, offset) in [(0, 0), (3, 3), (9, 3), (12, 7), (15, 11), (18, 14), (19, 15), (28, 19)] {
            assert_eq!(expansion.to_src_offset(at), offset);
        }
    }
}
